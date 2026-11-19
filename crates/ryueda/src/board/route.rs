//! The route commands (design 732): tracks, vias and holes added, edited
//! and deleted. A point is `[x, y]` (lengths) or a pad's or via's
//! reference standing for its centre; the file holds the coordinates.

use super::cmd::{board_of, find};
use super::file::*;
use super::load::BoardFiles;
use super::shapes::BoardShapes;
use crate::coord;
use crate::error::{Code, Error, Result};
use crate::id::Id;
use crate::project::Project;
use crate::server::check_revision;
use crate::session::Session;
use crate::units::Unit;
use crate::write::{Writes, commit, new_id};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::sync::Arc;

fn invalid(msg: impl Into<String>, param: &str) -> Error {
    Error::new(Code::InvalidParam, msg.into()).with("param", param)
}

/// A field of an ask, absent when missing or null.
fn field<'a>(a: &'a Value, k: &str) -> Option<&'a Value> {
    a.get(k).filter(|v| !v.is_null())
}

/// The design's nets as the board names them.
pub fn nets(p: &Project, design: &Id) -> Result<BTreeSet<String>> {
    let n = crate::netlist::flatten(p, design)?;
    Ok(n.nets
        .iter()
        .map(|x| super::occ::local(design, &x.id))
        .collect())
}

/// The net `text` names (with or without the design's prefix).
pub fn net_of(nets: &BTreeSet<String>, design: &Id, text: &str) -> Result<String> {
    let local = super::occ::local(design, text);
    if nets.contains(&local) {
        return Ok(local);
    }
    Err(Error::new(
        Code::BoardNetNotFound,
        format!("design {design} has no net {text}; `ryueda netlist` lists its nets and `ryueda board show B --pads` each pad's"),
    )
    .with("net", text))
}

fn layer_of(v: Option<&Value>, n: usize) -> Result<CopperLayer> {
    let text = v
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("a track needs --layer (top, in1 …, bottom)", "layer"))?;
    named_layer(text, n, "layer")
}

/// A copper layer the board has, by name.
fn named_layer(text: &str, n: usize, param: &str) -> Result<CopperLayer> {
    let list = super::load::layer_list(n);
    let layer = CopperLayer::parse(text)
        .ok_or_else(|| invalid(format!("--{param} is one of {list}, not {text}"), param))?;
    match layer.index(n) {
        Some(_) => Ok(layer),
        None => Err(Error::new(
            Code::BoardLayerNotFound,
            format!("the board has {n} copper layers and no {text}; they are {list}"),
        )
        .with("layer", text)),
    }
}

/// A length that must be positive.
fn positive(v: &Value, unit: Unit, param: &str) -> Result<i64> {
    let n = coord::nm(v, unit)?;
    if n <= 0 {
        return Err(invalid(format!("--{param} is a positive length"), param));
    }
    Ok(n)
}

/// Whether any ask names a point by reference: then the shapes are needed.
fn by_reference(asks: &[Value], key: &str) -> bool {
    asks.iter().any(|a| match field(a, key) {
        Some(Value::String(_)) => true,
        Some(Value::Array(l)) => l.iter().any(Value::is_string),
        _ => false,
    })
}

/// A point: `[x, y]` lengths, or a pad's (`R1/1`, `<board>:R1/1`) or
/// via's (`via/<id>`) reference standing for its centre.
fn point(
    v: &Value,
    unit: Unit,
    board: &Id,
    shapes: Option<&BoardShapes>,
    b: &BoardFiles,
) -> Result<[i64; 2]> {
    let Some(text) = v.as_str() else {
        return coord::point(v, unit);
    };
    let full = if text.contains(':') {
        text.to_owned()
    } else {
        format!("{board}:{text}")
    };
    if let Some(pd) = shapes.and_then(|s| s.pads.iter().find(|p| p.reference == full)) {
        return Ok([pd.at.x, pd.at.y]);
    }
    if let Some(vid) = full.strip_prefix(&format!("{board}:via/"))
        && let Ok(i) = Id::parse(vid, "via")
        && let Some(Ok(via)) = b.vias.get(&i).map(|l| l.item.as_ref())
    {
        return Ok(via.at);
    }
    Err(Error::new(
        Code::NotFound,
        format!("board {board} has no pad or via {text}; `ryueda board show {board} --pads` lists its pads"),
    )
    .with("point", text))
}

/// The findings before a write, if the board checks.
type Before = Option<Vec<super::drc::Finding>>;

/// The board a write names, checked against `expected`, and its findings
/// before the write.
fn start(session: &mut Session, board: &str, expected: Option<&str>) -> Result<(Id, Before)> {
    let b = find(session.project(), board)?;
    check_revision(&format!("board {}", b.id), &b.revision, expected)?;
    let id = b.id.clone();
    let before = super::drc::now(session, &id);
    Ok((id, before))
}

/// A write's answer: the board, its revision, what the write made and
/// what it did to the board's findings.
fn answer(session: &mut Session, id: &Id, made: Value, before: Before) -> Value {
    let mut v = made;
    v["board"] = json!(format!("board/{id}"));
    v["revision"] = json!(session.project().boards.get(id).map(|b| b.revision.clone()));
    v["drc"] = super::drc::delta(session, id, before);
    v
}

/// `board track add`: one track or several (all or nothing).
pub fn track_add(
    session: &mut Session,
    board: &str,
    asks: Vec<Value>,
    expected: Option<&str>,
) -> Result<Value> {
    let (id, before) = start(session, board, expected)?;
    if asks.is_empty() {
        return Err(invalid("give --net, --layer and --path, or --ops", "ops"));
    }
    let shapes: Option<Arc<BoardShapes>> = if by_reference(&asks, "path") {
        Some(session.board_shapes(&id)?)
    } else {
        None
    };
    let p = session.project();
    let unit = p.manifest.default_unit;
    let b = &p.boards[&id];
    let file = board_of(b)?;
    let rules = super::rules::of(p, b)?;
    let nets = nets(p, &file.design)?;
    let mut taken: BTreeSet<String> = b.tracks.keys().map(ToString::to_string).collect();
    let mut w = Writes::default();
    let mut made = Vec::new();
    for (i, a) in asks.iter().enumerate() {
        let net = field(a, "net")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid(format!("track {i} needs a net"), "net"))?;
        let net = net_of(&nets, &file.design, net)?;
        let layer = layer_of(field(a, "layer"), super::stackup::copper_count(file))?;
        let width = match field(a, "width") {
            None => rules.track_width_default(Some(net.as_str())).value,
            Some(v) => positive(v, unit, "width")?,
        };
        let path = field(a, "path").and_then(Value::as_array).ok_or_else(|| {
            invalid(
                format!("track {i} needs a path of two or more points"),
                "path",
            )
        })?;
        let points = path
            .iter()
            .map(|v| point(v, unit, &id, shapes.as_deref(), b))
            .collect::<Result<Vec<_>>>()?;
        let tid = new_id(field(a, "id").and_then(Value::as_str), "track", |s| {
            taken.contains(s)
        })?;
        taken.insert(tid.to_string());
        let t = Track {
            net,
            layer,
            width_nm: width,
            points,
        };
        w.put(route_path(&id, TRACKS, &tid), &t, TRACK_KIND);
        made.push(json!({"id": tid, "reference": format!("{id}:track/{tid}"), "track": super::file::value(&t, TRACK_KIND)}));
    }
    commit(session, &format!("board {id} track"), w)?;
    Ok(answer(session, &id, json!({"tracks": made}), before))
}

/// `board track edit`: the fields given replaced.
#[allow(clippy::too_many_arguments)]
pub fn track_edit(
    session: &mut Session,
    board: &str,
    track: &str,
    net: Option<&str>,
    layer: Option<&str>,
    width: Option<&str>,
    path: Option<&[Value]>,
    expected: Option<&str>,
) -> Result<Value> {
    let (id, before) = start(session, board, expected)?;
    let refs = path.is_some_and(|l| l.iter().any(Value::is_string));
    let shapes = if refs {
        Some(session.board_shapes(&id)?)
    } else {
        None
    };
    let p = session.project();
    let unit = p.manifest.default_unit;
    let b = &p.boards[&id];
    let file = board_of(b)?;
    let tid = Id::parse(track, "track")?;
    let mut t = b
        .tracks
        .get(&tid)
        .ok_or_else(|| {
            Error::new(Code::NotFound, format!("board {id} has no track {track}"))
                .with("track", track)
        })?
        .item
        .clone()?;
    if let Some(n) = net {
        t.net = net_of(&nets(p, &file.design)?, &file.design, n)?;
    }
    if let Some(l) = layer {
        t.layer = layer_of(Some(&json!(l)), super::stackup::copper_count(file))?;
    }
    if let Some(wd) = width {
        t.width_nm = positive(&json!(wd), unit, "width")?;
    }
    if let Some(list) = path {
        t.points = list
            .iter()
            .map(|v| point(v, unit, &id, shapes.as_deref(), b))
            .collect::<Result<_>>()?;
    }
    let mut w = Writes::default();
    w.put(route_path(&id, TRACKS, &tid), &t, TRACK_KIND);
    commit(session, &format!("board {id} track {tid}"), w)?;
    let made = json!({"tracks": [{"id": tid, "reference": format!("{id}:track/{tid}"), "track": super::file::value(&t, TRACK_KIND)}]});
    Ok(answer(session, &id, made, before))
}

/// Deletes routes of one folder by ID, all or nothing.
fn delete(
    session: &mut Session,
    board: &str,
    folder: &str,
    what: &str,
    ids: &[String],
    expected: Option<&str>,
) -> Result<Value> {
    let (id, before) = start(session, board, expected)?;
    if ids.is_empty() {
        return Err(invalid(format!("name the {what}s to delete"), what));
    }
    let b = &session.project().boards[&id];
    let mut w = Writes::default();
    for text in ids {
        let rid = Id::parse(text, what)?;
        let held = match folder {
            TRACKS => b.tracks.contains_key(&rid),
            VIAS => b.vias.contains_key(&rid),
            _ => b.holes.contains_key(&rid),
        };
        if !held {
            return Err(
                Error::new(Code::NotFound, format!("board {id} has no {what} {text}"))
                    .with(what, text.as_str()),
            );
        }
        w.delete(route_path(&id, folder, &rid));
    }
    commit(session, &format!("board {id} {what} delete"), w)?;
    Ok(answer(
        session,
        &id,
        json!({"deleted": ids.iter().map(|r| format!("{id}:{what}/{r}")).collect::<Vec<_>>()}),
        before,
    ))
}

pub fn track_delete(
    session: &mut Session,
    board: &str,
    ids: &[String],
    expected: Option<&str>,
) -> Result<Value> {
    delete(session, board, TRACKS, "track", ids, expected)
}

pub fn via_delete(
    session: &mut Session,
    board: &str,
    ids: &[String],
    expected: Option<&str>,
) -> Result<Value> {
    delete(session, board, VIAS, "via", ids, expected)
}

pub fn hole_delete(
    session: &mut Session,
    board: &str,
    ids: &[String],
    expected: Option<&str>,
) -> Result<Value> {
    delete(session, board, HOLES, "hole", ids, expected)
}

/// `board via add`: one via or several (all or nothing).
pub fn via_add(
    session: &mut Session,
    board: &str,
    asks: Vec<Value>,
    expected: Option<&str>,
) -> Result<Value> {
    let (id, before) = start(session, board, expected)?;
    if asks.is_empty() {
        return Err(invalid("give --net and --at, or --ops", "ops"));
    }
    let shapes = if by_reference(&asks, "at") {
        Some(session.board_shapes(&id)?)
    } else {
        None
    };
    let p = session.project();
    let unit = p.manifest.default_unit;
    let b = &p.boards[&id];
    let file = board_of(b)?;
    let rules = super::rules::of(p, b)?;
    let nets = nets(p, &file.design)?;
    let mut taken: BTreeSet<String> = b.vias.keys().map(ToString::to_string).collect();
    let mut w = Writes::default();
    let mut made = Vec::new();
    for (i, a) in asks.iter().enumerate() {
        let net = field(a, "net")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid(format!("via {i} needs a net"), "net"))?;
        let net = net_of(&nets, &file.design, net)?;
        let at = point(
            field(a, "at").ok_or_else(|| invalid(format!("via {i} needs --at"), "at"))?,
            unit,
            &id,
            shapes.as_deref(),
            b,
        )?;
        let drill = match field(a, "drill") {
            None => rules.via_default(Some(net.as_str())).0.drill_nm,
            Some(v) => positive(v, unit, "drill")?,
        };
        let diameter = match field(a, "diameter") {
            None => rules.via_default(Some(net.as_str())).0.diameter_nm,
            Some(v) => positive(v, unit, "diameter")?,
        };
        let vid = new_id(field(a, "id").and_then(Value::as_str), "via", |s| {
            taken.contains(s)
        })?;
        taken.insert(vid.to_string());
        let n = super::stackup::copper_count(file);
        let end = |param: &str| -> Result<Option<CopperLayer>> {
            field(a, param)
                .map(|v| {
                    let text = v.as_str().ok_or_else(|| {
                        invalid(format!("--{param} is a copper layer's name"), param)
                    })?;
                    named_layer(text, n, param)
                })
                .transpose()
        };
        let v = Via {
            net,
            at,
            drill_nm: drill,
            diameter_nm: diameter,
            from: end("from")?,
            to: end("to")?,
        };
        if v.span(n).is_none() {
            return Err(invalid("--from lies above --to", "from"));
        }
        w.put(route_path(&id, VIAS, &vid), &v, VIA_KIND);
        made.push(json!({"id": vid, "reference": format!("{id}:via/{vid}"), "via": super::file::value(&v, VIA_KIND)}));
    }
    commit(session, &format!("board {id} via"), w)?;
    Ok(answer(session, &id, json!({"vias": made}), before))
}

/// `board hole add`: one hole or several (all or nothing).
pub fn hole_add(
    session: &mut Session,
    board: &str,
    asks: Vec<Value>,
    expected: Option<&str>,
) -> Result<Value> {
    let (id, before) = start(session, board, expected)?;
    if asks.is_empty() {
        return Err(invalid("give --at and --drill, or --ops", "ops"));
    }
    let p = session.project();
    let unit = p.manifest.default_unit;
    let b = &p.boards[&id];
    let mut taken: BTreeSet<String> = b.holes.keys().map(ToString::to_string).collect();
    let mut w = Writes::default();
    let mut made = Vec::new();
    for (i, a) in asks.iter().enumerate() {
        let at = coord::point(
            field(a, "at").ok_or_else(|| invalid(format!("hole {i} needs --at"), "at"))?,
            unit,
        )?;
        let drill = positive(
            field(a, "drill").ok_or_else(|| invalid(format!("hole {i} needs --drill"), "drill"))?,
            unit,
            "drill",
        )?;
        let hid = new_id(field(a, "id").and_then(Value::as_str), "hole", |s| {
            taken.contains(s)
        })?;
        taken.insert(hid.to_string());
        let h = Hole {
            at,
            drill_nm: drill,
        };
        w.put(route_path(&id, HOLES, &hid), &h, HOLE_KIND);
        made.push(json!({"id": hid, "reference": format!("{id}:hole/{hid}"), "hole": super::file::value(&h, HOLE_KIND)}));
    }
    commit(session, &format!("board {id} hole"), w)?;
    Ok(answer(session, &id, json!({"holes": made}), before))
}

/// The asks a request carries: `ops`' list, else the one its fields make.
pub fn asks(ops: Option<Value>, one: Value) -> Result<Vec<Value>> {
    match ops {
        None => Ok(vec![one]),
        Some(Value::Array(l)) if !l.is_empty() => Ok(l),
        Some(_) => Err(invalid(
            "--ops is a JSON list of one or more objects",
            "ops",
        )),
    }
}
