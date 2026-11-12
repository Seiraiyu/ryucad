//! Extrudes and revolves: a sketch's loops swept straight or round (ryucad 27.C).

use super::Given;
use crate::doc::files::Feature;
use crate::doc::id::Id;
use crate::doc::sweep::{Extrude, Revolve};
use crate::error::Result;
use crate::expr::Env;
use crate::geometry::geom_json;
use crate::units::Unit;
use serde_json::{Value, json};

pub(crate) const EXTRUDE: [&str; 7] = [
    "sketch",
    "loops",
    "extent",
    "reverse",
    "surface",
    "operation",
    "targets",
];

pub(crate) const REVOLVE: [&str; 8] = [
    "sketch",
    "loops",
    "axis",
    "angle",
    "symmetric",
    "surface",
    "operation",
    "targets",
];

/// An extrude or revolve from its fields: the current ones (for an edit)
/// with those given laid over them.
pub(crate) fn sweep_from(
    ty: &str,
    current: Option<Value>,
    given: &Given,
    part: &Id,
    unit: Unit,
    env: &Env,
) -> Result<Feature> {
    // A new extrude's extent is blind unless it says otherwise.
    let fresh = match ty {
        "extrude" => json!({"extent": {"type": "blind"}}),
        _ => json!({}),
    };
    let mut v = current.unwrap_or(fresh);
    let o = v.as_object_mut().expect("an object");
    if given.fields.operation.as_deref() == Some("new") {
        o.remove("targets");
    }
    for (k, x) in given.fields.given() {
        // No loops is every loop, and false the default: nothing to write.
        if (k == "loops" && x == json!([])) || x == json!(false) {
            o.remove(&k);
            continue;
        }
        let x = match (k.as_str(), x) {
            // An up-to target is given as text, and kept as a file holds it.
            ("extent", Value::Object(mut e)) => {
                if let Some(Value::String(t)) = e.get("target") {
                    let g = geom_json(t, part, "/extent/target")?;
                    e.insert("target".into(), g);
                }
                Value::Object(e)
            }
            // An axis given as `<part>/…` is any axis; a bare ID a line of
            // the swept sketch.
            ("axis", Value::String(t)) if t.contains('/') => geom_json(&t, part, "/axis")?,
            (_, x) => x,
        };
        // Part of an extent keeps the rest; a new type keeps only what it
        // takes (blind and symmetric share a length).
        match (o.get_mut(&k), x) {
            (Some(Value::Object(old)), Value::Object(new)) if k == "extent" => {
                if let Some(t) = new.get("type").and_then(Value::as_str) {
                    let takes: &[&str] = match t {
                        "up_to" => &["target", "offset"],
                        "through_all" => &["both"],
                        _ => &["length"],
                    };
                    old.retain(|f, _| takes.contains(&f.as_str()));
                }
                old.extend(new)
            }
            (_, x) => {
                o.insert(k, x);
            }
        }
    }
    let mut o = crate::doc::files::Obj::new(&v, "")?;
    let f = match ty {
        "extrude" => Feature::Extrude(Extrude::from_fields(&mut o, unit, env)?),
        _ => Feature::Revolve(Revolve::from_fields(&mut o, env)?),
    };
    o.finish()?;
    Ok(f)
}
