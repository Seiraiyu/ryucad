//! Face tweaks (design 400, plan 17.A): a solid's faces moved
//! (translated, turned about an axis, offset along their normals) or
//! given another surface, by one kernel call under the feature's key and
//! the request's cancel. A tweak acts on its target: it joins its chain,
//! what it leaves is the target, and it keeps every face, edge and vertex
//! and their names and origins. It takes a sheet where the kernel does.

use crate::doc::files::Feature;
use crate::doc::geometry::GeomRef;
use crate::doc::id::Id;
use crate::doc::project::Part;
use crate::doc::reference::Reference;
use crate::doc::tweak::{MoveBy, MoveFace};
use crate::error::{Code, Error, Result};
use crate::geometry::{At, Geom, Line, Want};
use crate::proto::FeatureFields;
use crate::rebuild::{Built, Made, Results, Run};
use crate::solid::Solids;
use ryukei::geom::{Rigid, Surface, UnitVector3};
use ryukei::name::{Event, Name};
use ryukei::topo::EntityId;
// 17.A's tweaks keep the body's topology (`Topology::Keep`); changing it
// (ryukei 13's re-trim) is a later plan's.
use ryukei::tweak::{MoveSpec, OffsetFacesSpec, Topology};
use serde_json::{Value, json};
use std::sync::Arc;

/// Whether `f` may take a sheet as its target.
pub fn takes_sheets(f: &Feature) -> bool {
    matches!(f, Feature::MoveFace(_) | Feature::ReplaceFace(_))
}

/// What a tweak asks the kernel.
pub(crate) enum Spec {
    Move(MoveSpec),
    Offset(OffsetFacesSpec),
    Replace(ryukei::tweak::ReplaceSpec),
}

/// A tweak resolved in its view: its target as it sees it, and the spec.
pub(crate) struct Resolved<'a> {
    pub target: &'a Id,
    pub built: &'a Built,
    pub spec: Spec,
}

/// Right-handed turn by `angle` radians about the axis `l`, where it lies.
pub(crate) fn about(l: &Line, angle: f64) -> Rigid {
    let c = l.origin.to_vector();
    Rigid::translation(c)
        .then_after(&Rigid::rotation(l.dir, angle))
        .then_after(&Rigid::translation(-c))
}

/// A move of `d` meters along unit `u`.
pub(crate) fn along(u: UnitVector3, d: f64) -> Rigid {
    Rigid::translation(u.get() * d)
}

/// Where a tweak resolves its geometry: its part, as listed, in its view.
fn at<'a>(part: &'a Part, id: &'a Id, results: &'a dyn Results) -> At<'a> {
    At {
        part,
        order: part.order(),
        owner: id,
        results,
    }
}

/// A move's rigid motion, resolved in its view; `None` for an offset.
pub(crate) fn motion(
    part: &Part,
    id: &Id,
    m: &MoveFace,
    results: &dyn Results,
) -> Result<Option<Rigid>> {
    let at = at(part, id, results);
    match &m.by {
        MoveBy::Translate {
            direction,
            distance,
        } => {
            let v = match crate::geometry::resolve(
                &at,
                "direction",
                direction,
                &[Want::Axis, Want::Plane],
            )? {
                Geom::Axis(l) => l.dir.get(),
                Geom::Plane(p) => p.normal,
                _ => unreachable!("asked for an axis or a plane"),
            };
            let u = v.normalize().ok_or_else(|| {
                at.fail(
                    Code::ReferenceWrongKind,
                    "direction",
                    direction,
                    "gives no direction".to_owned(),
                )
            })?;
            Ok(Some(along(u, distance.meters())))
        }
        MoveBy::Rotate { axis, angle } => {
            let l = crate::geometry::one(&at, "axis", axis, Want::Axis)?.axis();
            Ok(Some(about(&l, angle.radians())))
        }
        MoveBy::Offset { .. } => Ok(None),
    }
}

/// The face `r` names (in `role` of tweak `id`) in the target's view:
/// `targets_mixed` for another solid's, `reference_wrong_kind` for an edge
/// or vertex, and whatever the reference fails with.
fn face(
    part: &Part,
    id: &Id,
    target: &Id,
    ty: &str,
    role: &str,
    r: &Reference,
    results: &dyn Results,
) -> Result<Name> {
    if r.feature != *target {
        return Err(Error::new(
            Code::TargetsMixed,
            format!(
                "{ty} {id}: its {role} {} is a face of {}, not of its target {target}; a {ty} acts on one solid",
                r.text(&part.id),
                r.feature
            ),
        )
        .with("feature", id)
        .with("role", role)
        .with("solids", [target.as_str(), r.feature.as_str()]));
    }
    let (_, e) = crate::reference::resolve(part, part.order(), id, role, r, results)?;
    match e {
        EntityId::Face(_) => Ok(r.name),
        e => Err(crate::reference::wrong_kind(
            part,
            id,
            role,
            r,
            crate::reference::kind(e),
            "a face",
        )),
    }
}

/// The surface `with` names (in `role` of tweak `id`): a face's, whole,
/// as its body holds it (any solid of the part, the target's own too, in
/// the feature's view); else a plane's, through its frame.
fn surface(
    part: &Part,
    id: &Id,
    role: &str,
    with: &GeomRef,
    results: &dyn Results,
) -> Result<Surface> {
    if let Some(r) = with.reference() {
        let (b, e) = crate::reference::resolve(part, part.order(), id, role, r, results)?;
        return match e {
            EntityId::Face(f) => {
                let face = b.body.face(f).expect("a live face");
                Ok(b.body.surface(face.surface).expect("its surface").clone())
            }
            e => Err(crate::reference::wrong_kind(
                part,
                id,
                role,
                r,
                crate::reference::kind(e),
                "a face, or a plane",
            )),
        };
    }
    let p = crate::geometry::one(&at(part, id, results), role, with, Want::Plane)?.plane();
    Ok(Surface::Plane(ryukei::geom::Plane::new(p.frame)))
}

/// Resolves a tweak (feature `id` of `part`): its target, through
/// [`crate::rebuild::target_kinds`] taking sheets, its faces and its
/// motion or surfaces, everything in its view.
pub(crate) fn resolve<'a>(
    part: &Part,
    solids: &Solids,
    id: &Id,
    f: &'a Feature,
    before: &'a dyn Results,
) -> Result<Resolved<'a>> {
    let ty = f.type_name();
    match f {
        Feature::MoveFace(m) => {
            let built =
                crate::rebuild::target_kinds(part, solids, id, "target", &m.target, before, true)?;
            let mut faces: Vec<(Name, f64)> = Vec::new();
            let d = match &m.by {
                MoveBy::Offset { distance, .. } => distance.meters(),
                _ => 0.0,
            };
            for (role, r) in m.faces() {
                let n = face(part, id, &m.target, ty, &role, r, before)?;
                // A grouped face's distance is its group's.
                let q = match (&m.by, role.strip_prefix("groups.")) {
                    (MoveBy::Offset { groups, .. }, Some(rest)) => {
                        let i: usize = rest
                            .split('.')
                            .next()
                            .and_then(|s| s.parse().ok())
                            .expect("groups.<i>");
                        groups[i].quantity.meters()
                    }
                    _ => d,
                };
                faces.push((n, q));
            }
            let spec = match motion(part, id, m, before)? {
                Some(motion) => Spec::Move(MoveSpec {
                    faces: faces.into_iter().map(|(n, _)| n).collect(),
                    motion,
                    tangent: m.tangent,
                    topology: Topology::Keep,
                }),
                None => Spec::Offset(OffsetFacesSpec {
                    faces,
                    tangent: m.tangent,
                    topology: Topology::Keep,
                }),
            };
            Ok(Resolved {
                target: &m.target,
                built,
                spec,
            })
        }
        Feature::ReplaceFace(x) => {
            let built =
                crate::rebuild::target_kinds(part, solids, id, "target", &x.target, before, true)?;
            let mut faces = Vec::new();
            for (i, g) in x.groups.iter().enumerate() {
                let s = surface(part, id, &format!("groups.{i}.with"), &g.with, before)?;
                for (j, r) in g.faces.iter().enumerate() {
                    let n = face(
                        part,
                        id,
                        &x.target,
                        ty,
                        &format!("groups.{i}.faces.{j}"),
                        r,
                        before,
                    )?;
                    faces.push((n, s.clone()));
                }
            }
            Ok(Resolved {
                target: &x.target,
                built,
                spec: Spec::Replace(ryukei::tweak::ReplaceSpec {
                    faces,
                    topology: Topology::Keep,
                }),
            })
        }
        _ => unreachable!("a tweak"),
    }
}

/// A kernel refusal in ryucad's terms, before its entities are placed.
pub(crate) struct Refusal {
    pub reason: &'static str,
    pub case: Option<&'static str>,
    pub message: &'static str,
    pub entities: Vec<(&'static str, Name)>,
}

/// `e`'s reason (its variant in snake case), an unsupported one's case,
/// a sentence, and the names it gives with their roles.
pub(crate) fn refusal(e: &ryukei::TweakError) -> Refusal {
    use ryukei::{TweakCase as C, TweakError as T};
    let r = |reason, message, entities| Refusal {
        reason,
        case: None,
        message,
        entities,
    };
    match *e {
        T::NoFaces => r("no_faces", "the tweak names no face", vec![]),
        T::NoSuchFace { face } => r(
            "no_such_face",
            "a face isn't the target's",
            vec![("face", face)],
        ),
        T::DuplicateFace { face } => r(
            "duplicate_face",
            "a face is named twice",
            vec![("face", face)],
        ),
        T::NotFinite => r(
            "not_finite",
            "a motion, distance or surface isn't finite",
            vec![],
        ),
        T::BadDistance => r(
            "bad_distance",
            "a distance is too small for the kernel's resolution (10 nm) and isn't zero",
            vec![],
        ),
        T::TangencyBroken { edge } => r(
            "tangency_broken",
            "a face tangent to a tweaked one along this edge would meet it at an angle",
            vec![("edge", edge)],
        ),
        T::EdgeVanishes { edge } => r(
            "edge_vanishes",
            "this edge's faces wouldn't meet where its vertices go: the edge would vanish",
            vec![("edge", edge)],
        ),
        T::VertexSplits { vertex } => r(
            "vertex_splits",
            "this vertex's edges wouldn't meet at one point: it would split",
            vec![("vertex", vertex)],
        ),
        T::FaceCollapses { face } => r(
            "face_collapses",
            "this face's new surface folds, or its loops would turn over",
            vec![("face", face)],
        ),
        T::ExtensionUnsupported { face } => r(
            "extension_unsupported",
            "this unchanged face would have to be carried past its surface's edge to meet its moved neighbours, and can't be",
            vec![("face", face)],
        ),
        T::Ambiguous { face } => r(
            "ambiguous",
            "the new surface is nearly square to this face: which side is out is unclear",
            vec![("face", face)],
        ),
        T::LaminarFace { face } => r(
            "laminar_face",
            "this face has a sheet's boundary edge: where the boundary goes on the new surface is undefined",
            vec![("face", face)],
        ),
        T::BadAngle => r(
            "bad_angle",
            "a draft angle that isn't finite, or outside the resolution to 90° less 0.01 rad",
            vec![],
        ),
        T::NoHinge { face } => r(
            "no_hinge",
            "the neutral plane doesn't cross this face",
            vec![("face", face)],
        ),
        T::HingeAmbiguous { face } => r(
            "hinge_ambiguous",
            "the neutral plane crosses this face in more than one place",
            vec![("face", face)],
        ),
        T::HingeNotOnFace { face } => r(
            "hinge_not_on_face",
            "none of the parting line's edges bound this face, or they aren't one chain",
            vec![("face", face)],
        ),
        T::HingeAlongPull { face } => r(
            "hinge_along_pull",
            "this face's hinge or normal runs along the pull somewhere: the draft has no direction there",
            vec![("face", face)],
        ),
        T::GapNotHealed { face } => r(
            "gap_not_healed",
            "the re-trim's walk round this face didn't close",
            vec![("face", face)],
        ),
        T::HealAmbiguous { face } => r(
            "heal_ambiguous",
            "which side of this face the re-trim keeps can't be decided",
            vec![("face", face)],
        ),
        T::RegionTooSmall { face } => r(
            "region_too_small",
            "this face reaches past the region its re-trim works in",
            vec![("face", face)],
        ),
        T::CapNotOnFace { face } => r(
            "cap_not_on_face",
            "the gap the deleted faces leave at this face doesn't lie on a hole of one face, so no cap closes it",
            vec![("face", face)],
        ),
        T::GrowWouldCap { face } => r(
            "grow_would_cap",
            "the gap the deleted faces leave at this face is closed by a cap, not by growing its neighbours",
            vec![("face", face)],
        ),
        T::NoFeature { face } => r(
            "no_feature",
            "this face is part of no feature (at most half the body's faces) that a cap closes",
            vec![("face", face)],
        ),
        T::NotABlend { face } => r(
            "not_a_blend",
            "this face is no recognised fillet's, chamfer's or corner's (or no support of its chamfer)",
            vec![("face", face)],
        ),
        T::BlendLost { face, reason } => Refusal {
            reason: "blend_lost",
            case: Some(match reason {
                ryukei::BlendLoss::NoEdge => "no_edge",
                ryukei::BlendLoss::Overflow => "overflow",
                ryukei::BlendLoss::RadiusTooLarge => "radius_too_large",
                ryukei::BlendLoss::SmoothEdge => "smooth_edge",
                ryukei::BlendLoss::Unsupported => "unsupported",
                ryukei::BlendLoss::NonManifold => "non_manifold",
                ryukei::BlendLoss::Uncertain => "uncertain",
            }),
            message: "the blend holding this face can't be made again after the edit",
            entities: vec![("face", face)],
        },
        T::BadTolerance => r(
            "bad_tolerance",
            "a recognition tolerance that isn't finite or lies outside 10 nm to 0.1 mm",
            vec![],
        ),
        T::SelfIntersecting { faces: [a, b] } => r(
            "self_intersecting",
            "two faces of the result would cross",
            vec![("a", a), ("b", b)],
        ),
        T::Uncertain { at } => r(
            "uncertain",
            "a decision rounding left uncertain",
            vec![("at", at)],
        ),
        T::ToleranceTooLarge { at, .. } => r(
            "tolerance_too_large",
            "the result would hold a tolerance past the kernel's cap",
            vec![("at", at)],
        ),
        T::Unsupported { at, case } => Refusal {
            reason: "unsupported",
            case: Some(match case {
                C::PartlySmooth => "partly_smooth",
                C::LoneVertex => "lone_vertex",
                C::CarriedCurve => "carried_curve",
                C::HingeChain => "hinge_chain",
            }),
            message: "the kernel can't tweak this face, edge or vertex yet",
            entities: vec![("at", at)],
        },
    }
}

/// What to do about a refusal, where there is a known way out.
pub(crate) fn hint(r: &Refusal) -> Option<&'static str> {
    match (r.reason, r.case) {
        ("tangency_broken", _) => Some(
            "set tangent true, so faces tangent to a tweaked face move with it; or put the fillet after this feature in the list, where it is built again on the tweaked edges",
        ),
        ("edge_vanishes" | "vertex_splits" | "face_collapses", _) => Some(
            "the tweak would change the body's topology, which ryucad can't do yet (ryukei 13): a smaller motion, or the faces it reaches listed too, may keep it",
        ),
        ("hinge_not_on_face", _) | ("unsupported", Some("hinge_chain")) => {
            Some("split the face along the parting line first (a split, or a sketch's cut)")
        }
        ("hinge_along_pull", _) => Some("give a pull more nearly square to the face"),
        ("cap_not_on_face", _) => {
            Some("heal the gap by growing its neighbours instead, or delete the faces round it too")
        }
        ("grow_would_cap", _) => Some("heal the gap by a cap instead"),
        ("no_feature", _) => Some(
            "name the feature's faces yourself: a hole's or a boss's faces, which leave a gap a cap closes",
        ),
        _ => None,
    }
}

/// The kernel's refusal of tweak `id` (`ty`) of `target` as `built` holds
/// it: `tweak_failed` with its reason, `kernel: tweak`, the kernel's text,
/// each entity it names as a reference into the target (with its kind and
/// origin; else its name alone), and a hint where there is one. Anything
/// but a tweak refusal is `tweak_failed` with reason `kernel`.
fn kernel_error(
    part: &Part,
    id: &Id,
    ty: &str,
    (target, built): (&Id, &Built),
    e: ryukei::Error,
) -> Error {
    let entity = |role: &str, n: &Name| -> Value {
        let mut v = json!({"role": role});
        match built.body.by_name(*n) {
            Some(x) => {
                v["reference"] = json!(format!("{}/{target}/{n}", part.id));
                v["kind"] = json!(crate::reference::kind(x));
                if let Some(o) = built.origins.get(&n.to_string()) {
                    v["origin"] = o.clone();
                }
            }
            None => v["name"] = json!(n.to_string()),
        }
        v
    };
    let r = match &e {
        ryukei::Error::InvalidTweak(t) => refusal(t),
        _ => Refusal {
            reason: "kernel",
            case: None,
            message: "the kernel refused it",
            entities: vec![],
        },
    };
    let mut err = Error::new(Code::TweakFailed, format!("{ty} {id}: {} ({e})", r.message))
        .with("feature", id)
        .with("solid", target)
        .with("reason", r.reason)
        .with("kernel", "tweak")
        .with("refusal", format!("{e:?}"));
    if let Some(c) = r.case {
        err = err.with("case", c);
    }
    if let Some(h) = hint(&r) {
        err = err.with("hint", h);
    }
    if !r.entities.is_empty() {
        err = err.with(
            "entities",
            r.entities
                .iter()
                .map(|(role, n)| entity(role, n))
                .collect::<Vec<_>>(),
        );
    }
    err
}

/// A tweak: its target as it sees it, through one kernel call. It makes
/// nothing, so every entity keeps its origin.
pub fn build(
    part: &Part,
    solids: &Solids,
    id: &Id,
    f: &Feature,
    before: &dyn Results,
    run: &Run,
) -> Result<Made> {
    let ty = f.type_name();
    let r = resolve(part, solids, id, f, before)?;
    let work = crate::work::work(run.job);
    let key = crate::rebuild::op_key(&part.id, id);
    let body = &r.built.body;
    let made = match &r.spec {
        Spec::Move(s) => ryukei::tweak::move_faces_with(key, body, s, &work),
        Spec::Offset(s) => ryukei::tweak::offset_faces_with(key, body, s, &work),
        Spec::Replace(s) => ryukei::tweak::replace_faces_with(key, body, s, &work),
    }
    .map_err(|e| {
        crate::work::stopped(&e, run.job)
            .unwrap_or_else(|| kernel_error(part, id, ty, (r.target, r.built), e))
    })?;
    let origins = crate::rebuild::replaced_origins(id, r.built, &made);
    let built = crate::rebuild::finish(id, ty, made, origins)?;
    Ok(Made::new(vec![(r.target.clone(), Arc::new(built))]))
}

/// Refuses a tweak (feature `id` of `part`, as a write would leave it)
/// whose target isn't a body before it, whose faces aren't its target's
/// faces in its view, or whose direction, axis or surfaces don't resolve
/// to their kinds. What the kernel makes of them only rebuild can tell.
pub fn check(part: &Part, id: &Id, f: &Feature, results: &dyn Results) -> Result<()> {
    resolve(part, &Solids::of(part), id, f, results).map(drop)
}

/// Sorts what a tweak (`ty`) was given as the CLI sends it: one `--face`
/// as `face`, several as `faces`, and `--face-distance` groups as objects
/// in `faces`. After it, `faces` holds the listed faces and `groups` the
/// groups; `face` is taken. Any other type is left alone.
pub fn take_given(ty: &str, f: &mut FeatureFields) -> Result<()> {
    if !crate::doc::tweak::TYPES.contains(&ty) {
        return Ok(());
    }
    let mut faces: Vec<Value> = f.face.take().map(|x| vec![json!(x)]).unwrap_or_default();
    let mut groups: Vec<Value> = Vec::new();
    let cleared = matches!(&f.faces, Some(Value::Array(a)) if a.is_empty());
    match f.faces.take() {
        None => {}
        Some(Value::Array(a)) => {
            for x in a {
                if x.is_object() {
                    groups.push(x);
                } else {
                    faces.push(x);
                }
            }
        }
        Some(_) => {
            return Err(Error::new(
                Code::InvalidParam,
                format!("a {ty}'s faces are a list of faces, each <part>/<solid>/<name>"),
            )
            .with("param", "faces"));
        }
    }
    if ty == "replace_face" && !faces.is_empty() {
        return Err(Error::new(
            Code::InvalidParam,
            "a replace_face's faces are in groups, each with the surface it takes: give --with before the faces it replaces (groups: [{with, faces}])",
        )
        .with("param", "faces"));
    }
    if !groups.is_empty() {
        if f.groups.is_some() {
            return Err(Error::new(
                Code::Usage,
                format!("give a {ty}'s groups once: as groups, or as faces with distances"),
            ));
        }
        f.groups = Some(Value::Array(groups));
    }
    if !faces.is_empty() || cleared {
        f.faces = Some(Value::Array(faces));
    }
    Ok(())
}

/// The target a new tweak takes when it names none: the part's one solid
/// before it, else the solid its first face names.
pub fn default_target(ty: &str, f: &FeatureFields, before: &[Id], part: &Id) -> Result<String> {
    if let [one] = before {
        return Ok(one.to_string());
    }
    let first = f
        .faces
        .as_ref()
        .and_then(|v| v.as_array())
        .and_then(|a| a.first())
        .cloned()
        .or_else(|| {
            let g = f.groups.as_ref()?.as_array()?.first()?;
            g.get("faces")?.as_array()?.first().cloned()
        });
    if let Some(t) = first.as_ref().and_then(Value::as_str)
        && let Ok(r) = Reference::parse(t, part)
    {
        return Ok(r.feature.to_string());
    }
    Err(Error::new(
        Code::InvalidParam,
        format!(
            "a {ty} names the solid it acts on (target): the part has {} before it",
            before.len()
        ),
    )
    .with("param", "target")
    .with("solids", before))
}

/// A tweak (`ty`) from `current`'s fields (none for a new one) with what
/// was given laid over them, faces as `<part>/<solid>/<name>` and
/// directions, axes and `with` as geometric references in text. A new
/// `by` keeps only the fields it takes; a new move naming no `by` is a
/// rotation with an axis, a translation with a direction, else an offset.
pub fn from_given(
    ty: &str,
    current: Option<&Feature>,
    f: &FeatureFields,
    part: &Id,
    unit: crate::units::Unit,
    env: &crate::expr::Env,
) -> Result<Feature> {
    let mut v = match current {
        Some(Feature::MoveFace(m)) => m.fields(),
        Some(Feature::ReplaceFace(r)) => r.fields(),
        _ => json!({}),
    };
    let o = v.as_object_mut().expect("fields are an object");
    let reference = |t: &str, at: &str| -> Result<Value> {
        Ok(Reference::parse(t, part)
            .map_err(|e| e.with("pointer", at))?
            .to_json())
    };
    let geom = |t: &str, param: &str| -> Result<Value> {
        Ok(GeomRef::parse(t, part)
            .map_err(|e| e.with("param", param))?
            .to_json())
    };
    let target = match (&f.target, &f.targets) {
        (Some(_), Some(_)) => {
            return Err(Error::new(
                Code::Usage,
                format!("give a {ty}'s target once"),
            ));
        }
        (Some(t), None) => Some(t.clone()),
        (None, Some(ts)) => match ts.as_slice() {
            [t] => Some(t.clone()),
            _ => {
                return Err(Error::new(
                    Code::InvalidParam,
                    format!("a {ty} acts on one target, not {}", ts.len()),
                )
                .with("param", "targets"));
            }
        },
        (None, None) => None,
    };
    if let Some(t) = target {
        o.insert("target".into(), json!(t));
    }
    // Each group's faces from text; `[]` for none.
    let groups = |gs: &Value, with: bool| -> Result<Option<Value>> {
        let Value::Array(gs) = gs else {
            return Err(Error::new(
                Code::InvalidParam,
                format!("a {ty}'s groups are a list, each with its faces"),
            )
            .with("param", "groups"));
        };
        let mut out = Vec::with_capacity(gs.len());
        for (i, g) in gs.iter().enumerate() {
            let mut g = g.clone();
            if let Some(Value::Array(fs)) = g.get_mut("faces") {
                for (j, x) in fs.iter_mut().enumerate() {
                    if let Some(t) = x.as_str().map(str::to_owned) {
                        *x = reference(&t, &format!("/groups/{i}/faces/{j}"))?;
                    }
                }
            }
            if with && let Some(t) = g.get("with").and_then(Value::as_str).map(str::to_owned) {
                g["with"] = geom(&t, "groups")?;
            }
            out.push(g);
        }
        Ok((!out.is_empty()).then_some(Value::Array(out)))
    };
    if ty == "replace_face" {
        if let Some(gs) = &f.groups {
            match groups(gs, true)? {
                Some(g) => o.insert("groups".into(), g),
                None => o.remove("groups"),
            };
        }
    } else {
        let by = f.by.clone().or_else(|| {
            (current.is_none()).then(|| {
                if f.axis.is_some() {
                    "rotate"
                } else if f.direction.is_some() {
                    "translate"
                } else {
                    "offset"
                }
                .to_owned()
            })
        });
        if let Some(by) = by {
            let Some(takes) = MoveBy::takes(&by) else {
                return Err(Error::new(
                    Code::InvalidParam,
                    format!("a move_face's by is translate, rotate or offset, not {by:?}"),
                )
                .with("param", "by"));
            };
            if o.get("by").and_then(Value::as_str) != Some(by.as_str()) {
                let motion = ["direction", "distance", "axis", "angle", "groups"];
                o.retain(|k, _| !motion.contains(&k.as_str()) || takes.contains(&k.as_str()));
                o.insert("by".into(), json!(by));
            }
        }
        if let Some(d) = &f.direction {
            o.insert("direction".into(), geom(d, "direction")?);
        }
        if let Some(a) = &f.axis {
            o.insert("axis".into(), geom(a, "axis")?);
        }
        if let Some(q) = &f.distance {
            o.insert("distance".into(), q.clone());
        }
        if let Some(q) = &f.angle {
            o.insert("angle".into(), q.clone());
        }
        if let Some(Value::Array(fs)) = &f.faces {
            let refs = fs
                .iter()
                .enumerate()
                .map(|(i, x)| match x.as_str() {
                    Some(t) => reference(t, &format!("/faces/{i}")),
                    None => Ok(x.clone()),
                })
                .collect::<Result<Vec<_>>>()?;
            if refs.is_empty() {
                o.remove("faces");
            } else {
                o.insert("faces".into(), json!(refs));
            }
        }
        if let Some(gs) = &f.groups {
            match groups(gs, false)? {
                Some(g) => o.insert("groups".into(), g),
                None => o.remove("groups"),
            };
        }
        match f.tangent {
            Some(true) => {
                o.remove("tangent");
            }
            Some(false) => {
                o.insert("tangent".into(), json!(false));
            }
            None => {}
        }
    }
    let mut r = crate::doc::files::Obj::new(&v, "")?;
    let out = match ty {
        "replace_face" => Feature::ReplaceFace(Box::new(
            crate::doc::tweak::ReplaceFace::from_fields(&mut r)?,
        )),
        _ => Feature::MoveFace(Box::new(MoveFace::from_fields(&mut r, unit, env)?)),
    };
    r.finish()?;
    Ok(out)
}

/// A tweak as output reports it: its file's fields, quantities as written
/// and in meters or radians, faces as `<part>/<solid>/<name>`, directions,
/// axes and `with` as text, a move's `tangent` always.
pub fn report(part: &Id, f: &Feature) -> Value {
    let pointer = |role: &str| format!("/{}", role.replace('.', "/"));
    match f {
        Feature::MoveFace(m) => {
            let mut v = m.fields();
            match &m.by {
                MoveBy::Translate {
                    direction,
                    distance,
                } => {
                    v["direction"] = json!(direction.text(part));
                    v["distance"] = distance.report();
                }
                MoveBy::Rotate { axis, angle } => {
                    v["axis"] = json!(axis.text(part));
                    v["angle"] = angle.report();
                }
                MoveBy::Offset { distance, groups } => {
                    v["distance"] = distance.report();
                    for (i, g) in groups.iter().enumerate() {
                        v["groups"][i]["distance"] = g.quantity.report();
                    }
                }
            }
            v["tangent"] = json!(m.tangent);
            for (role, r) in m.faces() {
                *v.pointer_mut(&pointer(&role)).expect("its own face") = json!(r.text(part));
            }
            v
        }
        Feature::ReplaceFace(x) => {
            let mut v = x.fields();
            for (i, g) in x.groups.iter().enumerate() {
                v["groups"][i]["with"] = json!(g.with.text(part));
            }
            for (role, r) in x.faces() {
                *v.pointer_mut(&pointer(&role)).expect("its own face") = json!(r.text(part));
            }
            v
        }
        _ => unreachable!("a tweak"),
    }
}

/// What a tweak resolved to, beside its fields: a move's rigid `motion`
/// (`{rotation, translation_m}`, the rotation's rows) where it resolves,
/// and once built what its journal says it `modified` (counts of faces,
/// edges and vertices; with `references`, each as
/// `<part>/<solid>/<name>`, sorted).
pub fn resolved(
    part: &Part,
    results: &dyn Results,
    id: &Id,
    f: &Feature,
    references: bool,
) -> Option<Value> {
    if !takes_sheets(f) {
        return None;
    }
    let mut v = json!({});
    if let Feature::MoveFace(m) = f
        && let Ok(Some(r)) = motion(part, id, m, results)
    {
        let t = r.translation_part();
        v["motion"] = json!({"rotation": r.rotation_matrix(), "translation_m": [t.x, t.y, t.z]});
    }
    let target = f.targets()[0];
    if let Some(Ok(out)) = results.result(id)
        && let Some(b) = out.solid(target)
    {
        let (mut faces, mut edges, mut vertices) = (0u64, 0u64, 0u64);
        let mut refs = Vec::new();
        for e in b.journal.events() {
            let Event::Modified(n) = e else { continue };
            match b.body.by_name(*n) {
                Some(EntityId::Face(_)) => faces += 1,
                Some(EntityId::Edge(_)) => edges += 1,
                Some(EntityId::Vertex(_)) => vertices += 1,
                _ => continue,
            }
            refs.push(format!("{}/{target}/{n}", part.id));
        }
        v["modified"] = json!({"faces": faces, "edges": edges, "vertices": vertices});
        if references {
            refs.sort();
            v["modified"]["references"] = json!(refs);
        }
    }
    Some(v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ryukei::geom::{Point3, UnitVector3, Vector3};

    fn close(a: Point3, b: Point3) {
        assert!((a - b).length() < 1e-12, "{a:?} against {b:?}");
    }

    #[test]
    fn a_turn_is_about_its_axis_where_it_lies() {
        // About the edge along +x at y = 0, z = 0.02: a quarter turn takes
        // (0, 0.03, 0.02) to (0, 0, 0.05), and the axis stays put.
        let l = crate::geometry::Line::along(Point3::new(0.0, 0.0, 0.02), UnitVector3::X);
        let r = about(&l, core::f64::consts::FRAC_PI_2);
        close(
            r.apply_point(Point3::new(0.0, 0.03, 0.02)),
            Point3::new(0.0, 0.0, 0.05),
        );
        close(
            r.apply_point(Point3::new(0.04, 0.0, 0.02)),
            Point3::new(0.04, 0.0, 0.02),
        );
        // A translation along a unit direction by a distance.
        let t = along(Vector3::new(0.0, 0.0, 2.0).normalize().unwrap(), 0.005);
        close(t.apply_point(Point3::ORIGIN), Point3::new(0.0, 0.0, 0.005));
    }

    #[test]
    fn only_tweaks_take_sheets() {
        use crate::doc::files::Feature;
        let n = "0123456789abcdef0123456789abcdef";
        let v = serde_json::json!({"format": crate::doc::tweak::FORMAT, "kind": "ryucad.feature",
            "type": "move_face", "target": "s", "by": "translate", "direction": "z",
            "distance": "1 mm", "faces": [{"feature": "s", "name": n}]});
        let f = Feature::from_json(&v, crate::units::Unit::Mm, &crate::expr::Env::empty()).unwrap();
        assert!(takes_sheets(&f));
        let shell = serde_json::json!({"format": 9, "kind": "ryucad.feature", "type": "shell",
            "target": "s", "thickness": "1 mm"});
        let f =
            Feature::from_json(&shell, crate::units::Unit::Mm, &crate::expr::Env::empty()).unwrap();
        assert!(!takes_sheets(&f));
    }

    #[test]
    fn every_refusal_has_a_reason_and_the_known_ways_out_a_hint() {
        use ryukei::{TweakCase as C, TweakError as T};
        let n = Name::from_u128(7);
        let all = [
            (T::NoFaces, "no_faces"),
            (T::NoSuchFace { face: n }, "no_such_face"),
            (T::DuplicateFace { face: n }, "duplicate_face"),
            (T::NotFinite, "not_finite"),
            (T::BadDistance, "bad_distance"),
            (T::TangencyBroken { edge: n }, "tangency_broken"),
            (T::EdgeVanishes { edge: n }, "edge_vanishes"),
            (T::VertexSplits { vertex: n }, "vertex_splits"),
            (T::FaceCollapses { face: n }, "face_collapses"),
            (T::ExtensionUnsupported { face: n }, "extension_unsupported"),
            (T::Ambiguous { face: n }, "ambiguous"),
            (T::LaminarFace { face: n }, "laminar_face"),
            (T::BadAngle, "bad_angle"),
            (T::NoHinge { face: n }, "no_hinge"),
            (T::HingeAmbiguous { face: n }, "hinge_ambiguous"),
            (T::HingeNotOnFace { face: n }, "hinge_not_on_face"),
            (T::HingeAlongPull { face: n }, "hinge_along_pull"),
            (T::GapNotHealed { face: n }, "gap_not_healed"),
            (T::HealAmbiguous { face: n }, "heal_ambiguous"),
            (T::RegionTooSmall { face: n }, "region_too_small"),
            (T::CapNotOnFace { face: n }, "cap_not_on_face"),
            (T::GrowWouldCap { face: n }, "grow_would_cap"),
            (T::NoFeature { face: n }, "no_feature"),
            (T::NotABlend { face: n }, "not_a_blend"),
            (
                T::BlendLost {
                    face: n,
                    reason: ryukei::BlendLoss::Overflow,
                },
                "blend_lost",
            ),
            (T::BadTolerance, "bad_tolerance"),
            (T::SelfIntersecting { faces: [n, n] }, "self_intersecting"),
            (T::Uncertain { at: n }, "uncertain"),
            (
                T::ToleranceTooLarge {
                    at: n,
                    needed: 1e-3,
                },
                "tolerance_too_large",
            ),
            (
                T::Unsupported {
                    at: n,
                    case: C::PartlySmooth,
                },
                "unsupported",
            ),
        ];
        for (e, reason) in all {
            let r = refusal(&e);
            assert_eq!(r.reason, reason, "{e:?}");
            assert!(!r.message.is_empty());
        }
        let hint = |e: T| hint(&refusal(&e));
        assert!(
            hint(T::TangencyBroken { edge: n })
                .unwrap()
                .contains("tangent")
        );
        for e in [
            T::EdgeVanishes { edge: n },
            T::VertexSplits { vertex: n },
            T::FaceCollapses { face: n },
        ] {
            assert!(hint(e).unwrap().contains("topology"), "{e:?}");
        }
        assert!(
            hint(T::HingeNotOnFace { face: n })
                .unwrap()
                .contains("split")
        );
        assert!(
            hint(T::Unsupported {
                at: n,
                case: C::HingeChain
            })
            .unwrap()
            .contains("split")
        );
        assert!(
            hint(T::HingeAlongPull { face: n })
                .unwrap()
                .contains("pull")
        );
        assert!(hint(T::CapNotOnFace { face: n }).unwrap().contains("grow"));
        assert!(hint(T::GrowWouldCap { face: n }).unwrap().contains("cap"));
        assert!(hint(T::NoFeature { face: n }).unwrap().contains("faces"));
        assert_eq!(hint(T::Uncertain { at: n }), None);
        assert_eq!(
            refusal(&T::Unsupported {
                at: n,
                case: C::CarriedCurve
            })
            .case,
            Some("carried_curve")
        );
        let roles: Vec<&str> = refusal(&T::SelfIntersecting { faces: [n, n] })
            .entities
            .iter()
            .map(|(r, _)| *r)
            .collect();
        assert_eq!(roles, ["a", "b"]);
    }

    #[test]
    fn check_is_resolve() {
        // The write-time check is the build's resolution, kernel left out.
        let _: fn(&Part, &Id, &Feature, &dyn Results) -> Result<()> = check;
    }

    fn call(s: &mut crate::session::Session, m: &str, p: Value) -> Result<Value> {
        crate::server::handle(s, crate::proto::Request::from_rpc(m, Some(p)).unwrap())
    }

    /// A project with part `p`: a box `b` 40 × 30 × 20 mm, a box `c` beside
    /// it.
    fn boxes() -> (tempfile::TempDir, crate::session::Session) {
        let dir = tempfile::tempdir().unwrap();
        crate::server::init(dir.path(), None).unwrap();
        let mut s = crate::session::Session::open(dir.path()).unwrap();
        call(&mut s, "part.create", json!({"part": "p"})).unwrap();
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "box", "id": "b",
            "params": {"x": 40, "y": 30, "z": 20}}),
        )
        .unwrap();
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "box", "id": "c",
            "params": {"x": 10, "y": 10, "z": 10}, "placement": {"origin": [60, 0, 0]}}),
        )
        .unwrap();
        (dir, s)
    }

    /// The names of feature `f`'s body's `kind` (`faces`, `edges`).
    fn names(s: &mut crate::session::Session, f: &str, kind: &str) -> Vec<String> {
        let v = call(s, "inspect", json!({"part": "p", "feature": f})).unwrap();
        v["bodies"][0][kind]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x["name"].as_str().unwrap().to_owned())
            .collect()
    }

    fn file(dir: &tempfile::TempDir, f: &str) -> Value {
        serde_json::from_str(
            &std::fs::read_to_string(dir.path().join(format!("parts/p/features/{f}.json")))
                .unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn the_protocol_adds_and_edits_a_move() {
        let (dir, mut s) = boxes();
        let b = names(&mut s, "b", "faces");
        // No target and two solids: its faces name it. No `by`: a direction
        // makes it a translation.
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "move_face", "id": "m",
            "direction": "z", "distance": "1 mm", "faces": [format!("p/b/{}", b[0])]}),
        )
        .unwrap();
        let v = file(&dir, "m");
        assert_eq!(
            (v["target"].as_str(), v["by"].as_str()),
            (Some("b"), Some("translate")),
            "{v}"
        );
        assert_eq!(v["format"], crate::doc::tweak::FORMAT);
        assert!(v.get("tangent").is_none(), "{v}");
        // A new `by` keeps only what it takes.
        call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "m", "by": "offset",
            "distance": "0.5 mm"}),
        )
        .unwrap();
        let v = file(&dir, "m");
        assert_eq!(v["by"], "offset");
        assert!(v.get("direction").is_none(), "{v}");
        call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "m", "tangent": false}),
        )
        .unwrap();
        assert_eq!(file(&dir, "m")["tangent"], false);
        call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "m", "tangent": true}),
        )
        .unwrap();
        assert!(file(&dir, "m").get("tangent").is_none());
        // One face as `face`, groups as objects in `faces` (what the CLI
        // sends for --face and --face-distance together).
        call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "m",
            "face": format!("p/b/{}", b[1]),
            "faces": [{"distance": "2 mm", "faces": [format!("p/b/{}", b[2])]}]}),
        )
        .unwrap();
        let v = file(&dir, "m");
        assert_eq!(v["faces"], json!([{"feature": "b", "name": b[1]}]), "{v}");
        assert_eq!(v["groups"][0]["distance"], "2 mm", "{v}");
        // A field a move doesn't take.
        let e = call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "m", "thickness": "1 mm"}),
        )
        .unwrap_err();
        assert_eq!(
            (e.code, e.details["param"].as_str()),
            (Code::InvalidParam, Some("thickness"))
        );
    }

    #[test]
    fn writes_refuse_what_cant_resolve() {
        let (_dir, mut s) = boxes();
        let (b, c) = (names(&mut s, "b", "faces"), names(&mut s, "c", "faces"));
        let edge = names(&mut s, "b", "edges")[0].clone();
        let add = |s: &mut crate::session::Session, extra: Value| {
            let mut v = json!({"part": "p", "type": "move_face", "target": "b",
                               "by": "translate", "direction": "z", "distance": "1 mm"});
            for (k, x) in extra.as_object().unwrap() {
                v[k] = x.clone();
            }
            call(s, "feature.add", v)
        };
        let e = add(&mut s, json!({"faces": [format!("p/c/{}", c[0])]})).unwrap_err();
        assert_eq!(e.code, Code::TargetsMixed, "{e}");
        let e = add(&mut s, json!({"faces": [format!("p/b/{edge}")]})).unwrap_err();
        assert_eq!(e.code, Code::ReferenceWrongKind, "{e}");
        // A whole solid is no direction.
        let e = add(
            &mut s,
            json!({"faces": [format!("p/b/{}", b[0])], "direction": "p/c"}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::ReferenceWrongKind, "{e}");
        let e = add(
            &mut s,
            json!({"faces": [format!("p/b/{}", b[0])], "by": "slide"}),
        )
        .unwrap_err();
        assert_eq!(
            (e.code, e.details["param"].as_str()),
            (Code::InvalidParam, Some("by"))
        );
        // Nothing written by any of them.
        assert!(add(&mut s, json!({"faces": [format!("p/b/{}", b[0])]})).is_ok());
    }

    /// The reference of `f`'s planar face facing up at `z` meters, of solid `solid`.
    fn up(s: &mut crate::session::Session, f: &str, solid: &str, z: f64) -> String {
        let v = call(s, "inspect", json!({"part": "p", "feature": f})).unwrap();
        let face = v["bodies"][0]["faces"]
            .as_array()
            .unwrap()
            .iter()
            .find(|x| {
                let sign = if x["reversed"] == true { -1.0 } else { 1.0 };
                x["surface"]["type"] == "plane"
                    && x["surface"]["frame"]["z"][2].as_f64().unwrap() * sign == 1.0
                    && (x["surface"]["frame"]["origin"][2].as_f64().unwrap() - z).abs() < 1e-12
            })
            .unwrap()["name"]
            .as_str()
            .unwrap()
            .to_owned();
        format!("p/{solid}/{face}")
    }

    #[test]
    fn a_move_reports_its_fields_motion_and_what_it_modified() {
        let (_dir, mut s) = boxes();
        let top = up(&mut s, "b", "b", 0.02);
        let shown = call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "move_face", "id": "m",
            "by": "translate", "direction": "z", "distance": "5 mm", "faces": [top.clone()]}),
        )
        .unwrap();
        let f = &shown["feature"];
        assert_eq!(f["distance"], json!({"value": "5 mm", "m": 0.005}), "{f}");
        assert_eq!(f["faces"], json!([top]), "{f}");
        assert_eq!(
            (f["direction"].as_str(), f["tangent"].as_bool()),
            (Some("z"), Some(true))
        );
        let r = call(&mut s, "rebuild", json!({"part": "p"})).unwrap();
        let m = r["parts"][0]["features"]
            .as_array()
            .unwrap()
            .iter()
            .find(|x| x["id"] == "m")
            .unwrap()["move_face"]
            .clone();
        assert_eq!(
            m["motion"]["translation_m"],
            json!([0.0, 0.0, 0.005]),
            "{m}"
        );
        // The top moved; its four edges moved, the four upright ones were
        // carried on; its four corners moved. The sides keep their planes.
        assert_eq!(m["modified"]["faces"], 1, "{m}");
        assert_eq!(
            (
                m["modified"]["edges"].as_u64(),
                m["modified"]["vertices"].as_u64()
            ),
            (Some(8), Some(4)),
            "{m}"
        );
        assert!(m["modified"].get("references").is_none());
        let i = call(&mut s, "inspect", json!({"part": "p", "feature": "m"})).unwrap();
        let refs = i["move_face"]["modified"]["references"].as_array().unwrap();
        assert!(refs.contains(&json!(top)), "{i}");
        assert_eq!(refs.len(), 13);
    }

    #[test]
    fn a_replacement_takes_a_faces_surface_or_a_plane() {
        let (_dir, mut s) = boxes();
        let top = up(&mut s, "b", "b", 0.02);
        let lid = up(&mut s, "c", "c", 0.01);
        // c's top, at 10 mm: b comes down to it.
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "replace_face", "id": "r",
            "groups": [{"with": lid, "faces": [top.clone()]}]}),
        )
        .unwrap();
        let part = Id::parse("p", "part").unwrap();
        let built = s.rebuilt(&part).unwrap().clone();
        let (_, out) = built
            .features
            .iter()
            .find(|(i, _)| i.as_str() == "r")
            .unwrap();
        assert!(out.is_ok(), "{out:?}");
        // An edge is no surface.
        let edge = names(&mut s, "b", "edges")[0].clone();
        let e = call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "replace_face",
            "groups": [{"with": format!("p/b/{edge}"), "faces": [top]}]}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::ReferenceWrongKind, "{e}");
        // A whole solid isn't a plane.
        let top = up(&mut s, "b", "b", 0.02);
        let e = call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "replace_face",
            "groups": [{"with": "p/c", "faces": [top]}]}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::ReferenceWrongKind, "{e}");
    }

    #[test]
    fn replacement_groups_read_as_the_cli_sends_them() {
        let (dir, mut s) = boxes();
        let top = up(&mut s, "b", "b", 0.02);
        // What `--with p/d --face <top>` sends, `d` a datum plane.
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "plane", "id": "d",
            "by": "offset", "from": "xy", "distance": 22}),
        )
        .unwrap();
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "replace_face", "id": "r",
            "groups": [{"with": "p/d", "faces": [top.clone()]}]}),
        )
        .unwrap();
        let v = file(&dir, "r");
        assert_eq!(v["groups"][0]["with"], json!({"feature": "d"}), "{v}");
        assert_eq!(v["target"], "b");
        // Faces outside a group are refused, saying where they go.
        let e = call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "r", "face": top}),
        )
        .unwrap_err();
        assert_eq!(
            (e.code, e.details["param"].as_str()),
            (Code::InvalidParam, Some("faces"))
        );
    }
}
