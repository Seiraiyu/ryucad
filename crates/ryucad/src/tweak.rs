//! Face tweaks (design 400, plan 17.A): a solid's faces moved
//! (translated, turned about an axis, offset along their normals) or
//! given another surface, by one kernel call under the feature's key and
//! the request's cancel. A tweak acts on its target: it joins its chain,
//! what it leaves is the target, and it keeps every face, edge and vertex
//! and their names and origins. It takes a sheet where the kernel does.

use crate::doc::files::Feature;
use crate::doc::id::Id;
use crate::doc::project::Part;
use crate::doc::reference::Reference;
use crate::doc::tweak::{MoveBy, MoveFace};
use crate::error::{Code, Error, Result};
use crate::geometry::{At, Geom, Line, Want};
use crate::rebuild::{Built, Made, Results, Run};
use serde_json::{Value, json};
use std::sync::Arc;
use crate::solid::Solids;
use ryukei::geom::{Rigid, UnitVector3};
use ryukei::name::Name;
use ryukei::topo::EntityId;
use ryukei::tweak::{MoveSpec, OffsetFacesSpec};

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
pub(crate) fn motion(part: &Part, id: &Id, m: &MoveFace, results: &dyn Results) -> Result<Option<Rigid>> {
    let at = at(part, id, results);
    match &m.by {
        MoveBy::Translate { direction, distance } => {
            let v = match crate::geometry::resolve(&at, "direction", direction, &[Want::Axis, Want::Plane])? {
                Geom::Axis(l) => l.dir.get(),
                Geom::Plane(p) => p.normal,
                _ => unreachable!("asked for an axis or a plane"),
            };
            let u = v.normalize().ok_or_else(|| {
                at.fail(Code::ReferenceWrongKind, "direction", direction, "gives no direction".to_owned())
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
        e => Err(crate::reference::wrong_kind(part, id, role, r, crate::reference::kind(e), "a face")),
    }
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
            let built = crate::rebuild::target_kinds(part, solids, id, "target", &m.target, before, true)?;
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
                        let i: usize = rest.split('.').next().and_then(|s| s.parse().ok()).expect("groups.<i>");
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
                }),
                None => Spec::Offset(OffsetFacesSpec { faces, tangent: m.tangent }),
            };
            Ok(Resolved { target: &m.target, built, spec })
        }
        Feature::ReplaceFace(_) => Err(Error::new(Code::Internal, "replace_face: Task 11")),
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
    let r = |reason, message, entities| Refusal { reason, case: None, message, entities };
    match *e {
        T::NoFaces => r("no_faces", "the tweak names no face", vec![]),
        T::NoSuchFace { face } => r("no_such_face", "a face isn't the target's", vec![("face", face)]),
        T::DuplicateFace { face } => r("duplicate_face", "a face is named twice", vec![("face", face)]),
        T::NotFinite => r("not_finite", "a motion, distance or surface isn't finite", vec![]),
        T::BadDistance => r("bad_distance", "a distance is too small for the kernel's resolution (10 nm) and isn't zero", vec![]),
        T::TangencyBroken { edge } => r("tangency_broken", "a face tangent to a tweaked one along this edge would meet it at an angle", vec![("edge", edge)]),
        T::EdgeVanishes { edge } => r("edge_vanishes", "this edge's faces wouldn't meet where its vertices go: the edge would vanish", vec![("edge", edge)]),
        T::VertexSplits { vertex } => r("vertex_splits", "this vertex's edges wouldn't meet at one point: it would split", vec![("vertex", vertex)]),
        T::FaceCollapses { face } => r("face_collapses", "this face's new surface folds, or its loops would turn over", vec![("face", face)]),
        T::ExtensionUnsupported { face } => r("extension_unsupported", "this unchanged face would have to be carried past its surface's edge to meet its moved neighbours, and can't be", vec![("face", face)]),
        T::Ambiguous { face } => r("ambiguous", "the new surface is nearly square to this face: which side is out is unclear", vec![("face", face)]),
        T::LaminarFace { face } => r("laminar_face", "this face has a sheet's boundary edge: where the boundary goes on the new surface is undefined", vec![("face", face)]),
        T::BadAngle => r("bad_angle", "a draft angle that isn't finite, or outside the resolution to 90° less 0.01 rad", vec![]),
        T::NoHinge { face } => r("no_hinge", "the neutral plane doesn't cross this face", vec![("face", face)]),
        T::HingeAmbiguous { face } => r("hinge_ambiguous", "the neutral plane crosses this face in more than one place", vec![("face", face)]),
        T::HingeNotOnFace { face } => r("hinge_not_on_face", "none of the parting line's edges bound this face, or they aren't one chain", vec![("face", face)]),
        T::HingeAlongPull { face } => r("hinge_along_pull", "this face's hinge or normal runs along the pull somewhere: the draft has no direction there", vec![("face", face)]),
        T::SelfIntersecting { faces: [a, b] } => r("self_intersecting", "two faces of the result would cross", vec![("a", a), ("b", b)]),
        T::Uncertain { at } => r("uncertain", "a decision rounding left uncertain", vec![("at", at)]),
        T::ToleranceTooLarge { at, .. } => r("tolerance_too_large", "the result would hold a tolerance past the kernel's cap", vec![("at", at)]),
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
        _ => None,
    }
}

/// The kernel's refusal of tweak `id` (`ty`) of `target` as `built` holds
/// it: `tweak_failed` with its reason, `kernel: tweak`, the kernel's text,
/// each entity it names as a reference into the target (with its kind and
/// origin; else its name alone), and a hint where there is one. Anything
/// but a tweak refusal is `tweak_failed` with reason `kernel`.
fn kernel_error(part: &Part, id: &Id, ty: &str, (target, built): (&Id, &Built), e: ryukei::Error) -> Error {
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
        _ => Refusal { reason: "kernel", case: None, message: "the kernel refused it", entities: vec![] },
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
            r.entities.iter().map(|(role, n)| entity(role, n)).collect::<Vec<_>>(),
        );
    }
    err
}

/// A tweak: its target as it sees it, through one kernel call. It makes
/// nothing, so every entity keeps its origin.
pub fn build(part: &Part, solids: &Solids, id: &Id, f: &Feature, before: &dyn Results, run: &Run) -> Result<Made> {
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
        close(r.apply_point(Point3::new(0.0, 0.03, 0.02)), Point3::new(0.0, 0.0, 0.05));
        close(r.apply_point(Point3::new(0.04, 0.0, 0.02)), Point3::new(0.04, 0.0, 0.02));
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
            (T::SelfIntersecting { faces: [n, n] }, "self_intersecting"),
            (T::Uncertain { at: n }, "uncertain"),
            (T::ToleranceTooLarge { at: n, needed: 1e-3 }, "tolerance_too_large"),
            (T::Unsupported { at: n, case: C::PartlySmooth }, "unsupported"),
        ];
        for (e, reason) in all {
            let r = refusal(&e);
            assert_eq!(r.reason, reason, "{e:?}");
            assert!(!r.message.is_empty());
        }
        let hint = |e: T| hint(&refusal(&e));
        assert!(hint(T::TangencyBroken { edge: n }).unwrap().contains("tangent"));
        for e in [
            T::EdgeVanishes { edge: n },
            T::VertexSplits { vertex: n },
            T::FaceCollapses { face: n },
        ] {
            assert!(hint(e).unwrap().contains("topology"), "{e:?}");
        }
        assert!(hint(T::HingeNotOnFace { face: n }).unwrap().contains("split"));
        assert!(hint(T::Unsupported { at: n, case: C::HingeChain }).unwrap().contains("split"));
        assert!(hint(T::HingeAlongPull { face: n }).unwrap().contains("pull"));
        assert_eq!(hint(T::Uncertain { at: n }), None);
        assert_eq!(refusal(&T::Unsupported { at: n, case: C::CarriedCurve }).case, Some("carried_curve"));
        let roles: Vec<&str> = refusal(&T::SelfIntersecting { faces: [n, n] })
            .entities.iter().map(|(r, _)| *r).collect();
        assert_eq!(roles, ["a", "b"]);
    }
}
