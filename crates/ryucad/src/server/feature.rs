//! Requests on features: add, edit, delete, move, list, show, and each type's fields and report.

use crate::doc::assembly::{Instance, Pose, Rev};
use crate::doc::files::{Feature, PartKind, Placement, Primitive, PrimitiveType, number};
use crate::doc::id::Id;
use crate::doc::project::{Part, feature_path};
use crate::doc::reference::Reference;
use crate::doc::sketch::{OnFace, OriginPlane, Plane, Sketch};
use crate::doc::solid::{Boolean, Combine};
use crate::doc::sweep::{Angle, Extent, Extrude, Revolve, RevolveAxis};
use crate::entity::EntityRef;
use crate::error::{Code, Error, Result};
use crate::expr::Env;
use crate::geometry::geom_json;
use crate::mesh;
use crate::param;
use crate::part::{
    check_revision, connector_text, feature, feature_id, insert_at, no_feature, order, part_id,
    part_kind, proposed, reference_texts,
};
use crate::proto::{FeatureFields, PlacementArg, Position};
use crate::rebuild;
use crate::save::save;
use crate::session::Session;
use crate::solid::Solids;
use crate::units::{Quantity, Unit};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};

/// How a feature appears in output: every quantity as written and in
/// meters, every reference as `<part>/<feature>/<name>`.
/// A hole's spec writes bare numbers in `unit`, the project's.
pub fn feature_report(p: &Part, unit: Unit, id: &Id, f: &Feature) -> Value {
    let part = &p.id;
    let mut v = match f {
        Feature::Primitive(p) => primitive_report(p),
        Feature::Sketch(s) => sketch_report(part, s),
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
        Feature::Boolean(b) => b.fields(),
        Feature::MoveFace(_)
        | Feature::ReplaceFace(_)
        | Feature::Draft(_)
        | Feature::DeleteFace(_)
        | Feature::DeleteBlend(_)
        | Feature::ResizeBlend(_) => crate::tweak::report(part, f),
        Feature::Datum(d) => crate::datum::report(part, d),
        Feature::Path(x) => crate::path::report(part, x),
        Feature::Sweep(w) => crate::along::feature_report(part, w),
        Feature::Loft(l) => crate::loft::feature_report(part, l),
        Feature::Planar(x) => {
            let mut v = x.fields();
            if let crate::doc::sweep::Source::Face(g) = &x.profile.source {
                v["profile"]["face"] = json!(g.text(part));
            }
            v
        }
        Feature::Instance(i) => i.fields(),
        Feature::Insert(i) => i.fields(),
        Feature::DeleteBody(d) => d.fields(),
        Feature::Import(i) => i.fields(),
        Feature::Fill(_) | Feature::Extract(_) | Feature::Sew(_) | Feature::Thicken(_) => {
            crate::surface::report(part, f)
        }
        Feature::Mate(m) => {
            let mut v = m.fields();
            for (side, c) in [("a", &m.a), ("b", &m.b)] {
                v[side]["text"] = json!(connector_text(part, c));
            }
            if let Some(q) = &m.angle {
                v["angle"] = q.report();
            }
            if let Some(q) = &m.distance {
                v["distance"] = q.report();
            }
            v
        }
        Feature::InstancePattern(x) => {
            let mut v = x.fields();
            for (role, c) in x.connectors() {
                v[role]["text"] = json!(connector_text(part, c));
            }
            v
        }
        Feature::Fillet(_) | Feature::Chamfer(_) => blend_report(part, f),
        Feature::Shell(_) | Feature::Offset(_) | Feature::Mirror(_) | Feature::Split(_) => {
            crate::body::report(part, f)
        }
        Feature::BaseFlange(_) | Feature::Flange(_) | Feature::Unfold(_) | Feature::Refold(_) => {
            crate::sheet_metal::report(p, f)
        }
        Feature::Hole(h) => hole_report(part, h, unit, p.hole_standards.as_deref()),
        Feature::Pattern(x) => pattern_report(part, x),
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
    };
    v["id"] = json!(id);
    v["type"] = json!(f.type_name());
    v
}

/// A fillet or chamfer as output reports it: its file's fields, every
/// size as written and in meters (an angle in radians), every edge and
/// face as `<part>/<feature>/<name>`.
fn blend_report(part: &Id, f: &Feature) -> Value {
    use crate::doc::blend::ChamferForm;
    let mut v = match f {
        Feature::Fillet(x) => x.fields(),
        Feature::Chamfer(x) => x.fields(),
        _ => unreachable!("a fillet or a chamfer"),
    };
    let sizes: Vec<Vec<(&str, &Quantity)>> = match f {
        Feature::Fillet(x) => x
            .groups
            .iter()
            .map(|g| vec![("radius", &g.radius)])
            .collect(),
        Feature::Chamfer(x) => x
            .groups
            .iter()
            .map(|g| match &g.form {
                ChamferForm::Equal { d } => vec![("d", d)],
                ChamferForm::Distances { d1, d2, .. } => vec![("d1", d1), ("d2", d2)],
                ChamferForm::DistanceAngle { d, angle, .. } => vec![("d", d), ("angle", angle)],
            })
            .collect(),
        _ => unreachable!("a fillet or a chamfer"),
    };
    for (i, group) in sizes.into_iter().enumerate() {
        for (k, q) in group {
            v["groups"][i][k] = q.report();
        }
    }
    for (role, r) in f.references() {
        let at = format!("/{}", role.replace('.', "/"));
        *v.pointer_mut(&at).expect("its own reference") = json!(r.text(part));
    }
    v
}

/// A hole as output reports it: its file's fields, every size as written
/// and in meters (an angle in radians), an up-to target as
/// `<part>/<feature>/<name>`.
fn hole_report(
    part: &Id,
    h: &crate::doc::hole::Hole,
    unit: Unit,
    tables: crate::hole::derive::Tables,
) -> Value {
    use crate::doc::hole::{Bottom, Depth, HoleForm};
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

/// A placement as output reports it: quantities as written and in meters.
fn placement_report(p: &Placement) -> Value {
    json!({
        "origin": p.origin.iter().map(Quantity::report).collect::<Vec<_>>(),
        "z": p.z.map(number),
        "x": p.x.map(number),
    })
}

/// A sketch as output reports it: its file's fields, with every dimension
/// as written and evaluated.
fn sketch_report(part: &Id, s: &Sketch) -> Value {
    let mut v = s.fields();
    if let Plane::Placement(p) = &s.plane {
        v["placement"] = placement_report(p);
    }
    reference_texts(part, s, &mut v);
    for (id, c) in &s.constraints {
        if let Some(q) = &c.value {
            v["constraints"][id.as_str()]["value"] = q.report();
        }
    }
    v
}

fn primitive_report(f: &Primitive) -> Value {
    let params: Map<String, Value> = f
        .params
        .iter()
        .map(|(k, q)| (k.clone(), q.report()))
        .collect();
    let mut v = json!({"params": params, "placement": placement_report(&f.placement)});
    f.combine.write(&mut v);
    v
}

/// Applies the parts of `arg` that are given to `p`, its expressions
/// reading `env`.
fn apply_placement(p: &mut Placement, arg: PlacementArg, unit: Unit, env: &Env) -> Result<()> {
    if let Some(origin) = arg.origin {
        for (i, v) in origin.iter().enumerate() {
            p.origin[i] = Quantity::from_json_in(v, unit, &|n| env.get(n))
                .map_err(|e| e.with("pointer", format!("/placement/origin/{i}")))?;
        }
    }
    if let Some(z) = arg.z {
        p.z = z;
    }
    if let Some(x) = arg.x {
        p.x = x;
    }
    // Refuse a placement that could never build, before it is written.
    rebuild::placement(p).map(|_| ())
}

/// What a `feature.add` or `feature.edit` gives besides the feature's
/// place: each part left out keeps its current or default value.
pub(crate) struct Given {
    pub(crate) params: Map<String, Value>,
    pub(crate) placement: Option<PlacementArg>,
    pub(crate) fields: FeatureFields,
}

impl Given {
    /// The names of the fields given.
    fn present(&self) -> Vec<String> {
        let mut out: Vec<String> = self.fields.given().keys().cloned().collect();
        if !self.params.is_empty() {
            out.push("params".into());
        }
        if self.placement.is_some() {
            out.push("placement".into());
        }
        out
    }

    /// Refuses the fields `ty` doesn't take.
    fn only(&self, ty: &str, allowed: &[&str]) -> Result<()> {
        match self
            .present()
            .into_iter()
            .find(|f| !allowed.contains(&f.as_str()))
        {
            None => Ok(()),
            Some(name) => Err(
                Error::new(Code::InvalidParam, format!("a {ty} has no {name}")).with("param", name),
            ),
        }
    }
}

/// Reads what a primitive is given: its sizes are parameters, but a
/// cylinder's, cone's or sphere's radius may come as the fillet's
/// `radius` field (the CLI's `--radius`), which is then that parameter
/// (issue #205). Any other field it doesn't take is refused, saying
/// where its sizes go.
fn primitive_fields(ty: PrimitiveType, given: &mut Given) -> Result<()> {
    if ty.required().contains(&"radius")
        && let Some(r) = given.fields.radius.take()
    {
        if given.params.contains_key("radius") {
            return Err(Error::new(
                Code::InvalidParam,
                format!(
                    "a {}'s radius is given twice: as radius and as a parameter",
                    ty.as_str()
                ),
            )
            .with("param", "radius"));
        }
        given.params.insert("radius".into(), r);
    }
    given.only(ty.as_str(), &PRIMITIVE).map_err(|mut e| {
        let sizes: Vec<&str> = ty.required().iter().chain(ty.optional()).copied().collect();
        e.message = format!(
            "{}: a {}'s sizes are parameters ({}), given as --param NAME=QUANTITY \
             (params in a request)",
            e.message,
            ty.as_str(),
            sizes.join(", ")
        );
        e
    })
}

/// The fields a sketch takes.
const SKETCH: [&str; 5] = ["plane", "placement", "face", "face_origin", "face_x"];

/// The fields a primitive, an extrude, a revolve and a boolean take.
const PRIMITIVE: [&str; 4] = ["params", "placement", "operation", "targets"];

const EXTRUDE: [&str; 7] = [
    "sketch",
    "loops",
    "extent",
    "reverse",
    "surface",
    "operation",
    "targets",
];

const REVOLVE: [&str; 8] = [
    "sketch",
    "loops",
    "axis",
    "angle",
    "symmetric",
    "surface",
    "operation",
    "targets",
];

const BOOLEAN: [&str; 4] = ["op", "target", "targets", "tools"];

const INSTANCE: [&str; 4] = ["of", "rev", "placement", "fixed"];

/// The fields an insert takes: its source (`of`), the revision it pins
/// and its placement.
const INSERT: [&str; 3] = ["of", "rev", "placement"];

const DELETE_BODY: [&str; 2] = ["solids", "solid"];

const MATE: [&str; 8] = [
    "form", "a", "b", "a_offset", "b_offset", "angle", "distance", "limits",
];

const INSTANCE_PATTERN: [&str; 13] = [
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
];

const FILLET: [&str; 4] = ["groups", "propagate", "radius", "edges"];

const CHAMFER: [&str; 9] = [
    "groups",
    "propagate",
    "form",
    "d",
    "d1",
    "d2",
    "angle",
    "face",
    "edges",
];

const PATTERN: [&str; 15] = [
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

/// A connector as a file holds it, from its text: an entity of an
/// occurrence of assembly `asm`, `<asm>/<path>/<solid>/<name>`, or an
/// occurrence's own frame, `<asm>/<path>`. `param` names the field for
/// errors.
fn connector_json(t: &str, asm: &Id, param: &str) -> Result<Value> {
    let r = EntityRef::parse(t)
        .map_err(|e| e.with("param", param))?
        .in_assembly();
    if r.part != *asm {
        return Err(Error::new(
            Code::InvalidParam,
            format!(
                "{t} is in {}, not assembly {asm}: a connector is on an occurrence of the assembly",
                r.part
            ),
        )
        .with("param", param));
    }
    if r.path.is_empty() {
        return Err(Error::new(
            Code::InvalidParam,
            format!("{t} names no instance: give <assembly>/<instance>[/…]/<solid>/<name>"),
        )
        .with("param", param));
    }
    let at = if r.is_occurrence() {
        json!("origin")
    } else {
        json!({"feature": r.feature, "name": r.name})
    };
    Ok(json!({"instance": r.path, "at": at}))
}

/// A connector read as an occurrence whose path's last step is a datum of
/// the part the rest of it reaches (`<asm>/<instance>/…/<datum>`): a step
/// names an instance while the part before it is an assembly, and a
/// feature once it is a part. Then it is at that datum.
fn datum_at(project: &crate::doc::project::Project, asm: &Id, c: &mut Value) {
    let Some(path) = c["instance"].as_array().cloned() else {
        return;
    };
    let Some((last, steps)) = path.split_last() else {
        return;
    };
    if steps.is_empty() {
        return;
    }
    let mut part = asm.clone();
    for step in steps {
        let Some(step) = step.as_str() else { return };
        // A copy's seed follows it, an instance of the same assembly.
        if step.contains(':') {
            continue;
        }
        let Ok(id) = Id::parse(step, "instance") else {
            return;
        };
        let Some(Ok(Feature::Instance(i))) =
            project.parts.get(&part).and_then(|p| p.features.get(&id))
        else {
            return;
        };
        part = i.part.clone();
    }
    let Some(p) = project.parts.get(&part) else {
        return;
    };
    let datum = last
        .as_str()
        .and_then(|l| Id::parse(l, "feature").ok())
        .filter(|d| matches!(p.features.get(d), Some(Ok(Feature::Datum(_)))));
    if part_kind(p) == PartKind::Part
        && let Some(d) = datum
    {
        c["instance"] = json!(steps);
        c["at"] = json!({"feature": d});
    }
}

/// A mate from `current`'s fields (none for a new one) with what was
/// given laid over them: connectors as text, each keeping its offset
/// unless one is given (`{}` for none); `angle` or `distance` `""` lets it
/// go; a new form keeps only the driven values it takes.
fn mate_from(
    current: Option<&crate::doc::mate::Mate>,
    given: &Given,
    (project, part): (&crate::doc::project::Project, &Id),
    unit: Unit,
    env: &Env,
) -> Result<Feature> {
    use crate::doc::mate::{Form, Mate};
    let f = &given.fields;
    let mut v = current.map_or_else(|| json!({}), Mate::fields);
    let o = v.as_object_mut().expect("an object");
    match (&f.form, current) {
        (Some(form), _) => {
            let Some(form) = Form::parse(form) else {
                return Err(Error::new(
                    Code::InvalidParam,
                    format!("{form:?} isn't a mate: fastened, revolute, slider, cylindrical, planar or ball"),
                )
                .with("param", "form"));
            };
            o.insert("form".into(), json!(form.as_str()));
            if !form.takes_angle() {
                o.remove("angle");
            }
            if !form.takes_distance() {
                o.remove("distance");
            }
            if let Some(Value::Object(l)) = o.get_mut("limits") {
                if !form.takes_angle() {
                    l.remove("angle");
                }
                if !form.takes_distance_limit() {
                    l.remove("distance");
                }
                if l.is_empty() {
                    o.remove("limits");
                }
            }
        }
        (None, None) => {
            return Err(Error::new(
                Code::InvalidParam,
                "a mate names its form: fastened, revolute, slider, cylindrical, planar or ball",
            )
            .with("param", "form"));
        }
        (None, Some(_)) => {}
    }
    for (side, text, offset) in [("a", &f.a, &f.a_offset), ("b", &f.b, &f.b_offset)] {
        if let Some(t) = text {
            let mut c = connector_json(t, part, side)?;
            datum_at(project, part, &mut c);
            if let Some(off) = o.get(side).and_then(|c| c.get("offset")) {
                c["offset"] = off.clone();
            }
            o.insert(side.into(), c);
        }
        if let Some(off) = offset {
            let Some(c) = o.get_mut(side) else {
                return Err(Error::new(
                    Code::InvalidParam,
                    format!("{side}_offset moves connector {side}: give {side} too"),
                )
                .with("param", format!("{side}_offset")));
            };
            if *off == json!({}) {
                c.as_object_mut().expect("a connector").remove("offset");
            } else {
                c["offset"] = off.clone();
            }
        }
        if !o.contains_key(side) {
            return Err(Error::new(
                Code::InvalidParam,
                format!("a mate joins two connectors: give {side}"),
            )
            .with("param", side));
        }
    }
    for (k, x) in [("angle", &f.angle), ("distance", &f.distance)] {
        match x {
            Some(Value::String(t)) if t.is_empty() => {
                o.remove(k);
            }
            Some(x) => {
                o.insert(k.into(), x.clone());
            }
            None => {}
        }
    }
    if let Some(given) = &f.limits {
        let bad = |what: &str| {
            Error::new(Code::InvalidParam, format!("limits: {what}")).with("param", "limits")
        };
        let Some(given) = given.as_object() else {
            return Err(bad("an object of angle and distance ranges"));
        };
        let mut cur = match o.remove("limits") {
            Some(Value::Object(m)) => m,
            _ => serde_json::Map::new(),
        };
        for (value, r) in given {
            if !matches!(value.as_str(), "angle" | "distance") {
                return Err(bad(&format!(
                    "{value:?} isn't a mate's value: angle or distance"
                )));
            }
            match r {
                Value::String(s) if s.is_empty() => {
                    cur.remove(value);
                }
                Value::Object(bounds) => {
                    let mut range = match cur.remove(value) {
                        Some(Value::Object(m)) => m,
                        _ => serde_json::Map::new(),
                    };
                    for (b, x) in bounds {
                        if !matches!(b.as_str(), "min" | "max") {
                            return Err(bad(&format!("a range has min and max, not {b:?}")));
                        }
                        if x.as_str() == Some("") {
                            range.remove(b);
                        } else {
                            range.insert(b.clone(), x.clone());
                        }
                    }
                    if !range.is_empty() {
                        cur.insert(value.clone(), Value::Object(range));
                    }
                }
                _ => return Err(bad("a range is {min, max}, or \"\" to clear it")),
            }
        }
        if !cur.is_empty() {
            o.insert("limits".into(), Value::Object(cur));
        }
    }
    let mut o = crate::doc::files::Obj::new(&v, "")?;
    let m = Mate::from_fields(&mut o, unit, env)?;
    o.finish()?;
    Ok(Feature::Mate(Box::new(m)))
}

/// An instance pattern from `current`'s fields (none for a new one, whose
/// form, left out, is circular with an axis, else linear) with what was
/// given laid over them, as a part's pattern takes them; directions and
/// the axis are `x`, `y`, `z` or a connector's text, whose z they take.
fn instance_pattern_from(
    current: Option<&crate::doc::mate::InstancePattern>,
    given: &Given,
    part: &Id,
    unit: Unit,
    env: &Env,
) -> Result<Feature> {
    use crate::doc::mate::InstancePattern;
    let f = &given.fields;
    let fresh = || json!({"form": if f.axis.is_some() { "circular" } else { "linear" }});
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
    let mut o = crate::doc::files::Obj::new(&v, "")?;
    let p = InstancePattern::from_fields(&mut o, unit, env)?;
    o.finish()?;
    Ok(Feature::InstancePattern(Box::new(p)))
}

/// A pattern from `current`'s fields (none for a new one, whose form,
/// left out, is what its fields say: a mirror with a plane, circular with
/// an axis, else linear) with what was given laid over them. A new form
/// keeps only the fields it takes; references (a direction, an axis, a
/// plane) are given as text and kept as a file holds them; `direction2`
/// `""` drops the second direction, and no skips, or false, is the
/// default.
fn pattern_from(
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

const HOLE: [&str; 22] = [
    "sketch",
    "points",
    "form",
    "diameter",
    "counterbore",
    "countersink",
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

/// A dry run's answer: the feature as it would be, sizes and all, with
/// nothing written.
fn dry(session: &Session, pid: &Id, fid: &Id, f: &Feature) -> Result<Value> {
    let p = session.project().part(pid)?;
    let unit = session.project().manifest.default_unit;
    Ok(json!({"dry_run": true, "feature": feature_report(p, unit, fid, f)}))
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

/// A hole from `current`'s fields (none for a new one, which is simple)
/// with what was given laid over them. A counterbore or countersink given
/// in part keeps the rest; a new form keeps only its own; an up-to target
/// is given as text and kept as a file holds it; no points is every point
/// and a flat bottom the default, so neither is written.
fn hole_from(
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
                    for m in ["counterbore", "countersink"] {
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
    // A thread's own fields, given flat.
    let flat: [(&str, Option<Value>); 7] = [
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
    Ok(Feature::Hole(Box::new(h)))
}

/// An instance: `current`'s fields (none for a new one), with what was
/// given laid over them. A new instance needs `of`: a part, or another
/// project's as `<dependency>:<part>` (a dependency of `deps` by name or
/// project ID), which needs a release label. Its revision is `working`
/// and its pose the assembly's frame unless given. The origin takes
/// quantities and expressions, held as meters.
fn instance_from(
    current: Option<&Instance>,
    given: &Given,
    unit: Unit,
    env: &Env,
    deps: &crate::doc::deps::Dependencies,
) -> Result<Instance> {
    let f = &given.fields;
    // Another project's: by `of`, else as the current instance is.
    let mut project = current.and_then(|c| c.rev.project().cloned());
    let part = match (&f.of, current) {
        (Some(of), _) => match of.split_once(':') {
            Some((dep, part)) => {
                let (id, _) =
                    crate::doc::deps::find(deps, dep).map_err(|e| e.with("param", "of"))?;
                project = Some(id.clone());
                Id::parse(part, "part").map_err(|e| e.with("param", "of"))?
            }
            None => {
                project = None;
                Id::parse(of, "part").map_err(|e| e.with("param", "of"))?
            }
        },
        (None, Some(c)) => c.part.clone(),
        (None, None) => {
            return Err(Error::new(
                Code::InvalidParam,
                "an instance names the part it places (of)",
            )
            .with("param", "of"));
        }
    };
    let rev = match f.rev.as_deref() {
        None => current.map_or(Rev::Working, |c| c.rev.clone()),
        Some("working") => Rev::Working,
        Some(l) => {
            Rev::Label(crate::doc::assembly::label_text(l).map_err(|e| e.with("param", "rev"))?)
        }
    };
    let rev = match (project, rev.label()) {
        (None, _) => match rev {
            Rev::Foreign { label, .. } => Rev::Label(label),
            r => r,
        },
        (Some(project), Some(label)) => Rev::Foreign {
            label: label.to_owned(),
            project,
        },
        (Some(project), None) => {
            return Err(Error::new(
                Code::InstanceForeignWorking,
                format!(
                    "{part} is another project's ({project}): place one of its official release labels (rev)"
                ),
            )
            .with("param", "rev")
            .with("project", &project)
            .with("part", &part));
        }
    };
    let mut pose = current.map_or(Pose::IDENTITY, |c| c.pose);
    if let Some(arg) = &given.placement {
        if let Some(origin) = &arg.origin {
            for (i, v) in origin.iter().enumerate() {
                let q = Quantity::from_json_in(v, unit, &|n| env.get(n))
                    .map_err(|e| e.with("pointer", format!("/placement/origin/{i}")))?;
                if q.dim() != crate::units::Dim::LENGTH {
                    return Err(Error::new(
                        Code::UnitMismatch,
                        format!("an instance's origin is a length, not {}", q.text()),
                    )
                    .with("pointer", format!("/placement/origin/{i}")));
                }
                pose.origin_m[i] = crate::doc::sketch::clean(q.meters());
            }
        }
        if let Some(z) = arg.z {
            pose.z = z.map(crate::doc::sketch::clean);
        }
        if let Some(x) = arg.x {
            pose.x = x.map(crate::doc::sketch::clean);
        }
    }
    // Refuse a pose that could never build, before it is written.
    crate::assembly::pose_rigid(&pose)?;
    // Which use of an imported product it is, while it places that part.
    let import = current
        .filter(|c| c.part == part)
        .and_then(|c| c.import.clone());
    Ok(Instance {
        part,
        rev,
        pose,
        fixed: f.fixed.unwrap_or(current.is_some_and(|c| c.fixed)),
        import,
    })
}

/// An insert from a request: `of` (a part, or `<dependency>:<part>`),
/// `rev` (`working`, a label; left out: as `current` has it, or for a
/// new insert the source's newest official label, else `working`) and
/// `placement`. Answers the insert and, for a new one with no `rev`,
/// which it took and why (design 521, question 624).
fn insert_from(
    session: &mut Session,
    current: Option<&crate::doc::insert::Insert>,
    given: &Given,
    unit: Unit,
    env: &Env,
) -> Result<(crate::doc::insert::Insert, Option<Value>)> {
    let deps = session.project().manifest.dependencies.clone();
    let f = &given.fields;
    let (part, project) = match (f.of.as_deref(), current) {
        (Some(of), _) => match of.split_once(':') {
            Some((dep, part)) => {
                let (id, _) =
                    crate::doc::deps::find(&deps, dep).map_err(|e| e.with("param", "of"))?;
                let id = id.clone();
                (
                    Id::parse(part, "part").map_err(|e| e.with("param", "of"))?,
                    Some(id),
                )
            }
            None => (
                Id::parse(of, "part").map_err(|e| e.with("param", "of"))?,
                None,
            ),
        },
        (None, Some(c)) => (c.part.clone(), c.rev.project().cloned()),
        (None, None) => {
            return Err(Error::new(
                Code::InvalidParam,
                "an insert names the part it inserts (of)",
            )
            .with("param", "of"));
        }
    };
    let mut took = None;
    let rev = match (f.rev.as_deref(), current) {
        (Some("working"), _) => Rev::Working,
        (Some(l), _) => {
            Rev::Label(crate::doc::assembly::label_text(l).map_err(|e| e.with("param", "rev"))?)
        }
        (None, Some(c)) if c.part == part => match &c.rev {
            Rev::Foreign { label, .. } => Rev::Label(label.clone()),
            r => r.clone(),
        },
        (None, _) => match crate::release::newest_official(session, &part, project.as_ref())? {
            Some(l) => {
                took = Some(json!({"rev": {"label": l}, "why": "newest_official_label"}));
                Rev::Label(l)
            }
            None => {
                took = Some(json!({"rev": "working", "why": "no_official_label"}));
                Rev::Working
            }
        },
    };
    let rev = match (project, rev) {
        (Some(project), Rev::Label(label)) => Rev::Foreign { project, label },
        (Some(project), _) => {
            return Err(Error::new(
                Code::InstanceForeignWorking,
                format!(
                    "{part} is another project's ({project}): insert one of its official release labels (rev)"
                ),
            )
            .with("param", "rev")
            .with("project", &project)
            .with("part", &part));
        }
        (None, r) => r,
    };
    let placement = match (&given.placement, current) {
        (Some(arg), Some(c)) => {
            let mut p = c
                .placement
                .clone()
                .unwrap_or_else(|| Placement::identity(unit));
            apply_placement(&mut p, arg.clone(), unit, env)?;
            Some(p)
        }
        (Some(arg), None) => Some(new_placement(arg.clone(), unit, env)?),
        (None, Some(c)) => c.placement.clone(),
        (None, None) => None,
    };
    // Refuse a placement that could never build, before it is written.
    if let Some(p) = &placement {
        rebuild::placement(p)?;
    }
    let placement = placement.filter(|p| !p.is_identity());
    Ok((
        crate::doc::insert::Insert {
            part,
            rev,
            placement,
        },
        took,
    ))
}

/// Refuses insert `id` of part `pid`, listed in `list`, that can't build:
/// a part that isn't there (`not_found`), an assembly (`wrong_part_kind`,
/// question 625), a cycle through inserts and instances
/// (`insert_cycle`), a label that doesn't resolve now
/// (`label_not_found`, `label_not_official`), a source that shows no
/// solid (`insert_source_empty`).
fn check_insert(session: &mut Session, pid: &Id, list: &[Id], id: &Id, f: &Feature) -> Result<()> {
    let ins = f.insert().expect("an insert");
    if let Rev::Foreign { .. } = &ins.rev {
        // The lock the write would need, from what was fetched, as a
        // foreign instance's.
        let mut project = session.project().clone();
        let q = proposed(project.part(pid)?, list, &[(id, f)]);
        project.parts.insert(pid.clone(), q);
        crate::deps::relock_keeping(session, &project).map_err(|e| e.with("feature", id))?;
        return Ok(());
    }
    let Some(src) = session.project().parts.get(&ins.part) else {
        return Err(Error::new(
            Code::NotFound,
            format!(
                "{id} inserts {}, which isn't a part of the project",
                ins.part
            ),
        )
        .with("feature", id)
        .with("part", &ins.part)
        .with("param", "of"));
    };
    if part_kind(src) == PartKind::Assembly {
        return Err(crate::insert::assembly_error(id, &ins.part));
    }
    match &ins.rev {
        Rev::Working => {
            let mut project = session.project().clone();
            let q = proposed(project.part(pid)?, list, &[(id, f)]);
            project.parts.insert(pid.clone(), q);
            let cycles = crate::assembly::Cycles::of(&project, &crate::assembly::Pins::new());
            let (a, c) = (
                (pid.clone(), Rev::Working),
                (ins.part.clone(), Rev::Working),
            );
            if cycles.closes(&a, &c) {
                return Err(crate::assembly::cycle_error(&cycles, &a, &c).with("feature", id));
            }
        }
        Rev::Label(l) => {
            crate::release::pinned(session, &ins.part, l).map_err(|e| e.with("feature", id))?;
        }
        Rev::Foreign { .. } => unreachable!("judged above"),
    }
    let model = session.model(&ins.part, &ins.rev)?;
    if model
        .occurrences
        .first()
        .is_none_or(|o| o.solids.is_empty())
    {
        return Err(Error::new(
            Code::InsertSourceEmpty,
            format!("{id} inserts {}, which shows no solid", ins.part),
        )
        .with("feature", id)
        .with("part", &ins.part));
    }
    Ok(())
}

/// Refuses instance `id` of assembly `pid`, listed in `list`, if its part
/// doesn't exist (`not_found`), if placing it would make the assembly
/// place itself (`assembly_cycle`), or if its label doesn't resolve now
/// (`label_not_found`, `label_not_official`).
fn check_instance(
    session: &mut Session,
    pid: &Id,
    list: &[Id],
    id: &Id,
    f: &Feature,
) -> Result<()> {
    let i = f.instance().expect("an instance");
    if let Rev::Foreign { .. } = &i.rev {
        // The lock the write would need, from what was fetched: never the
        // network. Only what the write adds is judged; what the lock holds
        // now stands.
        let mut project = session.project().clone();
        let q = proposed(project.part(pid)?, list, &[(id, f)]);
        project.parts.insert(pid.clone(), q);
        crate::deps::relock_keeping(session, &project).map_err(|e| e.with("feature", id))?;
        return Ok(());
    }
    if !session.project().parts.contains_key(&i.part) {
        return Err(
            Error::new(Code::NotFound, format!("no part {} to place", i.part))
                .with("part", &i.part)
                .with("param", "of"),
        );
    }
    match &i.rev {
        Rev::Working => {
            let mut project = session.project().clone();
            let q = proposed(project.part(pid)?, list, &[(id, f)]);
            project.parts.insert(pid.clone(), q);
            let cycles = crate::assembly::Cycles::of(&project, &crate::assembly::Pins::new());
            let (a, c) = ((pid.clone(), Rev::Working), (i.part.clone(), Rev::Working));
            if cycles.closes(&a, &c) {
                return Err(crate::assembly::cycle_error(&cycles, &a, &c).with("feature", id));
            }
        }
        Rev::Label(l) => {
            crate::release::pinned(session, &i.part, l).map_err(|e| e.with("feature", id))?;
        }
        Rev::Foreign { .. } => unreachable!("judged above"),
    }
    Ok(())
}

/// A primitive's operation and targets: `current`'s, with what was
/// given laid over them (`new` drops the targets).
fn combine_from(current: &Combine, given: &Given) -> Result<Combine> {
    let mut v = json!({});
    current.write(&mut v);
    let f = &given.fields;
    if let Some(op) = &f.operation {
        v["operation"] = json!(op);
        if op == "new" {
            v.as_object_mut().expect("an object").remove("targets");
        }
    }
    if let Some(t) = &f.targets {
        v["targets"] = json!(t);
    }
    let mut o = crate::doc::files::Obj::new(&v, "")?;
    let c = Combine::from_fields(&mut o)?;
    o.finish()?;
    Ok(c)
}

/// A boolean from `current`'s fields (none for a new one) with what was
/// given laid over them. Its target may come as `target`, or as the one
/// entry of `targets`.
fn boolean_from(current: Option<&Boolean>, given: &Given) -> Result<Boolean> {
    let mut v = current.map_or_else(|| json!({}), Boolean::fields);
    let f = &given.fields;
    let target = match (&f.target, &f.targets) {
        (Some(_), Some(_)) => {
            return Err(Error::new(Code::Usage, "give a boolean's target once"));
        }
        (Some(t), None) => Some(t.clone()),
        (None, Some(ts)) => match ts.as_slice() {
            [t] => Some(t.clone()),
            _ => {
                return Err(Error::new(
                    Code::InvalidParam,
                    format!("a boolean has one target, not {}", ts.len()),
                )
                .with("param", "targets"));
            }
        },
        (None, None) => None,
    };
    if let Some(op) = &f.op {
        v["op"] = json!(op);
    }
    if let Some(t) = target {
        v["target"] = json!(t);
    }
    if let Some(t) = &f.tools {
        v["tools"] = json!(t);
    }
    let mut o = crate::doc::files::Obj::new(&v, "")?;
    let b = Boolean::from_fields(&mut o)?;
    o.finish()?;
    Ok(b)
}

/// A fillet or chamfer (`ty`) from `current`'s fields (none for a new
/// one) with what was given laid over them: its `groups` whole, or its
/// one group's fields (`radius`, or `form`, `d`, `d1`, `d2`, `angle` and
/// `face`; and `edges`), and `propagate`. Edges and faces are given as
/// text and kept as a file holds them; a new form keeps only what it
/// takes.
fn blend_from(
    ty: &str,
    current: Option<&Feature>,
    given: &Given,
    part: &Id,
    unit: Unit,
    env: &Env,
) -> Result<Feature> {
    use crate::doc::blend::{Chamfer, Fillet};
    let f = &given.fields;
    let reference = |v: &Value, at: &str| -> Result<Value> {
        match v {
            Value::String(t) => Ok(Reference::parse(t, part)
                .map_err(|e| e.with("pointer", at))?
                .to_json()),
            v => Ok(v.clone()),
        }
    };
    let mut v = match current {
        Some(Feature::Fillet(x)) => x.fields(),
        Some(Feature::Chamfer(x)) => x.fields(),
        _ if ty == "chamfer" => json!({"groups": [{"form": "equal"}]}),
        _ => json!({"groups": [{}]}),
    };
    let sizes = [
        ("radius", &f.radius),
        ("d", &f.d),
        ("d1", &f.d1),
        ("d2", &f.d2),
        ("angle", &f.angle),
    ];
    let one = sizes.iter().any(|(_, x)| x.is_some())
        || f.edges.is_some()
        || f.form.is_some()
        || f.face.is_some();
    if let Some(groups) = &f.groups {
        if one {
            return Err(Error::new(
                Code::Usage,
                format!("give a {ty}'s groups, or its one group's fields, not both"),
            ));
        }
        let mut groups = groups.clone();
        if let Value::Array(gs) = &mut groups {
            for (i, g) in gs.iter_mut().enumerate() {
                if let Some(Value::Array(es)) = g.get_mut("edges") {
                    for (j, e) in es.iter_mut().enumerate() {
                        *e = reference(e, &format!("/groups/{i}/edges/{j}"))?;
                    }
                }
                if let Some(face) = g.get_mut("face") {
                    *face = reference(face, &format!("/groups/{i}/face"))?;
                }
            }
        }
        v["groups"] = groups;
    } else if one {
        let gs = v["groups"].as_array_mut().expect("groups");
        if gs.len() != 1 {
            return Err(Error::new(
                Code::InvalidParam,
                format!(
                    "this {ty} has {} groups: give them all, as groups",
                    gs.len()
                ),
            )
            .with("param", "groups"));
        }
        let g = gs[0].as_object_mut().expect("a group");
        if let Some(form) = &f.form {
            let takes: &[&str] = match form.as_str() {
                "equal" => &["d", "edges"],
                "distances" => &["d1", "d2", "face", "edges"],
                "distance_angle" => &["d", "angle", "face", "edges"],
                _ => &["edges"],
            };
            g.retain(|k, _| takes.contains(&k.as_str()));
            g.insert("form".into(), json!(form));
        }
        for (k, x) in sizes {
            if let Some(x) = x {
                g.insert(k.into(), x.clone());
            }
        }
        if let Some(face) = &f.face {
            g.insert("face".into(), reference(&json!(face), "/groups/0/face")?);
        }
        if let Some(edges) = &f.edges {
            let edges: Vec<Value> = edges
                .iter()
                .enumerate()
                .map(|(j, e)| reference(&json!(e), &format!("/groups/0/edges/{j}")))
                .collect::<Result<_>>()?;
            g.insert("edges".into(), json!(edges));
        }
    }
    match f.propagate {
        Some(false) => v["propagate"] = json!(false),
        Some(true) => {
            v.as_object_mut().expect("an object").remove("propagate");
        }
        None => {}
    }
    let mut o = crate::doc::files::Obj::new(&v, "")?;
    let out = match ty {
        "fillet" => Feature::Fillet(Fillet::from_fields(&mut o, unit, env)?),
        _ => Feature::Chamfer(Chamfer::from_fields(&mut o, unit, env)?),
    };
    o.finish()?;
    Ok(out)
}

/// Whether body `x` of part `pid`, as a feature at `at` of `list` would
/// see it, is a sheet in the current rebuild: the tip of its chain among
/// the features before `at`. A part no feature of which may make a
/// sheet is never rebuilt for this.
fn source_is_sheet(session: &mut Session, pid: &Id, list: &[Id], at: usize, x: &Id) -> bool {
    let Ok(p) = session.project().part(pid).cloned() else {
        return false;
    };
    let may = p
        .features
        .values()
        .any(|g| g.as_ref().is_ok_and(|g| g.may_make_sheet()));
    if !may {
        return false;
    }
    let solids = Solids::of_order(&p, &list[..at]);
    let Some(tip) = solids.tips().find(|(s, _)| *s == x).map(|(_, t)| t.clone()) else {
        return false;
    };
    let Ok(results) = session.rebuilt(pid) else {
        return false;
    };
    crate::rebuild::Results::result(results, &tip)
        .and_then(|r| r.as_ref().ok())
        .and_then(|o| o.solid(x))
        .is_some_and(|b| b.body.kind() == ryukei::topo::BodyKind::Sheet)
}

/// The solids a feature put at `at` in `order` could act on: founded
/// before it and not consumed.
fn solids_before(p: &Part, order: &[Id], at: usize) -> Vec<Id> {
    Solids::of_order(p, &order[..at])
        .tips()
        .map(|(s, _)| s.clone())
        .collect()
}

/// Refuses a write that would leave a feature acting on what isn't a
/// solid before it in `q` (the part as the write leaves it): the written
/// features, those that name them, and those that name a written
/// boolean's tools.
fn check_solids(q: &Part, written: &[&Id]) -> Result<()> {
    let solids = Solids::of(q);
    let mut named: Vec<&Id> = written.to_vec();
    for w in written {
        if let Some(Ok(Feature::Boolean(b))) = q.features.get(*w) {
            named.extend(&b.tools);
        }
        if let Some(Ok(f)) = q.features.get(*w) {
            named.extend(f.deletes());
        }
    }
    for id in q.order() {
        let Some(Ok(f)) = q.features.get(id) else {
            continue;
        };
        let targets = f.targets();
        let concerned = written.contains(&id)
            || targets.iter().any(|t| {
                named.contains(t) || written.iter().any(|w| w.as_str() == t.founder_str())
            });
        if targets.is_empty() || !concerned {
            continue;
        }
        crate::graph::check_order(q, q.order(), id, f)?;
        for t in targets {
            if let Some(by) = solids.consumed_by(t, id) {
                return Err(rebuild::consumed(id, t, by));
            }
            if let Some(by) = solids.deleted_by(t, id) {
                return Err(rebuild::deleted(id, t, by));
            }
            if !solids.is_solid(t) {
                return Err(rebuild::not_a_solid(q, &solids, id, "targets", t));
            }
        }
    }
    Ok(())
}

/// A `delete_body` from what it is given: `solids` (or one `solid`, as
/// the CLI sends a lone `--solid`), read as its file reads them.
fn delete_from(given: &Given) -> Result<crate::doc::body::DeleteBody> {
    let solids: Vec<String> = match (&given.fields.solids, &given.fields.solid) {
        (Some(s), None) => s.clone(),
        (None, Some(s)) => vec![s.clone()],
        (Some(_), Some(_)) => {
            return Err(Error::new(
                Code::InvalidParam,
                "a delete names its solids once: solids, or one solid",
            )
            .with("param", "solid"));
        }
        (None, None) => Vec::new(),
    };
    if solids.is_empty() {
        return Err(Error::new(
            Code::InvalidParam,
            "a delete names the solids it deletes: solids",
        )
        .with("param", "solids"));
    }
    let v = json!({"solids": solids});
    let mut o = crate::doc::files::Obj::new(&v, "")?;
    let d =
        crate::doc::body::DeleteBody::from_fields(&mut o).map_err(|e| e.with("param", "solids"))?;
    o.finish()?;
    Ok(d)
}

/// Refuses a `delete_body` its rebuild would refuse, against the part as
/// the write would leave it: the solids before it as they are now.
fn check_delete(session: &mut Session, pid: &Id, list: &[Id], id: &Id, f: &Feature) -> Result<()> {
    let Feature::DeleteBody(d) = f else {
        return Ok(());
    };
    let q = proposed(session.project().part(pid)?, list, &[(id, f)]);
    let solids = Solids::of(&q);
    let r = session.rebuilt(pid)?;
    crate::body::delete_body(&q, &solids, id, d, r).map(|_| ())
}

/// `invalid_param` for `feature.add` or `feature.edit` of an import:
/// its file, product and body come from the file, through `import`.
fn import_not_written_here(how: &str) -> Error {
    Error::new(
        Code::InvalidParam,
        format!(
            "an import can't be {how}ed with feature.{how}: import a STEP file with `import` (`--into` adds its bodies to a part)"
        ),
    )
    .with("type", "import")
}

/// An extrude or revolve from its fields: the current ones (for an edit)
/// with those given laid over them.
fn sweep_from(
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

/// A planar face from `current`'s fields (none for a new one) with what
/// was given laid over them: its profile's sketch or face, and loops.
fn planar_from(current: Option<Value>, given: &Given, part: &Id) -> Result<Feature> {
    let bad = |param: &str, msg: &str| Error::new(Code::InvalidParam, msg).with("param", param);
    let mut profile = current
        .and_then(|v| v.get("profile").cloned())
        .unwrap_or_else(|| json!({}));
    let f = &given.fields;
    match (&f.sketch, &f.face) {
        (Some(_), Some(_)) => {
            return Err(bad(
                "face",
                "a planar face's profile is a sketch or a planar face, not both",
            ));
        }
        (Some(sk), None) => profile = json!({"sketch": sk}),
        (None, Some(face)) => profile = json!({"face": geom_json(face, part, "/profile/face")?}),
        (None, None) => {}
    }
    if profile.as_object().is_none_or(|o| o.is_empty()) {
        return Err(bad(
            "sketch",
            "a planar face fills closed loops: name a sketch (sketch) or a planar face (face)",
        ));
    }
    match &f.loops {
        Some(l) if l.is_empty() => {
            if let Some(o) = profile.as_object_mut() {
                o.remove("loops");
            }
        }
        Some(l) => profile["loops"] = json!(l),
        None => {}
    }
    let v = json!({"profile": profile});
    let mut o = crate::doc::files::Obj::new(&v, "")?;
    let p = crate::doc::sweep::Planar::from_fields(&mut o)?;
    o.finish()?;
    Ok(Feature::Planar(Box::new(p)))
}

/// Refuses a sweep or a hole that names what isn't there: its sketch must
/// be a sketch listed before it in `order`, with the entities it names,
/// its axis a line and a hole's points points. Whether the loops make a
/// profile only rebuild can tell.
fn check_sweep(part: &Part, order: &[Id], id: &Id, f: &Feature) -> Result<()> {
    // A loft's sections: each sketch listed before it, with the entities
    // its loops name. How they match only its rebuild check tells.
    if let Feature::Loft(l) = f {
        for (k, s) in l.sections.iter().enumerate() {
            let Some(sketch) = s.sketch() else { continue };
            check_sketch(part, order, id, sketch)?;
            let Some(Ok(Feature::Sketch(sk))) = part.features.get(sketch) else {
                continue;
            };
            for (i, e) in s.loops.iter().enumerate() {
                if !sk.entities.contains_key(e) {
                    return Err(
                        Error::new(Code::NotFound, format!("sketch {sketch} has no {e}"))
                            .with("sketch", sketch)
                            .with("entity", e)
                            .with("pointer", format!("/sections/{k}/loops/{i}")),
                    );
                }
            }
        }
        return Ok(());
    }
    let sketch = f.profiles()[0];
    check_sketch(part, order, id, sketch)?;
    let Some(Ok(Feature::Sketch(sk))) = part.features.get(sketch) else {
        return Ok(());
    };
    // A hole's points must be points of its sketch; whether it has any
    // to drill when it lists none only rebuild can tell.
    if let Feature::Hole(h) = f {
        for (i, e) in h.points.iter().enumerate() {
            match sk.entities.get(e).map(|x| &x.geometry) {
                None => {
                    return Err(
                        Error::new(Code::NotFound, format!("sketch {sketch} has no {e}"))
                            .with("sketch", sketch)
                            .with("entity", e)
                            .with("pointer", format!("/points/{i}")),
                    );
                }
                Some(crate::doc::sketch::Geometry::Point { .. }) => {}
                Some(g) => {
                    return Err(Error::new(
                        Code::ReferenceWrongKind,
                        format!("a hole drills at points; {e} is a {}", g.type_name()),
                    )
                    .with("entity", e)
                    .with("pointer", format!("/points/{i}")));
                }
            }
        }
        return Ok(());
    }
    let (loops, axis) = match f {
        Feature::Extrude(e) => (&e.loops, None),
        Feature::Revolve(r) => match &r.axis {
            RevolveAxis::Line(l) => (&r.loops, Some(l)),
            RevolveAxis::Ref(_) => (&r.loops, None),
        },
        Feature::Sweep(w) => (&w.profile.loops, None),
        Feature::Planar(x) => (&x.profile.loops, None),
        Feature::BaseFlange(b) => (&b.loops, None),
        _ => unreachable!("a sweep"),
    };
    let at = if f.sweep().is_some() || matches!(f, Feature::Planar(_)) {
        "/profile"
    } else {
        ""
    };
    for (field, e) in loops
        .iter()
        .map(|e| ("loops", e))
        .chain(axis.map(|a| ("axis", a)))
    {
        match sk.entities.get(e).map(|x| &x.geometry) {
            None => {
                return Err(
                    Error::new(Code::NotFound, format!("sketch {sketch} has no {e}"))
                        .with("sketch", sketch)
                        .with("entity", e)
                        .with("pointer", format!("{at}/{field}")),
                );
            }
            Some(g)
                if field == "axis" && !matches!(g, crate::doc::sketch::Geometry::Line { .. }) =>
            {
                return Err(Error::new(
                    Code::ReferenceWrongKind,
                    format!("an axis is a line; {e} is a {}", g.type_name()),
                )
                .with("entity", e)
                .with("pointer", "/axis"));
            }
            Some(_) => {}
        }
    }
    Ok(())
}

/// Refuses a sketch that isn't a sketch listed before `id` in `order`.
fn check_sketch(part: &Part, order: &[Id], id: &Id, sketch: &Id) -> Result<()> {
    let at = |f: &Id| order.iter().position(|x| x == f);
    match (at(sketch), at(id)) {
        (None, _) => {
            return Err(Error::new(
                Code::NotFound,
                format!("part {} has no feature {sketch} to sweep", part.id),
            )
            .with("sketch", sketch));
        }
        (Some(s), Some(me)) if s >= me => {
            return Err(Error::new(
                Code::ReferenceForward,
                format!("{id} would sweep {sketch}, which comes after it"),
            )
            .with("feature", id)
            .with("sketch", sketch));
        }
        _ => {}
    }
    match part.features.get(sketch) {
        Some(Ok(f)) if f.sketch().is_none() => Err(Error::new(
            Code::ReferenceWrongKind,
            format!("{sketch} is a {}, not a sketch", f.type_name()),
        )
        .with("sketch", sketch)),
        _ => Ok(()),
    }
}

/// A new feature's placement from what was given. With z given and x left
/// out, x defaults to world X, or to world Y when z lies along X; either
/// way the file records it.
fn new_placement(arg: PlacementArg, unit: Unit, env: &Env) -> Result<Placement> {
    let mut placed = Placement::identity(unit);
    let along_x = arg.x.is_none()
        && arg.z.is_some_and(|z| {
            let mut p = Placement::identity(unit);
            p.z = z;
            rebuild::placement(&p).is_err_and(|e| e.details["kernel"] == "Degenerate")
        });
    if along_x {
        placed.x = [0.0, 1.0, 0.0];
    }
    apply_placement(&mut placed, arg, unit, env)?;
    Ok(placed)
}

/// A sketch's plane after an edit: an origin plane, its placement with
/// what was given applied, or a face with its origin and `x`. `current`
/// is `None` for a new sketch.
fn sketch_plane(
    current: Option<&Plane>,
    given: &Given,
    part: &Id,
    unit: Unit,
    env: &Env,
) -> Result<Plane> {
    let f = &given.fields;
    let chosen = [
        f.plane.is_some(),
        given.placement.is_some(),
        f.face.is_some(),
    ];
    if chosen.iter().filter(|c| **c).count() > 1 {
        return Err(Error::new(
            Code::Usage,
            "give a sketch one of a plane, a placement and a face",
        ));
    }
    // A face's origin or x, or none: the default.
    use crate::doc::geometry::GeomRef;
    let reference = |s: &Option<String>| -> Result<Option<Option<GeomRef>>> {
        match s.as_deref() {
            None => Ok(None),
            Some("") => Ok(Some(None)),
            Some(t) => GeomRef::parse(t, part).map(|r| Some(Some(r))),
        }
    };
    let (origin, x) = (reference(&f.face_origin)?, reference(&f.face_x)?);
    let plane = match (&f.plane, &given.placement, &f.face) {
        // An origin plane; a datum plane or another sketch; a face.
        (Some(p), _, _) => {
            match GeomRef::parse(p, part).map_err(|e| e.with("pointer", "/plane"))? {
                GeomRef::Plane(o) => Plane::Origin(o),
                g @ GeomRef::Feature(_) => Plane::Datum(g),
                GeomRef::Entity(r) => Plane::Face(OnFace {
                    face: r,
                    origin: None,
                    x: None,
                }),
                _ => {
                    return Err(Error::new(
                    Code::InvalidParam,
                    format!(
                        "a sketch's plane is xy, yz, zx, a datum plane or sketch <part>/<feature>, or a planar face; not {p}"
                    ),
                )
                .with("pointer", "/plane"));
                }
            }
        }
        (_, Some(arg), _) => {
            let mut p = match current.and_then(|c| c.placement(unit)) {
                Some(p) => p,
                None => new_placement(arg.clone(), unit, env)?,
            };
            apply_placement(&mut p, arg.clone(), unit, env)?;
            Plane::Placement(p)
        }
        (_, _, Some(face)) => Plane::Face(OnFace {
            face: Reference::parse(face, part)?,
            origin: None,
            x: None,
        }),
        _ => current.cloned().unwrap_or(Plane::Origin(OriginPlane::Xy)),
    };
    match plane {
        Plane::Face(mut on) => {
            // A new face keeps no old origin or x.
            let old = match current {
                Some(Plane::Face(c)) if f.face.is_none() && f.plane.is_none() => Some(c),
                _ => None,
            };
            on.origin = origin.unwrap_or_else(|| old.and_then(|c| c.origin.clone()));
            on.x = x.unwrap_or_else(|| old.and_then(|c| c.x.clone()));
            Ok(Plane::Face(on))
        }
        p if origin.is_some() || x.is_some() => {
            let _ = p;
            Err(Error::new(
                Code::Usage,
                "face_origin and face_x go with a sketch on a face",
            ))
        }
        p => Ok(p),
    }
}

/// Refuses a feature whose references don't resolve to what they must
/// now, in the part's current rebuild; `order` is the list as the write
/// leaves it. Gives the `reference_followed` warnings of those that
/// follow a face or edge a later feature split or merged (design 447).
fn check_references(
    session: &mut Session,
    part: &Id,
    order: &[Id],
    id: &Id,
    f: &Feature,
) -> Result<Vec<Value>> {
    // Nothing may change a reference import's body, whether or not the
    // feature references anything (a cut, a boolean). Only an import in
    // reference mode makes one, so a part without one isn't rebuilt here.
    if (!f.targets().is_empty() || f.source().is_some())
        && session.project().part(part)?.features.values().any(|g| {
            g.as_ref()
                .ok()
                .and_then(|g| g.import())
                .is_some_and(|i| i.mode() == crate::doc::import::Mode::Reference)
        })
    {
        let p = session.project().part(part)?.clone();
        let results = session.rebuilt(part)?;
        rebuild::check_not_reference(&proposed(&p, order, &[(id, f)]), id, f, results)?;
    }
    // A loft's sections are matched against the rebuild whatever they
    // reference, and what a feature acts on must be a solid (ryucad 18).
    // Only a feature whose file says so makes a sheet, so a part holding
    // none needs no rebuild to judge a feature that wants solids; one that
    // wants a sheet there names a solid, which the rebuild must refuse.
    let may_hold_sheets = session
        .project()
        .part(part)?
        .features
        .values()
        .any(|g| g.as_ref().is_ok_and(|g| g.may_make_sheet()));
    let read = crate::kind::bodies_read(f);
    let reads_bodies = !read.is_empty()
        && (may_hold_sheets
            || read
                .iter()
                .any(|r| r.want == Some(crate::kind::Want::Sheet)));
    let planar = matches!(f, Feature::Planar(_));
    // An unfold's allowance and a refold's unfold are judged against the
    // rebuild whatever they reference.
    if f.loft().is_none()
        && f.unfold().is_none()
        && f.refold().is_none()
        && f.references().is_empty()
        && f.geometry().is_empty()
        && !reads_bodies
        && !planar
        && !matches!(f, Feature::Split(s) if s.plane().is_none())
    {
        return Ok(Vec::new());
    }
    let p = session.project().part(part)?.clone();
    let results = session.rebuilt(part)?;
    let q = proposed(&p, order, &[(id, f)]);
    let checked = crate::kind::check(&q, order, id, f, results)
        .and_then(|()| check_resolved(&p, order, id, f, results));
    let followed = match &checked {
        Ok(()) => crate::reference::followed_by(&q, &Solids::of_order(&q, order), id, f, results),
        Err(_) => Vec::new(),
    };
    checked.map_err(|e| session.explain_in(part, order, e))?;
    Ok(followed)
}

/// [`check_references`] against `results`.
fn check_resolved(
    p: &Part,
    order: &[Id],
    id: &Id,
    f: &Feature,
    results: &crate::rebuild::PartResult,
) -> Result<()> {
    match f {
        Feature::Sketch(s) => crate::sketch::context(p, order, id, s, results).map(|_| ()),
        // What the target is; how far it is only rebuild can tell.
        Feature::Extrude(e) => match &e.extent {
            Extent::UpTo { target, .. } => check_up_to(p, order, id, "target", target, results),
            _ => Ok(()),
        },
        Feature::Hole(h) => match &h.depth {
            crate::doc::hole::Depth::UpTo { target, .. } => {
                check_up_to(p, order, id, "depth", target, results)
            }
            _ => Ok(()),
        },
        // What its directions, axis and plane are; where the instances
        // land only rebuild can tell.
        Feature::Pattern(x) => {
            let q = proposed(p, order, &[(id, f)]);
            crate::pattern::motions(&q, id, x, results).map(|_| ())
        }
        // Edges of one solid, a face where a face goes, no edge twice; what
        // the kernel makes of them only rebuild can tell.
        Feature::Fillet(_) | Feature::Chamfer(_) => {
            rebuild::check_blend(&proposed(p, order, &[(id, f)]), id, f, results)
        }
        // That it is an axis; whether it lies in the sketch's plane only
        // rebuild can tell.
        Feature::Revolve(r) => match &r.axis {
            RevolveAxis::Ref(g) => {
                let at = crate::geometry::At {
                    part: p,
                    order,
                    owner: id,
                    results,
                };
                crate::geometry::one(&at, "axis", g, crate::geometry::Want::Axis).map(|_| ())
            }
            RevolveAxis::Line(_) => Ok(()),
        },
        // That each reference is the kind its field wants; what they make
        // of each other (parallel, collinear) only rebuild can tell.
        Feature::Datum(d) => crate::datum::check(p, order, id, d, results),
        Feature::Path(x) => crate::path::check(p, order, id, x, results),
        Feature::Sweep(w) => crate::along::check(p, order, id, w, results),
        Feature::Loft(l) => crate::loft::check(p, order, id, l, results),
        // A planar face's profile is closed loops: its open curves are
        // refused when written (what else fails is rebuild's).
        Feature::Planar(x) => match crate::profile::planar(p, id, x, results) {
            Err(e) if e.code == Code::ProfileInvalid => Err(e),
            _ => Ok(()),
        },
        // Faces of its target, each once; what the kernel makes of them
        // only rebuild can tell.
        Feature::Shell(_) | Feature::Offset(_) | Feature::Mirror(_) | Feature::Split(_) => {
            crate::body::check(&proposed(p, order, &[(id, f)]), id, f, results)
        }
        // Faces of its target, its direction, axis or surfaces their
        // kinds; what the kernel makes of them only rebuild can tell.
        Feature::MoveFace(_)
        | Feature::ReplaceFace(_)
        | Feature::Draft(_)
        | Feature::DeleteFace(_)
        | Feature::DeleteBlend(_)
        | Feature::ResizeBlend(_) => {
            crate::tweak::check(&proposed(p, order, &[(id, f)]), id, f, results)
        }
        Feature::BaseFlange(_) | Feature::Flange(_) | Feature::Unfold(_) | Feature::Refold(_) => {
            crate::sheet_metal::check(&proposed(p, order, &[(id, f)]), id, f, results)
        }
        // Its sides edges and curves, its faces of one body; whether they
        // close up, or extract cleanly, only rebuild can tell.
        Feature::Fill(x) => {
            crate::surface::check_fill(&proposed(p, order, &[(id, f)]), id, x, results)
        }
        Feature::Extract(x) => {
            crate::surface::check_extract(&proposed(p, order, &[(id, f)]), id, x, results)
        }
        // Its sheet or faces; what the kernel makes of them only rebuild
        // can tell. A sew's sheets are judged by kind and order already.
        Feature::Thicken(x) => {
            crate::surface::check_thicken(&proposed(p, order, &[(id, f)]), id, x, results)
        }
        _ => Ok(()),
    }
}

/// Refuses an up-to target (`role` in feature `id`) that isn't a plane
/// (a planar face, a datum plane) or a point (a vertex, a datum point) now.
fn check_up_to(
    p: &Part,
    order: &[Id],
    id: &Id,
    role: &str,
    target: &crate::doc::geometry::GeomRef,
    results: &crate::rebuild::PartResult,
) -> Result<()> {
    use crate::geometry::{At, Want, resolve};
    let at = At {
        part: p,
        order,
        owner: id,
        results,
    };
    resolve(&at, role, target, &[Want::Point, Want::Plane]).map(|_| ())
}

/// Every feature type `feature.add` makes.
const FEATURE_TYPES: [&str; 28] = [
    "box",
    "cylinder",
    "cone",
    "sphere",
    "torus",
    "sketch",
    "extrude",
    "revolve",
    "sweep",
    "loft",
    "planar",
    "fill",
    "extract",
    "sew",
    "thicken",
    "boolean",
    "fillet",
    "chamfer",
    "hole",
    "pattern",
    "instance",
    "mate",
    "instance_pattern",
    "insert",
    "delete_body",
    "plane",
    "axis",
    "point",
];

#[allow(clippy::too_many_arguments)]
pub(crate) fn feature_add(
    session: &mut Session,
    part: &str,
    ty: &str,
    mut given: Given,
    id: Option<&str>,
    position: &Position,
    expected: Option<&str>,
    dry_run: bool,
) -> Result<Value> {
    let pid = part_id(part)?;
    let unit = session.project().manifest.default_unit;
    let p = session.project().part(&pid)?;
    check_revision(p, expected)?;
    let mut list = order(p)?;
    let env = p.env.clone();
    let at = insert_at(&list, position)?;
    // A feature its part can't hold is refused before its fields are read.
    if FEATURE_TYPES.contains(&ty) {
        let fid = match id {
            Some(id) => feature_id(id)?,
            None => {
                let taken: BTreeSet<&str> = p.features.keys().map(Id::as_str).collect();
                Id::generate(ty, |s| taken.contains(s))
            }
        };
        if let Some(e) = crate::assembly::kind_error_of(p, &fid, ty) {
            return Err(e);
        }
    }
    // A pattern with no targets acts on its first seed's.
    if ty == "pattern" && given.fields.targets.is_none() {
        let seed = given.fields.features.as_ref().and_then(|s| s.first());
        let theirs = seed
            .and_then(|s| Id::parse(s, "feature").ok())
            .and_then(|s| p.features.get(&s).cloned())
            .and_then(|f| f.ok());
        match (seed, theirs) {
            (_, Some(f)) if !f.targets().is_empty() => {
                given.fields.targets = Some(f.targets().iter().map(|t| t.to_string()).collect());
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
    }
    // A mirror whose operation is left out joins its own source (design
    // 327, question 20); one with no solid copies the part's one solid.
    // The file names both all the same.
    if ty == "mirror" && given.fields.solid.is_none() {
        let before = solids_before(p, &list, at);
        match before.as_slice() {
            [one] => given.fields.solid = Some(one.to_string()),
            _ => {
                return Err(Error::new(
                    Code::InvalidParam,
                    format!(
                        "a mirror names the solid it copies (solid): the part has {} before it",
                        before.len()
                    ),
                )
                .with("param", "solid")
                .with("solids", before));
            }
        }
    }
    // A sheet's copy can't be joined: left out, its operation is new
    // (design 388).
    if ty == "mirror" && given.fields.operation.is_none() {
        let sheet = given
            .fields
            .solid
            .as_deref()
            .and_then(|x| Id::parse(x, "feature").ok())
            .is_some_and(|x| source_is_sheet(session, &pid, &list, at, &x));
        if sheet {
            given.fields.operation = Some("new".into());
        } else {
            given.fields.operation = Some("join".into());
            if given.fields.targets.is_none() {
                given.fields.targets = given.fields.solid.clone().map(|s| vec![s]);
            }
        }
    }
    // An insert may look up its source's newest label: read it before
    // the part is borrowed for the rest.
    let mut took = None;
    let mut inserted = None;
    if ty == "insert" {
        given.only(ty, &INSERT)?;
        let (ins, t) = insert_from(session, None, &given, unit, &env)?;
        (inserted, took) = (Some(ins), t);
    }
    let p = session.project().part(&pid)?;
    // A join, cut, intersect or hole with no targets acts on the part's
    // one solid; the file names it all the same.
    let acts = ty == "hole"
        || given
            .fields
            .operation
            .as_deref()
            .is_some_and(|o| o != "new");
    if acts && given.fields.targets.is_none() {
        let before = solids_before(p, &list, at);
        match before.as_slice() {
            [one] => given.fields.targets = Some(vec![one.to_string()]),
            _ => {
                return Err(Error::new(
                    Code::InvalidParam,
                    format!(
                        "a {} names the solids it acts on (targets): the part has {} before it",
                        if ty == "hole" {
                            "hole"
                        } else {
                            given.fields.operation.as_deref().unwrap_or_default()
                        },
                        before.len()
                    ),
                )
                .with("param", "targets")
                .with("solids", before));
            }
        }
    }
    // A tweak sorts what it was given; with no target it acts on the part's
    // one solid, else the one its faces name. The file names it all the same.
    if crate::tweak::TYPES.contains(&ty) {
        crate::tweak::take_given(ty, &mut given.fields)?;
        if given.fields.target.is_none() && given.fields.targets.is_none() {
            let before = solids_before(p, &list, at);
            given.fields.target = Some(crate::tweak::default_target(
                ty,
                &given.fields,
                &before,
                &pid,
            )?);
        }
    }
    // A shell or offset with no target acts on the part's one solid; the
    // file names it all the same.
    if matches!(ty, "shell" | "offset" | "split")
        && given.fields.target.is_none()
        && given.fields.targets.is_none()
    {
        let before = solids_before(p, &list, at);
        match before.as_slice() {
            [one] => given.fields.target = Some(one.to_string()),
            _ => {
                return Err(Error::new(
                    Code::InvalidParam,
                    format!(
                        "a {ty} names the solid it acts on (target): the part has {} before it",
                        before.len()
                    ),
                )
                .with("param", "target")
                .with("solids", before));
            }
        }
    }
    if matches!(ty, "flange" | "unfold" | "refold")
        && given.fields.target.is_none()
        && given.fields.targets.is_none()
    {
        let before = solids_before(p, &list, at);
        let sheet: Vec<Id> = before
            .iter()
            .filter(|s| crate::sheet_metal::founder(p, s).is_some())
            .cloned()
            .collect();
        match (before.as_slice(), sheet.as_slice()) {
            ([one], _) | (_, [one]) => given.fields.target = Some(one.to_string()),
            _ => {
                return Err(Error::new(
                    Code::InvalidParam,
                    format!("a {ty} names the solid it acts on (target): the part has {} solids before it, {} of them founded by a base flange", before.len(), sheet.len()),
                )
                .with("param", "target")
                .with("solids", before));
            }
        }
    }
    let f = match ty {
        "sketch" => {
            given.only(ty, &SKETCH)?;
            Feature::Sketch(Sketch::new(sketch_plane(None, &given, &pid, unit, &env)?))
        }
        "extrude" | "revolve" => {
            given.only(ty, if ty == "extrude" { &EXTRUDE } else { &REVOLVE })?;
            sweep_from(ty, None, &given, &pid, unit, &env)?
        }
        "sweep" => {
            given.only(ty, &crate::doc::sweep::SWEEP_FIELDS)?;
            crate::along::from_given(None, &given.fields, p, unit, &env)?
        }
        "loft" => {
            given.only(ty, &crate::doc::sweep::LOFT_FIELDS)?;
            crate::loft::from_given(None, &given.fields, p, unit, &env)?
        }
        "planar" => {
            given.only(ty, &crate::doc::sweep::PLANAR_FIELDS)?;
            planar_from(None, &given, &pid)?
        }
        "fill" | "extract" | "sew" | "thicken" => {
            given.only(ty, crate::doc::surface::request_fields(ty))?;
            crate::surface::from_given(ty, None, &given.fields, &pid, unit, &env)?
        }
        "boolean" => {
            given.only(ty, &BOOLEAN)?;
            Feature::Boolean(boolean_from(None, &given)?)
        }
        "instance" => {
            given.only(ty, &INSTANCE)?;
            let deps = session.project().manifest.dependencies.clone();
            Feature::Instance(instance_from(None, &given, unit, &env, &deps)?)
        }
        "insert" => Feature::Insert(Box::new(
            inserted.take().expect("read before the part was borrowed"),
        )),
        "delete_body" => {
            given.only(ty, &DELETE_BODY)?;
            Feature::DeleteBody(Box::new(delete_from(&given)?))
        }
        "mate" => {
            given.only(ty, &MATE)?;
            mate_from(None, &given, (session.project(), &pid), unit, &env)?
        }
        "instance_pattern" => {
            given.only(ty, &INSTANCE_PATTERN)?;
            instance_pattern_from(None, &given, &pid, unit, &env)?
        }
        "fillet" | "chamfer" => {
            given.only(ty, if ty == "fillet" { &FILLET } else { &CHAMFER })?;
            blend_from(ty, None, &given, &pid, unit, &env)?
        }
        "shell" | "offset" | "mirror" | "split" => {
            given.only(ty, crate::doc::body::request_fields(ty))?;
            crate::body::from_given(ty, None, &given.fields, &pid, unit, &env)?
        }
        "move_face" | "replace_face" | "draft" | "delete_face" | "delete_blend"
        | "resize_blend" => {
            given.only(ty, crate::tweak::request_fields(ty))?;
            crate::tweak::from_given(ty, None, &given.fields, &pid, unit, &env)?
        }
        "base_flange" | "flange" | "unfold" | "refold" => {
            crate::sheet_metal::take_given(ty, &mut given.fields);
            given.only(ty, crate::doc::sheet_metal::request_fields(ty))?;
            crate::sheet_metal::from_given(ty, None, &given.fields, &pid, unit, &env)?
        }
        "hole" => {
            given.only(ty, &HOLE)?;
            let f = hole_from(None, &given, &pid, unit, &env)?;
            refuse_underived(session, &pid, &f)?;
            f
        }
        "pattern" => {
            given.only(ty, &PATTERN)?;
            pattern_from(None, &given, &pid, unit, &env)?
        }
        "import" => return Err(import_not_written_here("add")),
        "plane" | "axis" | "point" => {
            given.only(ty, crate::datum::request_fields(ty))?;
            crate::datum::from_given(ty, None, &given.fields, &pid, unit, &env)?
        }
        "path" | "helix" => {
            given.only(ty, crate::doc::path::request_fields(ty))?;
            crate::path::from_given(ty, None, &given.fields, &pid, unit, &env)?
        }
        _ => {
            let ty = PrimitiveType::parse(ty)?;
            primitive_fields(ty, &mut given)?;
            let combine = combine_from(&Combine::NEW, &given)?;
            Feature::Primitive(Primitive {
                ty,
                params: Primitive::params_from_json(ty, &Value::Object(given.params), unit, &env)?,
                placement: new_placement(given.placement.unwrap_or_default(), unit, &env)?,
                combine,
            })
        }
    };
    let fid = match id {
        Some(id) => feature_id(id)?,
        None => {
            // A set, not a scan per candidate: in a list of thousands the
            // first free number is thousands of candidates in (issue #25).
            let taken: BTreeSet<&str> = p.features.keys().map(Id::as_str).collect();
            Id::generate(f.type_name(), |s| taken.contains(s))
        }
    };
    if p.features.contains_key(&fid) {
        return Err(Error::new(
            Code::DuplicateId,
            format!("part {pid} already has a feature {fid}"),
        )
        .with("feature", &fid));
    }
    list.insert(at, fid.clone());
    if let Some(e) = crate::assembly::kind_error(p, &fid, &f) {
        return Err(e);
    }
    if f.instance().is_some() {
        check_instance(session, &pid, &list, &fid, &f)?;
    }
    if f.insert().is_some() {
        check_insert(session, &pid, &list, &fid, &f)?;
    }
    check_delete(session, &pid, &list, &fid, &f)?;
    // What its references follow: a split face or edge (design 447).
    let mut followed = Vec::new();
    if !f.connectors().is_empty() || f.instance_pattern().is_some() {
        crate::mate::check(session, &pid, &list, &fid, &f)?;
        let r = session.rebuilt(&pid)?;
        followed.extend(crate::mate::followed(&pid, &fid, &f, r));
    }
    let p = session.project().part(&pid)?;
    if !f.profiles().is_empty() {
        check_sweep(p, &list, &fid, &f)?;
    }
    check_solids(&proposed(p, &list, &[(&fid, &f)]), &[&fid])?;
    if let Feature::Pattern(x) = &f {
        crate::pattern::check_seeds(&proposed(p, &list, &[(&fid, &f)]), &fid, x)?;
    }
    followed.extend(check_references(session, &pid, &list, &fid, &f)?);
    if crate::assembly::assembles(&f) {
        let p = session.project().part(&pid)?;
        let q = proposed(p, &list, &[(&fid, &f)]);
        crate::mate::refuse_overconstrained(session, q)?;
    }
    if dry_run {
        return dry(session, &pid, &fid, &f);
    }
    let assemblies = crate::mate::current(session, &pid)?;
    save(session, &pid, &[(&fid, &f)], list)?;
    let assemblies = crate::mate::keep_current(session, &assemblies)?;
    let saved = session.project().part(&pid)?;
    let revision = &saved.revision;
    let mut v = json!({"feature": feature_report(saved, session.project().manifest.default_unit, &fid, &f), "revision": revision});
    if !assemblies.is_empty() {
        v["assemblies"] = json!(assemblies);
    }
    if !followed.is_empty() {
        v["warnings"] = json!(followed);
    }
    if let Some(t) = took {
        v["took"] = t;
    }
    Ok(v)
}

/// A feature with `expressions` set by JSON pointer, each one of its
/// expression fields, given alone; a sketch's dimensions are solved, so
/// they are set through `sketch.edit`.
fn edited_at(
    old: &Feature,
    given: &Given,
    expressions: &Map<String, Value>,
    unit: crate::units::Unit,
    env: &crate::expr::Env,
) -> Result<Feature> {
    if let Some(name) = given.present().into_iter().next() {
        return Err(Error::new(
            Code::InvalidParam,
            format!("give expressions by pointer alone, without {name}"),
        )
        .with("param", name));
    }
    if old.sketch().is_some() {
        return Err(Error::new(
            Code::InvalidParam,
            "a sketch's dimensions are solved: set them with sketch.edit",
        ));
    }
    if let Feature::Import(_) = old {
        return Err(import_not_written_here("edit"));
    }
    let mut v = old.to_json();
    let fields: Vec<String> = crate::doc::files::expression_fields(&v)
        .into_iter()
        .map(|(at, _)| at)
        .collect();
    for (at, x) in expressions {
        if !fields.contains(at) {
            return Err(Error::new(
                Code::InvalidParam,
                format!("{at} is no expression field of this feature"),
            )
            .with("pointer", at)
            .with("fields", &fields));
        }
        let e = crate::doc::files::expression(x).map_err(|e| e.with("pointer", at))?;
        *v.pointer_mut(at).expect("a field it has") = Value::String(e.text().to_owned());
    }
    Feature::from_json(&v, unit, env)
}

pub(crate) fn feature_edit(
    session: &mut Session,
    part: &str,
    id: &str,
    mut given: Given,
    expressions: &Map<String, Value>,
    expected: Option<&str>,
    dry_run: bool,
) -> Result<Value> {
    let (pid, fid) = (part_id(part)?, feature_id(id)?);
    let unit = session.project().manifest.default_unit;
    // An insert's edit may look up its source's newest label: read it
    // before the part is borrowed for the rest.
    let inserted = {
        let p = session.project().part(&pid)?;
        match feature(p, &fid)?.insert() {
            Some(i) if expressions.is_empty() => {
                let (i, env) = (i.clone(), p.env.clone());
                given.only("insert", &INSERT)?;
                Some(insert_from(session, Some(&i), &given, unit, &env)?.0)
            }
            _ => None,
        }
    };
    let p = session.project().part(&pid)?;
    check_revision(p, expected)?;
    let list = order(p)?;
    let old = feature(p, &fid)?.clone();
    let f = if !expressions.is_empty() {
        edited_at(&old, &given, expressions, unit, &p.env).map_err(|e| e.with("feature", &fid))?
    } else {
        match old.clone() {
            Feature::Primitive(mut f) => {
                primitive_fields(f.ty, &mut given)?;
                f.combine = combine_from(&f.combine, &given)?;
                let mut merged: Map<String, Value> = f
                    .params
                    .iter()
                    .map(|(k, q)| (k.clone(), Value::String(q.text())))
                    .collect();
                for (k, v) in given.params {
                    if v.is_null() {
                        merged.remove(&k);
                    } else {
                        merged.insert(k, v);
                    }
                }
                f.params = Primitive::params_from_json(f.ty, &Value::Object(merged), unit, &p.env)?;
                apply_placement(
                    &mut f.placement,
                    given.placement.unwrap_or_default(),
                    unit,
                    &p.env,
                )?;
                Feature::Primitive(f)
            }
            // Its geometry changes through sketch.edit; the plane moves the
            // sketch without changing its solve.
            Feature::Sketch(mut s) => {
                given.only("sketch", &SKETCH)?;
                s.plane = sketch_plane(Some(&s.plane), &given, &pid, unit, &p.env)?;
                Feature::Sketch(s)
            }
            Feature::Instance(i) => {
                given.only("instance", &INSTANCE)?;
                let deps = session.project().manifest.dependencies.clone();
                Feature::Instance(instance_from(Some(&i), &given, unit, &p.env, &deps)?)
            }
            Feature::Insert(_) => Feature::Insert(Box::new(
                inserted.clone().expect("read before the part was borrowed"),
            )),
            Feature::DeleteBody(d) => {
                given.only("delete_body", &DELETE_BODY)?;
                if given.fields.solids.is_none() && given.fields.solid.is_none() {
                    Feature::DeleteBody(d)
                } else {
                    Feature::DeleteBody(Box::new(delete_from(&given)?))
                }
            }
            Feature::Mate(m) => {
                given.only("mate", &MATE)?;
                mate_from(Some(&m), &given, (session.project(), &pid), unit, &p.env)?
            }
            Feature::InstancePattern(x) => {
                given.only("instance_pattern", &INSTANCE_PATTERN)?;
                instance_pattern_from(Some(&x), &given, &pid, unit, &p.env)?
            }
            Feature::Boolean(b) => {
                given.only("boolean", &BOOLEAN)?;
                Feature::Boolean(boolean_from(Some(&b), &given)?)
            }
            f @ (Feature::Fillet(_) | Feature::Chamfer(_)) => {
                let ty = f.type_name();
                given.only(ty, if ty == "fillet" { &FILLET } else { &CHAMFER })?;
                blend_from(ty, Some(&f), &given, &pid, unit, &p.env)?
            }
            f @ (Feature::Shell(_)
            | Feature::Offset(_)
            | Feature::Mirror(_)
            | Feature::Split(_)) => {
                let ty = f.type_name();
                given.only(ty, crate::doc::body::request_fields(ty))?;
                crate::body::from_given(ty, Some(&f), &given.fields, &pid, unit, &p.env)?
            }
            f @ (Feature::BaseFlange(_)
            | Feature::Flange(_)
            | Feature::Unfold(_)
            | Feature::Refold(_)) => {
                let ty = f.type_name();
                crate::sheet_metal::take_given(ty, &mut given.fields);
                given.only(ty, crate::doc::sheet_metal::request_fields(ty))?;
                crate::sheet_metal::from_given(ty, Some(&f), &given.fields, &pid, unit, &p.env)?
            }
            Feature::Pattern(x) => {
                given.only("pattern", &PATTERN)?;
                pattern_from(Some(&x), &given, &pid, unit, &p.env)?
            }
            Feature::Hole(h) => {
                given.only("hole", &HOLE)?;
                let f = hole_from(Some(&h), &given, &pid, unit, &p.env)?;
                refuse_underived(session, &pid, &f)?;
                check_sweep(p, &list, &fid, &f)?;
                f
            }
            Feature::Import(_) => return Err(import_not_written_here("edit").with("feature", &fid)),
            f
            @ (Feature::Fill(_) | Feature::Extract(_) | Feature::Sew(_) | Feature::Thicken(_)) => {
                let ty = f.type_name();
                given.only(ty, crate::doc::surface::request_fields(ty))?;
                crate::surface::from_given(ty, Some(&f), &given.fields, &pid, unit, &p.env)?
            }
            f @ (Feature::MoveFace(_)
            | Feature::ReplaceFace(_)
            | Feature::Draft(_)
            | Feature::DeleteFace(_)
            | Feature::DeleteBlend(_)
            | Feature::ResizeBlend(_)) => {
                let ty = f.type_name();
                crate::tweak::take_given(ty, &mut given.fields)?;
                given.only(ty, crate::tweak::request_fields(ty))?;
                crate::tweak::from_given(ty, Some(&f), &given.fields, &pid, unit, &p.env)?
            }
            Feature::Datum(d) => {
                let ty = d.type_name();
                given.only(ty, crate::datum::request_fields(ty))?;
                crate::datum::from_given(ty, Some(&d), &given.fields, &pid, unit, &p.env)?
            }
            Feature::Path(x) => {
                let ty = x.type_name();
                given.only(ty, crate::doc::path::request_fields(ty))?;
                crate::path::from_given(ty, Some(&x), &given.fields, &pid, unit, &p.env)?
            }
            Feature::Sweep(w) => {
                given.only("sweep", &crate::doc::sweep::SWEEP_FIELDS)?;
                let f = crate::along::from_given(Some(&w), &given.fields, p, unit, &p.env)?;
                if !f.profiles().is_empty() {
                    check_sweep(p, &list, &fid, &f)?;
                }
                f
            }
            Feature::Loft(l) => {
                given.only("loft", &crate::doc::sweep::LOFT_FIELDS)?;
                let f = crate::loft::from_given(Some(&l), &given.fields, p, unit, &p.env)?;
                check_sweep(p, &list, &fid, &f)?;
                f
            }
            Feature::Planar(x) => {
                given.only("planar", &crate::doc::sweep::PLANAR_FIELDS)?;
                let f = planar_from(Some(x.fields()), &given, &pid)?;
                if !f.profiles().is_empty() {
                    check_sweep(p, &list, &fid, &f)?;
                }
                f
            }
            f @ (Feature::Extrude(_) | Feature::Revolve(_)) => {
                let ty = f.type_name();
                given.only(ty, if ty == "extrude" { &EXTRUDE } else { &REVOLVE })?;
                let current = match &f {
                    Feature::Extrude(e) => e.fields(),
                    Feature::Revolve(r) => r.fields(),
                    _ => unreachable!("a sweep"),
                };
                let f = sweep_from(ty, Some(current), &given, &pid, unit, &p.env)?;
                check_sweep(p, &list, &fid, &f)?;
                f
            }
        }
    };
    // A solid's founder that stops founding it leaves what acts on it, or
    // refers to it, with nothing.
    let founded = |f: &Feature| f.founds();
    if founded(&old) && !founded(&f) {
        let users = crate::graph::dependents(p, &fid);
        if !users.is_empty() {
            return Err(Error::new(
                Code::HasDependents,
                format!(
                    "{fid} founds a solid that {} act on or refer to; change those first",
                    users.iter().map(Id::as_str).collect::<Vec<_>>().join(", ")
                ),
            )
            .with("feature", &fid)
            .with("features", users));
        }
    }
    if f.instance().is_some() && f != old {
        check_instance(session, &pid, &list, &fid, &f)?;
    }
    if f.insert().is_some() && f != old {
        check_insert(session, &pid, &list, &fid, &f)?;
    }
    check_delete(session, &pid, &list, &fid, &f)?;
    // What its references follow: a split face or edge (design 447).
    let mut followed = Vec::new();
    if (!f.connectors().is_empty() || f.instance_pattern().is_some()) && f != old {
        crate::mate::check(session, &pid, &list, &fid, &f)?;
        let r = session.rebuilt(&pid)?;
        followed.extend(crate::mate::followed(&pid, &fid, &f, r));
    }
    let p = session.project().part(&pid)?;
    check_solids(&proposed(p, &list, &[(&fid, &f)]), &[&fid])?;
    if let Feature::Pattern(x) = &f {
        crate::pattern::check_seeds(&proposed(p, &list, &[(&fid, &f)]), &fid, x)?;
    }
    // References already there were checked when they were written; a
    // fillet or chamfer that now grows its edges, or stops, may reach an
    // edge twice.
    let propagates = |f: &Feature| match f {
        Feature::Fillet(x) => x.propagate,
        Feature::Chamfer(x) => x.propagate,
        _ => false,
    };
    // A sweep's twist and scale are judged against its path's closing, a
    // loft's sections against each other.
    let sweep_changed = (f.sweep().is_some() || f.loft().is_some()) && f != old;
    if f.references() != old.references()
        || f.geometry() != old.geometry()
        || propagates(&f) != propagates(&old)
        || sweep_changed
        || (matches!(
            f,
            Feature::Shell(_)
                | Feature::Offset(_)
                | Feature::Mirror(_)
                | Feature::Split(_)
                | Feature::MoveFace(_)
                | Feature::ReplaceFace(_)
                | Feature::Draft(_)
                | Feature::DeleteFace(_)
                | Feature::DeleteBlend(_)
                | Feature::ResizeBlend(_)
        ) && f.targets() != old.targets())
        || ((f.base_flange().is_some()
            || f.flange().is_some()
            || f.unfold().is_some()
            || f.refold().is_some())
            && f != old)
        || (matches!(
            f,
            Feature::Fill(_) | Feature::Extract(_) | Feature::Sew(_) | Feature::Thicken(_)
        ) && f != old)
        || f.source() != old.source()
    {
        followed.extend(check_references(session, &pid, &list, &fid, &f)?);
    }
    if crate::assembly::assembles(&f) {
        let p = session.project().part(&pid)?;
        let q = proposed(p, &list, &[(&fid, &f)]);
        crate::mate::refuse_overconstrained(session, q)?;
    }
    if dry_run {
        return dry(session, &pid, &fid, &f);
    }
    let before = crate::sketch::current(session, &pid)?;
    let assemblies = crate::mate::current(session, &pid)?;
    save(session, &pid, &[(&fid, &f)], list)?;
    let sketches = crate::sketch::keep_current(session, &pid, &before)?;
    let assemblies = crate::mate::keep_current(session, &assemblies)?;
    // Each assembly the edit reaches whose mates fail now, whether the
    // edit broke them or drives an assembly they already left half
    // solved (issue #155).
    let reached = crate::mate::reached(session.project(), &pid);
    let mut warnings = crate::mate::failing_warnings(session, &reached, "");
    warnings.extend(followed);
    let saved = session.project().part(&pid)?;
    let revision = &saved.revision;
    let mut v = json!({"feature": feature_report(saved, session.project().manifest.default_unit, &fid, &f), "revision": revision});
    if !sketches.is_empty() {
        v["sketches"] = json!(sketches);
    }
    if !assemblies.is_empty() {
        v["assemblies"] = json!(assemblies);
    }
    if !warnings.is_empty() {
        v["warnings"] = json!(warnings);
    }
    Ok(v)
}

pub(crate) fn feature_delete(
    session: &mut Session,
    part: &str,
    id: &str,
    expected: Option<&str>,
) -> Result<Value> {
    let (pid, fid) = (part_id(part)?, feature_id(id)?);
    let p = session.project().part(&pid)?;
    check_revision(p, expected)?;
    // A broken feature file can still be deleted.
    if !p.features.contains_key(&fid) {
        return Err(no_feature(p, &fid));
    }
    let users = crate::graph::dependents(p, &fid);
    if !users.is_empty() {
        return Err(Error::new(
            Code::HasDependents,
            format!(
                "{fid} is read by {}; delete or change those first",
                users.iter().map(Id::as_str).collect::<Vec<_>>().join(", ")
            ),
        )
        .with("feature", &fid)
        .with("features", users));
    }
    let mut list = order(p)?;
    list.retain(|f| *f != fid);
    if part_kind(p) == PartKind::Assembly {
        let q = proposed(p, &list, &[]);
        crate::mate::refuse_overconstrained(session, q)?;
    }
    let assemblies = crate::mate::current(session, &pid)?;
    // The list first: a crash then leaves an unlisted file, never a listed
    // feature with no file.
    save(session, &pid, &[], list)?;
    let path = feature_path(session.root(), &pid, &fid);
    session.remove_file(&path)?;
    session.reload()?;
    let assemblies = crate::mate::keep_current(session, &assemblies)?;
    let revision = &session.project().part(&pid)?.revision;
    let mut v = json!({"deleted": fid, "revision": revision});
    if !assemblies.is_empty() {
        v["assemblies"] = json!(assemblies);
    }
    Ok(v)
}

pub(crate) fn feature_move(
    session: &mut Session,
    part: &str,
    id: &str,
    position: &Position,
    expected: Option<&str>,
) -> Result<Value> {
    let (pid, fid) = (part_id(part)?, feature_id(id)?);
    let p = session.project().part(&pid)?;
    check_revision(p, expected)?;
    let mut list = order(p)?;
    let from = list
        .iter()
        .position(|f| *f == fid)
        .ok_or_else(|| no_feature(p, &fid))?;
    if position.after.is_none() && !position.first {
        return Err(Error::new(Code::Usage, "give after or first"));
    }
    if position.after.as_deref() == Some(fid.as_str()) {
        return Err(Error::new(Code::Usage, "a feature can't move after itself"));
    }
    list.remove(from);
    list.insert(insert_at(&list, position)?, fid.clone());
    // Everything must still come after what it reads, and act only on
    // solids not yet consumed.
    for f in &list {
        if let Some(Ok(feature)) = p.features.get(f)
            && (*f == fid
                || crate::graph::inputs(feature)
                    .iter()
                    .any(|i| i.founder_str() == fid.as_str()))
        {
            crate::graph::check_order(p, &list, f, feature)?;
        }
    }
    check_solids(&proposed(p, &list, &[]), &[&fid])?;
    let revision = save(session, &pid, &[], list.clone())?;
    Ok(json!({"features": list, "revision": revision}))
}

/// `feature.list`: each feature as listed, `ok` when it builds (issue
/// #123: a file that loads may still fail to build), with the rebuild's
/// error where it doesn't and `blocked` where only a feature it needs
/// failed. A part that can't be rebuilt at all lists what loads.
pub(crate) fn feature_list(session: &mut Session, part: &str) -> Result<Value> {
    let pid = part_id(part)?;
    let built: BTreeMap<Id, std::result::Result<(), Error>> = match session.rebuilt(&pid) {
        Ok(r) => r
            .features
            .iter()
            .map(|(id, o)| (id.clone(), o.as_ref().map(|_| ()).map_err(Error::clone)))
            .collect(),
        Err(_) => BTreeMap::new(),
    };
    let p = session.project().part(&pid)?;
    let features: Vec<Value> = order(p)?
        .iter()
        .map(|id| match feature(p, id) {
            Ok(f) => {
                let mut v = json!({"id": id, "type": f.type_name(), "ok": true});
                if let Some(i) = f.insert() {
                    let r = crate::insert::report(i, None);
                    v["of"] = r["of"].clone();
                    v["rev"] = r["rev"].clone();
                }
                if let Some(Err(e)) = built.get(id) {
                    v["ok"] = json!(false);
                    v["error"] = json!(e);
                    if e.code == Code::DependencyFailed {
                        v["blocked"] = json!(true);
                    }
                }
                // A solid key below an insert is used through the insert.
                let mut uses: Vec<Id> = Vec::new();
                for i in crate::graph::inputs(f) {
                    let i = i.founder();
                    if !uses.contains(&i) {
                        uses.push(i);
                    }
                }
                if !uses.is_empty() {
                    v["uses"] = json!(uses);
                }
                v
            }
            Err(e) => {
                let mut v = json!({"id": id, "ok": false, "error": e});
                if let Some(t) = e.details.get("type").filter(|t| t.is_string()) {
                    v["type"] = t.clone();
                }
                v
            }
        })
        .collect();
    Ok(json!({"part": p.id, "features": features, "orphans": p.orphans(), "revision": p.revision}))
}

/// `param.list`.
pub(crate) fn param_list(
    session: &mut Session,
    part: Option<String>,
    scope: Option<crate::param::Scope>,
    rev: Option<String>,
) -> Result<Value> {
    let (_, session) = session.at(rev.as_deref())?;
    match param::Target::of(part, scope)? {
        param::Target::Part(p) => param::list(session, &p),
        param::Target::Project => param::list_project(session),
    }
}

/// `param.set`.
pub(crate) fn param_set(
    session: &mut Session,
    part: Option<String>,
    scope: Option<crate::param::Scope>,
    parameters: Map<String, Value>,
    expected_revision: Option<String>,
) -> Result<Value> {
    match param::Target::of(part, scope)? {
        param::Target::Part(p) => param::set(session, &p, parameters, expected_revision.as_deref()),
        param::Target::Project => {
            param::set_project(session, parameters, expected_revision.as_deref())
        }
    }
}

/// `param.delete`.
pub(crate) fn param_delete(
    session: &mut Session,
    part: Option<String>,
    scope: Option<crate::param::Scope>,
    name: String,
    expected_revision: Option<String>,
) -> Result<Value> {
    match param::Target::of(part, scope)? {
        param::Target::Part(p) => param::delete(session, &p, &name, expected_revision.as_deref()),
        param::Target::Project => {
            param::delete_project(session, &name, expected_revision.as_deref())
        }
    }
}

/// `param.rename`.
pub(crate) fn param_rename(
    session: &mut Session,
    part: Option<String>,
    scope: Option<crate::param::Scope>,
    from: String,
    to: String,
    expected_revision: Option<String>,
) -> Result<Value> {
    match param::Target::of(part, scope)? {
        param::Target::Part(p) => {
            param::rename(session, &p, &from, &to, expected_revision.as_deref())
        }
        param::Target::Project => {
            param::rename_project(session, &from, &to, expected_revision.as_deref())
        }
    }
}

/// `sketch.get`.
pub(crate) fn sketch_get(
    session: &mut Session,
    part: String,
    sketch: String,
    quality: Option<String>,
    rev: Option<String>,
) -> Result<Value> {
    let (_, session) = session.at(rev.as_deref())?;
    let quality = match quality {
        Some(q) => mesh::Quality::parse(&q)?,
        None => mesh::Quality::preferred(),
    };
    crate::sketch::get(session, &part, &sketch, quality)
}

/// `path.get`.
pub(crate) fn path_get(
    session: &mut Session,
    part: String,
    path: String,
    quality: Option<String>,
    rev: Option<String>,
) -> Result<Value> {
    let (_, session) = session.at(rev.as_deref())?;
    let quality = match quality {
        Some(q) => mesh::Quality::parse(&q)?,
        None => mesh::Quality::preferred(),
    };
    crate::path::get(session, &part, &path, quality)
}

/// `sweep.get`.
pub(crate) fn sweep_get(
    session: &mut Session,
    part: String,
    feature: String,
    quality: Option<String>,
    rev: Option<String>,
) -> Result<Value> {
    let (_, session) = session.at(rev.as_deref())?;
    let quality = match quality {
        Some(q) => mesh::Quality::parse(&q)?,
        None => mesh::Quality::preferred(),
    };
    crate::sweep_view::get(session, &part, &feature, quality)
}

/// `feature.show`.
pub(crate) fn feature_show(
    session: &mut Session,
    part: String,
    id: String,
    rev: Option<String>,
) -> Result<Value> {
    let (_, session) = session.at(rev.as_deref())?;
    let (part, fid) = (part_id(&part)?, feature_id(&id)?);
    let p = session.project().part(&part)?;
    let unit = session.project().manifest.default_unit;
    let mut v = feature_report(p, unit, &fid, feature(p, &fid)?);
    // The revision it was read at: an edit made from it says so.
    v["revision"] = json!(p.revision);
    Ok(v)
}
