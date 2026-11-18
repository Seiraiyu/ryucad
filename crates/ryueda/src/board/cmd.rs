//! The `board.*` commands' handlers.

use super::file::*;
use super::load::BoardFiles;
use super::occ;
use crate::coord;
use crate::error::{Code, Error, Result};
use crate::fault;
use crate::id::Id;
use crate::project::Project;
use crate::server::check_revision;
use crate::session::Session;
use crate::units::Unit;
use crate::write::{Writes, commit, new_id};
use serde_json::{Value, json};

/// The board `text` names: its ID, else its unique name.
pub fn find<'a>(p: &'a Project, text: &str) -> Result<&'a BoardFiles> {
    if let Ok(id) = Id::parse(text, "board")
        && let Some(b) = p.boards.get(&id)
    {
        return Ok(b);
    }
    let named: Vec<&BoardFiles> = p
        .boards
        .values()
        .filter(|b| b.board.item.as_ref().is_ok_and(|x| x.name == text))
        .collect();
    match named.as_slice() {
        [one] => Ok(one),
        [] => Err(Error::new(
            Code::BoardNotFound,
            format!("there is no board {text}; `ryueda board list` lists them"),
        )
        .with("board", text)),
        _ => Err(Error::new(
            Code::InvalidParam,
            format!("several boards are named {text}; name one by ID"),
        )
        .with(
            "boards",
            named.iter().map(|b| b.id.to_string()).collect::<Vec<_>>(),
        )),
    }
}

/// The board's file, or why it doesn't read.
pub fn board_of(b: &BoardFiles) -> Result<&Board> {
    b.board.item.as_ref().map_err(Clone::clone)
}

/// A contour from JSON whose coordinates are lengths (a JSON integer is
/// nanometres, a string a quantity, a bare number in the project's unit).
pub fn contour_from(v: &Value, unit: Unit) -> Result<Contour> {
    let bad = |why: &str| {
        Error::new(
            Code::InvalidParam,
            format!(
                "an outline {why}; give {{start: [x, y], edges: [{{type: line, to: [x, y]}} | {{type: arc, mid: [x, y], to: [x, y]}}]}}"
            ),
        )
    };
    let start = coord::point(v.get("start").ok_or_else(|| bad("needs a start"))?, unit)?;
    let mut edges = Vec::new();
    for e in v
        .get("edges")
        .and_then(Value::as_array)
        .ok_or_else(|| bad("needs edges"))?
    {
        let to = coord::point(e.get("to").ok_or_else(|| bad("edge needs a to"))?, unit)?;
        edges.push(match e.get("type").and_then(Value::as_str) {
            Some("line") => Op::Line { to },
            Some("arc") => Op::Arc {
                mid: coord::point(e.get("mid").ok_or_else(|| bad("arc needs a mid"))?, unit)?,
                to,
            },
            _ => return Err(bad("edge's type is line or arc")),
        });
    }
    Ok(Contour { start, edges })
}

fn named_elsewhere(p: &Project, name: &str, except: Option<&Id>) -> bool {
    p.boards
        .values()
        .any(|b| Some(&b.id) != except && b.board.item.as_ref().is_ok_and(|x| x.name == name))
}

/// The board's answer: its reference, file, revision and faults.
pub fn answer(p: &Project, id: &Id) -> Value {
    let b = &p.boards[id];
    let mut v = json!({
        "reference": format!("board/{id}"),
        "path": board_path(id),
        "revision": b.revision,
        "faults": fault::list(&b.faults()),
    });
    match &b.board.item {
        Ok(board) => v["board"] = super::file::value(board, BOARD_KIND),
        Err(e) => v["error"] = json!(e),
    }
    v
}

pub fn create(
    session: &mut Session,
    name: &str,
    design: &str,
    rect: Option<&[String; 2]>,
    outline: Option<&Value>,
    layers: Option<u32>,
    id: Option<&str>,
    expected: Option<&str>,
) -> Result<Value> {
    let p = session.project();
    check_revision("the project", &p.revision, expected)?;
    let unit = p.manifest.default_unit;
    let design = crate::netlist::one_design(p, Some(design))?;
    if named_elsewhere(p, name, None) {
        return Err(Error::new(
            Code::BoardExists,
            format!("a board is already named {name}"),
        )
        .with("name", name));
    }
    if let Some(asked) = id
        && let Ok(i) = Id::parse(asked, "board")
        && p.boards.contains_key(&i)
    {
        return Err(Error::new(
            Code::BoardExists,
            format!("there is already a board {asked}"),
        )
        .with("board", asked));
    }
    let outline = match (rect, outline) {
        (Some([w, h]), None) => {
            Contour::rect(coord::nm(&json!(w), unit)?, coord::nm(&json!(h), unit)?)
        }
        (None, Some(o)) => contour_from(o, unit)?,
        _ => {
            return Err(Error::new(
                Code::InvalidParam,
                "give the outline as --rect W,H or --outline OPS, one of them",
            ));
        }
    };
    let stackup = match layers {
        None | Some(2) => None,
        Some(n) => Some(super::stackup::even(super::preset::DEFAULT, n as usize)?),
    };
    let id = new_id(id, "board", |s| {
        Id::parse(s, "board").is_ok_and(|i| p.boards.contains_key(&i))
    })?;
    let board = Board {
        name: name.to_owned(),
        design,
        preset: super::preset::DEFAULT.name.into(),
        outline,
        cutouts: Vec::new(),
        stackup,
    };
    let mut w = Writes::default();
    w.put(board_path(&id), &board, BOARD_KIND);
    commit(session, &format!("board {name}"), w)?;
    let mut v = answer(session.project(), &id);
    v["drc"] = super::drc::delta(session, &id, Some(Vec::new()));
    Ok(v)
}

pub fn edit(
    session: &mut Session,
    board: &str,
    name: Option<&str>,
    outline: Option<&Value>,
    cutouts: Option<&Value>,
    layers: Option<u32>,
    stackup: Option<&Value>,
    expected: Option<&str>,
) -> Result<Value> {
    let p = session.project();
    let unit = p.manifest.default_unit;
    let b = find(p, board)?;
    check_revision(&format!("board {}", b.id), &b.revision, expected)?;
    let id = b.id.clone();
    let mut file = board_of(b)?.clone();
    if let Some(n) = name {
        if named_elsewhere(p, n, Some(&id)) {
            return Err(
                Error::new(Code::BoardExists, format!("a board is already named {n}"))
                    .with("name", n),
            );
        }
        file.name = n.to_owned();
    }
    if let Some(o) = outline {
        file.outline = contour_from(o, unit)?;
    }
    if let Some(c) = cutouts {
        let list = c
            .as_array()
            .ok_or_else(|| Error::new(Code::InvalidParam, "cutouts are a list of contours"))?;
        file.cutouts = list
            .iter()
            .map(|v| contour_from(v, unit))
            .collect::<Result<_>>()?;
    }
    if let Some(n) = layers {
        let preset = super::preset::by_name(&file.preset).unwrap_or(super::preset::DEFAULT);
        file.stackup = (n != 2)
            .then(|| super::stackup::even(preset, n as usize))
            .transpose()?;
    }
    if let Some(v) = stackup {
        file.stackup = Some(stackup_from(v, unit)?);
    }
    let mut w = Writes::default();
    w.put(board_path(&id), &file, BOARD_KIND);
    let before = super::drc::now(session, &id);
    commit(session, &format!("board {id}"), w)?;
    let mut v = answer(session.project(), &id);
    v["drc"] = super::drc::delta(session, &id, before);
    Ok(v)
}

/// A stackup from a request: thicknesses as `coord` reads lengths.
pub fn stackup_from(v: &Value, unit: Unit) -> Result<super::stackup::Stackup> {
    let mut v = v.clone();
    let layers = v
        .get_mut("layers")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| {
            Error::new(Code::InvalidParam, "a stackup is {layers: [...]}").with("param", "stackup")
        })?;
    for l in layers {
        if let Some(t) = l.get("thickness_nm").cloned() {
            l["thickness_nm"] = json!(coord::nm(&t, unit)?);
        }
    }
    let s: super::stackup::Stackup = serde_json::from_value(v).map_err(|e| {
        Error::new(Code::InvalidParam, format!("the stackup doesn't read: {e}"))
            .with("param", "stackup")
    })?;
    if let Some(f) = super::stackup::judge(&s).into_iter().next() {
        return Err(f.error);
    }
    Ok(s)
}

pub fn delete(session: &mut Session, board: &str, expected: Option<&str>) -> Result<Value> {
    let p = session.project();
    let b = find(p, board)?;
    check_revision(&format!("board {}", b.id), &b.revision, expected)?;
    let id = b.id.clone();
    let mut w = Writes::default();
    for occ in b.placements.keys() {
        w.delete(placement_path(&id, occ));
    }
    for t in b.tracks.keys() {
        w.delete(route_path(&id, TRACKS, t));
    }
    for v in b.vias.keys() {
        w.delete(route_path(&id, VIAS, v));
    }
    for h in b.holes.keys() {
        w.delete(route_path(&id, HOLES, h));
    }
    w.delete(board_path(&id));
    commit(session, &format!("board {id}"), w)?;
    Ok(json!({"deleted": format!("board/{id}")}))
}

/// Counts of findings by code.
pub(crate) fn counts(f: &[super::drc::Finding]) -> Value {
    let mut by: std::collections::BTreeMap<String, u64> = std::collections::BTreeMap::new();
    for x in f {
        *by.entry(json!(x.code).as_str().unwrap_or_default().to_owned())
            .or_default() += 1;
    }
    json!(by)
}

/// `board drc`: the board's findings (those of `codes` when given); fails
/// with `check_failed`, the answer in its details, when one is an error.
pub fn drc(session: &mut Session, board: &str, codes: &[String]) -> Result<Value> {
    let id = find(session.project(), board)?.id.clone();
    let wanted: Vec<Code> = codes
        .iter()
        .map(|c| {
            serde_json::from_value::<Code>(json!(c)).map_err(|_| {
                Error::new(Code::InvalidParam, format!("{c} isn't a finding's code"))
                    .with("param", "code")
            })
        })
        .collect::<Result<_>>()?;
    let c = session.board_check(&id)?;
    let findings: Vec<&super::drc::Finding> = c
        .findings
        .iter()
        .filter(|f| wanted.is_empty() || wanted.contains(&f.code))
        .collect();
    let errors = findings.iter().filter(|f| f.is_error()).count();
    let owned: Vec<super::drc::Finding> = findings.iter().map(|f| (*f).clone()).collect();
    let v = json!({
        "board": format!("board/{id}"),
        "revision": session.project().boards[&id].revision,
        "rules": c.preset.rules_json(),
        "findings": owned,
        "counts": counts(&owned),
        "errors": errors,
        "warnings": owned.len() - errors,
        "incomplete": c.incomplete,
    });
    if errors > 0 {
        return Err(Error::new(
            Code::CheckFailed,
            format!("board {id} has {errors} DRC error(s)"),
        )
        .with("report", v));
    }
    Ok(v)
}

/// `board status`: placed and unplaced parts, nets routed and open, and
/// findings by code.
pub fn status(session: &mut Session, board: &str) -> Result<Value> {
    let id = find(session.project(), board)?.id.clone();
    let c = session.board_check(&id)?;
    let b = &session.project().boards[&id];
    let mut pads_per_net: std::collections::BTreeMap<&str, usize> =
        std::collections::BTreeMap::new();
    for pt in &c.conn.built.layout.points {
        *pads_per_net.entry(pt.net.as_str()).or_default() += 1;
    }
    let nets: Vec<&str> = pads_per_net
        .into_iter()
        .filter(|(_, n)| *n >= 2)
        .map(|(k, _)| k)
        .collect();
    let open: std::collections::BTreeSet<&str> =
        c.conn.report.opens.iter().map(|o| o.net.as_str()).collect();
    let unplaced = c
        .findings
        .iter()
        .filter(|f| f.code == Code::BoardUnplaced)
        .count();
    Ok(json!({
        "board": format!("board/{id}"),
        "revision": b.revision,
        "placed": b.placements.len(),
        "unplaced": unplaced,
        "nets": {"total": nets.len(), "open": open.len(), "routed": nets.len() - open.len()},
        "findings": counts(&c.findings),
        "errors": c.errors(),
        "warnings": c.warnings(),
        "incomplete": c.incomplete,
    }))
}

pub fn list(p: &Project) -> Value {
    let boards: Vec<Value> = p
        .boards
        .values()
        .map(|b| {
            let mut v = json!({
                "id": b.id,
                "reference": format!("board/{}", b.id),
                "revision": b.revision,
                "placed": b.placements.len(),
            });
            match &b.board.item {
                Ok(x) => {
                    v["name"] = json!(x.name);
                    v["design"] = json!(x.design);
                }
                Err(e) => v["error"] = json!(e),
            }
            v
        })
        .collect();
    json!({"boards": boards})
}

/// A rotation in millidegrees, within a turn: degrees as text (`"90"`),
/// whole millidegrees as a JSON integer (as `coord` reads angles), none
/// for 0.
fn rotation(v: Option<&Value>) -> Result<i64> {
    match v {
        None | Some(Value::Null) => Ok(0),
        Some(v) => Ok(coord::mdeg(v)?.rem_euclid(360_000)),
    }
}

/// `board place`: one part, or several all or nothing.
pub fn place(
    session: &mut Session,
    board: &str,
    one: Option<(&str, &[String; 2], Option<&str>)>,
    ops: Option<&Value>,
    expected: Option<&str>,
) -> Result<Value> {
    let p = session.project();
    let unit = p.manifest.default_unit;
    let b = find(p, board)?;
    check_revision(&format!("board {}", b.id), &b.revision, expected)?;
    let id = b.id.clone();
    let design = board_of(b)?.design.clone();
    let asks: Vec<Value> = match (one, ops) {
        (Some((r, at, rot)), None) => vec![json!({"ref": r, "at": at, "rotation": rot})],
        (None, Some(Value::Array(list))) if !list.is_empty() => list.clone(),
        _ => {
            return Err(Error::new(
                Code::InvalidParam,
                "give REF --at X,Y, or --ops with a list of {ref, at: [x, y], rotation?}",
            ));
        }
    };
    let (_, occs) = occ::occurrences(p, &design)?;
    let mut w = Writes::default();
    let mut placed = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for (i, a) in asks.iter().enumerate() {
        let r = a.get("ref").and_then(Value::as_str).ok_or_else(|| {
            Error::new(Code::InvalidParam, format!("op {i} needs a ref")).with("op", i)
        })?;
        let o = occ::resolve(&occs, &design, r)?;
        if o.footprint_ref.is_none() {
            return Err(Error::new(
                Code::InvalidParam,
                format!(
                    "{} has no footprint to place; choose one in the design",
                    o.reference
                ),
            )
            .with("ref", r));
        }
        if !seen.insert(o.reference.clone()) {
            return Err(Error::new(
                Code::InvalidParam,
                format!("{} is placed twice in one request", o.reference),
            )
            .with("ref", r));
        }
        let at = a.get("at").ok_or_else(|| {
            Error::new(Code::InvalidParam, format!("op {i} needs at")).with("op", i)
        })?;
        let placement = Placement {
            at: coord::point(at, unit)?,
            rotation_mdeg: rotation(a.get("rotation"))?,
        };
        w.put(
            placement_path(&id, &o.reference),
            &placement,
            PLACEMENT_KIND,
        );
        placed.push(json!({
            "occurrence": o.reference,
            "refdes": o.refdes,
            "at": placement.at,
            "rotation_mdeg": placement.rotation_mdeg,
        }));
    }
    let before = super::drc::now(session, &id);
    commit(session, &format!("board {id} place"), w)?;
    let b = &session.project().boards[&id];
    let mut v = json!({"board": format!("board/{id}"), "revision": b.revision, "placed": placed});
    v["drc"] = super::drc::delta(session, &id, before);
    Ok(v)
}

/// `board unplace`: a placement's file removed. A placement whose
/// occurrence is gone is named by its occurrence reference.
pub fn unplace(
    session: &mut Session,
    board: &str,
    r: &str,
    expected: Option<&str>,
) -> Result<Value> {
    let p = session.project();
    let b = find(p, board)?;
    check_revision(&format!("board {}", b.id), &b.revision, expected)?;
    let id = b.id.clone();
    let design = board_of(b)?.design.clone();
    let occurrence = match occ::occurrences(p, &design) {
        Ok((_, occs)) => match occ::resolve(&occs, &design, r) {
            Ok(o) => o.reference.clone(),
            Err(e) if e.code == Code::BoardRefAmbiguous => return Err(e),
            Err(_) => occ::local(&design, r),
        },
        Err(_) => occ::local(&design, r),
    };
    if !b.placements.contains_key(&occurrence) {
        return Err(
            Error::new(Code::NotFound, format!("{r} isn't placed on board {id}")).with("ref", r),
        );
    }
    let mut w = Writes::default();
    w.delete(placement_path(&id, &occurrence));
    let before = super::drc::now(session, &id);
    commit(session, &format!("board {id} unplace"), w)?;
    let b = &session.project().boards[&id];
    let mut v =
        json!({"board": format!("board/{id}"), "revision": b.revision, "unplaced": occurrence});
    v["drc"] = super::drc::delta(session, &id, before);
    Ok(v)
}

/// `board show`: the board's file and, per occurrence of its design with a
/// footprint, its placement or none; how many are unplaced, and the
/// placements whose occurrence is gone.
pub fn show(p: &Project, board: &str, pads: bool) -> Result<Value> {
    let b = find(p, board)?;
    let mut v = answer(p, &b.id);
    let Ok(file) = board_of(b) else {
        return Ok(v);
    };
    if file.stackup.is_some() {
        let n = super::stackup::copper_count(file);
        v["layers"] = json!(
            (0..n)
                .map(|i| CopperLayer::at(i, n).short())
                .collect::<Vec<_>>()
        );
    }
    match occ::occurrences(p, &file.design) {
        Err(e) => v["design_error"] = json!(e),
        Ok((_, occs)) => {
            let mut unplaced = 0;
            let list: Vec<Value> = occs
                .iter()
                .filter(|o| o.footprint_ref.is_some())
                .map(|o| {
                    let placement = b
                        .placements
                        .get(&o.reference)
                        .and_then(|l| l.item.as_ref().ok());
                    if placement.is_none() {
                        unplaced += 1;
                    }
                    json!({
                        "reference": o.reference,
                        "refdes": o.refdes,
                        "value": o.value,
                        "footprint": o.footprint_ref,
                        "dnp": o.dnp,
                        "placement": placement.map(|pl| json!({"at": pl.at, "rotation_mdeg": pl.rotation_mdeg})),
                    })
                })
                .collect();
            let orphans: Vec<&String> = b
                .placements
                .keys()
                .filter(|k| !occs.iter().any(|o| &o.reference == *k))
                .collect();
            v["occurrences"] = json!(list);
            v["unplaced"] = json!(unplaced);
            v["orphans"] = json!(orphans);
            // What can't be built, always; the pads when asked.
            match super::shapes::build(p, b) {
                Ok(s) => {
                    v["notices"] = fault::list(&s.notices);
                    if pads {
                        v["pads"] = super::shapes::pads_json(&s);
                    }
                }
                Err(e) => v["shapes_error"] = json!(e),
            }
        }
    }
    Ok(v)
}

/// `board render`: the board's shapes (kept by the session) drawn.
#[allow(clippy::too_many_arguments)]
pub fn render(
    session: &mut Session,
    board: &str,
    layers: &[String],
    side: Option<&str>,
    net: Option<&str>,
    window: Option<&[String; 4]>,
    size: Option<u32>,
    format: &str,
) -> Result<ryu_session::rpc::Answer> {
    let p = session.project();
    let unit = p.manifest.default_unit;
    let id = find(p, board)?.id.clone();
    let bottom = match side {
        None | Some("top") => false,
        Some("bottom") => true,
        Some(o) => {
            return Err(Error::new(
                Code::InvalidParam,
                format!("--side is top or bottom, not {o}"),
            )
            .with("param", "side"));
        }
    };
    let svg = match format {
        "png" => false,
        "svg" => true,
        o => {
            return Err(Error::new(
                Code::InvalidParam,
                format!("{o} isn't a format: png or svg"),
            )
            .with("param", "format"));
        }
    };
    let window = match window {
        None => None,
        Some(w) => {
            let v: Vec<i64> = w
                .iter()
                .map(|x| coord::nm(&json!(x), unit))
                .collect::<Result<_>>()?;
            if v[2] <= v[0] || v[3] <= v[1] {
                return Err(Error::new(
                    Code::InvalidParam,
                    "--window is X0,Y0,X1,Y1 with X1 > X0 and Y1 > Y0",
                )
                .with("param", "window"));
            }
            Some(ryu_board::geom::Rect::new(
                ryu_board::geom::Point::new(v[0], v[1]),
                ryu_board::geom::Point::new(v[2], v[3]),
            ))
        }
    };
    let shapes = session.board_shapes(&id)?;
    let ask = super::render::Ask {
        layers,
        bottom,
        net,
        window,
        size: size.unwrap_or(super::render::DEFAULT_SIZE),
        svg,
    };
    let (mut v, mime, bytes) = super::render::render(id.as_str(), &shapes, &ask)?;
    v["notices"] = fault::list(&shapes.notices);
    v["revision"] = json!(session.project().boards[&id].revision);
    Ok(ryu_session::rpc::Answer::bytes(v, mime, bytes))
}
