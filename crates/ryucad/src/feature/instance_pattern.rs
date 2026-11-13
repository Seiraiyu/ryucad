//! Instance patterns: instances repeated in an assembly (ryucad 27.C).

use super::Given;
use super::mate::connector_json;
use super::{BodyUse, FeatureKind, Write};
use crate::doc::files::Feature;
use crate::doc::id::Id;
use crate::doc::project::Part;
use crate::error::{Code, Error, Result};
use crate::expr::Env;
use crate::units::Unit;
use serde_json::Value;
use serde_json::json;

pub(crate) const INSTANCE_PATTERN: [&str; 16] = [
    "form",
    "instances",
    "direction",
    "spacing",
    "count",
    "direction2",
    "spacing2",
    "count2",
    "reverse",
    "reverse2",
    "axis",
    "angle",
    "skip",
    "on",
    "from",
    "points",
];

/// An instance pattern from `current`'s fields (none for a new one, whose
/// form, left out, is circular with an axis, else linear) with what was
/// given laid over them, as a part's pattern takes them; directions and
/// the axis are `x`, `y`, `z` or a connector's text, whose z they take.
pub(crate) fn instance_pattern_from(
    current: Option<&crate::doc::mate::InstancePattern>,
    given: &Given,
    part: &Id,
    unit: Unit,
    env: &Env,
) -> Result<Feature> {
    use crate::doc::mate::InstancePattern;
    let f = &given.fields;
    let fresh = || {
        let form = match (&f.on, &f.points, &f.axis) {
            (Some(_), Some(_), _) => "points",
            (Some(_), None, _) => "feature",
            (None, _, Some(_)) => "circular",
            _ => "linear",
        };
        json!({ "form": form })
    };
    let mut v = current.map_or_else(fresh, InstancePattern::fields);
    let o = v.as_object_mut().expect("an object");
    const SECOND: [&str; 4] = ["direction2", "spacing2", "count2", "reverse2"];
    if let Some(form) = &f.form {
        let takes: &[&str] = match form.as_str() {
            "linear" => &[
                "direction",
                "spacing",
                "count",
                "reverse",
                "direction2",
                "spacing2",
                "count2",
                "reverse2",
                "skip",
            ],
            "circular" => &["axis", "count", "angle", "skip"],
            "feature" => &["on", "from"],
            "points" => &["on", "from", "points"],
            _ => &[],
        };
        o.retain(|k, _| takes.contains(&k.as_str()) || k == "instances");
        o.insert("form".into(), json!(form));
    }
    if f.direction2.as_deref() == Some("") {
        o.retain(|k, _| !SECOND.contains(&k.as_str()));
    }
    for (k, x) in [
        ("direction", &f.direction),
        ("direction2", &f.direction2),
        ("axis", &f.axis),
    ] {
        if let Some(t) = x.as_deref().filter(|t| !t.is_empty()) {
            let d = if t.contains('/') {
                connector_json(t, part, k)?
            } else {
                json!(t)
            };
            o.insert(k.into(), d);
        }
    }
    for (k, x) in [
        ("spacing", &f.spacing),
        ("count", &f.count),
        ("spacing2", &f.spacing2),
        ("count2", &f.count2),
        ("angle", &f.angle),
    ] {
        if let Some(x) = x {
            o.insert(k.into(), x.clone());
        }
    }
    for (k, x) in [("reverse", f.reverse), ("reverse2", f.reverse2)] {
        match x {
            Some(true) => {
                o.insert(k.into(), json!(true));
            }
            Some(false) => {
                o.remove(k);
            }
            None => {}
        }
    }
    match &f.skip {
        Some(x) if x == &json!([]) => {
            o.remove("skip");
        }
        Some(x) => {
            o.insert("skip".into(), x.clone());
        }
        None => {}
    }
    if let Some(x) = &f.instances {
        o.insert("instances".into(), json!(x));
    }
    if let Some(t) = f.on.as_deref().filter(|t| !t.is_empty()) {
        o.insert("on".into(), driver_json(t, part)?);
    }
    match f.from.as_deref() {
        Some("") => {
            o.remove("from");
        }
        Some(t) => {
            o.insert("from".into(), json!(t));
        }
        None => {}
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
    let mut o = crate::doc::files::Obj::new(&v, "")?;
    let p = InstancePattern::from_fields(&mut o, unit, env)?;
    o.finish()?;
    Ok(Feature::InstancePattern(Box::new(p)))
}

/// A following pattern's `on` from its text: `<assembly>/<instance>[/…]/<feature>`,
/// from assembly `asm`, as connectors are given.
fn driver_json(t: &str, asm: &Id) -> Result<Value> {
    let bad = || {
        Error::new(
            Code::InvalidParam,
            format!("{t}: on is {asm}/<instance>[/<instance>…]/<feature>, from this assembly"),
        )
        .with("param", "on")
    };
    let rest = t.strip_prefix(&format!("{asm}/")).ok_or_else(bad)?;
    let mut steps: Vec<&str> = rest.split('/').collect();
    let feature = steps.pop().filter(|_| !steps.is_empty()).ok_or_else(bad)?;
    Ok(json!({"instance": steps, "feature": feature}))
}

/// Instance patterns.
pub(crate) struct InstancePatterns;

impl FeatureKind for InstancePatterns {
    fn types(&self) -> &'static [&'static str] {
        &["instance_pattern"]
    }

    fn fields(&self, _ty: &str) -> &'static [&'static str] {
        &INSTANCE_PATTERN
    }

    fn body(&self, _ty: &str) -> BodyUse {
        BodyUse::NONE
    }

    fn from_given(
        &self,
        _ty: &str,
        current: Option<&Feature>,
        cx: &mut Write<'_>,
    ) -> Result<Feature> {
        instance_pattern_from(
            current.and_then(Feature::instance_pattern),
            &cx.given,
            cx.part,
            cx.unit,
            cx.env,
        )
    }

    fn report(&self, part: &Part, _unit: crate::units::Unit, f: &Feature) -> Value {
        let x = f.instance_pattern().expect("an instance pattern");
        let mut v = x.fields();
        for (role, c) in x.connectors() {
            v[role]["text"] = json!(crate::part::connector_text(&part.id, c));
        }
        if let Some(d) = x.form.driver() {
            v["on"]["text"] = json!(format!("{}/{}", part.id, d.text()));
        }
        v
    }

    /// Solved with the whole assembly, over what its instances place.
    fn rebuild(&self, cx: &super::RebuildCx<'_>, f: &Feature) -> super::Outcome {
        (
            Err(super::built_elsewhere(
                cx.id,
                f,
                "is solved with its assembly",
            )),
            None,
        )
    }
}
