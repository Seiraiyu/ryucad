//! Holes drilled at a sketch's points (ryucad 27.C).

use super::Given;
use super::{BodyUse, Check, FeatureKind, Write};
use crate::doc::files::Feature;
use crate::doc::id::Id;
use crate::doc::project::Part;
use crate::error::{Code, Error, Result};
use crate::expr::Env;
use crate::geometry::geom_json;
use crate::session::Session;
use crate::units::Unit;
use serde_json::{Value, json};

/// A hole as output reports it: its file's fields, every size as written
/// and in meters (an angle in radians), an up-to target as
/// `<part>/<feature>/<name>`.
fn hole_report(
    part: &Id,
    h: &crate::doc::hole::Hole,
    unit: Unit,
    tables: crate::hole::derive::Tables,
) -> Value {
    use crate::doc::hole::{Bottom, Depth, Far, HoleForm};
    let mut v = h.fields();
    v["spec"] = json!(crate::hole::spec::canonical(h, unit));
    match crate::hole::derive::derive(h, tables) {
        Ok(s) => v["sizes"] = s.to_json(),
        Err(e) => v["sizes_error"] = error_json(&e),
    }
    if let Some(d) = &h.diameter {
        v["diameter"] = d.report();
    }
    // Each mouth size as written; a derived one isn't in the file.
    let mouth = |sizes: &[(&str, &Option<crate::units::Quantity>)]| {
        Value::Object(
            sizes
                .iter()
                .filter_map(|(k, q)| Some(((*k).to_owned(), q.as_ref()?.report())))
                .collect(),
        )
    };
    match &h.form {
        HoleForm::Simple => {}
        HoleForm::Counterbore { diameter, depth } => {
            v["counterbore"] = mouth(&[("diameter", diameter), ("depth", depth)]);
        }
        HoleForm::Countersink { diameter, angle } => {
            v["countersink"] = mouth(&[("diameter", diameter), ("angle", angle)]);
        }
        HoleForm::Counterdrill {
            diameter,
            depth,
            angle,
        } => {
            v["counterdrill"] =
                mouth(&[("diameter", diameter), ("depth", depth), ("angle", angle)]);
        }
        HoleForm::Spotface { diameter, depth } => {
            v["spotface"] = mouth(&[("diameter", diameter), ("depth", depth)]);
        }
    }
    match &h.depth {
        Depth::Blind(q) => v["depth"] = json!({"blind": q.report()}),
        Depth::ThroughAll => {}
        Depth::ThroughNext => v["depth"] = json!("through_next"),
        Depth::Tip(q) => v["depth"] = json!({"tip": q.report()}),
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
    if let Some(n) = &h.near {
        let mut x = json!({"diameter": n.diameter.report()});
        if let Some(a) = &n.angle {
            x["angle"] = a.report();
        }
        v["near"] = x;
    }
    match &h.far {
        Some(Far::Countersink { diameter, angle }) => {
            v["far"] =
                json!({"countersink": {"diameter": diameter.report(), "angle": angle.report()}});
        }
        Some(Far::Counterbore { diameter, depth }) => {
            v["far"] =
                json!({"counterbore": {"diameter": diameter.report(), "depth": depth.report()}});
        }
        None => {}
    }
    v
}

/// An error as a report holds it: its code, message and details.
fn error_json(e: &Error) -> Value {
    let mut v = json!({"code": e.code, "message": e.message});
    if let Value::Object(d) = &e.details {
        for (k, x) in d {
            v[k] = x.clone();
        }
    }
    v
}

pub(crate) const HOLE: [&str; 27] = [
    "sketch",
    "points",
    "form",
    "diameter",
    "counterbore",
    "countersink",
    "counterdrill",
    "spotface",
    "near",
    "far",
    "thread_wall",
    "depth",
    "bottom",
    "targets",
    "spec",
    "thread",
    "fastener",
    "fastener_fit",
    "fit",
    "tolerance",
    "pitch",
    "tpi",
    "class",
    "hand",
    "starts",
    "thread_depth",
    "tap_drill",
];

/// A hole whose sizes don't derive is refused when written: a project
/// table it needs names what to add (`needs_project_table`).
fn refuse_underived(session: &Session, pid: &Id, f: &Feature) -> Result<()> {
    if let Feature::Hole(h) = f {
        let p = session.project().part(pid)?;
        crate::hole::derive::derive(h, p.hole_standards.as_deref())?;
    }
    Ok(())
}

/// `--tap-drill`'s words as a thread's `drill`: "table" (ryucad's rule,
/// written as nothing), "roll-form", "engagement:N".
fn tap_drill_json(s: &str) -> Result<Value> {
    match s {
        "table" => Ok(json!("")),
        "roll-form" => Ok(json!("roll_form")),
        _ => match s
            .strip_prefix("engagement:")
            .and_then(|n| n.parse::<u32>().ok())
        {
            Some(n) => Ok(json!({"engagement": n})),
            None => Err(Error::new(
                Code::InvalidParam,
                "tap_drill is table, roll-form or engagement:N",
            )
            .with("param", "tap_drill")),
        },
    }
}

/// A hole's mouth objects, one per form but simple.
const MOUTHS: [&str; 4] = ["counterbore", "countersink", "counterdrill", "spotface"];

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
    // both ways is usage.
    if let Some(text) = &f.spec {
        let spec = crate::hole::spec::Spec::parse(text)?;
        let (named, defaults) = spec.apply(unit)?;
        let beside = f.given();
        for k in named.keys() {
            if beside.contains_key(k) && !defaults.contains(&k.as_str()) {
                return Err(Error::new(
                    Code::Usage,
                    format!("{k} is given twice: by the spec, and as {k}"),
                )
                .with("param", k.as_str()));
            }
        }
        for (k, x) in named {
            match (k.as_str(), &x) {
                (_, Value::String(s)) if s.is_empty() => {
                    o.remove(&k);
                }
                ("bottom", Value::String(s)) if s == "flat" => {
                    o.remove("bottom");
                }
                ("form", Value::String(form)) => {
                    for m in MOUTHS {
                        if form != m {
                            o.remove(m);
                        }
                    }
                    o.insert(k, x);
                }
                _ => {
                    o.insert(k, x);
                }
            }
        }
    }
    // A thread or fastener: a whole object laid over the current one,
    // "STANDARD:SIZE" for a new one, "" to take it away.
    for (k, x) in [("thread", &f.thread), ("fastener", &f.fastener)] {
        match x {
            None => {}
            Some(Value::String(s)) if s.is_empty() => {
                o.remove(k);
            }
            Some(Value::String(s)) => {
                let (standard, size) = s.split_once(':').ok_or_else(|| {
                    Error::new(
                        Code::InvalidParam,
                        format!("{k} is STANDARD:SIZE, such as iso_metric:M6"),
                    )
                    .with("param", k)
                })?;
                o.insert(k.into(), json!({"standard": standard, "size": size}));
            }
            Some(Value::Object(new)) => match o.get_mut(k) {
                Some(Value::Object(old)) => old.extend(new.clone()),
                _ => {
                    o.insert(k.into(), Value::Object(new.clone()));
                }
            },
            Some(_) => {
                return Err(Error::new(
                    Code::InvalidParam,
                    format!("{k} is an object, STANDARD:SIZE or \"\""),
                )
                .with("param", k));
            }
        }
    }
    // A near chamfer and a far side: whole values, "" takes one away.
    for (k, x) in [("near", &f.near), ("far", &f.far)] {
        match x {
            None => {}
            Some(Value::String(s)) if s.is_empty() => {
                o.remove(k);
            }
            Some(x) => {
                o.insert(k.into(), x.clone());
            }
        }
    }
    // A thread's own fields, given flat.
    let flat: [(&str, Option<Value>); 8] = [
        ("pitch", f.pitch.clone()),
        ("tpi", f.tpi.clone()),
        ("class", f.class.clone().map(Value::String)),
        (
            "hand",
            f.hand
                .clone()
                .map(|h| if h == "right" { json!("") } else { json!(h) }),
        ),
        (
            "starts",
            f.starts.map(|n| if n <= 1 { json!("") } else { json!(n) }),
        ),
        ("depth", f.thread_depth.clone()),
        (
            "drill",
            f.tap_drill.as_deref().map(tap_drill_json).transpose()?,
        ),
        ("wall", f.thread_wall.clone().map(Value::String)),
    ];
    for (k, x) in flat {
        let Some(x) = x else { continue };
        let Some(Value::Object(t)) = o.get_mut("thread") else {
            return Err(Error::new(
                Code::InvalidParam,
                format!("a hole with no thread has no {k}: give --thread first"),
            )
            .with("param", k));
        };
        if x == "" {
            t.remove(k);
        } else {
            t.insert(k.into(), x);
        }
    }
    if let Some(fit) = &f.fastener_fit {
        let Some(Value::Object(x)) = o.get_mut("fastener") else {
            return Err(Error::new(
                Code::InvalidParam,
                "a hole with no fastener has no fastener_fit",
            )
            .with("param", "fastener_fit"));
        };
        x.insert("fit".into(), json!(fit));
    }
    match f.fit.as_deref() {
        Some("") => {
            o.remove("fit");
        }
        Some(c) => {
            o.insert("fit".into(), json!(c));
        }
        None => {}
    }
    match &f.tolerance {
        Some(Value::String(s)) if s.is_empty() => {
            o.remove("tolerance");
        }
        Some(t) => {
            o.insert("tolerance".into(), t.clone());
        }
        None => {}
    }
    if let Some(form) = &f.form {
        o.insert("form".into(), json!(form));
        for k in MOUTHS {
            if form != k {
                o.remove(k);
            }
        }
    }
    for (k, x) in [
        ("counterbore", &f.counterbore),
        ("countersink", &f.countersink),
        ("counterdrill", &f.counterdrill),
        ("spotface", &f.spotface),
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
    Ok(Feature::Hole(Box::new(h)))
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
        refuse_underived(cx.session, cx.part, &f)?;
        cx.check_profile(&f)?;
        Ok(f)
    }

    fn report(&self, part: &Part, unit: Unit, f: &Feature) -> Value {
        let h = f.hole().expect("a hole");
        hole_report(&part.id, h, unit, part.hole_standards.as_deref())
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

    fn rebuild(&self, cx: &super::RebuildCx<'_>, f: &Feature) -> super::Outcome {
        crate::rebuild::body_step(cx, f)
    }

    fn alone(&self, _part: &Id, id: &Id, f: &Feature) -> Result<crate::rebuild::Built> {
        Err(super::built_elsewhere(id, f, "is built from its sketch"))
    }

    fn reads<'f>(&self, f: &'f Feature) -> Vec<crate::kind::Read<'f>> {
        crate::kind::targets(f, "targets", None, None)
    }
}
