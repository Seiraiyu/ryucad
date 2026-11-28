//! Batch edits of a footprint (ryueda 7.F): `add`, `remove`, `set` and
//! `move` ops on its pads, graphics and keepouts (by ID), its
//! `reference` and `value` fields (by name) and its models (by index),
//! applied in order, all or nothing, with `edit`'s error shapes. An op's
//! fields are the file's own; lengths and angles are quantities under
//! names without the file's unit suffixes (`size`, `width`, `rotation`;
//! whole numbers are nanometres and millidegrees), and `null` on a `set`
//! takes an optional field off. On `add`, the object's key holds its new
//! ID, or `null` for the next free one (`p<n>`, `g<n>`, `k<n>`).

use crate::coord;
use crate::edit::{moved, next_id, no_next_id, offset, only, op_error, typed};
use crate::error::{Code, Error, Result};
use crate::footprint::Footprint;
use crate::id::Id;
use crate::units::Unit;
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

/// What a batch did besides changing the footprint.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Applied {
    /// The IDs `add` ops gave or were given, in order.
    pub added: Vec<Id>,
}

/// How an op's field becomes the file's.
#[derive(Clone, Copy)]
enum Q {
    /// As given.
    Plain,
    /// A point: `[x, y]` lengths.
    Point,
    /// A list of points.
    Points,
    /// A length.
    Len,
    /// Two lengths, `[w, h]`.
    Pair,
    /// An angle.
    Angle,
    /// Three lengths.
    Lens3,
    /// Three angles.
    Angles3,
    /// A pad's drill: `shape`, `diameter`, `size`, `offset`.
    Drill,
    /// A pad's thermal: `spoke`, `gap`, `angle`.
    Thermal,
    /// A pad's shape: a type's name (`rect`) or the file's object.
    Shape,
}

/// An op's field: its name in the op, its key in the file, and how it
/// converts.
struct F(&'static str, &'static str, Q);

const PAD: &[F] = &[
    F("number", "number", Q::Plain),
    F("kind", "kind", Q::Plain),
    F("shape", "shape", Q::Shape),
    F("at", "at", Q::Point),
    F("rotation", "rotation_mdeg", Q::Angle),
    F("size", "size", Q::Pair),
    F("drill", "drill", Q::Drill),
    F("layers", "layers", Q::Plain),
    F("mask_margin", "mask_margin_nm", Q::Len),
    F("paste_margin", "paste_margin_nm", Q::Len),
    F("paste_ratio_ppm", "paste_ratio_ppm", Q::Plain),
    F("clearance", "clearance_nm", Q::Len),
    F("zone_connect", "zone_connect", Q::Plain),
    F("thermal", "thermal", Q::Thermal),
    F("property", "property", Q::Plain),
    F("die_length", "die_length_nm", Q::Len),
];

const DRILL: &[F] = &[
    F("shape", "shape", Q::Plain),
    F("diameter", "diameter_nm", Q::Len),
    F("size", "size", Q::Pair),
    F("offset", "offset", Q::Point),
];

const THERMAL: &[F] = &[
    F("spoke", "spoke_nm", Q::Len),
    F("gap", "gap_nm", Q::Len),
    F("angle", "angle_mdeg", Q::Angle),
];

const GRAPHIC: &[F] = &[
    F("layer", "layer", Q::Plain),
    F("type", "type", Q::Plain),
    F("start", "start", Q::Point),
    F("mid", "mid", Q::Point),
    F("end", "end", Q::Point),
    F("center", "center", Q::Point),
    F("points", "points", Q::Points),
    F("width", "width_nm", Q::Len),
    F("radius", "radius_nm", Q::Len),
    F("fill", "fill", Q::Plain),
    F("text", "text", Q::Plain),
    F("at", "at", Q::Point),
    F("rotation", "rotation_mdeg", Q::Angle),
    F("size", "size_nm", Q::Pair),
    F("thickness", "thickness_nm", Q::Len),
    F("hidden", "hidden", Q::Plain),
];

const KEEPOUT: &[F] = &[
    F("polygon", "polygon", Q::Points),
    F("keeps", "keeps", Q::Plain),
    F("layers", "layers", Q::Plain),
];

const FIELD: &[F] = &[
    F("layer", "layer", Q::Plain),
    F("at", "at", Q::Point),
    F("rotation", "rotation_mdeg", Q::Angle),
    F("size", "size_nm", Q::Pair),
    F("thickness", "thickness_nm", Q::Len),
    F("hidden", "hidden", Q::Plain),
];

const MODEL: &[F] = &[
    F("offset", "offset_nm", Q::Lens3),
    F("rotation", "rotation_mdeg", Q::Angles3),
    F("scale_ppm", "scale_ppm", Q::Plain),
];

/// The field an op names: `reference` or `value`.
fn field_slot<'a>(
    f: &'a mut Footprint,
    v: &Value,
    i: usize,
) -> Result<&'a mut Option<crate::footprint::Field>> {
    match v.as_str() {
        Some("reference") => Ok(&mut f.fields.reference),
        Some("value") => Ok(&mut f.fields.value),
        _ => Err(op_error(i, "field", "field is reference or value")),
    }
}

/// The model an op names, by its index.
fn model_index(f: &Footprint, v: &Value, i: usize) -> Result<usize> {
    let n = v
        .as_u64()
        .ok_or_else(|| op_error(i, "model", "model is an index into the models, from 0"))?
        as usize;
    if n >= f.models.len() {
        return Err(
            Error::new(Code::NotFound, format!("the footprint has no model {n}"))
                .with("pointer", format!("/ops/{i}/model")),
        );
    }
    Ok(n)
}

/// A model's file map, `path` turned into its STEP reference.
fn model_map(
    mut into: Map<String, Value>,
    o: &Map<String, Value>,
    i: usize,
    hash: &dyn Fn(&str) -> Result<String>,
) -> Result<Map<String, Value>> {
    if let Some(p) = o.get("path") {
        let path = p
            .as_str()
            .ok_or_else(|| op_error(i, "path", "path is the model's file, as text"))?;
        let blake3 = hash(path).map_err(|e| e.with("pointer", format!("/ops/{i}/path")))?;
        into.insert(
            "model".into(),
            json!({"step": {"path": path, "blake3": blake3}}),
        );
    }
    Ok(into)
}

fn names(table: &[F]) -> impl Iterator<Item = &'static str> + '_ {
    table.iter().map(|f| f.0)
}

/// One value converted, `ptr` its place for errors.
fn conv(v: &Value, q: Q, unit: Unit, ptr: &str) -> Result<Value> {
    let at = |e: Error| e.with("pointer", ptr.to_owned());
    let list = |v: &Value, n: usize, f: &dyn Fn(&Value) -> Result<i64>| -> Result<Value> {
        let a = v.as_array().filter(|a| a.len() == n).ok_or_else(|| {
            Error::new(Code::InvalidParam, format!("a list of {n}")).with("pointer", ptr.to_owned())
        })?;
        Ok(json!(a.iter().map(f).collect::<Result<Vec<i64>>>()?))
    };
    Ok(match q {
        Q::Plain => v.clone(),
        Q::Point | Q::Pair => json!(coord::point(v, unit).map_err(at)?),
        Q::Points => {
            let a = v.as_array().ok_or_else(|| {
                Error::new(Code::InvalidParam, "a list of [x, y]").with("pointer", ptr.to_owned())
            })?;
            json!(
                a.iter()
                    .map(|p| coord::point(p, unit))
                    .collect::<Result<Vec<_>>>()
                    .map_err(at)?
            )
        }
        Q::Len => json!(coord::nm(v, unit).map_err(at)?),
        Q::Angle => json!(coord::mdeg(v).map_err(at)?),
        Q::Lens3 => list(v, 3, &|x| coord::nm(x, unit)).map_err(at)?,
        Q::Angles3 => list(v, 3, &|x| coord::mdeg(x)).map_err(at)?,
        Q::Drill | Q::Thermal => {
            let (table, what) = match q {
                Q::Drill => (DRILL, "drill"),
                _ => (THERMAL, "thermal"),
            };
            let o = v.as_object().ok_or_else(|| {
                Error::new(Code::InvalidParam, format!("a {what} is an object"))
                    .with("pointer", ptr.to_owned())
            })?;
            if let Some(k) = o.keys().find(|k| !names(table).any(|n| n == k.as_str())) {
                return Err(Error::new(
                    Code::InvalidParam,
                    format!(
                        "a {what} takes no {k} (it takes {})",
                        names(table).collect::<Vec<_>>().join(", ")
                    ),
                )
                .with("pointer", format!("{ptr}/{k}")));
            }
            let mut into = Map::new();
            fields_into(o, table, unit, ptr, &mut into)?;
            Value::Object(into)
        }
        Q::Shape => match v {
            Value::String(t) => json!({"type": t}),
            other => other.clone(),
        },
    })
}

/// Writes `o`'s fields of `table` into `into` as the file has them; a
/// `null` takes the field off.
fn fields_into(
    o: &Map<String, Value>,
    table: &[F],
    unit: Unit,
    ptr: &str,
    into: &mut Map<String, Value>,
) -> Result<()> {
    for F(name, key, q) in table {
        let Some(v) = o.get(*name) else { continue };
        if v.is_null() {
            into.remove(*key);
            continue;
        }
        into.insert(
            (*key).to_owned(),
            conv(v, *q, unit, &format!("{ptr}/{name}"))?,
        );
    }
    Ok(())
}

/// The object an op names: its key and its value.
fn object(o: &Map<String, Value>, i: usize) -> Result<(&'static str, &Value)> {
    let mut found = ["pad", "graphic", "keepout", "field", "model"]
        .into_iter()
        .filter_map(|k| o.get(k).map(|v| (k, v)));
    match (found.next(), found.next()) {
        (Some(one), None) => Ok(one),
        _ => Err(op_error(
            i,
            "",
            "an op names one pad, graphic, keepout, field or model",
        )),
    }
}

/// An existing object of `m` by the ID at `key`.
fn existing<T>(m: &BTreeMap<Id, T>, v: &Value, i: usize, key: &str) -> Result<Id> {
    let text = v
        .as_str()
        .ok_or_else(|| op_error(i, key, format!("{key} is an ID, as text")))?;
    let id = Id::parse(text, key).map_err(|e| e.with("pointer", format!("/ops/{i}/{key}")))?;
    if !m.contains_key(&id) {
        return Err(
            Error::new(Code::NotFound, format!("the footprint has no {key} {id}"))
                .with(key, &id)
                .with("pointer", format!("/ops/{i}/{key}")),
        );
    }
    Ok(id)
}

/// A new object's ID: the one given, which must be free, or the next.
fn fresh<T>(m: &BTreeMap<Id, T>, v: &Value, i: usize, key: &str, prefix: &str) -> Result<Id> {
    if v.is_null() {
        return next_id(m, prefix).ok_or_else(|| no_next_id(i, key, prefix));
    }
    let text = v.as_str().ok_or_else(|| {
        op_error(
            i,
            key,
            format!("{key} is a new ID, as text, or null for the next"),
        )
    })?;
    let id = Id::parse(text, key).map_err(|e| e.with("pointer", format!("/ops/{i}/{key}")))?;
    if m.contains_key(&id) {
        return Err(Error::new(
            Code::DuplicateId,
            format!("the footprint already has {key} {id}"),
        )
        .with("pointer", format!("/ops/{i}/{key}")));
    }
    Ok(id)
}

/// An object as its file's map.
fn as_map<T: serde::Serialize>(t: &T) -> Map<String, Value> {
    match serde_json::to_value(t).expect("footprints serialize") {
        Value::Object(m) => m,
        _ => unreachable!("a footprint object is an object"),
    }
}

/// Applies a batch of edits to `f`, in order, all or nothing. Lengths
/// typed as bare numbers are in `unit`. `hash` gives a model file's
/// BLAKE3 by its path in the project.
pub fn apply(
    f: &mut Footprint,
    ops: &[Value],
    unit: Unit,
    hash: &dyn Fn(&str) -> Result<String>,
) -> Result<Applied> {
    let mut t = f.clone();
    let applied = apply_to(&mut t, ops, unit, hash)?;
    *f = t;
    Ok(applied)
}

fn apply_to(
    f: &mut Footprint,
    ops: &[Value],
    unit: Unit,
    hash: &dyn Fn(&str) -> Result<String>,
) -> Result<Applied> {
    let mut applied = Applied::default();
    for (i, op) in ops.iter().enumerate() {
        let o = op
            .as_object()
            .ok_or_else(|| op_error(i, "", "an op is an object such as {\"op\": \"add\", ...}"))?;
        let verb = o.get("op").and_then(Value::as_str);
        let (what, v) = object(o, i)?;
        let ptr = format!("/ops/{i}");
        match (verb, what) {
            (Some("add"), "pad") => {
                let mut fields = vec!["op", "pad"];
                fields.extend(names(PAD));
                only(o, i, &fields)?;
                let id = fresh(&f.pads, v, i, "pad", "p")?;
                let mut into = Map::new();
                fields_into(o, PAD, unit, &ptr, &mut into)?;
                f.pads.insert(id.clone(), typed(into, i, "pad")?);
                applied.added.push(id);
            }
            (Some("set"), "pad") => {
                let mut fields = vec!["op", "pad"];
                fields.extend(names(PAD).filter(|n| *n != "at"));
                only(o, i, &fields)?;
                let id = existing(&f.pads, v, i, "pad")?;
                let mut into = as_map(&f.pads[&id]);
                fields_into(o, PAD, unit, &ptr, &mut into)?;
                f.pads.insert(id, typed(into, i, "pad")?);
            }
            (Some("move"), "pad") => {
                only(o, i, &["op", "pad", "at", "by"])?;
                let id = existing(&f.pads, v, i, "pad")?;
                let p = f.pads.get_mut(&id).expect("found");
                p.at = match (o.get("at"), o.get("by")) {
                    (Some(at), None) => coord::point(at, unit)
                        .map_err(|e| e.with("pointer", format!("{ptr}/at")))?,
                    (None, Some(_)) => {
                        let d = offset(o, i, unit)?;
                        moved(p.at, d, i)?
                    }
                    _ => return Err(op_error(i, "", "move a pad to at, or by [dx, dy]")),
                };
            }
            (Some("remove"), "pad") => {
                only(o, i, &["op", "pad"])?;
                let id = existing(&f.pads, v, i, "pad")?;
                f.pads.remove(&id);
            }
            (Some("add"), "graphic") => {
                let mut fields = vec!["op", "graphic"];
                fields.extend(names(GRAPHIC));
                only(o, i, &fields)?;
                let id = fresh(&f.graphics, v, i, "graphic", "g")?;
                let mut into = Map::new();
                fields_into(o, GRAPHIC, unit, &ptr, &mut into)?;
                f.graphics.insert(id.clone(), typed(into, i, "graphic")?);
                applied.added.push(id);
            }
            (Some("set"), "graphic") => {
                let mut fields = vec!["op", "graphic"];
                fields.extend(names(GRAPHIC));
                only(o, i, &fields)?;
                let id = existing(&f.graphics, v, i, "graphic")?;
                let mut into = as_map(&f.graphics[&id]);
                fields_into(o, GRAPHIC, unit, &ptr, &mut into)?;
                f.graphics.insert(id, typed(into, i, "graphic")?);
            }
            (Some("move"), "graphic") => {
                only(o, i, &["op", "graphic", "by"])?;
                let id = existing(&f.graphics, v, i, "graphic")?;
                let d = offset(o, i, unit)?;
                for p in f.graphics.get_mut(&id).expect("found").shape.points_mut() {
                    *p = moved(*p, d, i)?;
                }
            }
            (Some("remove"), "graphic") => {
                only(o, i, &["op", "graphic"])?;
                let id = existing(&f.graphics, v, i, "graphic")?;
                f.graphics.remove(&id);
            }
            (Some("add"), "keepout") => {
                let mut fields = vec!["op", "keepout"];
                fields.extend(names(KEEPOUT));
                only(o, i, &fields)?;
                let id = fresh(&f.keepouts, v, i, "keepout", "k")?;
                let mut into = Map::new();
                fields_into(o, KEEPOUT, unit, &ptr, &mut into)?;
                f.keepouts.insert(id.clone(), typed(into, i, "keepout")?);
                applied.added.push(id);
            }
            (Some("set"), "keepout") => {
                let mut fields = vec!["op", "keepout"];
                fields.extend(names(KEEPOUT));
                only(o, i, &fields)?;
                let id = existing(&f.keepouts, v, i, "keepout")?;
                let mut into = as_map(&f.keepouts[&id]);
                fields_into(o, KEEPOUT, unit, &ptr, &mut into)?;
                f.keepouts.insert(id, typed(into, i, "keepout")?);
            }
            (Some("move"), "keepout") => {
                only(o, i, &["op", "keepout", "by"])?;
                let id = existing(&f.keepouts, v, i, "keepout")?;
                let d = offset(o, i, unit)?;
                for p in &mut f.keepouts.get_mut(&id).expect("found").polygon {
                    *p = moved(*p, d, i)?;
                }
            }
            (Some("remove"), "keepout") => {
                only(o, i, &["op", "keepout"])?;
                let id = existing(&f.keepouts, v, i, "keepout")?;
                f.keepouts.remove(&id);
            }
            (Some("add"), "field") => {
                let mut fields = vec!["op", "field"];
                fields.extend(names(FIELD));
                only(o, i, &fields)?;
                let mut into = Map::new();
                fields_into(o, FIELD, unit, &ptr, &mut into)?;
                let new = typed(into, i, "field")?;
                let slot = field_slot(f, v, i)?;
                if slot.is_some() {
                    return Err(Error::new(
                        Code::DuplicateId,
                        "the footprint already has that field; set it",
                    )
                    .with("pointer", format!("{ptr}/field")));
                }
                *slot = Some(new);
            }
            (Some("set" | "move" | "remove"), "field") => {
                let fields: Vec<&str> = match verb {
                    Some("set") => ["op", "field"].into_iter().chain(names(FIELD)).collect(),
                    Some("move") => vec!["op", "field", "at", "by"],
                    _ => vec!["op", "field"],
                };
                only(o, i, &fields)?;
                let by = if verb == Some("move") && o.contains_key("by") {
                    Some(offset(o, i, unit)?)
                } else {
                    None
                };
                let slot = field_slot(f, v, i)?;
                if slot.is_none() {
                    return Err(
                        Error::new(Code::NotFound, "the footprint has no such field")
                            .with("pointer", format!("{ptr}/field")),
                    );
                }
                if verb == Some("remove") {
                    *slot = None;
                    continue;
                }
                let current = slot.as_mut().expect("checked above");
                match (verb, o.get("at"), by) {
                    (Some("set"), _, _) => {
                        let mut into = as_map(&*current);
                        fields_into(o, FIELD, unit, &ptr, &mut into)?;
                        *current = typed(into, i, "field")?;
                    }
                    (_, Some(at), None) => {
                        current.at = coord::point(at, unit)
                            .map_err(|e| e.with("pointer", format!("{ptr}/at")))?
                    }
                    (_, None, Some(d)) => current.at = moved(current.at, d, i)?,
                    _ => return Err(op_error(i, "", "move a field to at, or by [dx, dy]")),
                }
            }
            (Some("add"), "model") => {
                let mut fields = vec!["op", "model", "path"];
                fields.extend(names(MODEL));
                only(o, i, &fields)?;
                if !v.is_null() {
                    return Err(op_error(
                        i,
                        "model",
                        "a new model's index is null: it goes last",
                    ));
                }
                if !o.contains_key("path") {
                    return Err(op_error(i, "path", "a model names its file: path"));
                }
                let mut into = Map::new();
                fields_into(o, MODEL, unit, &ptr, &mut into)?;
                let into = model_map(into, o, i, hash)?;
                f.models.push(typed(into, i, "model")?);
            }
            (Some("set"), "model") => {
                let mut fields = vec!["op", "model", "path"];
                fields.extend(names(MODEL));
                only(o, i, &fields)?;
                let n = model_index(f, v, i)?;
                let mut into = as_map(&f.models[n]);
                fields_into(o, MODEL, unit, &ptr, &mut into)?;
                let into = model_map(into, o, i, hash)?;
                f.models[n] = typed(into, i, "model")?;
            }
            (Some("remove"), "model") => {
                only(o, i, &["op", "model"])?;
                let n = model_index(f, v, i)?;
                f.models.remove(n);
            }
            (Some("move"), "model") => {
                return Err(op_error(
                    i,
                    "op",
                    "a model is placed by set offset, not moved",
                ));
            }
            _ => return Err(op_error(i, "op", "op is add, remove, set or move")),
        }
    }
    Ok(applied)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::footprint::{PadKind, PadShape};

    pub(crate) fn empty() -> Footprint {
        serde_json::from_value(json!({"name": "T"})).unwrap()
    }

    fn no_models(_: &str) -> Result<String> {
        Err(Error::new(Code::NotFound, "no models here"))
    }

    pub(crate) fn ok(f: &mut Footprint, ops: Value) -> Applied {
        apply(f, ops.as_array().unwrap(), Unit::Mm, &no_models).unwrap()
    }

    pub(crate) fn fails(f: &Footprint, ops: Value) -> (Code, String) {
        let mut t = f.clone();
        let e = apply(&mut t, ops.as_array().unwrap(), Unit::Mm, &no_models).unwrap_err();
        // Every refusal's details are its code's declared keys (design
        // 698): the footprint fuzz target found `pad` undeclared.
        crate::error::details::check(&e);
        let at = e.details["pointer"].as_str().unwrap_or("").to_owned();
        (e.code, at)
    }

    #[test]
    fn pads_are_added_set_moved_and_removed() {
        let mut f = empty();
        let a = ok(
            &mut f,
            json!([
                {"op": "add", "pad": null, "number": "1", "kind": "smd",
                 "shape": {"type": "roundrect", "ratio_ppm": 250000},
                 "at": ["-0.51", 0], "size": ["0.54", "0.64"],
                 "layers": ["copper.top", "mask.top", "paste.top"]},
                {"op": "add", "pad": "ep", "number": "3", "kind": "through", "shape": "circle",
                 "at": [0, "2 mm"], "size": ["1.7", "1.7"], "rotation": "90 deg",
                 "drill": {"shape": "round", "diameter": "1"}, "layers": ["copper.all"],
                 "thermal": {"spoke": "0.3", "gap": "0.25"}, "mask_margin": "0.05"},
                {"op": "set", "pad": "p1", "number": "2", "size": ["0.6", "0.64"]},
                {"op": "move", "pad": "p1", "by": ["0.01", 0]},
                {"op": "move", "pad": "ep", "at": [0, "2.5"]}
            ]),
        );
        assert_eq!(
            a.added.iter().map(Id::as_str).collect::<Vec<_>>(),
            ["p1", "ep"]
        );
        let p1 = &f.pads[&Id::parse("p1", "pad").unwrap()];
        assert_eq!(p1.number.as_deref(), Some("2"));
        assert_eq!((p1.at, p1.size), ([-500_000, 0], [600_000, 640_000]));
        assert_eq!(p1.shape, PadShape::Roundrect { ratio_ppm: 250_000 });
        let ep = &f.pads[&Id::parse("ep", "pad").unwrap()];
        assert_eq!(
            (ep.kind, ep.at, ep.rotation_mdeg),
            (PadKind::Through, [0, 2_500_000], 90_000)
        );
        assert_eq!(ep.margins.mask_margin_nm, Some(50_000));
        assert_eq!(
            ep.thermal.as_ref().map(|t| (t.spoke_nm, t.gap_nm)),
            Some((300_000, 250_000))
        );
        // `null` takes an optional field off.
        ok(
            &mut f,
            json!([{"op": "set", "pad": "ep", "thermal": null, "number": null}]),
        );
        let ep = &f.pads[&Id::parse("ep", "pad").unwrap()];
        assert_eq!((ep.thermal.is_none(), ep.number.is_none()), (true, true));
        ok(&mut f, json!([{"op": "remove", "pad": "ep"}]));
        assert_eq!(f.pads.len(), 1);
        // The next free ID after p1 is p2.
        let a = ok(
            &mut f,
            json!([{"op": "add", "pad": null, "number": "1", "kind": "smd",
            "shape": "rect", "at": [0, 0], "size": ["1", "1"], "layers": ["copper.top"]}]),
        );
        assert_eq!(a.added[0].as_str(), "p2");
    }

    #[test]
    fn bad_pad_ops_say_where() {
        let mut f = empty();
        ok(
            &mut f,
            json!([{"op": "add", "pad": "p1", "number": "1", "kind": "smd", "shape": "rect",
            "at": [0, 0], "size": ["1", "1"], "layers": ["copper.top"]}]),
        );
        assert_eq!(
            fails(
                &f,
                json!([{"op": "add", "pad": "p1", "kind": "smd", "shape": "rect",
            "at": [0, 0], "size": [1, 1], "layers": []}])
            ),
            (Code::DuplicateId, "/ops/0/pad".into())
        );
        assert_eq!(
            fails(&f, json!([{"op": "remove", "pad": "p9"}])),
            (Code::NotFound, "/ops/0/pad".into())
        );
        assert_eq!(
            fails(&f, json!([{"op": "set", "pad": "p1", "colour": "red"}])),
            (Code::InvalidParam, "/ops/0/colour".into())
        );
        assert_eq!(
            fails(&f, json!([{"op": "set", "pad": "p1", "at": [0, 0]}])),
            (Code::InvalidParam, "/ops/0/at".into())
        );
        assert_eq!(
            fails(
                &f,
                json!([{"op": "move", "pad": "p1", "by": ["1 furlong", 0]}])
            ),
            (Code::InvalidQuantity, "/ops/0/by".into())
        );
        assert_eq!(
            fails(&f, json!([{"op": "set", "pad": "p1", "kind": "blob"}])),
            (Code::InvalidParam, "/ops/0".into())
        );
        assert_eq!(
            fails(&f, json!([{"op": "add"}])),
            (Code::InvalidParam, "/ops/0".into())
        );
        assert_eq!(
            fails(&f, json!([{"op": "explode", "pad": "p1"}])),
            (Code::InvalidParam, "/ops/0/op".into())
        );
        assert_eq!(
            fails(&f, json!([{"op": "remove", "pad": "p1", "graphic": "g1"}])),
            (Code::InvalidParam, "/ops/0".into())
        );
        // All or nothing: the first op's pad isn't kept when the second fails.
        let before = f.clone();
        let mut t = f.clone();
        let r = apply(
            &mut t,
            json!([{"op": "remove", "pad": "p1"}, {"op": "remove", "pad": "p1"}])
                .as_array()
                .unwrap(),
            Unit::Mm,
            &no_models,
        );
        assert!(r.is_err());
        assert_eq!(f, before);
    }
    #[test]
    fn graphics_and_keepouts_are_added_edited_moved_and_removed() {
        let mut f = empty();
        let a = ok(
            &mut f,
            json!([
                {"op": "add", "graphic": null, "layer": "courtyard.top", "type": "rect",
                 "start": ["-0.93", "0.47"], "end": ["0.93", "-0.47"], "width": "0.05"},
                {"op": "add", "graphic": "label", "layer": "fab.top", "type": "text",
                 "text": "${REFERENCE}", "at": [0, 0], "size": ["0.26", "0.26"], "thickness": "0.04"},
                {"op": "add", "keepout": null, "polygon": [[0, 0], ["1", 0], ["1", "1"]],
                 "keeps": ["tracks", "vias"], "layers": ["copper.top"]},
                {"op": "set", "graphic": "g1", "width": "0.1"},
                {"op": "move", "graphic": "g1", "by": ["0.07", 0]},
                {"op": "move", "keepout": "k1", "by": [0, "-1"]}
            ]),
        );
        assert_eq!(
            a.added.iter().map(Id::as_str).collect::<Vec<_>>(),
            ["g1", "label", "k1"]
        );
        let g1 = &f.graphics[&Id::parse("g1", "graphic").unwrap()];
        let crate::footprint::Shape::Rect {
            start, width_nm, ..
        } = &g1.shape
        else {
            panic!("a rect")
        };
        assert_eq!((*start, *width_nm), ([-860_000, 470_000], 100_000));
        let k1 = &f.keepouts[&Id::parse("k1", "keepout").unwrap()];
        assert_eq!(k1.polygon[2], [1_000_000, 0]);
        ok(
            &mut f,
            json!([{"op": "remove", "graphic": "label"}, {"op": "remove", "keepout": "k1"}]),
        );
        assert_eq!((f.graphics.len(), f.keepouts.len()), (1, 0));
        assert_eq!(
            fails(&f, json!([{"op": "set", "graphic": "g1", "type": "blob"}])),
            (Code::InvalidParam, "/ops/0".into())
        );
        assert_eq!(
            fails(&f, json!([{"op": "move", "graphic": "g9", "by": [0, 0]}])),
            (Code::NotFound, "/ops/0/graphic".into())
        );
    }

    #[test]
    fn fields_and_models_are_added_set_moved_and_removed() {
        let mut f = empty();
        let hash = |path: &str| -> Result<String> {
            match path {
                "library/models/r.step" => Ok("a".repeat(64)),
                _ => Err(Error::new(Code::NotFound, format!("no file {path}"))),
            }
        };
        let ops = json!([
            {"op": "add", "field": "reference", "layer": "silk.top", "at": [0, "1.17"],
             "size": ["1", "1"], "thickness": "0.15"},
            {"op": "add", "field": "value", "layer": "fab.top", "at": [0, "-1.17"], "size": ["1", "1"]},
            {"op": "move", "field": "value", "by": [0, "-0.1"]},
            {"op": "set", "field": "value", "hidden": true},
            {"op": "add", "model": null, "path": "library/models/r.step", "rotation": [0, 0, "90 deg"]},
            {"op": "set", "model": 0, "offset": [0, 0, "0.1"]}
        ]);
        apply(&mut f, ops.as_array().unwrap(), Unit::Mm, &hash).unwrap();
        let r = f.fields.reference.as_ref().unwrap();
        assert_eq!(
            (r.at, r.size_nm, r.thickness_nm),
            ([0, 1_170_000], [1_000_000; 2], 150_000)
        );
        let v = f.fields.value.as_ref().unwrap();
        assert_eq!((v.at, v.hidden), ([0, -1_270_000], true));
        assert_eq!(f.models.len(), 1);
        assert_eq!(
            (f.models[0].rotation_mdeg, f.models[0].offset_nm),
            ([0, 0, 90_000], [0, 0, 100_000])
        );
        let crate::footprint::Model::Step(s) = &f.models[0].model;
        assert_eq!(
            (s.path.as_str(), s.blake3.len()),
            ("library/models/r.step", 64)
        );
        ok(
            &mut f,
            json!([{"op": "remove", "field": "value"}, {"op": "remove", "model": 0}]),
        );
        assert!(f.fields.value.is_none() && f.models.is_empty());
        assert_eq!(
            fails(
                &f,
                json!([{"op": "add", "field": "reference", "layer": "silk.top", "at": [0, 0], "size": [1, 1]}])
            ),
            (Code::DuplicateId, "/ops/0/field".into())
        );
        assert_eq!(
            fails(&f, json!([{"op": "set", "field": "value", "hidden": true}])),
            (Code::NotFound, "/ops/0/field".into())
        );
        assert_eq!(
            fails(&f, json!([{"op": "set", "field": "colour"}])),
            (Code::InvalidParam, "/ops/0/field".into())
        );
        assert_eq!(
            fails(&f, json!([{"op": "remove", "model": 3}])),
            (Code::NotFound, "/ops/0/model".into())
        );
        assert_eq!(
            fails(
                &f,
                json!([{"op": "add", "model": null, "path": "library/models/x.step"}])
            )
            .0,
            Code::NotFound
        );
    }

    /// The review's find: a move by a huge offset overflowed (an
    /// `internal` panic in a debug build). Every kind of move is refused
    /// as out of range instead, at its `by`.
    #[test]
    fn a_move_out_of_range_is_refused() {
        let mut f = empty();
        ok(
            &mut f,
            json!([
                {"op": "add", "pad": "p1", "number": "1", "kind": "smd", "shape": "rect", "at": [1, -1], "size": [1000, 1000], "layers": ["copper.top"]},
                {"op": "add", "graphic": "g1", "type": "line", "layer": "silk.top", "start": [1, -1], "end": [2, -1], "width": 100},
                {"op": "add", "keepout": "k1", "polygon": [[1, -1], [2, -1], [2, 2]], "keeps": ["tracks"], "layers": ["copper.top"]},
                {"op": "add", "field": "reference", "layer": "silk.top", "at": [1, -1], "size": [1000, 1000]}
            ]),
        );
        for (key, id) in [
            ("pad", "p1"),
            ("graphic", "g1"),
            ("keepout", "k1"),
            ("field", "reference"),
        ] {
            for by in [json!([i64::MAX, 0]), json!([0, i64::MIN])] {
                let mut t = f.clone();
                let e = apply(
                    &mut t,
                    json!([{"op": "move", key: id, "by": by}])
                        .as_array()
                        .unwrap(),
                    Unit::Mm,
                    &no_models,
                )
                .unwrap_err();
                assert_eq!(
                    (
                        e.code,
                        e.details["pointer"].as_str(),
                        e.details["reason"].as_str()
                    ),
                    (Code::InvalidParam, Some("/ops/0/by"), Some("out_of_range")),
                    "{key}"
                );
                crate::error::details::check(&e);
            }
        }
    }

    /// Round 2 of the review: after `p18446744073709551615` there is no
    /// next `p<n>`; an add asking for one is refused, never an overflow.
    #[test]
    fn no_next_free_id_is_refused() {
        for (key, prefix, fields) in [
            (
                "pad",
                "p",
                json!({"number": "1", "kind": "smd", "shape": "rect", "at": [0, 0], "size": [1000, 1000], "layers": ["copper.top"]}),
            ),
            (
                "graphic",
                "g",
                json!({"type": "line", "layer": "silk.top", "start": [0, 0], "end": [10, 0], "width": 100}),
            ),
            (
                "keepout",
                "k",
                json!({"polygon": [[0, 0], [10, 0], [10, 10]], "keeps": ["tracks"], "layers": ["copper.top"]}),
            ),
        ] {
            let mut last = fields.clone();
            last["op"] = json!("add");
            last[key] = json!(format!("{prefix}{}", u64::MAX));
            let mut next = fields.clone();
            next["op"] = json!("add");
            next[key] = Value::Null;
            let mut f = empty();
            let e = apply(&mut f, &[last, next], Unit::Mm, &no_models).unwrap_err();
            assert_eq!(
                (
                    e.code,
                    e.details["pointer"].as_str(),
                    e.details["reason"].as_str()
                ),
                (
                    Code::InvalidParam,
                    Some(format!("/ops/1/{key}").as_str()),
                    Some("no_next_id")
                ),
                "{key}"
            );
            crate::error::details::check(&e);
        }
    }
}
