//! `board text add`, `board drawing add | edit | delete` (design 924).
//! Points and lengths in a drawing's ops are lengths (strings) or
//! nanometres (integers); its file holds nanometres.

use super::drawing::{DRAWING_KIND, DRAWINGS, Graphic};
use super::file::route_path;
use super::route::{answer, field, start};
use crate::coord;
use crate::error::{Code, Error, Result};
use crate::id::Id;
use crate::session::Session;
use crate::units::Unit;
use crate::write::{Writes, commit, new_id};
use serde_json::{Value, json};

/// Every point (`at`, `start`, `end`, `mid`, `center`, `points`) and size
/// (`width_nm`, `radius_nm`, `size_nm`, `thickness_nm`) of a drawing's
/// JSON in nanometres.
fn in_nm(v: &Value, unit: Unit) -> Result<Value> {
    let mut out = v.clone();
    let Some(o) = out.as_object_mut() else {
        return Err(
            Error::new(Code::InvalidParam, "a drawing is a JSON object").with("param", "ops")
        );
    };
    for k in ["at", "start", "end", "mid", "center"] {
        if let Some(p) = o.get(k).cloned() {
            o.insert(k.into(), json!(coord::point(&p, unit)?));
        }
    }
    if let Some(Value::Array(pts)) = o.get("points").cloned() {
        let pts: Vec<[i64; 2]> = pts
            .iter()
            .map(|p| coord::point(p, unit))
            .collect::<Result<_>>()?;
        o.insert("points".into(), json!(pts));
    }
    for k in ["width_nm", "radius_nm", "thickness_nm"] {
        if let Some(x) = o.get(k).cloned() {
            o.insert(k.into(), json!(coord::nm(&x, unit)?));
        }
    }
    if let Some(s) = o.get("size_nm").cloned() {
        o.insert("size_nm".into(), json!(coord::point(&s, unit)?));
    }
    if let Some(r) = o.get("rotation").cloned() {
        o.remove("rotation");
        o.insert("rotation_mdeg".into(), json!(coord::mdeg(&r)?));
    }
    o.remove("id");
    Ok(out)
}

fn graphic(v: &Value, unit: Unit) -> Result<Graphic> {
    serde_json::from_value(in_nm(v, unit)?).map_err(|e| {
        Error::new(Code::InvalidParam, format!("a drawing is {{layer, type: line|rect|arc|circle|polygon|bezier|text, …}} as a footprint's graphic: {e}")).with("param", "ops")
    })
}

/// `board.drawing.add` (and `board.text.add`, whose one op is a text):
/// one drawing or several, all or nothing.
pub fn add(
    session: &mut Session,
    board: &str,
    asks: Vec<Value>,
    expected: Option<&str>,
) -> Result<Value> {
    let (id, before) = start(session, board, expected)?;
    let p = session.project();
    let unit = p.manifest.default_unit;
    let mut taken: std::collections::BTreeSet<String> = p.boards[&id]
        .drawings
        .keys()
        .map(ToString::to_string)
        .collect();
    let mut w = Writes::default();
    let mut made = Vec::new();
    for a in &asks {
        let g = graphic(a, unit)?;
        let did = new_id(field(a, "id").and_then(Value::as_str), "drawing", |s| {
            taken.contains(s)
        })?;
        taken.insert(did.to_string());
        w.put(route_path(&id, DRAWINGS, &did), &g, DRAWING_KIND);
        made.push(json!({"id": did, "reference": format!("{id}:drawing/{did}"), "drawing": super::file::value(&g, DRAWING_KIND)}));
    }
    commit(session, &format!("board {id} drawing"), w)?;
    Ok(answer(session, &id, json!({"drawings": made}), before))
}

/// `board.drawing.edit`: the drawing's JSON with `changes`' keys over it.
pub fn edit(
    session: &mut Session,
    board: &str,
    drawing: &str,
    changes: &Value,
    expected: Option<&str>,
) -> Result<Value> {
    let (id, before) = start(session, board, expected)?;
    let p = session.project();
    let unit = p.manifest.default_unit;
    let did = Id::parse(drawing, "drawing")?;
    let g = p.boards[&id]
        .drawings
        .get(&did)
        .and_then(|l| l.item.as_ref().ok())
        .cloned()
        .ok_or_else(|| {
            Error::new(
                Code::NotFound,
                format!("board {id} has no drawing {drawing}"),
            )
            .with("drawing", drawing)
        })?;
    let mut v = serde_json::to_value(&g).expect("serialises");
    let Some(c) = changes.as_object() else {
        return Err(Error::new(
            Code::InvalidParam,
            "--ops is a JSON object of the keys to change",
        )
        .with("param", "ops"));
    };
    for (k, x) in in_nm(&Value::Object(c.clone()), unit)?
        .as_object()
        .into_iter()
        .flatten()
    {
        v[k] = x.clone();
    }
    let g = graphic(&v, unit)?;
    let mut w = Writes::default();
    w.put(route_path(&id, DRAWINGS, &did), &g, DRAWING_KIND);
    commit(session, &format!("board {id} drawing {did}"), w)?;
    Ok(answer(
        session,
        &id,
        json!({"drawings": [{"id": did, "reference": format!("{id}:drawing/{did}"), "drawing": super::file::value(&g, DRAWING_KIND)}]}),
        before,
    ))
}

pub fn delete(
    session: &mut Session,
    board: &str,
    ids: &[String],
    expected: Option<&str>,
) -> Result<Value> {
    super::route::delete(session, board, DRAWINGS, "drawing", ids, expected)
}
