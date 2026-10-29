//! Batch edits of a symbol: `add`, `remove`, `set` and `move` ops on its
//! pins (by number) and graphics (by ID), applied in order, all or
//! nothing, as ryucad's `sketch.edit` applies its ops. Ops take lengths
//! and angles as quantities (`"2.54 mm"`, `"90 deg"`; whole numbers are
//! nanometres and millidegrees) under names without the file's unit
//! suffixes: `at`, `length`, `radius`, `size`, `rotation`.

use crate::coord;
use crate::error::{Code, Error, Result};
use crate::id::Id;
use crate::symbol::{Graphic, Pin, Symbol};
use crate::units::Unit;
use serde_json::{Map, Value, json};

/// A pin's length when an `add` gives none: KiCad's 100 mil.
pub const PIN_LENGTH_NM: i64 = 2_540_000;

/// What a batch did besides changing the symbol.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Applied {
    /// The graphic IDs `add` ops gave or were given, in order.
    pub added: Vec<Id>,
    /// Pin numbers removed or renumbered: what library components map.
    pub pins_gone: Vec<String>,
}

fn op_error(i: usize, field: &str, msg: impl Into<String>) -> Error {
    let ptr = if field.is_empty() {
        format!("/ops/{i}")
    } else {
        format!("/ops/{i}/{field}")
    };
    Error::new(Code::InvalidParam, msg).with("pointer", ptr)
}

/// Refuses fields an op doesn't take.
fn only(o: &Map<String, Value>, i: usize, fields: &[&str]) -> Result<()> {
    match o.keys().find(|k| !fields.contains(&k.as_str())) {
        None => Ok(()),
        Some(k) => {
            let accepts: Vec<&str> = fields.iter().copied().filter(|f| *f != "op").collect();
            Err(op_error(
                i,
                k,
                format!("this op takes no {k} (it takes {})", accepts.join(", ")),
            )
            .with("accepts", accepts))
        }
    }
}

/// The fields a pin op takes as the file has them, quantities converted:
/// `length` becomes `length_nm`, `at` nanometres.
const PIN_FIELDS: [&str; 7] = ["name", "type", "unit", "at", "side", "length", "shape"];

/// The fields a graphic op takes: `radius`, `size` and `rotation` become
/// `radius_nm`, `size_nm` and `rotation_mdeg`.
const GRAPHIC_FIELDS: [&str; 12] = [
    "start", "mid", "end", "points", "center", "radius", "fill", "unit", "text", "at", "size",
    "rotation",
];

/// Writes the op's fields of `names` into `into` as the file has them.
fn file_fields(
    o: &Map<String, Value>,
    i: usize,
    names: &[&str],
    unit: Unit,
    into: &mut Map<String, Value>,
) -> Result<()> {
    let at = |f: &str| {
        let ptr = format!("/ops/{i}/{f}");
        move |e: Error| e.with("pointer", ptr)
    };
    for name in names {
        let Some(v) = o.get(*name) else { continue };
        let (key, value) = match *name {
            "at" | "start" | "mid" | "end" | "center" => {
                (*name, json!(coord::point(v, unit).map_err(at(name))?))
            }
            "points" => {
                let list = v
                    .as_array()
                    .ok_or_else(|| op_error(i, name, "points is a list of [x, y]"))?;
                let points = list
                    .iter()
                    .map(|p| coord::point(p, unit))
                    .collect::<Result<Vec<_>>>()
                    .map_err(at(name))?;
                ("points", json!(points))
            }
            "length" => ("length_nm", json!(coord::nm(v, unit).map_err(at(name))?)),
            "radius" => ("radius_nm", json!(coord::nm(v, unit).map_err(at(name))?)),
            "size" => ("size_nm", json!(coord::nm(v, unit).map_err(at(name))?)),
            "rotation" => ("rotation_mdeg", json!(coord::mdeg(v).map_err(at(name))?)),
            // `"unit": null` makes it common to every unit.
            "unit" if v.is_null() => {
                into.remove("unit");
                continue;
            }
            n => (n, v.clone()),
        };
        into.insert(key.to_owned(), value);
    }
    Ok(())
}

/// `into` read as a `T`, or the op's fault.
fn typed<T: serde::de::DeserializeOwned>(
    into: Map<String, Value>,
    i: usize,
    what: &str,
) -> Result<T> {
    serde_json::from_value(Value::Object(into))
        .map_err(|e| op_error(i, "", format!("not a {what}: {e}")))
}

fn pin_number(o: &Map<String, Value>, i: usize, field: &str) -> Result<String> {
    o.get(field)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| op_error(i, field, format!("{field} is a pin number, as text")))
}

fn graphic_id(o: &Map<String, Value>, i: usize, s: &Symbol) -> Result<Id> {
    let text = o
        .get("graphic")
        .and_then(Value::as_str)
        .ok_or_else(|| op_error(i, "graphic", "graphic is a graphic's ID (g1)"))?;
    let id =
        Id::parse(text, "graphic").map_err(|e| e.with("pointer", format!("/ops/{i}/graphic")))?;
    if !s.graphics.contains_key(&id) {
        return Err(
            Error::new(Code::NotFound, format!("the symbol has no graphic {id}"))
                .with("graphic", &id)
                .with("pointer", format!("/ops/{i}/graphic")),
        );
    }
    Ok(id)
}

fn existing_pin(o: &Map<String, Value>, i: usize, s: &Symbol) -> Result<String> {
    let n = pin_number(o, i, "pin")?;
    if !s.pins.contains_key(&n) {
        return Err(
            Error::new(Code::PinNotFound, format!("the symbol has no pin {n}"))
                .with("pin", &n)
                .with("pointer", format!("/ops/{i}/pin")),
        );
    }
    Ok(n)
}

fn offset(o: &Map<String, Value>, i: usize, unit: Unit) -> Result<[i64; 2]> {
    let by = o
        .get("by")
        .ok_or_else(|| op_error(i, "by", "move by [dx, dy]"))?;
    coord::point(by, unit).map_err(|e| e.with("pointer", format!("/ops/{i}/by")))
}

/// Applies a batch of edits to `s`, in order. Lengths typed as bare
/// numbers are in `unit`.
pub fn apply(s: &mut Symbol, ops: &[Value], unit: Unit) -> Result<Applied> {
    let mut applied = Applied::default();
    for (i, op) in ops.iter().enumerate() {
        let o = op
            .as_object()
            .ok_or_else(|| op_error(i, "", "an op is an object such as {\"op\": \"add\", ...}"))?;
        let verb = o.get("op").and_then(Value::as_str);
        let on_pin = o.contains_key("pin");
        if on_pin == o.contains_key("graphic") {
            return Err(op_error(i, "", "an op names a pin or a graphic"));
        }
        match (verb, on_pin) {
            (Some("add"), true) => {
                let mut fields: Vec<&str> = vec!["op", "pin", "alternates"];
                fields.extend(PIN_FIELDS);
                only(o, i, &fields)?;
                let n = pin_number(o, i, "pin")?;
                if s.pins.contains_key(&n) {
                    return Err(Error::new(
                        Code::DuplicateId,
                        format!("the symbol already has pin {n}"),
                    )
                    .with("pin", &n)
                    .with("pointer", format!("/ops/{i}/pin")));
                }
                let mut into = Map::new();
                into.insert("name".into(), json!("~"));
                into.insert("length_nm".into(), json!(PIN_LENGTH_NM));
                file_fields(o, i, &PIN_FIELDS, unit, &mut into)?;
                if let Some(a) = o.get("alternates") {
                    into.insert("alternates".into(), a.clone());
                }
                let pin: Pin = typed(into, i, "pin")?;
                s.pins.insert(n, pin);
            }
            (Some("add"), false) => {
                let mut fields: Vec<&str> = vec!["op", "graphic", "id"];
                fields.extend(GRAPHIC_FIELDS);
                only(o, i, &fields)?;
                let ty = o.get("graphic").and_then(Value::as_str).ok_or_else(|| {
                    op_error(
                        i,
                        "graphic",
                        "add a graphic of a type: rect, polyline, arc, circle, bezier or text",
                    )
                })?;
                let id = match o.get("id") {
                    Some(v) => {
                        let id = v
                            .as_str()
                            .ok_or_else(|| op_error(i, "id", "id is text"))
                            .and_then(|t| {
                                Id::parse(t, "graphic")
                                    .map_err(|e| e.with("pointer", format!("/ops/{i}/id")))
                            })?;
                        if s.graphics.contains_key(&id) {
                            return Err(Error::new(
                                Code::DuplicateId,
                                format!("the symbol already has {id}"),
                            )
                            .with("pointer", format!("/ops/{i}/id")));
                        }
                        id
                    }
                    None => s.next_graphic(),
                };
                let mut into = Map::new();
                into.insert("type".into(), json!(ty));
                file_fields(o, i, &GRAPHIC_FIELDS, unit, &mut into)?;
                let g: Graphic = typed(into, i, "graphic")?;
                s.graphics.insert(id.clone(), g);
                applied.added.push(id);
            }
            (Some("remove"), true) => {
                only(o, i, &["op", "pin"])?;
                let n = existing_pin(o, i, s)?;
                s.pins.remove(&n);
                applied.pins_gone.push(n);
            }
            (Some("remove"), false) => {
                only(o, i, &["op", "graphic"])?;
                let id = graphic_id(o, i, s)?;
                s.graphics.remove(&id);
            }
            (Some("set"), true) => {
                let mut fields: Vec<&str> = vec!["op", "pin", "number", "alternates"];
                fields.extend(PIN_FIELDS.iter().filter(|f| **f != "at"));
                only(o, i, &fields)?;
                let n = existing_pin(o, i, s)?;
                let mut into = match serde_json::to_value(&s.pins[&n]).expect("pins serialize") {
                    Value::Object(m) => m,
                    _ => unreachable!("a pin is an object"),
                };
                file_fields(o, i, &PIN_FIELDS, unit, &mut into)?;
                if let Some(a) = o.get("alternates") {
                    into.insert("alternates".into(), a.clone());
                }
                let pin: Pin = typed(into, i, "pin")?;
                let to = match o.get("number") {
                    Some(_) => pin_number(o, i, "number")?,
                    None => n.clone(),
                };
                if to != n {
                    if s.pins.contains_key(&to) {
                        return Err(Error::new(
                            Code::DuplicateId,
                            format!("the symbol already has pin {to}"),
                        )
                        .with("pin", &to)
                        .with("pointer", format!("/ops/{i}/number")));
                    }
                    s.pins.remove(&n);
                    applied.pins_gone.push(n);
                }
                s.pins.insert(to, pin);
            }
            (Some("set"), false) => {
                let mut fields: Vec<&str> = vec!["op", "graphic"];
                fields.extend(GRAPHIC_FIELDS);
                only(o, i, &fields)?;
                let id = graphic_id(o, i, s)?;
                let mut into =
                    match serde_json::to_value(&s.graphics[&id]).expect("graphics serialize") {
                        Value::Object(m) => m,
                        _ => unreachable!("a graphic is an object"),
                    };
                file_fields(o, i, &GRAPHIC_FIELDS, unit, &mut into)?;
                let g: Graphic = typed(into, i, "graphic")?;
                s.graphics.insert(id, g);
            }
            (Some("move"), true) => {
                only(o, i, &["op", "pin", "at", "by"])?;
                let n = existing_pin(o, i, s)?;
                let pin = s.pins.get_mut(&n).expect("found");
                match (o.get("at"), o.get("by")) {
                    (Some(at), None) => {
                        pin.at = coord::point(at, unit)
                            .map_err(|e| e.with("pointer", format!("/ops/{i}/at")))?
                    }
                    (None, Some(_)) => {
                        let d = offset(o, i, unit)?;
                        pin.at = [pin.at[0] + d[0], pin.at[1] + d[1]];
                    }
                    _ => return Err(op_error(i, "", "move a pin to at, or by [dx, dy]")),
                }
            }
            (Some("move"), false) => {
                only(o, i, &["op", "graphic", "by"])?;
                let id = graphic_id(o, i, s)?;
                let d = offset(o, i, unit)?;
                for p in s.graphics.get_mut(&id).expect("found").points_mut() {
                    *p = [p[0] + d[0], p[1] + d[1]];
                }
            }
            _ => return Err(op_error(i, "op", "op is add, remove, set or move")),
        }
    }
    Ok(applied)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::symbol::tests::resistor;
    use crate::symbol::{Fill, PinType, Side};

    fn ok(s: &mut Symbol, ops: Value) -> Applied {
        apply(s, ops.as_array().unwrap(), Unit::Mm).unwrap()
    }

    fn fails(s: &Symbol, ops: Value) -> Error {
        let mut t = s.clone();
        apply(&mut t, ops.as_array().unwrap(), Unit::Mm).unwrap_err()
    }

    #[test]
    fn pins_are_added_set_renumbered_moved_and_removed() {
        let mut s = resistor();
        ok(
            &mut s,
            json!([
                {"op": "add", "pin": "3", "name": "SENSE", "type": "input", "at": ["2.54", "0"], "side": "left"},
                {"op": "set", "pin": "3", "number": "S", "shape": "inverted", "length": "100 mil"},
                {"op": "move", "pin": "S", "by": [0, "-1.27 mm"]}
            ]),
        );
        let p = &s.pins["S"];
        assert_eq!(
            (p.name.as_str(), p.ty, p.side),
            ("SENSE", PinType::Input, Side::Left)
        );
        assert_eq!((p.at, p.length_nm), ([2_540_000, -1_270_000], 2_540_000));
        assert!(!s.pins.contains_key("3"));
        let a = ok(
            &mut s,
            json!([{"op": "remove", "pin": "S"}, {"op": "set", "pin": "1", "unit": 1}]),
        );
        assert_eq!(a.pins_gone, ["S"]);
        assert_eq!(s.pins["1"].unit, Some(1));
        ok(&mut s, json!([{"op": "set", "pin": "1", "unit": null}]));
        assert_eq!(s.pins["1"].unit, None);
    }

    #[test]
    fn graphics_are_added_with_the_next_id_and_edited() {
        let mut s = resistor();
        let a = ok(
            &mut s,
            json!([
                {"op": "add", "graphic": "circle", "center": [0, 0], "radius": "0.5 mm", "fill": "body"},
                {"op": "add", "graphic": "text", "id": "label", "text": "R", "at": [0, 0], "size": "1.27", "rotation": "90"},
                {"op": "add", "graphic": "polyline", "points": [[0, 0], ["1", "1"]]},
                {"op": "move", "graphic": "g1", "by": ["1 mm", 0]},
                {"op": "set", "graphic": "g2", "radius": 250000}
            ]),
        );
        assert_eq!(
            a.added.iter().map(Id::as_str).collect::<Vec<_>>(),
            ["g2", "label", "g3"]
        );
        assert_eq!(
            s.graphics[&Id::parse("g2", "g").unwrap()],
            Graphic::Circle {
                center: [0, 0],
                radius_nm: 250_000,
                fill: Fill::Body,
                unit: None
            }
        );
        let Graphic::Text {
            rotation_mdeg,
            size_nm,
            ..
        } = &s.graphics[&Id::parse("label", "g").unwrap()]
        else {
            panic!("a text");
        };
        assert_eq!((*rotation_mdeg, *size_nm), (90_000, 1_270_000));
        let Graphic::Rect { start, .. } = &s.graphics[&Id::parse("g1", "g").unwrap()] else {
            panic!("a rect");
        };
        assert_eq!(start[0], -16_000);
        ok(&mut s, json!([{"op": "remove", "graphic": "label"}]));
        assert_eq!(s.graphics.len(), 3);
    }

    #[test]
    fn bad_ops_say_where() {
        let s = resistor();
        let at = |e: Error| {
            (
                e.code,
                e.details["pointer"].as_str().unwrap_or("").to_owned(),
            )
        };
        assert_eq!(
            at(fails(
                &s,
                json!([{"op": "add", "pin": "1", "type": "input", "at": [0, 0], "side": "up"}])
            )),
            (Code::DuplicateId, "/ops/0/pin".into())
        );
        assert_eq!(
            at(fails(&s, json!([{"op": "remove", "pin": "9"}]))),
            (Code::PinNotFound, "/ops/0/pin".into())
        );
        assert_eq!(
            at(fails(&s, json!([{"op": "remove", "graphic": "g9"}]))),
            (Code::NotFound, "/ops/0/graphic".into())
        );
        assert_eq!(
            at(fails(&s, json!([{}, {"op": "explode", "pin": "1"}]))),
            (Code::InvalidParam, "/ops/0".into())
        );
        assert_eq!(
            at(fails(&s, json!([{"op": "explode", "pin": "1"}]))),
            (Code::InvalidParam, "/ops/0/op".into())
        );
        assert_eq!(
            at(fails(
                &s,
                json!([{"op": "set", "pin": "1", "colour": "red"}])
            )),
            (Code::InvalidParam, "/ops/0/colour".into())
        );
        assert_eq!(
            at(fails(&s, json!([{"op": "set", "pin": "1", "at": [0, 0]}]))),
            (Code::InvalidParam, "/ops/0/at".into())
        );
        assert_eq!(
            at(fails(&s, json!([{"op": "add", "graphic": "blob"}]))),
            (Code::InvalidParam, "/ops/0".into())
        );
        assert_eq!(
            at(fails(
                &s,
                json!([{"op": "move", "graphic": "g1", "by": ["1 furlong", 0]}])
            )),
            (Code::InvalidQuantity, "/ops/0/by".into())
        );
        assert_eq!(
            at(fails(&s, json!([{"op": "set", "pin": "1", "number": "2"}]))),
            (Code::DuplicateId, "/ops/0/number".into())
        );
        assert_eq!(
            at(fails(
                &s,
                json!([{"op": "remove", "pin": "1", "graphic": "g1"}])
            )),
            (Code::InvalidParam, "/ops/0".into())
        );
    }
}
