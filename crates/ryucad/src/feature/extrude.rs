//! Extrudes and revolves: a sketch's loops swept straight or round (ryucad 27.C).

use super::Given;
use super::check_up_to;
use super::{BodyUse, Check, FeatureKind, Write};
use crate::doc::files::Feature;
use crate::doc::id::Id;
use crate::doc::project::Part;
use crate::doc::sweep::{Angle, Extent, RevolveAxis};
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

/// Extrudes and revolves.
pub(crate) struct Extrusions;

impl FeatureKind for Extrusions {
    fn types(&self) -> &'static [&'static str] {
        &["extrude", "revolve"]
    }

    fn fields(&self, ty: &str) -> &'static [&'static str] {
        if ty == "extrude" { &EXTRUDE } else { &REVOLVE }
    }

    fn body(&self, _ty: &str) -> BodyUse {
        BodyUse::WHEN_NEW
    }

    fn from_given(
        &self,
        ty: &str,
        current: Option<&Feature>,
        cx: &mut Write<'_>,
    ) -> Result<Feature> {
        let fields = match current {
            None => None,
            Some(Feature::Extrude(e)) => Some(e.fields()),
            Some(Feature::Revolve(r)) => Some(r.fields()),
            Some(_) => unreachable!("a sweep"),
        };
        let f = sweep_from(ty, fields, &cx.given, cx.part, cx.unit, cx.env)?;
        cx.check_profile(&f)?;
        Ok(f)
    }

    fn report(&self, part: &Part, _unit: crate::units::Unit, f: &Feature) -> Value {
        let part = &part.id;
        match f {
            Feature::Extrude(e) => {
                let mut v = e.fields();
                match &e.extent {
                    Extent::Blind(q) | Extent::Symmetric(q) => v["extent"]["length"] = q.report(),
                    Extent::ThroughAll { .. } => {}
                    Extent::UpTo { target, offset } => {
                        v["extent"]["target"] = json!(target.text(part));
                        if let Some(q) = offset {
                            v["extent"]["offset"] = q.report();
                        }
                    }
                }
                v
            }
            Feature::Revolve(r) => {
                let mut v = r.fields();
                if let Angle::Of(q) = &r.angle {
                    v["angle"] = q.report();
                }
                if let RevolveAxis::Ref(e) = &r.axis {
                    v["axis"] = json!(e.text(part));
                }
                v
            }
            _ => unreachable!("an extrude or a revolve"),
        }
    }

    fn check(&self, cx: &Check<'_>, f: &Feature) -> Result<()> {
        match f {
            // What the target is; how far it is only rebuild can tell.
            Feature::Extrude(e) => match &e.extent {
                Extent::UpTo { target, .. } => {
                    check_up_to(cx.part, cx.order, cx.id, "target", target, cx.results)
                }
                _ => Ok(()),
            },
            // That it is an axis; whether it lies in the sketch's plane
            // only rebuild can tell.
            Feature::Revolve(r) => match &r.axis {
                RevolveAxis::Ref(g) => {
                    let at = crate::geometry::At {
                        part: cx.part,
                        order: cx.order,
                        owner: cx.id,
                        results: cx.results,
                    };
                    crate::geometry::one(&at, "axis", g, crate::geometry::Want::Axis).map(|_| ())
                }
                RevolveAxis::Line(_) => Ok(()),
            },
            _ => unreachable!("an extrude or a revolve"),
        }
    }
}
