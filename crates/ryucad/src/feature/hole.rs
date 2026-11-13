//! Holes drilled at a sketch's points (ryucad 27.C).

use super::Given;
use super::{BodyUse, Check, FeatureKind, Write};
use crate::doc::files::Feature;
use crate::doc::id::Id;
use crate::doc::project::Part;
use crate::error::Result;
use crate::expr::Env;
use crate::geometry::geom_json;
use crate::units::Unit;
use serde_json::{Value, json};

/// A hole as output reports it: its file's fields, every size as written
/// and in meters (an angle in radians), an up-to target as
/// `<part>/<feature>/<name>`.
fn hole_report(part: &Id, h: &crate::doc::hole::Hole) -> Value {
    use crate::doc::hole::{Bottom, Depth, HoleForm};
    let mut v = h.fields();
    v["diameter"] = h.diameter.report();
    match &h.form {
        HoleForm::Simple => {}
        HoleForm::Counterbore { diameter, depth } => {
            v["counterbore"] = json!({"diameter": diameter.report(), "depth": depth.report()});
        }
        HoleForm::Countersink { diameter, angle } => {
            v["countersink"] = json!({"diameter": diameter.report(), "angle": angle.report()});
        }
    }
    match &h.depth {
        Depth::Blind(q) => v["depth"] = json!({"blind": q.report()}),
        Depth::ThroughAll => {}
        Depth::UpTo { target, offset } => {
            v["depth"]["up_to"]["target"] = json!(target.text(part));
            if let Some(q) = offset {
                v["depth"]["up_to"]["offset"] = q.report();
            }
        }
    }
    if let Bottom::Drill(q) = &h.bottom {
        v["bottom"] = json!({"drill": q.report()});
    }
    v
}

pub(crate) const HOLE: [&str; 9] = [
    "sketch",
    "points",
    "form",
    "diameter",
    "counterbore",
    "countersink",
    "depth",
    "bottom",
    "targets",
];

/// A hole from `current`'s fields (none for a new one, which is simple)
/// with what was given laid over them. A counterbore or countersink given
/// in part keeps the rest; a new form keeps only its own; an up-to target
/// is given as text and kept as a file holds it; no points is every point
/// and a flat bottom the default, so neither is written.
pub(crate) fn hole_from(
    current: Option<&crate::doc::hole::Hole>,
    given: &Given,
    part: &Id,
    unit: Unit,
    env: &Env,
) -> Result<Feature> {
    use crate::doc::hole::Hole;
    let mut v = current.map_or_else(|| json!({"form": "simple"}), Hole::fields);
    let o = v.as_object_mut().expect("an object");
    let f = &given.fields;
    if let Some(form) = &f.form {
        o.insert("form".into(), json!(form));
        for k in ["counterbore", "countersink"] {
            if form != k {
                o.remove(k);
            }
        }
    }
    for (k, x) in [
        ("counterbore", &f.counterbore),
        ("countersink", &f.countersink),
    ] {
        match (o.get_mut(k), x) {
            (Some(Value::Object(old)), Some(Value::Object(new))) => old.extend(new.clone()),
            (_, Some(x)) => {
                o.insert(k.into(), x.clone());
            }
            (_, None) => {}
        }
    }
    if let Some(s) = &f.sketch {
        o.insert("sketch".into(), json!(s));
    }
    match &f.points {
        Some(p) if p.is_empty() => {
            o.remove("points");
        }
        Some(p) => {
            o.insert("points".into(), json!(p));
        }
        None => {}
    }
    if let Some(d) = &f.diameter {
        o.insert("diameter".into(), d.clone());
    }
    if let Some(d) = &f.depth {
        let mut d = d.clone();
        if let Some(Value::String(t)) = d.pointer("/up_to/target") {
            d["up_to"]["target"] = geom_json(t, part, "/depth/up_to/target")?;
        }
        o.insert("depth".into(), d);
    }
    match &f.bottom {
        Some(b) if b == "flat" => {
            o.remove("bottom");
        }
        Some(b) => {
            o.insert("bottom".into(), b.clone());
        }
        None => {}
    }
    if let Some(t) = &f.targets {
        o.insert("targets".into(), json!(t));
    }
    let mut o = crate::doc::files::Obj::new(&v, "")?;
    let h = Hole::from_fields(&mut o, unit, env)?;
    o.finish()?;
    Ok(Feature::Hole(h))
}

/// Holes.
pub(crate) struct Holes;

impl FeatureKind for Holes {
    fn types(&self) -> &'static [&'static str] {
        &["hole"]
    }

    fn fields(&self, _ty: &str) -> &'static [&'static str] {
        &HOLE
    }

    fn body(&self, _ty: &str) -> BodyUse {
        BodyUse::NONE
    }

    fn acts_without_operation(&self) -> bool {
        true
    }

    fn from_given(
        &self,
        _ty: &str,
        current: Option<&Feature>,
        cx: &mut Write<'_>,
    ) -> Result<Feature> {
        let f = hole_from(
            current.and_then(Feature::hole),
            &cx.given,
            cx.part,
            cx.unit,
            cx.env,
        )?;
        cx.check_profile(&f)?;
        Ok(f)
    }

    fn report(&self, part: &Part, f: &Feature) -> Value {
        hole_report(&part.id, f.hole().expect("a hole"))
    }

    // What the target is; how far it is only rebuild can tell.
    fn check(&self, cx: &Check<'_>, f: &Feature) -> Result<()> {
        match &f.hole().expect("a hole").depth {
            crate::doc::hole::Depth::UpTo { target, .. } => {
                super::check_up_to(cx.part, cx.order, cx.id, "depth", target, cx.results)
            }
            _ => Ok(()),
        }
    }
}
