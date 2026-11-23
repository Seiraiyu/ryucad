//! `board keepout add | edit | delete` (design 924). A keepout that
//! doesn't close, or names a layer the board lacks, is refused by the
//! commit with its fault's code.

use super::file::{Contour, KEEPOUT_KIND, KEEPOUTS, Op, route_path};
use super::keepout::{Keep, Keepout};
use super::route::{answer, asks, field, start};
use crate::coord;
use crate::error::{Code, Error, Result};
use crate::id::Id;
use crate::session::Session;
use crate::units::Unit;
use crate::write::{Writes, commit, new_id};
use serde_json::{Value, json};

fn invalid(msg: impl Into<String>, param: &str) -> Error {
    Error::new(Code::InvalidParam, msg.into()).with("param", param)
}

/// `[x0, y0, x1, y1]` (lengths) as a counterclockwise rectangle.
fn rect(v: &Value, unit: Unit) -> Result<Contour> {
    let n: Vec<i64> = match v.as_array() {
        Some(l) if l.len() == 4 => l
            .iter()
            .map(|x| coord::nm(x, unit))
            .collect::<Result<_>>()?,
        _ => return Err(invalid("--rect is X0,Y0,X1,Y1", "rect")),
    };
    let (x0, x1) = (n[0].min(n[2]), n[0].max(n[2]));
    let (y0, y1) = (n[1].min(n[3]), n[1].max(n[3]));
    Ok(Contour {
        start: [x0, y0],
        edges: vec![
            Op::Line { to: [x1, y0] },
            Op::Line { to: [x1, y1] },
            Op::Line { to: [x0, y1] },
            Op::Line { to: [x0, y0] },
        ],
    })
}

fn contour_of(a: &Value, unit: Unit) -> Result<Option<Contour>> {
    match (field(a, "contour"), field(a, "rect")) {
        (Some(_), Some(_)) => Err(invalid("give --contour or --rect, not both", "contour")),
        (Some(c), None) => super::cmd::contour_from(c, unit).map(Some),
        (None, Some(r)) => rect(r, unit).map(Some),
        (None, None) => Ok(None),
    }
}

fn keeps_of(a: &Value) -> Result<Option<Vec<Keep>>> {
    let Some(l) = field(a, "keep").and_then(Value::as_array) else {
        return Ok(None);
    };
    let mut out: Vec<Keep> = l
        .iter()
        .map(|k| {
            serde_json::from_value(k.clone()).map_err(|_| {
                invalid(
                    format!(
                        "--keep is copper, tracks, vias, pads, pours, placements or holes, not {k}"
                    ),
                    "keep",
                )
            })
        })
        .collect::<Result<_>>()?;
    // Sorted by name, as a merge of the set leaves it.
    out.sort_by_key(|k| serde_json::to_string(k).unwrap_or_default());
    out.dedup();
    Ok(Some(out))
}

fn layers_of(a: &Value) -> Option<Vec<String>> {
    field(a, "layer").and_then(Value::as_array).map(|l| {
        l.iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect()
    })
}

/// `board.keepout.add`: one keepout or several (all or nothing); on every
/// layer when none is named.
pub fn add(
    session: &mut Session,
    board: &str,
    asks_: Vec<Value>,
    expected: Option<&str>,
) -> Result<Value> {
    let (id, before) = start(session, board, expected)?;
    let p = session.project();
    let unit = p.manifest.default_unit;
    let b = &p.boards[&id];
    let mut taken: std::collections::BTreeSet<String> =
        b.keepouts.keys().map(ToString::to_string).collect();
    let mut w = Writes::default();
    let mut made = Vec::new();
    for (i, a) in asks_.iter().enumerate() {
        let contour = contour_of(a, unit)?
            .ok_or_else(|| invalid(format!("keepout {i} needs --contour or --rect"), "contour"))?;
        let keeps = keeps_of(a)?
            .filter(|k| !k.is_empty())
            .ok_or_else(|| invalid(format!("keepout {i} needs --keep"), "keep"))?;
        let layers = layers_of(a)
            .filter(|l| !l.is_empty())
            .unwrap_or_else(|| vec!["all".into()]);
        let kid = new_id(field(a, "id").and_then(Value::as_str), "keepout", |s| {
            taken.contains(s)
        })?;
        taken.insert(kid.to_string());
        let k = Keepout {
            contour,
            layers,
            keeps,
        };
        w.put(route_path(&id, KEEPOUTS, &kid), &k, KEEPOUT_KIND);
        made.push(json!({"id": kid, "reference": format!("{id}:keepout/{kid}"), "keepout": super::file::value(&k, KEEPOUT_KIND)}));
    }
    commit(session, &format!("board {id} keepout"), w)?;
    Ok(answer(session, &id, json!({"keepouts": made}), before))
}

/// `board.keepout.edit`: a keepout's contour, layers or kinds replaced.
pub fn edit(
    session: &mut Session,
    board: &str,
    keepout: &str,
    a: &Value,
    expected: Option<&str>,
) -> Result<Value> {
    let (id, before) = start(session, board, expected)?;
    let p = session.project();
    let unit = p.manifest.default_unit;
    let kid = Id::parse(keepout, "keepout")?;
    let mut k = p.boards[&id]
        .keepouts
        .get(&kid)
        .and_then(|l| l.item.as_ref().ok())
        .cloned()
        .ok_or_else(|| {
            Error::new(
                Code::NotFound,
                format!("board {id} has no keepout {keepout}"),
            )
            .with("keepout", keepout)
        })?;
    if let Some(c) = contour_of(a, unit)? {
        k.contour = c;
    }
    if let Some(l) = layers_of(a).filter(|l| !l.is_empty()) {
        k.layers = l;
    }
    if let Some(x) = keeps_of(a)?.filter(|x| !x.is_empty()) {
        k.keeps = x;
    }
    let mut w = Writes::default();
    w.put(route_path(&id, KEEPOUTS, &kid), &k, KEEPOUT_KIND);
    commit(session, &format!("board {id} keepout {kid}"), w)?;
    Ok(answer(
        session,
        &id,
        json!({"keepouts": [{"id": kid, "reference": format!("{id}:keepout/{kid}"), "keepout": super::file::value(&k, KEEPOUT_KIND)}]}),
        before,
    ))
}

pub fn delete(
    session: &mut Session,
    board: &str,
    ids: &[String],
    expected: Option<&str>,
) -> Result<Value> {
    super::route::delete(session, board, KEEPOUTS, "keepout", ids, expected)
}
