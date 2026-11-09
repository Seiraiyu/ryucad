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
use crate::rebuild::{Built, Results};
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
}
