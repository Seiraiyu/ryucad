//! The `board.*` commands' handlers.

use super::file::*;
use super::load::BoardFiles;
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
    p.boards.values().any(|b| {
        Some(&b.id) != except && b.board.item.as_ref().is_ok_and(|x| x.name == name)
    })
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
        Ok(board) => v["board"] = crate::doc::to_value(board, BOARD_KIND, FORMAT),
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
    id: Option<&str>,
    expected: Option<&str>,
) -> Result<Value> {
    let p = session.project();
    check_revision("the project", &p.revision, expected)?;
    let unit = p.manifest.default_unit;
    let design = crate::netlist::one_design(p, Some(design))?;
    if named_elsewhere(p, name, None) {
        return Err(
            Error::new(Code::BoardExists, format!("a board is already named {name}"))
                .with("name", name),
        );
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
    let id = new_id(id, "board", |s| {
        Id::parse(s, "board").is_ok_and(|i| p.boards.contains_key(&i))
    })?;
    let board = Board {
        name: name.to_owned(),
        design,
        preset: JLC_2L.into(),
        outline,
        cutouts: Vec::new(),
    };
    let mut w = Writes::default();
    w.put(board_path(&id), &board, BOARD_KIND);
    commit(session, &format!("board {name}"), w)?;
    Ok(answer(session.project(), &id))
}

pub fn edit(
    session: &mut Session,
    board: &str,
    name: Option<&str>,
    outline: Option<&Value>,
    cutouts: Option<&Value>,
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
        let list = c.as_array().ok_or_else(|| {
            Error::new(Code::InvalidParam, "cutouts are a list of contours")
        })?;
        file.cutouts = list
            .iter()
            .map(|v| contour_from(v, unit))
            .collect::<Result<_>>()?;
    }
    let mut w = Writes::default();
    w.put(board_path(&id), &file, BOARD_KIND);
    commit(session, &format!("board {id}"), w)?;
    Ok(answer(session.project(), &id))
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
    w.delete(board_path(&id));
    commit(session, &format!("board {id}"), w)?;
    Ok(json!({"deleted": format!("board/{id}")}))
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

/// `board show`: the board's file and, per occurrence of its design, its
/// placement or none.
pub fn show(p: &Project, board: &str, _pads: bool) -> Result<Value> {
    let b = find(p, board)?;
    Ok(answer(p, &b.id))
}
