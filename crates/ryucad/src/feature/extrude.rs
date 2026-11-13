//! Extrudes and revolves: a sketch's loops swept straight or round (ryucad 27.C).

use super::Given;
use super::check_up_to;
use super::{BodyUse, Check, FeatureKind, Write};
use crate::doc::files::Feature;
use crate::doc::id::Id;
use crate::doc::project::Part;
use crate::doc::sketch::Geometry;
use crate::doc::sweep::{Angle, Extent, RevolveAxis};
use crate::doc::sweep::{Extrude, Revolve};
use crate::error::Result;
use crate::error::{Code, Error};
use crate::expr::Env;
use crate::geometry::geom_json;
use crate::profile::Keys;
use crate::rebuild::{Built, Results, UpTo, finish, op_key, sketch_input, through_all};
use crate::units::Unit;
use core::f64::consts::TAU;
use ryukei::SweepError;
use ryukei::geom::{Axis, Point3};
use ryukei::sweep;
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

    fn rebuild(&self, cx: &super::RebuildCx<'_>, f: &Feature) -> super::Outcome {
        crate::rebuild::body_step(cx, f)
    }

    fn own_body(
        &self,
        cx: &super::RebuildCx<'_>,
        f: &Feature,
        targets: &[&crate::rebuild::Built],
    ) -> Result<crate::rebuild::Built> {
        build_sweep(cx.part, cx.id, f, targets, cx.before)
    }

    fn alone(&self, _part: &Id, id: &Id, f: &Feature) -> Result<crate::rebuild::Built> {
        Err(super::built_elsewhere(id, f, "is built from its sketch"))
    }
}

/// An extrude or a revolve, from its sketch's result; `targets` are the
/// solids it acts on, which a through-all extrude reaches past.
pub(crate) fn build_sweep(
    part: &Part,
    id: &Id,
    f: &Feature,
    targets: &[&Built],
    before: &dyn Results,
) -> Result<Built> {
    let ty = f.type_name();
    let sketch = f.profiles()[0];
    let solved = sketch_input(part, id, sketch, before)?;
    let (loops, axis) = match f {
        Feature::Extrude(e) => (&e.loops, None),
        Feature::Revolve(r) => match &r.axis {
            RevolveAxis::Line(l) => (&r.loops, Some(l)),
            RevolveAxis::Ref(_) => (&r.loops, None),
        },
        _ => unreachable!("a sweep"),
    };
    // A surface sweep takes its sketch's loops and open chains, and
    // founds the sheet they sweep (ryucad 18).
    let of = if f.surface() {
        crate::profile::surface
    } else {
        crate::profile::profile
    };
    let (valid, keys) = of(&part.id, sketch, solved, loops)
        .map_err(|e| e.with("feature", id).with("sketch", sketch))?;
    let key = op_key(&part.id, id);
    let made = match f {
        Feature::Extrude(e) => {
            let n = valid.frame().z().get();
            let dir = if e.reverse { -n } else { n };
            let (start, end) = match &e.extent {
                Extent::Blind(q) => (0.0, q.meters()),
                Extent::Symmetric(q) => (-q.meters() / 2.0, q.meters() / 2.0),
                Extent::UpTo { target, offset } => {
                    let to = UpTo {
                        ty: "extrude",
                        role: "target",
                        target,
                        offset: offset.as_ref(),
                        from: valid.frame().origin(),
                        dir,
                    };
                    (0.0, to.length(part, id, before)?)
                }
                Extent::ThroughAll { both } => {
                    through_all("extrude", id, valid.frame().origin(), dir, *both, targets)?
                }
            };
            sweep::extrude(key, &valid, dir, start, end)
        }
        Feature::Revolve(r) => {
            let axis = match &r.axis {
                RevolveAxis::Line(l) => sweep_axis(solved, id, l)?,
                RevolveAxis::Ref(g) => ref_axis(part, id, g, solved, before)?,
            };
            let (start, end) = match &r.angle {
                Angle::Full => (0.0, TAU),
                Angle::Of(q) if r.symmetric => (-q.radians() / 2.0, q.radians() / 2.0),
                Angle::Of(q) => (0.0, q.radians()),
            };
            sweep::revolve(key, &valid, axis, start, end)
        }
        _ => unreachable!("a sweep"),
    };
    let made = made.map_err(|e| sweep_error(id, ty, e, &keys, axis))?;
    let mut origins = crate::profile::origins(key, sketch, &valid, &keys);
    for o in origins.values_mut() {
        o["feature"] = json!(id);
    }
    finish(id, ty, made, origins)
}

/// A revolve's axis: a line of its sketch, from its start to its end.
fn sweep_axis(solved: &crate::sketch::Solved, id: &Id, axis: &Id) -> Result<Axis> {
    let s = &solved.sketch;
    let at = |p: &Id| match s.entities[p].geometry {
        Geometry::Point { m } => solved.frame.apply_point(Point3::new(m[0], m[1], 0.0)),
        _ => unreachable!("validated: lines join points"),
    };
    match s.entities.get(axis).map(|e| &e.geometry) {
        Some(Geometry::Line { start, end }) => {
            let (a, b) = (at(start), at(end));
            Axis::new(a, b - a).map_err(|e| {
                Error::new(
                    Code::InvalidExtent,
                    format!("revolve {id}: its axis {axis} has no length"),
                )
                .with("feature", id)
                .with("kernel", format!("{e:?}"))
            })
        }
        Some(g) => Err(Error::new(
            Code::ReferenceWrongKind,
            format!(
                "revolve {id}: its axis {axis} is a {}, not a line",
                g.type_name()
            ),
        )
        .with("feature", id)
        .with("axis", axis)),
        None => Err(Error::new(
            Code::NotFound,
            format!("revolve {id}: its sketch has no {axis}"),
        )
        .with("feature", id)
        .with("axis", axis)),
    }
}

/// A revolve's axis from any axis lying in the sketch's plane (a straight
/// edge from its start to its end, a datum axis, another sketch's line),
/// put exactly in the plane.
fn ref_axis(
    part: &Part,
    id: &Id,
    g: &crate::doc::geometry::GeomRef,
    solved: &crate::sketch::Solved,
    before: &dyn Results,
) -> Result<Axis> {
    use crate::geometry::{At, Want, one};
    let at = At {
        part,
        order: part.order(),
        owner: id,
        results: before,
    };
    let axis = one(&at, "axis", g, Want::Axis)?.axis();
    let (a, b) = (axis.origin, axis.to);
    let inv = solved.frame.inverse();
    let (a, b) = (inv.apply_point(a), inv.apply_point(b));
    let text = g.text(&part.id);
    let invalid = |msg: &str| {
        Error::new(
            Code::InvalidExtent,
            format!("revolve {id}: its axis {text} {msg}"),
        )
        .with("feature", id)
        .with("role", "axis")
        .with("reference", &text)
    };
    if a.z.abs().max(b.z.abs()) > ryukei::tol::LINEAR_RESOLUTION {
        return Err(invalid("doesn't lie in the sketch's plane"));
    }
    let flat = |p: Point3| solved.frame.apply_point(Point3::new(p.x, p.y, 0.0));
    let (a, b) = (flat(a), flat(b));
    Axis::new(a, b - a).map_err(|k| invalid("has no length").with("kernel", format!("{k:?}")))
}

/// The kernel's refusal of a sweep, in the sketch's terms.
fn sweep_error(id: &Id, ty: &str, e: ryukei::Error, keys: &Keys, axis: Option<&Id>) -> Error {
    let kernel = format!("{e:?}");
    let (code, msg, entities) = match e {
        ryukei::Error::InvalidSweep(
            SweepError::CrossesAxis { segment }
            | SweepError::TouchesAxisInside { segment }
            | SweepError::PinchedOnAxis { segment },
        ) => {
            let seg = keys.segments[&segment.0].clone();
            let mut entities = vec![seg.clone()];
            entities.extend(axis.cloned());
            let msg = format!("{seg} crosses or touches the axis");
            (Code::ProfileInvalid, msg, entities)
        }
        ryukei::Error::InvalidSweep(SweepError::ChainOnAxis { segment }) => {
            let seg = keys.segments[&segment.0].clone();
            let mut entities = vec![seg.clone()];
            entities.extend(axis.cloned());
            let msg = format!(
                "{seg} lies along the axis: a surface revolve's chain may end on it, never run along it"
            );
            (Code::ProfileInvalid, msg, entities)
        }
        ryukei::Error::InvalidSweep(SweepError::ShortSpan | SweepError::BadAngle) => (
            Code::InvalidExtent,
            "the sweep is too short".to_owned(),
            Vec::new(),
        ),
        e => (Code::RebuildFailed, e.to_string(), Vec::new()),
    };
    let mut err = Error::new(code, format!("{ty} {id}: {msg}"))
        .with("feature", id)
        .with("kernel", kernel);
    if !entities.is_empty() {
        err = err.with("entities", entities);
    }
    err
}
