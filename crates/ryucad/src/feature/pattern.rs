//! Patterns of joins, cuts and holes (ryucad 27.C).

use super::Given;
use super::{BodyUse, Check, FeatureKind, Place, Write};
use crate::doc::files::Feature;
use crate::doc::id::Id;
use crate::doc::project::Part;
use crate::doc::solid::Operation;
use crate::error::{Code, Error, Result};
use crate::expr::Env;
use crate::geometry::geom_json;
use crate::rebuild::{Built, Made, Results, ToolBody, apply, finish, instance_json, read};
use crate::solid::Solids;
use crate::units::Unit;
use rayon::prelude::*;
use ryukei::boolean::BooleanOp;
use ryukei::geom::Rigid;
use serde_json::{Value, json};
use std::sync::Arc;

/// A pattern as output reports it: its file's fields, every spacing,
/// count and angle as written and as a value, every reference as
/// `<part>/<feature>/<name>`.
fn pattern_report(part: &Id, x: &crate::doc::pattern::Pattern) -> Value {
    use crate::doc::pattern::Kind;
    let mut v = x.fields();
    match &x.kind {
        Kind::Linear { first, second } => {
            v["spacing"] = first.spacing.report();
            v["count"] = first.count.report();
            if let Some(r) = second {
                v["spacing2"] = r.spacing.report();
                v["count2"] = r.count.report();
            }
        }
        Kind::Circular { count, angle, .. } => {
            v["count"] = count.report();
            v["angle"] = angle.report();
        }
        Kind::Mirror { .. } => {}
    }
    for (role, r) in x.geometry() {
        v[role] = json!(r.text(part));
    }
    v
}

pub(crate) const PATTERN: [&str; 15] = [
    "form",
    "features",
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
    "plane",
    "skip",
    "targets",
];

/// A pattern from `current`'s fields (none for a new one, whose form,
/// left out, is what its fields say: a mirror with a plane, circular with
/// an axis, else linear) with what was given laid over them. A new form
/// keeps only the fields it takes; references (a direction, an axis, a
/// plane) are given as text and kept as a file holds them; `direction2`
/// `""` drops the second direction, and no skips, or false, is the
/// default.
pub(crate) fn pattern_from(
    current: Option<&crate::doc::pattern::Pattern>,
    given: &Given,
    part: &Id,
    unit: Unit,
    env: &Env,
) -> Result<Feature> {
    use crate::doc::pattern::Pattern;
    let f = &given.fields;
    let fresh = || {
        let form = match (&f.plane, &f.axis) {
            (Some(_), _) => "mirror",
            (None, Some(_)) => "circular",
            _ => "linear",
        };
        json!({ "form": form })
    };
    let mut v = current.map_or_else(fresh, Pattern::fields);
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
            _ => &[],
        };
        o.retain(|k, _| {
            takes.contains(&k.as_str()) || ["features", "targets"].contains(&k.as_str())
        });
        o.insert("form".into(), json!(form));
    }
    // A geometric reference given as text, kept as a file holds it.
    let entity = |t: &str, at: &str| geom_json(t, part, at);
    if f.direction2.as_deref() == Some("") {
        o.retain(|k, _| !SECOND.contains(&k.as_str()));
    }
    for (k, x) in [
        ("direction", &f.direction),
        ("direction2", &f.direction2),
        ("axis", &f.axis),
        ("plane", &f.plane),
    ] {
        if let Some(t) = x.as_deref().filter(|t| !t.is_empty()) {
            o.insert(k.into(), entity(t, &format!("/{k}"))?);
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
    if let Some(x) = &f.features {
        o.insert("features".into(), json!(x));
    }
    if let Some(t) = &f.targets {
        o.insert("targets".into(), json!(t));
    }
    // What the form needs and nothing gave it.
    let needs: &[(&str, &str)] = match o.get("form").and_then(Value::as_str) {
        Some("linear") => &[
            (
                "direction",
                "an origin axis, a straight edge or a planar face",
            ),
            ("spacing", "the distance between instances"),
            ("count", "how many instances, the seed's included"),
        ],
        Some("circular") => &[
            (
                "axis",
                "an origin axis, a straight or circular edge, or a cylindrical or conical face",
            ),
            ("count", "how many instances, the seed's included"),
            (
                "angle",
                "the span: 360 deg spaces the count evenly round a whole turn, less spans first to last",
            ),
        ],
        Some("mirror") => &[("plane", "an origin plane or a planar face")],
        _ => &[],
    };
    let form = o.get("form").and_then(Value::as_str).unwrap_or_default();
    if let Some((k, what)) = needs.iter().find(|(k, _)| !o.contains_key(*k)) {
        return Err(Error::new(
            Code::InvalidParam,
            format!("a {form} pattern needs {k}: {what}"),
        )
        .with("param", *k)
        .with("pointer", format!("/{k}")));
    }
    let mut o = crate::doc::files::Obj::new(&v, "")?;
    let p = Pattern::from_fields(&mut o, unit, env)?;
    o.finish()?;
    Ok(Feature::Pattern(p))
}

/// Patterns of joins, cuts and holes.
pub(crate) struct Patterns;

impl FeatureKind for Patterns {
    fn types(&self) -> &'static [&'static str] {
        &["pattern"]
    }

    fn fields(&self, _ty: &str) -> &'static [&'static str] {
        &PATTERN
    }

    fn body(&self, _ty: &str) -> BodyUse {
        BodyUse::NONE
    }

    // A pattern with no targets acts on its first seed's.
    fn before_targets(&self, _ty: &str, cx: &mut Write<'_>) -> Result<()> {
        if cx.given.fields.targets.is_some() {
            return Ok(());
        }
        let Place::New { id, .. } = cx.place else {
            return Ok(());
        };
        let pid = cx.part;
        let p = cx.session.project().part(pid)?;
        let seed = cx.given.fields.features.as_ref().and_then(|s| s.first());
        let theirs = seed
            .and_then(|s| Id::parse(s, "feature").ok())
            .and_then(|s| p.features.get(&s).cloned())
            .and_then(|f| f.ok());
        match (seed, theirs) {
            (_, Some(f)) if !f.targets().is_empty() => {
                cx.given.fields.targets = Some(f.targets().iter().map(|t| t.to_string()).collect());
            }
            // Its seed's own fault says what's wrong with it.
            (Some(seed), Some(f)) => {
                return Err(Error::new(
                    Code::ReferenceWrongKind,
                    format!(
                        "{}its seed {seed} is a {} that joins or cuts nothing; a pattern repeats joins, cuts and holes",
                        id.map(|i| format!("pattern {i}: ")).unwrap_or_default(),
                        f.type_name()
                    ),
                )
                .with("seed", seed)
                .with("pointer", "/features/0"));
            }
            (Some(seed), None)
                if Id::parse(seed, "feature").is_ok_and(|s| !p.features.contains_key(&s)) =>
            {
                return Err(Error::new(
                    Code::NotFound,
                    format!("a pattern's seed {seed} isn't a feature of part {pid}"),
                )
                .with("seed", seed)
                .with("pointer", "/features/0"));
            }
            _ => {}
        }
        Ok(())
    }

    fn from_given(
        &self,
        _ty: &str,
        current: Option<&Feature>,
        cx: &mut Write<'_>,
    ) -> Result<Feature> {
        pattern_from(
            current.and_then(Feature::pattern),
            &cx.given,
            cx.part,
            cx.unit,
            cx.env,
        )
    }

    fn report(&self, part: &Part, _unit: crate::units::Unit, f: &Feature) -> Value {
        pattern_report(&part.id, f.pattern().expect("a pattern"))
    }

    // What its directions, axis and plane are; where the instances land
    // only rebuild can tell.
    fn check(&self, cx: &Check<'_>, f: &Feature) -> Result<()> {
        let x = f.pattern().expect("a pattern");
        crate::pattern::motions(&cx.proposed(f), cx.id, x, cx.results).map(|_| ())
    }
}

/// A pattern: its targets as it sees them, with every instance of every
/// seed's own body, moved, in one kernel call per target. An instance
/// that changes nothing, or that alone the kernel refuses, is named.
pub(crate) fn build_pattern(
    part: &Part,
    solids: &Solids,
    id: &Id,
    f: &Feature,
    p: &crate::doc::pattern::Pattern,
    before: &dyn Results,
    run: &crate::rebuild::Run,
) -> Result<Made> {
    use crate::pattern::{Motion, instance_key};
    let targets: Vec<(&Id, &Built)> = p
        .targets
        .iter()
        .map(|t| Ok((t, read(part, solids, id, f, "targets", t, before)?)))
        .collect::<Result<_>>()?;
    let (operation, seeds) = crate::pattern::seeds(part, id, p, before)?;
    let motions = crate::pattern::motions(part, id, p, before)?;
    if motions.is_empty() {
        return Err(Error::new(
            Code::OperationNoEffect,
            format!("pattern {id} has no instance but its seeds: every other one is skipped"),
        )
        .with("feature", id)
        .with("pointer", "/skip"));
    }
    // A mirror image is a body of its own: each seed's, at each place,
    // reflected.
    let mut mirrored: Vec<Built> = Vec::new();
    for (_, m) in &motions {
        let Motion::Mirror(plane) = m else { continue };
        for (seed, a) in &seeds {
            for (_, place) in &a.places {
                let body = a
                    .body
                    .body
                    .transformed(place)
                    .and_then(|b| b.mirrored(plane))
                    .map_err(|e| {
                        Error::new(
                            Code::RebuildFailed,
                            format!("pattern {id}: the mirror image of {seed} leaves the size box"),
                        )
                        .with("feature", id)
                        .with("seed", *seed)
                        .with("kernel", format!("{e:?}"))
                    })?;
                let journal = a.body.journal.clone();
                let origins = (*a.body.origins).clone();
                mirrored.push(finish(id, "pattern", (body, journal), origins)?);
            }
        }
    }
    let mut images = mirrored.iter();
    let mut tools: Vec<ToolBody> = Vec::new();
    for ([i, j], m) in &motions {
        let instance = instance_json(p, [*i, *j]);
        for (seed, a) in &seeds {
            for (point, place) in &a.places {
                let mut label = json!({"feature": id, "seed": seed, "instance": instance});
                if let Some(pt) = point {
                    label["point"] = json!(pt);
                }
                let (built, place) = match m {
                    Motion::Rigid(r) => (a.body.as_ref(), r.then_after(place)),
                    Motion::Mirror(_) => {
                        (images.next().expect("an image per place"), Rigid::IDENTITY)
                    }
                };
                tools.push(ToolBody {
                    key: instance_key(&part.id, id, seed, point.as_ref(), [*i, *j]),
                    built,
                    place,
                    name: id,
                    label: Some(label),
                });
            }
        }
    }
    let kop = match operation {
        Operation::Join => BooleanOp::Unite,
        _ => BooleanOp::Subtract,
    };
    let made: Vec<(Id, Arc<Built>)> = targets
        .par_iter()
        .map(|(t, b)| {
            let r = apply(
                part,
                id,
                "pattern",
                (kop, operation.as_str()),
                t,
                b,
                &tools,
                run,
            )?;
            Ok(((*t).clone(), Arc::new(r)))
        })
        // The first failure in order, not the first a thread found.
        .collect::<Vec<_>>()
        .into_iter()
        .collect::<Result<_>>()?;
    Ok(Made::new(made))
}
