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

#[allow(clippy::too_many_arguments)]
pub fn create(
    session: &mut Session,
    name: &str,
    design: &str,
    rect: Option<&[String; 2]>,
    outline: Option<&Value>,
    layers: Option<u32>,
    preset: Option<&str>,
    id: Option<&str>,
    expected: Option<&str>,
) -> Result<Value> {
    let p = session.project();
    check_revision("the project", &p.revision, expected)?;
    let preset_name = preset.unwrap_or(&super::preset::DEFAULT.name).to_owned();
    let chosen = super::preset::resolve(p, &preset_name).ok_or_else(|| {
        Error::new(
            Code::BoardPresetNotFound,
            format!(
                "there is no preset {preset_name}; presets are {}",
                super::preset::names_in(p)
            ),
        )
        .with("preset", &preset_name)
    })?;
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
        Some(n) => Some(super::stackup::even(&chosen, n as usize)?),
    };
    let id = new_id(id, "board", |s| {
        Id::parse(s, "board").is_ok_and(|i| p.boards.contains_key(&i))
    })?;
    let board = Board {
        name: name.to_owned(),
        design,
        preset: preset_name,
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

#[allow(clippy::too_many_arguments)]
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
    if layers.is_some() && stackup.is_some() {
        return Err(
            Error::new(Code::InvalidParam, "give --layers or --stackup, not both")
                .with("param", "stackup"),
        );
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
    let mut v = json!({
        "board": format!("board/{id}"),
        "revision": session.project().boards[&id].revision,
        "rules": c.preset.rules_json(),
        "findings": owned,
        "counts": counts(&owned),
        "errors": errors,
        "warnings": owned
            .iter()
            .filter(|f| f.severity == crate::fault::Severity::Warning)
            .count(),
        "incomplete": c.incomplete,
    });
    // ryueda 7.B: waived findings counted only when there are any, so
    // boards without waivers answer as before.
    let waived = owned.iter().filter(|f| f.waived.is_some()).count();
    if waived > 0 {
        v["waived"] = json!(waived);
    }
    // ryueda 7.A's rules, shown for boards with a stackup of their own:
    // added to `rules` they would move every stage 5 answer.
    if board_of(&session.project().boards[&id]).is_ok_and(|f| f.stackup.is_some()) {
        v["layer_rules"] = c.preset.layer_rules_json();
    }
    // ryueda 7.B: the board's own rules, resolved, when it has them.
    if session.project().boards[&id].rules.is_some() {
        v["board_rules"] = c.rules.to_json();
    }
    if errors > 0 {
        return Err(Error::new(
            Code::CheckFailed,
            format!("board {id} has {errors} DRC error(s)"),
        )
        .with("report", v));
    }
    Ok(v)
}

/// `board.measure`: two items' exact distance on each copper layer they
/// share, with the clearance the board's rules ask of them.
pub fn measure(
    session: &mut Session,
    board: &str,
    a: &str,
    b: &str,
    layer: Option<&str>,
) -> Result<Value> {
    let id = find(session.project(), board)?.id.clone();
    let c = session.board_check(&id)?;
    let mut v = super::measure::measure(&c.conn, &c.shapes, &c.rules, a, b, layer)?;
    v["revision"] = json!(session.project().boards[&id].revision);
    Ok(v)
}

/// `board ratsnest` (design 924): each open net's unrouted connections,
/// the nearest links that would join its islands, narrowed to a net or
/// to the connections with an end on a part; and how many parts are
/// unplaced (they have no connections).
pub fn ratsnest(
    session: &mut Session,
    board: &str,
    net: Option<&str>,
    reference: Option<&str>,
) -> Result<Value> {
    let id = find(session.project(), board)?.id.clone();
    let c = session.board_check(&id)?;
    let p = session.project();
    let b = &p.boards[&id];
    let file = board_of(b)?;
    let net = match net {
        Some(n) => Some(super::route::net_of(
            &super::route::nets(p, &file.design)?,
            &file.design,
            n,
        )?),
        None => None,
    };
    let (_, occs) = occ::occurrences(p, &file.design)?;
    // A part by its label (`R1`) or its occurrence path (`c0/r1`): its
    // pads' references are by label either way.
    let prefix = match reference {
        Some(r) => {
            let Some(o) = occs.iter().find(|o| o.label() == r || o.reference == r) else {
                return Err(crate::error::Error::new(
                    crate::error::Code::NotFound,
                    format!("board {id} has no part {r}"),
                )
                .with("reference", r));
            };
            Some(format!("{id}:{}/", o.label()))
        }
        None => None,
    };
    let mut out = Vec::new();
    for o in &c.conn.report.opens {
        if net.as_ref().is_some_and(|n| *n != o.net) {
            continue;
        }
        for x in super::connect::connections(&c.conn, o) {
            let mut v = super::connect::connection_json(&c.conn, &x);
            if let Some(pre) = &prefix
                && !(v["a"].as_str().is_some_and(|a| a.starts_with(pre.as_str()))
                    || v["b"].as_str().is_some_and(|b| b.starts_with(pre.as_str())))
            {
                continue;
            }
            v["net"] = json!(o.net);
            out.push(v);
        }
    }
    let unplaced = occs
        .iter()
        .filter(|o| o.footprint_ref.is_some() && !b.placements.contains_key(&o.reference))
        .count();
    Ok(json!({
        "board": format!("board/{id}"),
        "revision": b.revision,
        "connections": out,
        "unplaced": unplaced,
    }))
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
    let mut v = json!({
        "board": format!("board/{id}"),
        "revision": b.revision,
        "placed": b.placements.len(),
        "unplaced": unplaced,
        "nets": {"total": nets.len(), "open": open.len(), "routed": nets.len() - open.len()},
        "findings": counts(&c.findings),
        "errors": c.errors(),
        "warnings": c.warnings(),
        "incomplete": c.incomplete,
    });
    if c.waived() > 0 {
        v["waived"] = json!(c.waived());
    }
    Ok(v)
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

fn length(a: &Value, k: &str, unit: Unit) -> Result<Option<i64>> {
    a.get(k)
        .filter(|v| !v.is_null())
        .map(|v| coord::nm(v, unit))
        .transpose()
}

fn flag(a: &Value, k: &str) -> bool {
    a.get(k).and_then(Value::as_bool).unwrap_or(false)
}

fn given<'a>(a: &'a Value, k: &str) -> Option<&'a Value> {
    a.get(k).filter(|v| !v.is_null())
}

fn side_of(text: Option<&str>) -> Result<PlacementSide> {
    match text {
        None | Some("top") => Ok(PlacementSide::Top),
        Some("bottom") => Ok(PlacementSide::Bottom),
        Some(x) => Err(Error::new(
            Code::InvalidParam,
            format!("--side is top or bottom, not {x}"),
        )
        .with("param", "side")),
    }
}

fn edge_side(text: &str) -> Result<EdgeSide> {
    serde_json::from_value(json!(text)).map_err(|_| {
        Error::new(
            Code::InvalidParam,
            format!("--edge is left, right, top, bottom or any, not {text}"),
        )
        .with("param", "edge")
    })
}

/// One constraint from a request: `within` a length.
fn constraint_of(v: &Value, unit: Unit) -> Result<Constraint> {
    let within = || {
        length(v, "within", unit)?.ok_or_else(|| {
            Error::new(Code::InvalidParam, "--near and --edge need --within")
                .with("param", "within")
        })
    };
    if let Some(n) = v.get("near").and_then(Value::as_str) {
        Ok(Constraint::Near {
            near: n.to_owned(),
            within_nm: within()?,
        })
    } else if let Some(e) = v.get("edge").and_then(Value::as_str) {
        Ok(Constraint::Edge {
            edge: edge_side(e)?,
            within_nm: within()?,
        })
    } else if let Some(s) = v.get("side").and_then(Value::as_str) {
        Ok(Constraint::Side {
            side: side_of(Some(s))?,
        })
    } else {
        Err(Error::new(
            Code::InvalidParam,
            "a constraint is {near, within}, {edge, within} or {side}",
        )
        .with("param", "constraints"))
    }
}

/// A field's place from a request: `{at?, rotation?, hidden?, size?}`.
fn field_of(v: &Value, unit: Unit) -> Result<FieldPlace> {
    Ok(FieldPlace {
        at: given(v, "at").map(|x| coord::point(x, unit)).transpose()?,
        rotation_mdeg: given(v, "rotation")
            .map(|x| rotation(Some(x)))
            .transpose()?,
        hidden: v.get("hidden").and_then(Value::as_bool),
        size_nm: given(v, "size")
            .map(|x| coord::point(x, unit))
            .transpose()?,
    })
}

/// A placement as answers print it: what stage 5 printed, and the side,
/// constraints, group and field places only when set.
pub fn placement_json(pl: &Placement) -> Value {
    let mut v = json!({"at": pl.at, "rotation_mdeg": pl.rotation_mdeg});
    if !pl.side.is_top() {
        v["side"] = json!(pl.side.name());
    }
    if !pl.constraints.is_empty() {
        v["constraints"] = json!(pl.constraints);
    }
    if let Some(g) = &pl.group {
        v["group"] = json!(g);
    }
    if !pl.fields.is_empty() {
        v["fields"] = json!(pl.fields);
    }
    v
}

/// `board place`: one part, or several all or nothing. A placed part
/// keeps its constraints, group and field places unless the ask changes
/// them, and its side unless the ask moves it (`at`: top unless `side`
/// says bottom); without `at` it is placed by the legaliser when the ask gives a
/// near or edge constraint, else it stays where it is. Parts placed by
/// one request don't see each other's new places: place such parts in
/// turn.
pub fn place(
    session: &mut Session,
    board: &str,
    asks: Vec<Value>,
    expected: Option<&str>,
) -> Result<Value> {
    let id = {
        let b = find(session.project(), board)?;
        check_revision(&format!("board {}", b.id), &b.revision, expected)?;
        b.id.clone()
    };
    // The board as it is, for the legaliser.
    let check = session.board_check(&id).ok();
    let p = session.project();
    let unit = p.manifest.default_unit;
    let b = &p.boards[&id];
    let design = board_of(b)?.design.clone();
    if asks.is_empty() {
        return Err(Error::new(
            Code::InvalidParam,
            "give REF with --at, --near or --edge, or --ops with a list of {ref, at?, …}",
        ));
    }
    let (_, occs) = occ::occurrences(p, &design)?;
    let mut w = Writes::default();
    let mut placed = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for (i, a) in asks.iter().enumerate() {
        let r_text = a.get("ref").and_then(Value::as_str).ok_or_else(|| {
            Error::new(Code::InvalidParam, format!("op {i} needs a ref")).with("op", i)
        })?;
        let o = occ::resolve(&occs, &design, r_text)?;
        if o.footprint_ref.is_none() {
            return Err(Error::new(
                Code::InvalidParam,
                format!(
                    "{} has no footprint to place; choose one in the design",
                    o.reference
                ),
            )
            .with("ref", r_text));
        }
        if !seen.insert(o.reference.clone()) {
            return Err(Error::new(
                Code::InvalidParam,
                format!("{} is placed twice in one request", o.reference),
            )
            .with("ref", r_text));
        }
        let had = b
            .placements
            .get(&o.reference)
            .and_then(|l| l.item.as_ref().ok())
            .cloned();
        let mut pl = had.clone().unwrap_or_default();
        if let Some(r) = given(a, "rotation") {
            pl.rotation_mdeg = rotation(Some(r))?;
        }
        // A move (`at`) is to the top unless it says otherwise, as stage 5
        // placed parts; a request without `at` keeps the part's side.
        if given(a, "side").is_some() || given(a, "at").is_some() {
            pl.side = side_of(a.get("side").and_then(Value::as_str))?;
        }
        // Constraints: any given replace the list.
        let mut asked: Vec<Constraint> = match given(a, "constraints") {
            Some(Value::Array(l)) => l
                .iter()
                .map(|c| constraint_of(c, unit))
                .collect::<Result<_>>()?,
            Some(_) => {
                return Err(Error::new(
                    Code::InvalidParam,
                    "constraints is a list of {near, within}, {edge, within} or {side}",
                )
                .with("param", "constraints"));
            }
            None => Vec::new(),
        };
        if let Some(n) = given(a, "near") {
            asked.push(constraint_of(
                &json!({"near": n, "within": a.get("within")}),
                unit,
            )?);
        }
        if let Some(e) = given(a, "edge") {
            asked.push(constraint_of(
                &json!({"edge": e, "within": a.get("within")}),
                unit,
            )?);
        }
        if flag(a, "hold_side") {
            asked.push(Constraint::Side { side: pl.side });
        }
        let constrained = !asked.is_empty();
        if constrained {
            pl.constraints = asked;
        }
        if flag(a, "unconstrain") {
            pl.constraints.clear();
        }
        if let Some(g) = a.get("group").and_then(Value::as_str) {
            pl.group = Some(g.to_owned());
        }
        if flag(a, "ungroup") {
            pl.group = None;
        }
        if let Some(f) = given(a, "fields") {
            if let Some(v) = given(f, "reference") {
                pl.fields.reference = Some(field_of(v, unit)?);
            }
            if let Some(v) = given(f, "value") {
                pl.fields.value = Some(field_of(v, unit)?);
            }
        }
        {
            let r = pl.fields.reference.get_or_insert_with(FieldPlace::default);
            if let Some(at) = given(a, "ref_at") {
                r.at = Some(coord::point(at, unit)?);
            }
            if let Some(rot) = given(a, "ref_rotation") {
                r.rotation_mdeg = Some(rotation(Some(rot))?);
            }
            if flag(a, "ref_hide") {
                r.hidden = Some(true);
            }
            if flag(a, "ref_show") {
                r.hidden = Some(false);
            }
            if *r == FieldPlace::default() {
                pl.fields.reference = None;
            }
        }
        // Where: asked, else found, else kept.
        let mut legalised = None;
        if let Some(at) = given(a, "at") {
            pl.at = coord::point(at, unit)?;
        } else if constrained
            && pl
                .constraints
                .iter()
                .any(|c| !matches!(c, Constraint::Side { .. }))
        {
            let c = check.as_ref().ok_or_else(|| {
                Error::new(
                    Code::InvalidParam,
                    format!("board {id} doesn't check, so the legaliser can't run; give --at"),
                )
            })?;
            let ask = super::place::Ask {
                occ: o,
                rotation_mdeg: pl.rotation_mdeg,
                side: pl.side,
                constraints: &pl.constraints,
                now: had.as_ref().map(|h| h.at),
            };
            let found = super::place::legalise(id.as_str(), &c.shapes, &c.rules, &c.preset, &ask)?;
            pl.at = coord::point(&found["at"], unit)?;
            legalised = Some(json!({"searched_nm": found["searched_nm"], "tried": found["tried"]}));
        } else if had.is_none() {
            return Err(Error::new(
                Code::InvalidParam,
                format!(
                    "{} isn't placed: give --at, or --near or --edge with --within",
                    o.reference
                ),
            )
            .with("ref", r_text));
        }
        w.put(placement_path(&id, &o.reference), &pl, PLACEMENT_KIND);
        let mut entry = placement_json(&pl);
        entry["occurrence"] = json!(o.reference);
        entry["refdes"] = json!(o.refdes);
        if let Some(l) = legalised {
            entry["legalised"] = l;
        }
        placed.push(entry);
    }
    // The legaliser's check is let go before the write: the board's kept
    // state edits its copper in place only when nothing else holds it.
    drop(check);
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
pub fn show(p: &Project, board: &str, pads: bool, pours: bool) -> Result<Value> {
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
                        "placement": placement.map(placement_json),
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
                    if pours {
                        v["pours"] = pours_json(&s, &b.id);
                    }
                }
                Err(e) => v["shapes_error"] = json!(e),
            }
        }
    }
    Ok(v)
}

/// `board show --pours`' list: each fill's net, layer, regions, area,
/// islands removed, thermals and resolved rules. The area is a string:
/// it can pass 2^53.
pub fn pours_json(s: &super::shapes::BoardShapes, id: &crate::id::Id) -> Value {
    let names = super::stackup::names(s.n());
    json!(s
        .pours
        .iter()
        .map(|f| json!({"id": f.id, "reference": format!("{id}:pour/{}", f.id), "net": f.net,
            "layer": names[f.layer],
            "regions": (0..f.regions.len()).map(|k| format!("{id}:pour/{}#{k}", f.id)).collect::<Vec<_>>(),
            "area_nm2": (f.polygons.area2() / 2).to_string(),
            "islands_removed": f.islands_removed,
            "thermals": f.thermals.iter().map(|(p, k)| json!({"pad": p, "spokes": k})).collect::<Vec<_>>(),
            "rules": {"clearance_nm": f.rules.clearance, "min_width_nm": f.rules.min_width,
                      "thermal_gap_nm": f.rules.gap, "thermal_spoke_nm": f.rules.spoke}}))
        .collect::<Vec<_>>())
}

/// What `board render` draws over the layers (ryueda 7.E).
#[derive(Clone, Copy, Debug, Default)]
pub struct Marks {
    pub ratsnest: bool,
    pub findings: bool,
}

type Point = ryu_board::geom::Point;

/// Where a finding is: the centre of the bounds of the shapes its refs
/// name (pads, tracks, vias, holes, pours, courtyards by owner); none
/// when it names no shape.
fn place_of(s: &super::shapes::BoardShapes, refs: &[String]) -> Option<Point> {
    let mut b: Option<ryu_board::geom::Rect> = None;
    let mut take = |r: Option<ryu_board::geom::Rect>| {
        if let Some(r) = r {
            b = Some(b.map_or(r, |x| x.union(r)));
        }
    };
    for r in refs {
        let r = r.split('#').next().unwrap_or(r);
        for p in s.pads.iter().filter(|p| p.reference == r) {
            take(p.copper.bounds());
        }
        for c in s.courtyards.iter().filter(|c| c.owner == r) {
            take(c.shape.bounds());
        }
        for (owner, shape) in s.layers.values().flatten() {
            if owner == r {
                take(shape.bounds());
            }
        }
    }
    b.map(|b| Point::new((b.min.x + b.max.x) / 2, (b.min.y + b.max.y) / 2))
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
    marks: Marks,
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
    let (lines, points) = if marks.ratsnest || marks.findings {
        let c = session.board_check(&id)?;
        let lines: Vec<(Point, Point)> = if marks.ratsnest {
            c.conn
                .report
                .opens
                .iter()
                .flat_map(|o| super::connect::connections(&c.conn, o))
                .map(|x| (x.distance.a, x.distance.b))
                .collect()
        } else {
            Vec::new()
        };
        let points: Vec<Point> = if marks.findings {
            c.findings
                .iter()
                .filter_map(|f| place_of(&shapes, &f.refs))
                .collect()
        } else {
            Vec::new()
        };
        (lines, points)
    } else {
        (Vec::new(), Vec::new())
    };
    let ask = super::render::Ask {
        layers,
        bottom,
        net,
        window,
        size: size.unwrap_or(super::render::DEFAULT_SIZE),
        svg,
        ratsnest: &lines,
        findings: &points,
    };
    let (mut v, mime, bytes) = super::render::render(id.as_str(), &shapes, &ask)?;
    v["notices"] = fault::list(&shapes.notices);
    v["revision"] = json!(session.project().boards[&id].revision);
    Ok(ryu_session::rpc::Answer::bytes(v, mime, bytes))
}
