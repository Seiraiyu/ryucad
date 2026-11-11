//! Face tweaks (design 400, plans 17.A and 17.B): a solid's faces moved
//! (translated, turned about an axis, offset along their normals), given
//! another surface, or drafted about a neutral plane or a parting line, by
//! one kernel call under the feature's key and the request's cancel. A tweak acts on its target: it joins its chain,
//! what it leaves is the target, and it keeps every face, edge and vertex
//! and their names and origins. It takes a sheet where the kernel does.

use crate::doc::files::Feature;
use crate::doc::geometry::GeomRef;
use crate::doc::id::Id;
use crate::doc::project::Part;
use crate::doc::reference::Reference;
use crate::doc::tweak::{Draft, DraftHinge, MoveBy, MoveFace};
use crate::error::{Code, Error, Result};
use crate::geometry::{At, Geom, Line, Want};
use crate::proto::FeatureFields;
use crate::rebuild::{Built, Made, Results, Run};
use crate::solid::Solids;
use ryukei::geom::{Rigid, Surface, UnitVector3, Vector3};
use ryukei::name::{Event, Name};
use ryukei::topo::EntityId;
use ryukei::tweak::{Blends, DraftFace, DraftSpec, Hinge, MoveSpec, OffsetFacesSpec, Topology};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;

/// Whether `f` is a face tweak, reported as one.
pub fn takes_sheets(f: &Feature) -> bool {
    matches!(
        f,
        Feature::MoveFace(_)
            | Feature::ReplaceFace(_)
            | Feature::Draft(_)
            | Feature::DeleteFace(_)
            | Feature::DeleteBlend(_)
            | Feature::ResizeBlend(_)
    )
}

/// The face tweaks and direct edits this module builds (ryucad 17 and 22).
pub const TYPES: [&str; 6] = [
    "move_face",
    "replace_face",
    "draft",
    "delete_face",
    "delete_blend",
    "resize_blend",
];

/// The work units a direct edit may spend before it fails with
/// `edit_budget_exceeded` (`edit.budget`'s default): the smallest power of
/// two at least four times [`LARGEST_EDIT`] (and at least `1 << 20`),
/// measured in plan 22.A (Task 20) in a release build on bc2 (Intel Core
/// Ultra 5 250K Plus, 18 threads, load average about 2 to 4), 2026-11-10.
/// Units are the same on every machine. Plan 22.B measures again with the
/// committed corpus.
pub const DEFAULT_EDIT_BUDGET: u64 = 1 << 24;

/// The most work units a direct edit that builds was measured spending
/// (plan 22.A, Task 20; the box and date as [`DEFAULT_EDIT_BUDGET`]'s):
/// ryukei's `edit_props` seed 70, a replace on 10 faces, 3,249,017 units.
/// ryucad's own largest is 29,957 (`direct_edits`' groove run through);
/// the goldens' 12,478 (`topology_tweaks`' slide).
pub const LARGEST_EDIT: u64 = 3_249_017;

const _: () = {
    assert!(DEFAULT_EDIT_BUDGET.is_power_of_two());
    assert!(DEFAULT_EDIT_BUDGET >= 4 * LARGEST_EDIT);
};

/// The fields a request may give feature type `ty` of this module.
pub fn request_fields(ty: &str) -> &'static [&'static str] {
    match ty {
        "delete_face" => crate::doc::edit::DELETE_REQUEST,
        "delete_blend" => crate::doc::edit::DELETE_BLEND_REQUEST,
        "resize_blend" => crate::doc::edit::RESIZE_REQUEST,
        _ => crate::doc::tweak::request_fields(ty),
    }
}

/// Whether `f`'s kernel call runs under the edit budget: a `delete_face`,
/// a `delete_blend` or a `resize_blend`, or a tweak that may change topology or remake its blends. A tweak that
/// keeps both runs as ryucad 17's, unbudgeted.
pub fn budgeted(f: &Feature) -> bool {
    matches!(
        f,
        Feature::DeleteFace(_) | Feature::DeleteBlend(_) | Feature::ResizeBlend(_)
    ) || f.retrim().is_some_and(|r| !r.is_keep())
}

/// What a file's `topology` asks the kernel.
fn topology(r: crate::doc::edit::Retrim) -> Topology {
    if r.topology {
        Topology::Change
    } else {
        Topology::Keep
    }
}

/// What a file's `blends` asks the kernel.
fn blends(r: crate::doc::edit::Retrim) -> Blends {
    if r.blends {
        Blends::Rebuild
    } else {
        Blends::Keep
    }
}

/// The kernel's healing for a file's.
fn heal(h: crate::doc::edit::Heal) -> ryukei::tweak::Heal {
    match h {
        crate::doc::edit::Heal::Auto => ryukei::tweak::Heal::Auto,
        crate::doc::edit::Heal::Cap => ryukei::tweak::Heal::Cap,
        crate::doc::edit::Heal::Grow => ryukei::tweak::Heal::Grow,
    }
}

/// Recognition's tolerance for a blend edit's: its own, else the
/// kernel's 10 µm.
fn recognition(t: Option<&crate::units::Quantity>) -> f64 {
    t.map_or(ryukei::tweak::DEFAULT_TOLERANCE, |q| q.meters())
}

/// Each seed of a delete with the faces it found, by name.
type Found = Vec<(Name, Vec<Name>)>;

/// A delete's spec on `body`: its faces, then each seed's feature's
/// (`feature_faces`), each face once in the order first named; and what
/// each seed found.
fn expand(
    body: &ryukei::topo::Body,
    faces: &[Name],
    seeds: &[Name],
    how: ryukei::tweak::Heal,
) -> core::result::Result<(ryukei::tweak::DeleteSpec, Found), ryukei::Error> {
    let mut all: Vec<Name> = Vec::new();
    for n in faces {
        if !all.contains(n) {
            all.push(*n);
        }
    }
    let mut found = Vec::with_capacity(seeds.len());
    for s in seeds {
        let of = ryukei::tweak::feature_faces(body, *s)?;
        for n in &of {
            if !all.contains(n) {
                all.push(*n);
            }
        }
        found.push((*s, of));
    }
    Ok((
        ryukei::tweak::DeleteSpec {
            faces: all,
            heal: how,
        },
        found,
    ))
}

/// What a delete's report keeps (`Made::report`): its target, and each
/// seed with the faces it found, by name.
pub(crate) fn delete_report(target: &Id, found: &[(Name, Vec<Name>)]) -> Value {
    json!({
        "target": target,
        "seeds": found.iter().map(|(s, of)| json!({
            "seed": s.to_string(),
            "faces": of.iter().map(|n| n.to_string()).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
    })
}

/// What a blend delete's report keeps: its target, each blend it took
/// (as [`crate::blends::blend_raw`]) and the faces of the corners they
/// reached, by name.
pub(crate) fn blend_delete_report(
    target: &Id,
    rec: &ryukei::tweak::Recognition,
    taken: &std::collections::BTreeSet<usize>,
) -> Value {
    json!({
        "target": target,
        "blends": taken.iter().map(|&i| crate::blends::blend_raw(rec, i)).collect::<Vec<_>>(),
        "corners": crate::blends::corners_of(rec, taken)
            .into_iter()
            .map(|j| rec.corners[j].face.to_string())
            .collect::<Vec<_>>(),
    })
}

/// What a tweak asks the kernel.
pub(crate) enum Spec {
    Move(MoveSpec),
    Offset(OffsetFacesSpec),
    Replace(ryukei::tweak::ReplaceSpec),
    /// A draft, whether its hinge is a parting line, and each listed
    /// face's signed angle (radians) as the file asks it, for its report.
    Draft {
        spec: DraftSpec,
        parting: bool,
        asked: Vec<(Name, f64)>,
    },
    /// A delete: the faces listed, the seeds whose features go whole
    /// (found when it builds), and how gaps heal.
    Delete {
        faces: Vec<Name>,
        seeds: Vec<Name>,
        heal: ryukei::tweak::Heal,
    },
    /// A blend delete: every blend at most `below` meters and those
    /// holding `faces`, recognised at `tolerance`.
    DeleteBlend {
        below: Option<f64>,
        faces: Vec<Name>,
        tolerance: f64,
    },
    /// A resize: each group's faces and the kind it gives their blends,
    /// recognised at `tolerance`.
    ResizeBlend {
        groups: Vec<(Vec<Name>, ryukei::blend::BlendKind)>,
        tolerance: f64,
    },
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

/// The unit direction of `v`, which `role`'s `g` gave:
/// `reference_wrong_kind` where it has no length.
fn unit(at: &At, role: &str, g: &GeomRef, v: Vector3) -> Result<UnitVector3> {
    v.normalize().ok_or_else(|| {
        at.fail(
            Code::ReferenceWrongKind,
            role,
            g,
            "gives no direction".to_owned(),
        )
    })
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
            let u = unit(&at, "direction", direction, v)?;
            Ok(Some(along(u, distance.meters())))
        }
        MoveBy::Rotate { axis, angle } => {
            let l = crate::geometry::one(&at, "axis", axis, Want::Axis)?.axis();
            Ok(Some(about(&l, angle.radians())))
        }
        MoveBy::Offset { .. } => Ok(None),
    }
}

/// The line a move goes along or turns about, resolved in its view, for
/// the window to draw: a rotation's axis where it lies, or a
/// translation's direction through its reference's point, turned the way
/// the faces go (a negative distance runs it backwards); `None` for an
/// offset.
pub(crate) fn line(
    part: &Part,
    id: &Id,
    m: &MoveFace,
    results: &dyn Results,
) -> Result<Option<Line>> {
    let at = at(part, id, results);
    match &m.by {
        MoveBy::Translate {
            direction,
            distance,
        } => {
            let (origin, v) = match crate::geometry::resolve(
                &at,
                "direction",
                direction,
                &[Want::Axis, Want::Plane],
            )? {
                Geom::Axis(l) => (l.origin, l.dir.get()),
                Geom::Plane(p) => (p.point, p.normal),
                _ => unreachable!("asked for an axis or a plane"),
            };
            let u = unit(&at, "direction", direction, v)?;
            let u = if distance.meters() < 0.0 { -u } else { u };
            Ok(Some(Line::along(origin, u)))
        }
        MoveBy::Rotate { axis, .. } => Ok(Some(
            crate::geometry::one(&at, "axis", axis, Want::Axis)?.axis(),
        )),
        MoveBy::Offset { .. } => Ok(None),
    }
}

/// The face (or, with `edge`, the edge) `r` names (in `role` of tweak
/// `id`) in the target's view: `targets_mixed` for another solid's,
/// `reference_wrong_kind` for another kind, and whatever the reference
/// fails with.
#[allow(clippy::too_many_arguments)]
fn named(
    part: &Part,
    id: &Id,
    target: &Id,
    ty: &str,
    role: &str,
    r: &Reference,
    results: &dyn Results,
    edge: bool,
) -> Result<Name> {
    let what = if edge { "an edge" } else { "a face" };
    if r.feature != *target {
        return Err(Error::new(
            Code::TargetsMixed,
            format!(
                "{ty} {id}: its {role} {} is {what} of {}, not of its target {target}; a {ty} acts on one solid",
                r.text(&part.id),
                r.feature
            ),
        )
        .with("feature", id)
        .with("role", role)
        .with("solids", [target.as_str(), r.feature.as_str()]));
    }
    let (_, e) = crate::reference::resolve(part, part.order(), id, role, r, results)?;
    match (e, edge) {
        (EntityId::Face(_), false) | (EntityId::Edge(_), true) => Ok(r.name),
        (e, _) => Err(crate::reference::wrong_kind(
            part,
            id,
            role,
            r,
            crate::reference::kind(e),
            what,
        )),
    }
}

fn face(
    part: &Part,
    id: &Id,
    target: &Id,
    ty: &str,
    role: &str,
    r: &Reference,
    results: &dyn Results,
) -> Result<Name> {
    named(part, id, target, ty, role, r, results, false)
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

/// A draft's pull (turned round by `reverse`) and hinge, resolved in its
/// view (feature `id` of `part`): a neutral plane's point and unit normal,
/// or a parting line's edges (each an edge of the target) and its pull,
/// an axis or a plane's normal.
pub(crate) fn hinge(
    part: &Part,
    id: &Id,
    d: &Draft,
    results: &dyn Results,
) -> Result<(UnitVector3, Hinge)> {
    let at = at(part, id, results);
    let (pull, hinge) = match &d.hinge {
        DraftHinge::Neutral { plane } => {
            let p = crate::geometry::one(&at, "neutral", plane, Want::Plane)?.plane();
            (
                unit(&at, "neutral", plane, p.normal)?,
                Hinge::Neutral { origin: p.point },
            )
        }
        DraftHinge::Parting { edges, pull } => {
            let v = match crate::geometry::resolve(&at, "pull", pull, &[Want::Axis, Want::Plane])? {
                Geom::Axis(l) => l.dir.get(),
                Geom::Plane(p) => p.normal,
                _ => unreachable!("asked for an axis or a plane"),
            };
            let mut names = Vec::with_capacity(edges.len());
            for (i, r) in edges.iter().enumerate() {
                names.push(named(
                    part,
                    id,
                    &d.target,
                    "draft",
                    &format!("parting.{i}"),
                    r,
                    results,
                    true,
                )?);
            }
            (unit(&at, "pull", pull, v)?, Hinge::PartingLine(names))
        }
    };
    Ok((if d.reverse { -pull } else { pull }, hinge))
}

/// Resolves a tweak (feature `id` of `part`): its target, through
/// [`crate::rebuild::read`], as the table says (a sheet too), its faces and its
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
            let built = crate::rebuild::read(part, solids, id, f, "target", &m.target, before)?;
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
                    topology: topology(m.retrim),
                    blends: blends(m.retrim),
                }),
                None => Spec::Offset(OffsetFacesSpec {
                    faces,
                    tangent: m.tangent,
                    topology: topology(m.retrim),
                    blends: blends(m.retrim),
                }),
            };
            Ok(Resolved {
                target: &m.target,
                built,
                spec,
            })
        }
        Feature::ReplaceFace(x) => {
            let built = crate::rebuild::read(part, solids, id, f, "target", &x.target, before)?;
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
                    topology: topology(x.retrim),
                    blends: blends(x.retrim),
                }),
            })
        }
        Feature::Draft(d) => {
            let built = crate::rebuild::read(part, solids, id, f, "target", &d.target, before)?;
            let (pull, hinge) = hinge(part, id, d, before)?;
            let mut faces = Vec::new();
            let mut asked = Vec::new();
            for (role, r) in d.drafted() {
                let n = face(part, id, &d.target, ty, &role, r, before)?;
                let a = d.angle_of(r);
                faces.push(DraftFace {
                    face: n,
                    angle: role.starts_with("groups.").then_some(a),
                    flip: d.flip.contains(r),
                });
                asked.push((n, a));
            }
            Ok(Resolved {
                target: &d.target,
                built,
                spec: Spec::Draft {
                    spec: DraftSpec {
                        pull,
                        angle: d.angle.radians(),
                        hinge,
                        faces,
                        tangent: d.tangent,
                        topology: topology(d.retrim),
                        blends: blends(d.retrim),
                    },
                    parting: matches!(d.hinge, DraftHinge::Parting { .. }),
                    asked,
                },
            })
        }
        Feature::DeleteFace(x) => {
            let built = crate::rebuild::read(part, solids, id, f, "target", &x.target, before)?;
            let mut faces = Vec::with_capacity(x.faces.len());
            for (i, r) in x.faces.iter().enumerate() {
                faces.push(face(
                    part,
                    id,
                    &x.target,
                    ty,
                    &format!("faces.{i}"),
                    r,
                    before,
                )?);
            }
            let mut seeds = Vec::with_capacity(x.seeds.len());
            for (i, r) in x.seeds.iter().enumerate() {
                seeds.push(face(
                    part,
                    id,
                    &x.target,
                    ty,
                    &format!("seeds.{i}"),
                    r,
                    before,
                )?);
            }
            Ok(Resolved {
                target: &x.target,
                built,
                spec: Spec::Delete {
                    faces,
                    seeds,
                    heal: heal(x.heal),
                },
            })
        }
        Feature::DeleteBlend(x) => {
            let built = crate::rebuild::read(part, solids, id, f, "target", &x.target, before)?;
            let mut faces = Vec::with_capacity(x.faces.len());
            for (i, r) in x.faces.iter().enumerate() {
                faces.push(face(
                    part,
                    id,
                    &x.target,
                    ty,
                    &format!("faces.{i}"),
                    r,
                    before,
                )?);
            }
            Ok(Resolved {
                target: &x.target,
                built,
                spec: Spec::DeleteBlend {
                    below: x.below.as_ref().map(|q| q.meters()),
                    faces,
                    tolerance: recognition(x.tolerance.as_ref()),
                },
            })
        }
        Feature::ResizeBlend(x) => {
            use crate::doc::blend::ChamferForm;
            use crate::doc::edit::BlendSize;
            use ryukei::blend::{BlendKind, Chamfer};
            let built = crate::rebuild::read(part, solids, id, f, "target", &x.target, before)?;
            let mut groups = Vec::with_capacity(x.groups.len());
            for (i, g) in x.groups.iter().enumerate() {
                let support = |r: &Reference| {
                    face(
                        part,
                        id,
                        &x.target,
                        ty,
                        &format!("groups.{i}.face"),
                        r,
                        before,
                    )
                };
                let kind = match &g.size {
                    BlendSize::Fillet { radius } => BlendKind::Fillet {
                        radius: radius.meters(),
                    },
                    BlendSize::Chamfer(ChamferForm::Equal { d }) => {
                        BlendKind::Chamfer(Chamfer::Equal {
                            distance: d.meters(),
                        })
                    }
                    BlendSize::Chamfer(ChamferForm::Distances { d1, d2, face: on }) => {
                        BlendKind::Chamfer(Chamfer::Distances {
                            first: d1.meters(),
                            second: d2.meters(),
                            first_face: support(on)?,
                        })
                    }
                    BlendSize::Chamfer(ChamferForm::DistanceAngle { d, angle, face: on }) => {
                        BlendKind::Chamfer(Chamfer::DistanceAngle {
                            distance: d.meters(),
                            angle: angle.radians(),
                            face: support(on)?,
                        })
                    }
                };
                let mut faces = Vec::with_capacity(g.faces.len());
                for (j, r) in g.faces.iter().enumerate() {
                    faces.push(face(
                        part,
                        id,
                        &x.target,
                        ty,
                        &format!("groups.{i}.faces.{j}"),
                        r,
                        before,
                    )?);
                }
                groups.push((faces, kind));
            }
            Ok(Resolved {
                target: &x.target,
                built,
                spec: Spec::ResizeBlend {
                    groups,
                    tolerance: recognition(x.tolerance.as_ref()),
                },
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
            "set tangent true, so faces tangent to a tweaked face move with it; or blends: rebuild (--blends rebuild), which makes the fillet again on the tweaked faces; or put the fillet after this feature in the list, where it is built again on the tweaked edges",
        ),
        ("edge_vanishes" | "vertex_splits" | "face_collapses", _) => Some(
            "the tweak would change the solid's topology: set topology: change (--topology change) to let faces grow over their neighbours, shrink away or split, and edges and vertices come and go; or a smaller motion, or the faces it reaches listed too, may keep it",
        ),
        ("hinge_not_on_face", _) | ("unsupported", Some("hinge_chain")) => {
            Some("split the face along the parting line first (a split, or a sketch's cut)")
        }
        ("hinge_along_pull", _) => Some("give a pull more nearly square to the face"),
        ("gap_not_healed", _) => Some(
            "delete the faces round the gap too, try heal: grow or heal: cap, or give one face of a hole, boss or pocket as a seed to delete it whole",
        ),
        ("heal_ambiguous" | "region_too_small", _) => Some(
            "a smaller motion, or the neighbouring faces listed too, gives the edit less to decide",
        ),
        ("cap_not_on_face", _) => Some(
            "heal the gap by growing its neighbours instead (heal: grow), or delete the faces round it too",
        ),
        ("grow_would_cap", _) => Some("heal the gap by a cap instead (heal: cap or auto)"),
        ("no_feature", _) => Some(
            "name the feature's faces yourself: a hole's or a boss's faces, which leave a gap a cap closes",
        ),
        ("not_a_blend", _) => Some(
            "ryucad blends <part> lists the faces recognised as fillets, chamfers and corners: name one of those (a chamfer's distances face is one of its two supports); a blend made from a file may be read at a looser tolerance (tolerance, up to 0.1 mm)",
        ),
        ("blend_lost", _) => Some(
            "delete the blend first (delete_blend), make the edit, and fillet or chamfer the edges again after it; or a size nearer the blend's own",
        ),
        ("bad_tolerance", _) => Some("give a tolerance from 0.00001 mm to 0.1 mm"),
        _ => None,
    }
}

/// [`hint`] for a feature that does (`changes`) or doesn't change
/// topology, refused on `body` (the target as built): a self-intersection
/// and a lone vertex are topology changes only where the tweak keeps its
/// topology.
pub(crate) fn hint_for(
    r: &Refusal,
    changes: bool,
    body: &ryukei::topo::Body,
) -> Option<&'static str> {
    // ryukei re-trims an `Uncertain` only at one of the body's vertices
    // (`ryukei::tweak::retrim::takes`); every other one is refused again
    // under `topology: change`, so only those get its hint.
    let at_vertex = || {
        r.entities.iter().any(|(role, n)| {
            *role == "at" && matches!(body.by_name(*n), Some(ryukei::topo::EntityId::Vertex(_)))
        })
    };
    match (r.reason, r.case) {
        ("self_intersecting", _) | ("unsupported", Some("lone_vertex")) if !changes => Some(
            "the tweak would change the solid's topology: set topology: change (--topology change) to let the kernel re-trim the faces it reaches",
        ),
        // The fixed-topology engine leaves a face reaching past its
        // neighbour (a wall drafted past the box's top) uncertain at the
        // vertex where they meet.
        ("uncertain", _) if !changes && at_vertex() => Some(
            "a decision rounding left uncertain, as where a tweaked face reaches past its neighbour: set topology: change (--topology change) to let the kernel re-trim the faces it reaches",
        ),
        ("self_intersecting", _) | ("unsupported", Some("lone_vertex")) => None,
        _ => hint(r),
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
    changes: bool,
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
        ryukei::Error::NotASolid { .. } => Refusal {
            reason: "not_a_solid",
            case: None,
            message: "this edit needs a solid, and the target is a sheet",
            entities: vec![],
        },
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
    if let Some(h) = hint_for(&r, changes, &built.body) {
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

/// What to do about a budgeted edit of type `ty` that would turn the
/// solid inside out. A delete's gap was healed the wrong way round
/// (ryukei #692); a tweak has no gap, `heal` or `seeds` (`request_fields`
/// refuses them), so it is pointed at a smaller motion or more faces. No
/// tweak case is known to reach the guard yet: every case of #692 seen is
/// a delete's.
fn inside_out_hint(ty: &str) -> &'static str {
    if ty == "delete_face" {
        "the kernel healed the gap the wrong way round (ryukei #692): delete the faces round the gap too, try heal: cap or heal: grow, or give one face of a hole, boss or pocket as a seed"
    } else {
        "the kernel re-trimmed the faces the wrong way round (ryukei #692): a smaller motion, or the neighbouring faces listed too, gives the edit less to decide"
    }
}

/// A direct edit's result's signed volume (m³) where it is negative: a
/// solid turned inside out, its faces wound against their region. The
/// body checks decide winding only for planar faces with straight edges,
/// and mass properties don't read it (ryukei #692), so a coarse mesh's
/// signed volume says (1 mm chord, half a radian: a gross inversion, not
/// a measurement). A sheet, or a body that doesn't mesh: `None`.
fn inside_out(body: &ryukei::topo::Body) -> Option<f64> {
    if body.kind() != ryukei::topo::BodyKind::Solid {
        return None;
    }
    let m = ryukei::mesh::tessellate(body, &ryukei::mesh::MeshParams::new(1e-3, 0.5)).ok()?;
    if !m.faults.is_empty() {
        return None;
    }
    let mut v = 0.0;
    for [a, b, c] in m.welded_triangles() {
        let [a, b, c] = [a, b, c].map(|i| m.positions[i as usize].to_vector());
        v += a.dot(b.cross(c)) / 6.0;
    }
    (v < 0.0).then_some(v)
}

/// `edit_budget_exceeded` for a direct edit (feature `id`, `ty`) its
/// budget stopped; `None` for any other error.
pub(crate) fn budget_error(id: &Id, ty: &str, e: &ryukei::Error) -> Option<Error> {
    let ryukei::Error::BudgetExceeded { spent, budget } = *e else {
        return None;
    };
    Some(
        Error::new(
            Code::EditBudgetExceeded,
            format!("{ty} {id}: the edit didn't finish within edit.budget ({budget} work units; it had spent {spent})"),
        )
        .with("feature", id)
        .with("spent", spent)
        .with("budget", budget)
        .with(
            "hint",
            "raise edit.budget (ryucad config set edit.budget <units>) to let it run longer, or edit less at once",
        ),
    )
}

/// Origins of a body a direct edit made of `built`: as
/// [`crate::rebuild::replaced_origins`] gives them (a grown edge in
/// place of a fillet is `{feature, replaces}`), but where that gives an
/// entity the feature's own origin, a split's child takes its parent's,
/// and a merge's child its parents' when they share one (a parent being
/// a split's child too). A tweak that
/// keeps its topology journals neither, so its origins are ryucad 17's.
pub(crate) fn edit_origins(
    id: &Id,
    built: &Built,
    made: &(ryukei::topo::Body, ryukei::name::Journal),
) -> crate::rebuild::Origins {
    // Splits first, then merges, whatever order the journal gives them:
    // the re-trim splits a face it only re-trims and merges the pieces
    // again (ryukei 13), journalling the merge first, and the merged face
    // is still the one it was.
    let mut from: BTreeMap<String, Value> = BTreeMap::new();
    for e in made.1.events() {
        if let Event::Split { parent, children } = e
            && let Some(o) = built.origins.get(&parent.to_string())
        {
            for c in children {
                from.entry(c.to_string()).or_insert_with(|| o.clone());
            }
        }
    }
    for e in made.1.events() {
        if let Event::Merged { parents, child } = e {
            let of = |p: &Name| {
                let k = p.to_string();
                built.origins.get(&k).or_else(|| from.get(&k)).cloned()
            };
            let mut theirs = parents.iter().map(of);
            if let Some(Some(o)) = theirs.next()
                && theirs.all(|x| x.as_ref() == Some(&o))
            {
                from.insert(child.to_string(), o);
            }
        }
    }
    let own = json!({"feature": id});
    crate::rebuild::replaced_origins(id, built, made)
        .iter()
        .map(|(k, o)| {
            let o = match from.get(k) {
                Some(p) if *o == own => p.clone(),
                _ => o.clone(),
            };
            (k.clone(), o)
        })
        .collect()
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
    let budgeted = budgeted(f);
    let changes = f.retrim().is_some_and(|r| r.topology)
        || matches!(
            f,
            Feature::DeleteFace(_) | Feature::DeleteBlend(_) | Feature::ResizeBlend(_)
        );
    let work = if budgeted {
        crate::work::work(run.job).with_budget(run.edit_budget)
    } else {
        crate::work::work(run.job)
    };
    let key = crate::rebuild::op_key(&part.id, id);
    let body = &r.built.body;
    let (made, report) = match &r.spec {
        Spec::Move(s) => (ryukei::tweak::move_faces_with(key, body, s, &work), None),
        Spec::Offset(s) => (ryukei::tweak::offset_faces_with(key, body, s, &work), None),
        Spec::Replace(s) => (ryukei::tweak::replace_faces_with(key, body, s, &work), None),
        Spec::Draft {
            spec,
            parting,
            asked,
        } => match ryukei::tweak::draft_with(key, body, spec, &work) {
            Ok((b, j, rep)) => (
                Ok((b, j)),
                Some(draft_report(r.target, *parting, asked, &rep)),
            ),
            Err(e) => (Err(e), None),
        },
        Spec::Delete { faces, seeds, heal } => match expand(body, faces, seeds, *heal) {
            Ok((spec, found)) => (
                ryukei::tweak::delete_faces_with(key, body, &spec, &work),
                Some(delete_report(r.target, &found)),
            ),
            Err(e) => (Err(e), None),
        },
        Spec::DeleteBlend {
            below,
            faces,
            tolerance,
        } => {
            let options = ryukei::tweak::RecogniseOptions {
                tolerance: *tolerance,
            };
            match ryukei::tweak::recognise_blends_with(body, &options, &work) {
                Err(e) => (Err(e), None),
                Ok(rec) => {
                    let taken = crate::blends::taken(&rec, *below, faces);
                    let spec = ryukei::tweak::DeleteBlends {
                        max_size: *below,
                        faces: faces.clone(),
                        options,
                    };
                    (
                        ryukei::tweak::delete_blends_with(key, body, &spec, &work),
                        Some(blend_delete_report(r.target, &rec, &taken)),
                    )
                }
            }
        }
        Spec::ResizeBlend { groups, tolerance } => {
            let options = ryukei::tweak::RecogniseOptions {
                tolerance: *tolerance,
            };
            match ryukei::tweak::recognise_blends_with(body, &options, &work) {
                Err(e) => (Err(e), None),
                Ok(rec) => {
                    let sizes = crate::blends::sizes(id, &rec, groups)?;
                    (
                        ryukei::tweak::resize_blends_with(key, body, &sizes, &options, &work),
                        Some(crate::blends::resize_report(r.target, &rec, &sizes)),
                    )
                }
            }
        }
    };
    let made = made.map_err(|e| {
        crate::work::stopped(&e, run.job)
            .or_else(|| budget_error(id, ty, &e))
            .unwrap_or_else(|| kernel_error(part, id, ty, (r.target, r.built), e, changes))
    })?;
    if budgeted && let Some(v) = inside_out(&made.0) {
        return Err(Error::new(
            Code::TweakFailed,
            format!("{ty} {id}: the edit would turn the solid inside out (its volume would be {:.6} mm³)", v * 1e9),
        )
        .with("feature", id)
        .with("solid", r.target)
        .with("reason", "inside_out")
        .with("kernel", "tweak")
        .with("volume_mm3", v * 1e9)
        .with("hint", inside_out_hint(ty)));
    }
    let origins = edit_origins(id, r.built, &made);
    let built = crate::rebuild::finish(id, ty, made, origins)?;
    let report = if budgeted {
        let mut rep = report.unwrap_or_else(|| json!({"target": r.target}));
        rep["spent"] = json!(work.spent());
        Some(rep)
    } else {
        report
    };
    Ok(Made {
        report: report.map(Arc::new),
        ..Made::new(vec![(r.target.clone(), Arc::new(built))])
    })
}

/// What a draft's report keeps (`Made::report`, so the disk cache holds
/// it): its target, whether its hinge is a parting line, and each drafted
/// face's least and greatest angle from its pull (radians, by name),
/// with `asked`, the signed angle the file asks, for the faces it lists
/// (faces joined by tangency have none).
pub(crate) fn draft_report(
    target: &Id,
    parting: bool,
    asked: &[(Name, f64)],
    r: &ryukei::tweak::DraftReport,
) -> Value {
    let angles: Vec<Value> = r
        .faces
        .iter()
        .map(|a| {
            let mut v =
                json!({"face": a.face.to_string(), "least": a.least, "greatest": a.greatest});
            if let Some((_, t)) = asked.iter().find(|(n, _)| *n == a.face) {
                v["asked"] = json!(t);
            }
            v
        })
        .collect();
    json!({
        "target": target,
        "hinge": if parting { "parting" } else { "neutral" },
        "angles": angles,
    })
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
    if !TYPES.contains(&ty) {
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
    if ty == "delete_face" || ty == "delete_blend" {
        // The CLI's --seed goes as a pattern's features.
        if ty == "delete_face" && f.seeds.is_none() {
            f.seeds = f.features.take();
        }
        if !groups.is_empty() {
            return Err(Error::new(
                Code::InvalidParam,
                format!("a {ty}'s faces are a list of faces, each <part>/<solid>/<name>: it has no groups"),
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
        })
        .or_else(|| f.seeds.as_ref()?.first().map(|t| json!(t)));
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
        Some(Feature::Draft(d)) => d.fields(),
        Some(Feature::DeleteFace(x)) => x.fields(),
        Some(Feature::DeleteBlend(x)) => x.fields(),
        Some(Feature::ResizeBlend(x)) => x.fields(),
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
        retrim_given(o, f)?;
    } else if ty == "delete_blend" {
        delete_blend_given(o, f, &reference)?;
    } else if ty == "resize_blend" {
        resize_given(o, f, &reference)?;
    } else if ty == "delete_face" {
        delete_given(o, f, &reference)?;
    } else {
        if ty == "draft" {
            draft_given(o, f, &reference, &geom)?;
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
        retrim_given(o, f)?;
    }
    let mut r = crate::doc::files::Obj::new(&v, "")?;
    let out = match ty {
        "replace_face" => Feature::ReplaceFace(Box::new(
            crate::doc::tweak::ReplaceFace::from_fields(&mut r)?,
        )),
        "draft" => Feature::Draft(Box::new(crate::doc::tweak::Draft::from_fields(
            &mut r, env,
        )?)),
        "delete_face" => {
            Feature::DeleteFace(Box::new(crate::doc::edit::DeleteFace::from_fields(&mut r)?))
        }
        "delete_blend" => Feature::DeleteBlend(Box::new(
            crate::doc::edit::DeleteBlend::from_fields(&mut r, unit, env)?,
        )),
        "resize_blend" => Feature::ResizeBlend(Box::new(
            crate::doc::edit::ResizeBlend::from_fields(&mut r, unit, env)?,
        )),
        _ => Feature::MoveFace(Box::new(MoveFace::from_fields(&mut r, unit, env)?)),
    };
    r.finish()?;
    Ok(out)
}

/// A text list of faces (`faces` as the request gives them after
/// [`take_given`]) as references, each at `/<key>/<i>`; `None` when not
/// given.
fn face_list(
    f: &FeatureFields,
    key: &str,
    reference: &dyn Fn(&str, &str) -> Result<Value>,
) -> Result<Option<Vec<Value>>> {
    let Some(Value::Array(a)) = &f.faces else {
        return Ok(None);
    };
    a.iter()
        .filter_map(|x| x.as_str())
        .enumerate()
        .map(|(i, t)| reference(t, &format!("/{key}/{i}")))
        .collect::<Result<Vec<_>>>()
        .map(Some)
}

/// A scalar given as text or a number, `""` taking it away.
fn put(o: &mut serde_json::Map<String, Value>, key: &str, given: &Option<Value>) {
    match given {
        None => {}
        Some(Value::String(s)) if s.is_empty() => {
            o.remove(key);
        }
        Some(v) => {
            o.insert(key.into(), v.clone());
        }
    }
}

/// A blend delete's own fields from what was given, over `o` (its file's):
/// its faces as text (`[]` for none), `below` and `tolerance`.
fn delete_blend_given(
    o: &mut serde_json::Map<String, Value>,
    f: &FeatureFields,
    reference: &dyn Fn(&str, &str) -> Result<Value>,
) -> Result<()> {
    if let Some(refs) = face_list(f, "faces", reference)? {
        if refs.is_empty() {
            o.remove("faces");
        } else {
            o.insert("faces".into(), json!(refs));
        }
    }
    put(o, "below", &f.below);
    put(o, "tolerance", &f.tolerance);
    Ok(())
}

/// A resize's own fields from what was given, over `o` (its file's): its
/// groups whole (faces and a chamfer's face as text), or its one group's
/// fields: a fillet's `radius` (which takes a chamfer's form away), a
/// chamfer's `form` (keeping only what it takes) and sizes, its
/// `chamfer_face`, and the blends' `faces`. A file of several groups
/// takes them whole.
fn resize_given(
    o: &mut serde_json::Map<String, Value>,
    f: &FeatureFields,
    reference: &dyn Fn(&str, &str) -> Result<Value>,
) -> Result<()> {
    let sizes = [
        ("radius", &f.radius),
        ("d", &f.d),
        ("d1", &f.d1),
        ("d2", &f.d2),
        ("angle", &f.angle),
    ];
    let faces = face_list(f, "groups/0/faces", reference)?;
    let one = sizes.iter().any(|(_, x)| x.is_some())
        || f.form.is_some()
        || f.chamfer_face.is_some()
        || faces.is_some();
    if let Some(gs) = &f.groups {
        if one {
            return Err(Error::new(
                Code::Usage,
                "give a resize_blend's groups, or its one group's fields (radius, or form, d, d1, d2, angle and chamfer_face; and faces), not both",
            ));
        }
        let Value::Array(gs) = gs else {
            return Err(Error::new(
                Code::InvalidParam,
                "a resize_blend's groups are a list, each a size and the blends' faces",
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
            if let Some(t) = g.get("face").and_then(Value::as_str).map(str::to_owned) {
                g["face"] = reference(&t, &format!("/groups/{i}/face"))?;
            }
            out.push(g);
        }
        o.insert("groups".into(), Value::Array(out));
    } else if one {
        let gs = o
            .entry("groups")
            .or_insert_with(|| json!([{}]))
            .as_array_mut()
            .expect("groups are a list");
        if gs.len() != 1 {
            return Err(Error::new(
                Code::InvalidParam,
                format!(
                    "this resize_blend has {} groups: give them all, as groups",
                    gs.len()
                ),
            )
            .with("param", "groups"));
        }
        let g = gs[0].as_object_mut().expect("a group");
        if f.radius.is_some() {
            g.retain(|k, _| k == "faces");
        }
        if let Some(form) = &f.form {
            let takes: &[&str] = match form.as_str() {
                "equal" => &["d", "faces"],
                "distances" => &["d1", "d2", "face", "faces"],
                "distance_angle" => &["d", "angle", "face", "faces"],
                _ => &["faces"],
            };
            g.retain(|k, _| takes.contains(&k.as_str()));
            g.insert("form".into(), json!(form));
        }
        for (k, x) in sizes {
            if let Some(x) = x {
                g.insert(k.into(), x.clone());
            }
        }
        if let Some(t) = &f.chamfer_face {
            g.insert("face".into(), reference(t, "/groups/0/face")?);
        }
        if let Some(refs) = faces {
            g.insert("faces".into(), json!(refs));
        }
    }
    put(o, "tolerance", &f.tolerance);
    Ok(())
}

/// A delete's own fields from what was given, over `o` (its file's):
/// faces and seeds as text, each list replacing the file's (`[]` for
/// none), and its healing (`auto` taking it away). A heal it doesn't
/// know, or neither faces nor seeds left, is refused at its param.
fn delete_given(
    o: &mut serde_json::Map<String, Value>,
    f: &FeatureFields,
    reference: &dyn Fn(&str, &str) -> Result<Value>,
) -> Result<()> {
    let texts: Option<Vec<String>> = match &f.faces {
        None => None,
        Some(Value::Array(a)) => Some(
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_owned))
                .collect(),
        ),
        Some(_) => None,
    };
    for (key, list) in [("faces", texts.as_ref()), ("seeds", f.seeds.as_ref())] {
        let Some(list) = list else { continue };
        let refs = list
            .iter()
            .enumerate()
            .map(|(i, t)| reference(t, &format!("/{key}/{i}")))
            .collect::<Result<Vec<_>>>()?;
        if refs.is_empty() {
            o.remove(key);
        } else {
            o.insert(key.into(), json!(refs));
        }
    }
    match f.heal.as_deref() {
        None => {}
        Some("auto") => {
            o.remove("heal");
        }
        Some(h @ ("cap" | "grow")) => {
            o.insert("heal".into(), json!(h));
        }
        Some(h) => {
            return Err(Error::new(
                Code::InvalidParam,
                format!("a delete_face's heal is auto, cap or grow, not {h:?}"),
            )
            .with("param", "heal"));
        }
    }
    if !o.contains_key("faces") && !o.contains_key("seeds") {
        return Err(Error::new(
            Code::InvalidParam,
            "a delete_face names the faces it deletes (faces), or a face of each hole, boss or pocket it deletes whole (seeds)",
        )
        .with("param", "faces"));
    }
    Ok(())
}

/// A draft's own fields from what was given, over `o` (its file's): an
/// angle; a neutral plane, which takes the parting line and pull away; a
/// parting line (edges as text), which takes the neutral plane away; a
/// pull; `reverse`; and flipped faces (a bool is a datum's or split's
/// flip, refused).
fn draft_given(
    o: &mut serde_json::Map<String, Value>,
    f: &FeatureFields,
    reference: &dyn Fn(&str, &str) -> Result<Value>,
    geom: &dyn Fn(&str, &str) -> Result<Value>,
) -> Result<()> {
    if let Some(q) = &f.angle {
        o.insert("angle".into(), q.clone());
    }
    if let Some(n) = &f.neutral {
        o.remove("parting");
        o.remove("pull");
        o.insert("neutral".into(), geom(n, "neutral")?);
    }
    if let Some(es) = &f.parting {
        o.remove("neutral");
        let refs = es
            .iter()
            .enumerate()
            .map(|(i, t)| reference(t, &format!("/parting/{i}")))
            .collect::<Result<Vec<_>>>()?;
        o.insert("parting".into(), json!(refs));
    }
    if let Some(p) = &f.pull {
        o.insert("pull".into(), geom(p, "pull")?);
    }
    match f.reverse {
        Some(true) => {
            o.insert("reverse".into(), json!(true));
        }
        Some(false) => {
            o.remove("reverse");
        }
        None => {}
    }
    match &f.flip {
        None => {}
        Some(crate::proto::Flip::On(_)) => {
            return Err(Error::new(
                Code::InvalidParam,
                "a draft's flip is the faces it drafts against its pull, each <part>/<solid>/<name>",
            )
            .with("param", "flip"));
        }
        Some(crate::proto::Flip::Faces(fs)) if fs.is_empty() => {
            o.remove("flip");
        }
        Some(crate::proto::Flip::Faces(fs)) => {
            let refs = fs
                .iter()
                .enumerate()
                .map(|(i, t)| reference(t, &format!("/flip/{i}")))
                .collect::<Result<Vec<_>>>()?;
            o.insert("flip".into(), json!(refs));
        }
    }
    Ok(())
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
            retrim_report(&mut v, m.retrim);
            for (role, r) in m.faces() {
                *v.pointer_mut(&pointer(&role)).expect("its own face") = json!(r.text(part));
            }
            v
        }
        Feature::ReplaceFace(x) => {
            let mut v = x.fields();
            retrim_report(&mut v, x.retrim);
            for (i, g) in x.groups.iter().enumerate() {
                v["groups"][i]["with"] = json!(g.with.text(part));
            }
            for (role, r) in x.faces() {
                *v.pointer_mut(&pointer(&role)).expect("its own face") = json!(r.text(part));
            }
            v
        }
        Feature::Draft(d) => {
            let mut v = d.fields();
            retrim_report(&mut v, d.retrim);
            v["angle"] = d.angle.report();
            for (i, g) in d.groups.iter().enumerate() {
                v["groups"][i]["angle"] = g.quantity.report();
            }
            match &d.hinge {
                DraftHinge::Neutral { plane } => v["neutral"] = json!(plane.text(part)),
                DraftHinge::Parting { edges, pull } => {
                    v["parting"] = json!(edges.iter().map(|r| r.text(part)).collect::<Vec<_>>());
                    v["pull"] = json!(pull.text(part));
                }
            }
            v["reverse"] = json!(d.reverse);
            v["tangent"] = json!(d.tangent);
            for (role, r) in d.drafted() {
                *v.pointer_mut(&pointer(&role)).expect("its own face") = json!(r.text(part));
            }
            if !d.flip.is_empty() {
                v["flip"] = json!(d.flip.iter().map(|r| r.text(part)).collect::<Vec<_>>());
            }
            v
        }
        Feature::DeleteFace(x) => {
            let mut v = x.fields();
            v["heal"] = json!(x.heal.as_str());
            for (role, r) in x.references() {
                *v.pointer_mut(&pointer(&role)).expect("its own face") = json!(r.text(part));
            }
            v
        }
        Feature::DeleteBlend(x) => {
            let mut v = x.fields();
            if let Some(b) = &x.below {
                v["below"] = b.report();
            }
            if let Some(t) = &x.tolerance {
                v["tolerance"] = t.report();
            }
            for (role, r) in x.references() {
                *v.pointer_mut(&pointer(&role)).expect("its own face") = json!(r.text(part));
            }
            v
        }
        Feature::ResizeBlend(x) => {
            let mut v = x.fields();
            for (i, g) in x.groups.iter().enumerate() {
                let q = &mut v["groups"][i];
                match &g.size {
                    crate::doc::edit::BlendSize::Fillet { radius } => q["radius"] = radius.report(),
                    crate::doc::edit::BlendSize::Chamfer(form) => {
                        use crate::doc::blend::ChamferForm as F;
                        match form {
                            F::Equal { d } => q["d"] = d.report(),
                            F::Distances { d1, d2, .. } => {
                                q["d1"] = d1.report();
                                q["d2"] = d2.report();
                            }
                            F::DistanceAngle { d, angle, .. } => {
                                q["d"] = d.report();
                                q["angle"] = angle.report();
                            }
                        }
                    }
                }
            }
            if let Some(t) = &x.tolerance {
                v["tolerance"] = t.report();
            }
            for (role, r) in x.references() {
                *v.pointer_mut(&pointer(&role)).expect("its own face") = json!(r.text(part));
            }
            v
        }
        _ => unreachable!("a tweak"),
    }
}

/// A face tweak's `topology` and `blends` from what was given, over `o`
/// (its file's): `keep` takes each away, `change` and `rebuild` set it.
fn retrim_given(o: &mut serde_json::Map<String, Value>, f: &FeatureFields) -> Result<()> {
    for (key, given, other) in [
        ("topology", &f.topology, "change"),
        ("blends", &f.blends, "rebuild"),
    ] {
        match given.as_deref() {
            None => {}
            Some("keep") => {
                o.remove(key);
            }
            Some(x) if x == other => {
                o.insert(key.into(), json!(other));
            }
            Some(x) => {
                return Err(Error::new(
                    Code::InvalidParam,
                    format!("a face tweak's {key} is keep or {other}, not {x:?}"),
                )
                .with("param", key));
            }
        }
    }
    Ok(())
}

/// A face tweak's `topology` and `blends` as a report gives them: each
/// always, `keep` when not set.
fn retrim_report(v: &mut Value, r: crate::doc::edit::Retrim) {
    v["topology"] = json!(if r.topology { "change" } else { "keep" });
    v["blends"] = json!(if r.blends { "rebuild" } else { "keep" });
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
    unit: crate::units::Unit,
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
    if let Feature::MoveFace(m) = f
        && let Ok(Some(l)) = line(part, id, m, results)
    {
        let (o, d) = (l.origin, l.dir.get());
        v["line"] = json!({"origin_m": [o.x, o.y, o.z], "direction": [d.x, d.y, d.z]});
    }
    if let Feature::Draft(d) = f
        && let Ok((pull, h)) = hinge(part, id, d, results)
    {
        let p = pull.get();
        v["pull"] = json!([p.x, p.y, p.z]);
        if let Hinge::Neutral { origin: o } = h {
            v["neutral"] = json!({"origin_m": [o.x, o.y, o.z], "normal": [p.x, p.y, p.z]});
        }
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
        // A direct edit's new and lost entities, against its target as it
        // saw it (a split face's pieces are new, the face it was is gone),
        // and what it spent of its budget.
        if budgeted(f)
            && let Ok(was) =
                crate::rebuild::read(part, &Solids::of(part), id, f, "target", target, results)
        {
            let (had, has) = (
                crate::rebuild::names(&was.body),
                crate::rebuild::names(&b.body),
            );
            v["created"] = tally(
                (&part.id, target),
                &b.body,
                has.difference(&had),
                references,
            );
            v["deleted"] = tally(
                (&part.id, target),
                &was.body,
                had.difference(&has),
                references,
            );
        }
        if let crate::rebuild::Output::Solids(m) = out
            && let Some(rep) = &m.report
            && let Some(n) = rep.get("spent")
        {
            v["spent"] = n.clone();
        }
        if let crate::rebuild::Output::Solids(m) = out
            && let Some(rep) = &m.report
            && let Some(Value::Array(found)) = rep.get("seeds")
        {
            let at = |n: &Value| {
                json!(format!(
                    "{}/{target}/{}",
                    part.id,
                    n.as_str().unwrap_or_default()
                ))
            };
            v["seeds"] = Value::Array(
                found
                    .iter()
                    .map(|x| json!({
                        "seed": at(&x["seed"]),
                        "faces": x["faces"].as_array().into_iter().flatten().map(at).collect::<Vec<_>>(),
                    }))
                    .collect(),
            );
        }
        // A blend edit's kept report: each blend it took, each corner, each
        // blend it made again, as references and quantities.
        if let crate::rebuild::Output::Solids(m) = out
            && let Some(rep) = &m.report
        {
            let show = |x: &Value| crate::blends::render(&part.id, target, unit, x);
            if let Some(Value::Array(bs)) = rep.get("blends") {
                v["blends"] = Value::Array(bs.iter().map(show).collect());
            }
            if let Some(Value::Array(cs)) = rep.get("corners") {
                v["corners"] = Value::Array(
                    cs.iter()
                        .map(|c| {
                            json!(format!(
                                "{}/{target}/{}",
                                part.id,
                                c.as_str().unwrap_or_default()
                            ))
                        })
                        .collect(),
                );
            }
            if let Some(Value::Array(rs)) = rep.get("resized") {
                v["resized"] = Value::Array(rs.iter().map(show).collect());
            }
        }
        if let crate::rebuild::Output::Solids(m) = out
            && let Some(rep) = &m.report
            && let Some(a) = angles(&part.id, rep)
        {
            v["angles"] = a;
        }
    }
    Some(v)
}

/// Counts of `names` by kind as `body` holds them, and with `references`
/// each as `<part>/<target>/<name>`, sorted.
fn tally<'a>(
    (part, target): (&Id, &Id),
    body: &ryukei::topo::Body,
    names: impl Iterator<Item = &'a Name>,
    references: bool,
) -> Value {
    let (mut faces, mut edges, mut vertices) = (0u64, 0u64, 0u64);
    let mut refs = Vec::new();
    for n in names {
        match body.by_name(*n) {
            Some(EntityId::Face(_)) => faces += 1,
            Some(EntityId::Edge(_)) => edges += 1,
            Some(EntityId::Vertex(_)) => vertices += 1,
            _ => continue,
        }
        refs.push(format!("{part}/{target}/{n}"));
    }
    let mut v = json!({"faces": faces, "edges": edges, "vertices": vertices});
    if references {
        refs.sort();
        v["references"] = json!(refs);
    }
    v
}

/// A draft report's angles as output gives them: each face as
/// `<part>/<solid>/<name>`, its least and greatest angle from its pull in
/// degrees and radians, and `exact` where the two are the same bits (the
/// kernel's closed form for planes and cones).
fn angles(part: &Id, rep: &Value) -> Option<Value> {
    let target = rep["target"].as_str()?;
    let deg = |x: f64| json!({"value": format!("{} deg", crate::measure::number(x / crate::units::DEG)), "rad": x});
    Some(Value::Array(
        rep["angles"]
            .as_array()?
            .iter()
            .filter_map(|a| {
                let (lo, hi) = (a["least"].as_f64()?, a["greatest"].as_f64()?);
                Some(json!({
                    "face": format!("{part}/{target}/{}", a["face"].as_str()?),
                    "least": deg(lo),
                    "greatest": deg(hi),
                    "exact": lo.to_bits() == hi.to_bits(),
                    "varies": varies(rep, a),
                }))
            })
            .collect(),
    ))
}

/// How far (radians) a face's angle may lie from the angle asked, or
/// spread, before a draft warns: a millidegree.
pub const VARIES: f64 = 1e-3 * crate::units::DEG;

/// Whether a face's angles in a draft's report (its `least` and
/// `greatest`, and the `asked` angle where the report gives one) vary:
/// past [`VARIES`] from the angle asked, or spread wider than it. Only a
/// parting-line draft's can (see [`draft_warnings`]).
fn varies(rep: &Value, a: &Value) -> bool {
    if rep["hinge"] != "parting" {
        return false;
    }
    let (Some(lo), Some(hi)) = (a["least"].as_f64(), a["greatest"].as_f64()) else {
        return false;
    };
    let off = a["asked"]
        .as_f64()
        .is_some_and(|t| (lo - t).abs() > VARIES || (hi - t).abs() > VARIES);
    off || hi - lo > VARIES
}

/// `draft_angle_varies` for each face of a parting-line draft (feature
/// `id` of `part`, its report `rep`) whose angle lies more than
/// [`VARIES`] from the angle its file asks or spreads wider than it. A
/// neutral-plane draft holds its angle by construction (design 384), so a
/// spline's certified bounds, wider than a millidegree, aren't a varying
/// angle; nothing is said for it, nor for a report that isn't a draft's.
pub fn draft_warnings(part: &Id, id: &Id, rep: &Value) -> Vec<Value> {
    if rep["hinge"] != "parting" {
        return Vec::new();
    }
    let (Some(target), Some(angles)) = (rep["target"].as_str(), rep["angles"].as_array()) else {
        return Vec::new();
    };
    let deg = |x: f64| x / crate::units::DEG;
    angles
        .iter()
        .filter_map(|a| {
            if !varies(rep, a) {
                return None;
            }
            let (lo, hi) = (a["least"].as_f64()?, a["greatest"].as_f64()?);
            let asked = a["asked"].as_f64();
            let face = format!("{part}/{target}/{}", a["face"].as_str()?);
            let asked_text = asked
                .map(|t| format!(", not the {}° asked", crate::measure::number(deg(t))))
                .unwrap_or_default();
            let mut w = json!({
                "code": "draft_angle_varies",
                "part": part,
                "feature": id,
                "face": face,
                "least_rad": lo,
                "greatest_rad": hi,
                "message": format!(
                    "draft {id}: face {face} lies {}° to {}° from the pull{asked_text}: a parting line climbing along the pull holds the angle in the section square to it, not from the pull",
                    crate::measure::number(deg(lo)),
                    crate::measure::number(deg(hi)),
                ),
            });
            if let Some(t) = asked {
                w["asked_rad"] = json!(t);
            }
            Some(w)
        })
        .collect()
}

/// A read-only query's kernel refusal (`feature_faces.get`'s, `blends.get`'s):
/// `tweak_failed` with its reason, `kernel: tweak`, the kernel's text and
/// a hint, `key` naming what was asked about.
pub(crate) fn query_error(key: &str, at: &str, e: ryukei::Error) -> Error {
    let r = match &e {
        ryukei::Error::InvalidTweak(t) => refusal(t),
        _ => Refusal {
            reason: "kernel",
            case: None,
            message: "the kernel refused it",
            entities: vec![],
        },
    };
    let mut err = Error::new(Code::TweakFailed, format!("{at}: {} ({e})", r.message))
        .with(key, at)
        .with("reason", r.reason)
        .with("kernel", "tweak")
        .with("refusal", format!("{e:?}"));
    if let Some(h) = hint(&r) {
        err = err.with("hint", h);
    }
    err
}

/// `feature_faces.get`: the faces of the hole, boss or pocket `face` (one
/// face of a part's solid, `<part>/<solid>/<name>`) belongs to, in the
/// part as it shows at `rev`. A face of no such feature is `tweak_failed`
/// `no_feature`, a reference import's `reference_body`.
pub fn feature_faces_get(
    session: &mut crate::session::Session,
    face: &str,
    rev: Option<&str>,
) -> Result<Value> {
    let (commit, s) = session.at(rev)?;
    let part = crate::entity::EntityRef::parse(face)?.part;
    if crate::server::part_kind(s.project().part(&part)?) == crate::doc::files::PartKind::Assembly {
        return Err(Error::new(
            Code::InvalidParam,
            format!("{face}: feature-faces takes a part's face, not an assembly's"),
        )
        .with("face", face));
    }
    let found = crate::measure::locate(s, face)?;
    let [l] = found.as_slice() else {
        return Err(Error::new(
            Code::InvalidParam,
            format!(
                "{face}: feature-faces takes one face of a part's solid, <part>/<solid>/<name>"
            ),
        )
        .with("face", face));
    };
    let ryukei::measure::Entity::Face(f) = l.entity else {
        return Err(Error::new(
            Code::InvalidParam,
            format!(
                "{face} is {} {}, not a face",
                if l.kind == "edge" { "an" } else { "a" },
                l.kind
            ),
        )
        .with("face", face)
        .with("kind", l.kind));
    };
    if l.built.reference {
        return Err(Error::new(
            Code::ReferenceBody,
            format!("{face} is a face of a reference import, which nothing edits: convert it to a solid first"),
        )
        .with("face", face)
        .with("hint", format!("ryucad import convert {}/{}", l.part, l.solid)));
    }
    let seed = l.built.body.face(f).expect("a live face").name;
    let names = ryukei::tweak::feature_faces(&l.built.body, seed)
        .map_err(|e| query_error("face", face, e))?;
    let mut v = json!({
        "face": face,
        "solid": format!("{}/{}", l.part, l.solid),
        "faces": names.iter().map(|n| format!("{}/{}/{n}", l.part, l.solid)).collect::<Vec<_>>(),
    });
    if let Some(c) = commit {
        v["commit"] = json!(c);
    }
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hints_point_at_topology_and_blends() {
        use ryukei::TweakError as T;
        let n = Name::from_u128(7);
        let (body, _) = ryukei::primitive::block(
            ryukei::name::OpKey::from_bytes(b"b"),
            0.01,
            0.01,
            0.01,
            &ryukei::geom::Rigid::IDENTITY,
        )
        .unwrap();
        let hint_for = |r: &Refusal, changes: bool| hint_for(r, changes, &body);
        let h = |e: T, changes: bool| hint_for(&refusal(&e), changes).unwrap_or_default();
        assert!(h(T::EdgeVanishes { edge: n }, false).contains("topology: change"));
        assert!(h(T::VertexSplits { vertex: n }, false).contains("topology: change"));
        assert!(h(T::FaceCollapses { face: n }, false).contains("topology: change"));
        assert!(h(T::SelfIntersecting { faces: [n, n] }, false).contains("topology: change"));
        assert_eq!(
            hint_for(&refusal(&T::SelfIntersecting { faces: [n, n] }), true),
            None
        );
        // ryukei re-trims an Uncertain only at one of the body's vertices:
        // at a face (or a name the body lacks) the hint would be refused
        // again, so there is none.
        let vertex = body.vertices().next().unwrap();
        let vertex = body
            .name_of(ryukei::topo::EntityId::Vertex(vertex))
            .unwrap();
        let face = body.faces().next().unwrap();
        let face = body.name_of(ryukei::topo::EntityId::Face(face)).unwrap();
        assert!(h(T::Uncertain { at: vertex }, false).contains("topology: change"));
        assert_eq!(hint_for(&refusal(&T::Uncertain { at: vertex }), true), None);
        assert_eq!(hint_for(&refusal(&T::Uncertain { at: face }), false), None);
        assert_eq!(hint_for(&refusal(&T::Uncertain { at: n }), false), None);
        let lone = T::Unsupported {
            at: n,
            case: ryukei::TweakCase::LoneVertex,
        };
        assert!(h(lone, false).contains("topology: change"));
        assert!(h(T::TangencyBroken { edge: n }, false).contains("blends: rebuild"));
        assert!(h(T::TangencyBroken { edge: n }, false).contains("tangent true"));
        assert!(h(T::GapNotHealed { face: n }, true).contains("heal"));
        assert!(h(T::HealAmbiguous { face: n }, true).contains("smaller motion"));
        assert!(h(T::RegionTooSmall { face: n }, true).contains("smaller motion"));
    }

    #[test]
    fn a_budget_refusal_has_its_own_code() {
        let id = Id::parse("d", "feature").unwrap();
        let spent = ryukei::Error::BudgetExceeded {
            spent: 9,
            budget: 4,
        };
        let e = budget_error(&id, "delete_face", &spent).unwrap();
        assert_eq!(e.code, Code::EditBudgetExceeded);
        assert_eq!(e.details["spent"], 9);
        assert_eq!(e.details["budget"], 4);
        assert_eq!(e.details["feature"], "d");
        assert!(e.details["hint"].as_str().unwrap().contains("edit.budget"));
        assert!(budget_error(&id, "delete_face", &ryukei::Error::Cancelled).is_none());
    }

    #[test]
    fn an_inside_out_tweak_is_not_told_to_heal() {
        let delete = inside_out_hint("delete_face");
        assert!(delete.contains("heal: cap") && delete.contains("seed"));
        for ty in ["move_face", "replace_face", "draft"] {
            let h = inside_out_hint(ty);
            assert!(h.contains("smaller motion"), "{ty}: {h}");
            assert!(!h.contains("heal") && !h.contains("seed"), "{ty}: {h}");
        }
    }

    #[test]
    fn only_deletes_and_retrims_are_budgeted() {
        let read = |v: Value| {
            Feature::from_json(&v, crate::units::Unit::Mm, &crate::expr::Env::empty()).unwrap()
        };
        let face = json!({"feature": "b", "name": format!("{:032x}", 1)});
        let mv = |extra: Value| {
            let mut v = json!({"format": crate::doc::edit::FORMAT, "kind": "ryucad.feature",
                "type": "move_face", "target": "b", "by": "translate", "direction": "x",
                "distance": "1 mm", "faces": [face]});
            for (k, x) in extra.as_object().unwrap() {
                v[k] = x.clone();
            }
            read(v)
        };
        assert!(!budgeted(&mv(json!({}))));
        assert!(budgeted(&mv(json!({"topology": "change"}))));
        assert!(budgeted(&mv(json!({"blends": "rebuild"}))));
        assert!(budgeted(&read(json!({"format": crate::doc::edit::FORMAT,
            "kind": "ryucad.feature", "type": "delete_face", "target": "b", "faces": [face]}))));
    }
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
    fn the_protocol_sets_and_clears_topology_and_blends() {
        let (dir, mut s) = boxes();
        let b = names(&mut s, "b", "faces");
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "move_face", "id": "m",
            "direction": "z", "distance": "1 mm", "faces": [format!("p/b/{}", b[0])],
            "topology": "change", "blends": "rebuild"}),
        )
        .unwrap();
        let v = file(&dir, "m");
        assert_eq!(
            (v["topology"].as_str(), v["blends"].as_str()),
            (Some("change"), Some("rebuild"))
        );
        assert_eq!(v["format"], crate::doc::edit::FORMAT);
        // feature.show gives both always.
        let shown = call(&mut s, "feature.show", json!({"part": "p", "id": "m"})).unwrap();
        assert_eq!(shown["topology"], "change", "{shown}");
        // keep clears each; the file goes back to ryucad 17's format.
        call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "m", "topology": "keep",
            "blends": "keep"}),
        )
        .unwrap();
        let v = file(&dir, "m");
        assert!(
            v.get("topology").is_none() && v.get("blends").is_none(),
            "{v}"
        );
        assert_eq!(v["format"], crate::doc::tweak::FORMAT);
        let shown = call(&mut s, "feature.show", json!({"part": "p", "id": "m"})).unwrap();
        assert_eq!(
            (shown["topology"].as_str(), shown["blends"].as_str()),
            (Some("keep"), Some("keep")),
            "{shown}"
        );
        // Anything else is refused at its param.
        let e = call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "m", "topology": "grow"}),
        )
        .unwrap_err();
        assert_eq!(
            (e.code, e.details["param"].as_str()),
            (Code::InvalidParam, Some("topology"))
        );
        // A primitive takes neither.
        let e = call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "c", "topology": "change"}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::InvalidParam);
    }

    #[test]
    fn the_protocol_adds_and_edits_a_delete() {
        let (dir, mut s) = boxes();
        let b = names(&mut s, "b", "faces");
        // Seeds as the CLI sends --seed (features); no target, two solids:
        // the seed names it.
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "delete_face", "id": "d",
            "features": [format!("p/b/{}", b[0])], "heal": "grow"}),
        )
        .unwrap();
        let v = file(&dir, "d");
        assert_eq!(
            (v["target"].as_str(), v["heal"].as_str()),
            (Some("b"), Some("grow")),
            "{v}"
        );
        assert_eq!(v["seeds"][0]["name"], json!(b[0]), "{v}");
        assert_eq!(v["format"], crate::doc::edit::FORMAT);
        // A face as `face`; heal auto isn't written; seeds cleared.
        call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "d",
            "face": format!("p/b/{}", b[1]), "heal": "auto", "seeds": []}),
        )
        .unwrap();
        let v = file(&dir, "d");
        assert!(v.get("heal").is_none() && v.get("seeds").is_none(), "{v}");
        assert_eq!(v["faces"][0]["name"], json!(b[1]), "{v}");
        // Neither faces nor seeds; a field it doesn't take; a bad heal.
        let e = call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "d", "faces": []}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::InvalidParam, "{e}");
        let e = call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "d", "tangent": false}),
        )
        .unwrap_err();
        assert_eq!(
            (e.code, e.details["param"].as_str()),
            (Code::InvalidParam, Some("tangent"))
        );
        let e = call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "d", "heal": "patch"}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::InvalidParam, "{e}");
        // A face of another solid.
        let c = names(&mut s, "c", "faces");
        let e = call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "d",
            "faces": [format!("p/c/{}", c[0])]}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::TargetsMixed, "{e}");
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
    fn a_moves_line_is_its_axis_or_its_direction_the_way_faces_go() {
        let (_dir, mut s) = boxes();
        let top = up(&mut s, "b", "b", 0.02);
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "move_face", "id": "m",
            "by": "translate", "direction": "z", "distance": "-2 mm", "faces": [top.clone()]}),
        )
        .unwrap();
        let line = |s: &mut crate::session::Session| {
            let r = call(s, "rebuild", json!({"part": "p"})).unwrap();
            r["parts"][0]["features"]
                .as_array()
                .unwrap()
                .iter()
                .find(|x| x["id"] == "m")
                .unwrap()["move_face"]["line"]
                .clone()
        };
        // Down 2 mm along z: the line runs down through z's origin.
        assert_eq!(
            line(&mut s),
            json!({"origin_m": [0.0, 0.0, 0.0], "direction": [0.0, 0.0, -1.0]})
        );
        // A turn about x: the axis as it lies.
        call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "m", "by": "rotate", "axis": "x", "angle": "1 deg"}),
        )
        .unwrap();
        assert_eq!(
            line(&mut s),
            json!({"origin_m": [0.0, 0.0, 0.0], "direction": [1.0, 0.0, 0.0]})
        );
        // An offset moves each face its own way: no line.
        call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "m", "by": "offset", "distance": "1 mm"}),
        )
        .unwrap();
        assert!(line(&mut s).is_null());
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

    #[test]
    fn draft_angles_are_the_kernels() {
        assert_eq!(
            crate::doc::tweak::MIN_DRAFT_ANGLE,
            ryukei::tol::ANGULAR_RESOLUTION
        );
        assert_eq!(crate::doc::tweak::MAX_DRAFT_ANGLE, ryukei::tweak::MAX_ANGLE);
    }

    /// The reference to the planar face of `f`'s body of `solid` whose
    /// outward normal is `n` along `axis`.
    fn facing(
        s: &mut crate::session::Session,
        f: &str,
        solid: &str,
        axis: usize,
        n: f64,
    ) -> String {
        let v = call(s, "inspect", json!({"part": "p", "feature": f})).unwrap();
        let face = v["bodies"][0]["faces"]
            .as_array()
            .unwrap()
            .iter()
            .find(|x| {
                let sign = if x["reversed"] == true { -1.0 } else { 1.0 };
                x["surface"]["type"] == "plane"
                    && x["surface"]["frame"]["z"][axis].as_f64().unwrap() * sign == n
            })
            .unwrap()["name"]
            .as_str()
            .unwrap()
            .to_owned();
        format!("p/{solid}/{face}")
    }

    /// `{feature, name}` of a reference `p/<solid>/<name>`.
    fn wire(r: &str) -> Value {
        let mut it = r.split('/').skip(1);
        json!({"feature": it.next().unwrap(), "name": it.next().unwrap()})
    }

    fn draft_feature(v: Value) -> Feature {
        let mut v = v;
        v["format"] = json!(crate::doc::tweak::DRAFT_FORMAT);
        v["kind"] = json!("ryucad.feature");
        v["type"] = json!("draft");
        Feature::from_json(&v, crate::units::Unit::Mm, &crate::expr::Env::empty()).unwrap()
    }

    /// A draft's spec, whether its hinge is a parting line, and each
    /// listed face's asked angle.
    type Drafted = (DraftSpec, bool, Vec<(Name, f64)>);

    /// Part `p` of `s` with draft `d` written last, and what it resolves
    /// to: the kernel's spec, whether its hinge is a parting line, and each
    /// listed face's asked angle.
    fn resolved_draft(s: &mut crate::session::Session, v: Value) -> Result<Drafted> {
        let pid = Id::parse("p", "part").unwrap();
        let part = s.project().part(&pid).unwrap().clone();
        let d = Id::parse("d", "feature").unwrap();
        let f = draft_feature(v);
        let mut order = part.order().to_vec();
        order.push(d.clone());
        let p = crate::server::proposed(&part, &order, &[(&d, &f)]);
        let results = s.rebuilt(&pid).unwrap();
        match resolve(&p, &Solids::of(&p), &d, &f, results)?.spec {
            Spec::Draft {
                spec,
                parting,
                asked,
            } => Ok((spec, parting, asked)),
            _ => panic!("a draft's spec"),
        }
    }

    #[test]
    fn a_draft_resolves_its_hinge_pull_faces_and_flips() {
        let (_dir, mut s) = boxes();
        let (east, north) = (
            facing(&mut s, "b", "b", 0, 1.0),
            facing(&mut s, "b", "b", 1, 1.0),
        );
        // About xy, the pull turned round; one wall at the draft's angle,
        // one in a group of its own, flipped.
        let (spec, parting, asked) = resolved_draft(
            &mut s,
            json!({"target": "b", "angle": "2 deg", "neutral": "xy", "reverse": true,
                   "faces": [wire(&east)],
                   "groups": [{"angle": "5 deg", "faces": [wire(&north)]}],
                   "flip": [wire(&north)]}),
        )
        .unwrap();
        assert_eq!(spec.pull.get(), ryukei::geom::Vector3::new(0.0, 0.0, -1.0));
        assert_eq!(
            spec.hinge,
            ryukei::tweak::Hinge::Neutral {
                origin: ryukei::geom::Point3::ORIGIN
            }
        );
        assert!(!parting && spec.tangent);
        assert!((spec.angle - 2f64.to_radians()).abs() < 1e-15);
        assert_eq!((spec.faces[0].angle, spec.faces[0].flip), (None, false));
        assert!((spec.faces[1].angle.unwrap() - 5f64.to_radians()).abs() < 1e-15);
        assert!(spec.faces[1].flip);
        assert!((asked[0].1 - 2f64.to_radians()).abs() < 1e-15);
        assert!((asked[1].1 - 5f64.to_radians()).abs() < 1e-15);
        // From a parting line: the bottom edge of the east wall, pull z.
        let edge = names(&mut s, "b", "edges")[0].clone();
        let (spec, parting, _) = resolved_draft(
            &mut s,
            json!({"target": "b", "angle": "2 deg",
                   "parting": [{"feature": "b", "name": edge}], "pull": "z",
                   "faces": [wire(&east)]}),
        )
        .unwrap();
        assert!(parting);
        assert!(matches!(&spec.hinge, ryukei::tweak::Hinge::PartingLine(e) if e.len() == 1));
        assert_eq!(spec.pull.get(), ryukei::geom::Vector3::new(0.0, 0.0, 1.0));
        // A face where a parting edge goes; another solid's face; a whole
        // solid as the neutral plane.
        let other = facing(&mut s, "c", "c", 2, 1.0);
        for (v, code) in [
            (
                json!({"target": "b", "angle": "2 deg", "parting": [wire(&north)], "pull": "z",
                    "faces": [wire(&east)]}),
                Code::ReferenceWrongKind,
            ),
            (
                json!({"target": "b", "angle": "2 deg", "neutral": "xy",
                    "faces": [wire(&other)]}),
                Code::TargetsMixed,
            ),
            (
                json!({"target": "b", "angle": "2 deg", "neutral": {"feature": "c"},
                    "faces": [wire(&east)]}),
                Code::ReferenceWrongKind,
            ),
        ] {
            let e = resolved_draft(&mut s, v.clone())
                .err()
                .unwrap_or_else(|| panic!("{v}"));
            assert_eq!(e.code, code, "{v}: {e}");
        }
    }

    /// Part `p` of `s` with draft `d` added last, rebuilt: `d`'s result.
    fn built_draft(s: &mut crate::session::Session, v: Value) -> Result<crate::rebuild::Output> {
        let pid = Id::parse("p", "part").unwrap();
        let part = s.project().part(&pid).unwrap().clone();
        let d = Id::parse("d", "feature").unwrap();
        let f = draft_feature(v);
        let mut order = part.order().to_vec();
        order.push(d.clone());
        let p = crate::server::proposed(&part, &order, &[(&d, &f)]);
        let out = s.rebuilt_as(p).unwrap();
        out.features
            .iter()
            .find(|(i, _)| *i == d)
            .unwrap()
            .1
            .clone()
    }

    #[test]
    fn a_draft_builds_and_keeps_the_kernels_report() {
        let (_dir, mut s) = boxes();
        let walls: Vec<Value> = [(0, 1.0), (0, -1.0), (1, 1.0), (1, -1.0)]
            .iter()
            .map(|&(a, n)| wire(&facing(&mut s, "b", "b", a, n)))
            .collect();
        let out = built_draft(
            &mut s,
            json!({"target": "b", "angle": "2 deg", "neutral": "xy", "faces": walls}),
        )
        .unwrap();
        let crate::rebuild::Output::Solids(m) = &out else {
            panic!("{out:?}")
        };
        let rep = m.report.as_ref().expect("a draft's report");
        assert_eq!(
            (rep["target"].as_str(), rep["hinge"].as_str()),
            (Some("b"), Some("neutral"))
        );
        let angles = rep["angles"].as_array().unwrap();
        assert_eq!(angles.len(), 4);
        for a in angles {
            // Planes: the closed form, the same bits both ways.
            assert_eq!(a["least"], a["greatest"], "{a}");
            assert!(
                (a["least"].as_f64().unwrap() - 2f64.to_radians()).abs() < 1e-12,
                "{a}"
            );
            assert!(
                (a["asked"].as_f64().unwrap() - 2f64.to_radians()).abs() < 1e-15,
                "{a}"
            );
        }
        assert!(rep.get("mode").is_none());
        // The top doesn't cross xy: no_hinge, the face named.
        let top = wire(&facing(&mut s, "b", "b", 2, 1.0));
        let e = built_draft(
            &mut s,
            json!({"target": "b", "angle": "2 deg", "neutral": "xy", "faces": [top]}),
        )
        .unwrap_err();
        assert_eq!(
            (
                e.code,
                e.details["reason"].as_str(),
                e.details["kernel"].as_str()
            ),
            (Code::TweakFailed, Some("no_hinge"), Some("tweak"))
        );
        assert_eq!(e.details["entities"][0]["kind"], "face", "{e}");
    }

    #[test]
    fn the_protocol_adds_and_edits_a_draft() {
        let (dir, mut s) = boxes();
        let (east, north) = (
            facing(&mut s, "b", "b", 0, 1.0),
            facing(&mut s, "b", "b", 1, 1.0),
        );
        // No target and two solids: its faces name it.
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "draft", "id": "d", "angle": "2 deg", "neutral": "xy",
                   "faces": [east.clone(), {"angle": "5 deg", "faces": [north.clone()]}]}),
        )
        .unwrap();
        let v = file(&dir, "d");
        assert_eq!(v["format"], crate::doc::tweak::DRAFT_FORMAT);
        assert_eq!(v["target"], "b", "{v}");
        assert_eq!(v["neutral"], "xy", "{v}");
        assert_eq!(v["groups"][0]["angle"], "5 deg", "{v}");
        assert!(
            v.get("tangent").is_none() && v.get("reverse").is_none(),
            "{v}"
        );
        // A parting line replaces the neutral plane; a flipped face; the
        // pull reversed; tangency off.
        let edge = format!("p/b/{}", names(&mut s, "b", "edges")[0]);
        call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "d", "parting": [edge], "pull": "z", "reverse": true,
                   "flip": [north.clone()], "tangent": false}),
        )
        .unwrap();
        let v = file(&dir, "d");
        assert!(v.get("neutral").is_none(), "{v}");
        assert_eq!(
            (v["pull"].as_str(), v["reverse"].as_bool()),
            (Some("z"), Some(true))
        );
        assert_eq!(v["flip"][0]["name"], wire(&north)["name"], "{v}");
        assert_eq!(v["tangent"], false);
        // Back to a neutral plane: the parting line and its pull go.
        call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "d", "neutral": "xy", "flip": [], "reverse": false}),
        )
        .unwrap();
        let v = file(&dir, "d");
        assert!(v.get("parting").is_none() && v.get("pull").is_none(), "{v}");
        assert!(v.get("flip").is_none() && v.get("reverse").is_none(), "{v}");
        // A bool is no draft's flip; a field a draft doesn't take.
        for (extra, param) in [
            (json!({"flip": true}), "flip"),
            (json!({"distance": "1 mm"}), "distance"),
        ] {
            let mut p = json!({"part": "p", "id": "d"});
            for (k, x) in extra.as_object().unwrap() {
                p[k] = x.clone();
            }
            let e = call(&mut s, "feature.edit", p).unwrap_err();
            assert_eq!(
                (e.code, e.details["param"].as_str()),
                (Code::InvalidParam, Some(param)),
                "{e}"
            );
        }
        // The angle is checked when written.
        let e = call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "d", "angle": "89.9 deg"}),
        )
        .unwrap_err();
        assert_eq!(
            (e.code, e.details["pointer"].as_str()),
            (Code::InvalidParam, Some("/angle"))
        );
    }

    #[test]
    fn a_draft_reports_its_fields_pull_and_angles() {
        let (_dir, mut s) = boxes();
        let east = facing(&mut s, "b", "b", 0, 1.0);
        let shown = call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "draft", "id": "d", "target": "b", "angle": "2 deg",
                   "neutral": "xy", "reverse": true, "faces": [east.clone()]}),
        )
        .unwrap();
        let f = &shown["feature"];
        assert_eq!(f["angle"]["value"], "2 deg", "{f}");
        assert!((f["angle"]["rad"].as_f64().unwrap() - 2f64.to_radians()).abs() < 1e-15);
        assert_eq!(f["faces"], json!([east]), "{f}");
        assert_eq!(
            (
                f["neutral"].as_str(),
                f["reverse"].as_bool(),
                f["tangent"].as_bool()
            ),
            (Some("xy"), Some(true), Some(true))
        );
        let r = call(&mut s, "rebuild", json!({"part": "p"})).unwrap();
        let d = r["parts"][0]["features"]
            .as_array()
            .unwrap()
            .iter()
            .find(|x| x["id"] == "d")
            .unwrap()["draft"]
            .clone();
        // Reversed: the pull is -z, and xy's point is the neutral plane's.
        assert_eq!(d["pull"], json!([0.0, 0.0, -1.0]), "{d}");
        assert_eq!(
            d["neutral"],
            json!({"origin_m": [0.0, 0.0, 0.0], "normal": [0.0, 0.0, -1.0]})
        );
        // Pulled down, the east wall drafted about its bottom leans out
        // going up: its angle from the pull is the one asked, exactly.
        let a = &d["angles"][0];
        assert_eq!(a["face"], east, "{d}");
        assert_eq!(a["exact"], true, "{d}");
        assert_eq!(a["least"]["value"], "2 deg", "{d}");
        assert_eq!(a["least"], a["greatest"]);
        // About a neutral plane the angle never varies.
        assert_eq!(a["varies"], false, "{d}");
        assert_eq!(d["modified"]["faces"], 1, "{d}");
        assert!(
            r["parts"][0]["features"]
                .as_array()
                .unwrap()
                .iter()
                .all(|x| x.get("import").is_none()),
            "{r}"
        );
        // inspect --feature gives the same, with what it modified by name.
        let i = call(&mut s, "inspect", json!({"part": "p", "feature": "d"})).unwrap();
        assert_eq!(i["draft"]["angles"], d["angles"]);
        assert!(
            i["draft"]["modified"]["references"]
                .as_array()
                .unwrap()
                .contains(&json!(east))
        );
    }

    #[test]
    fn a_parting_drafts_angle_off_what_it_asks_warns() {
        let (part, id) = (
            Id::parse("p", "part").unwrap(),
            Id::parse("d", "feature").unwrap(),
        );
        let t = 3f64.to_radians();
        let n = "0123456789abcdef0123456789abcdef";
        let rep = |hinge: &str, lo: f64, hi: f64| {
            json!({"target": "b", "hinge": hinge,
                   "angles": [{"face": n, "least": lo, "greatest": hi, "asked": t}]})
        };
        // At the angle asked, or within a millidegree: nothing.
        assert!(draft_warnings(&part, &id, &rep("parting", t, t)).is_empty());
        // Each angle the output gives says whether it varies, as the
        // warning does.
        let out = |r: &Value| angles(&part, r).unwrap()[0]["varies"].clone();
        assert_eq!(out(&rep("parting", t, t)), false);
        assert_eq!(out(&rep("parting", 2.5f64.to_radians(), t)), true);
        assert_eq!(out(&rep("neutral", 2.5f64.to_radians(), t)), false);
        let close = t + 0.5e-3f64.to_radians();
        assert!(draft_warnings(&part, &id, &rep("parting", close, close)).is_empty());
        // A parting line climbing along the pull: the face is at 2.5°.
        let w = draft_warnings(
            &part,
            &id,
            &rep("parting", 2.5f64.to_radians(), 2.5f64.to_radians()),
        );
        assert_eq!(w.len(), 1);
        assert_eq!(w[0]["code"], "draft_angle_varies");
        assert_eq!(w[0]["face"], format!("p/b/{n}"));
        assert_eq!(w[0]["feature"], "d");
        assert!(
            w[0]["message"].as_str().unwrap().contains("parting line"),
            "{}",
            w[0]
        );
        // A spread wider than a millidegree, the asked angle inside it.
        let w = draft_warnings(&part, &id, &rep("parting", t - 0.01, t + 0.01));
        assert_eq!(w.len(), 1);
        // A neutral plane holds the angle by construction: a spline's
        // certified bounds wider than that say nothing.
        assert!(draft_warnings(&part, &id, &rep("neutral", t - 0.01, t + 0.01)).is_empty());
        // Any other report (an import's, a fill's) says nothing here.
        assert!(draft_warnings(&part, &id, &json!({"sides": []})).is_empty());
    }

    #[test]
    fn blend_edits_are_made_from_what_was_given() {
        let part = Id::parse("p", "part").unwrap();
        let env = crate::expr::Env::empty();
        let n = |k: u128| format!("p/b/{k:032x}");
        let given = |v: serde_json::Value| -> FeatureFields { serde_json::from_value(v).unwrap() };
        // A blend delete by size, then by face too.
        let mut f = given(json!({"target": "b", "below": "2 mm"}));
        take_given("delete_blend", &mut f).unwrap();
        let d = from_given(
            "delete_blend",
            None,
            &f,
            &part,
            crate::units::Unit::Mm,
            &env,
        )
        .unwrap();
        assert_eq!(d.to_json()["below"], "2 mm");
        let mut f = given(json!({"faces": [n(1)], "tolerance": "0.001 mm"}));
        take_given("delete_blend", &mut f).unwrap();
        let d = from_given(
            "delete_blend",
            Some(&d),
            &f,
            &part,
            crate::units::Unit::Mm,
            &env,
        )
        .unwrap();
        assert_eq!(d.to_json()["faces"][0]["name"], format!("{:032x}", 1));
        assert_eq!(d.to_json()["tolerance"], "0.001 mm");
        // Neither left: refused at faces.
        let mut f = given(json!({"target": "b"}));
        take_given("delete_blend", &mut f).unwrap();
        let e = from_given(
            "delete_blend",
            None,
            &f,
            &part,
            crate::units::Unit::Mm,
            &env,
        )
        .unwrap_err();
        assert_eq!(e.details["pointer"], "/faces");
        // A resize's one group from the flags: a radius and two faces.
        let mut f = given(json!({"target": "b", "radius": "3 mm", "faces": [n(1), n(2)]}));
        take_given("resize_blend", &mut f).unwrap();
        let r = from_given(
            "resize_blend",
            None,
            &f,
            &part,
            crate::units::Unit::Mm,
            &env,
        )
        .unwrap();
        let v = r.to_json();
        assert_eq!(v["groups"][0]["radius"], "3 mm");
        assert_eq!(v["groups"][0]["faces"].as_array().unwrap().len(), 2);
        // Made a chamfer by distances: the radius goes, the faces stay.
        let f =
            given(json!({"form": "distances", "d1": "1 mm", "d2": "2 mm", "chamfer_face": n(9)}));
        let r = from_given(
            "resize_blend",
            Some(&r),
            &f,
            &part,
            crate::units::Unit::Mm,
            &env,
        )
        .unwrap();
        let g = &r.to_json()["groups"][0];
        assert!(g.get("radius").is_none(), "{g}");
        assert_eq!(
            (g["form"].as_str(), g["d2"].as_str()),
            (Some("distances"), Some("2 mm"))
        );
        assert_eq!(g["face"]["name"], format!("{:032x}", 9));
        assert_eq!(g["faces"].as_array().unwrap().len(), 2);
        // Groups whole, with the one group's flags too: refused.
        let mut f = given(json!({"radius": "3 mm",
                                 "groups": [{"radius": "1 mm", "faces": [n(1)]}]}));
        take_given("resize_blend", &mut f).unwrap();
        let e = from_given(
            "resize_blend",
            Some(&r),
            &f,
            &part,
            crate::units::Unit::Mm,
            &env,
        )
        .unwrap_err();
        assert_eq!(e.code, Code::Usage);
        assert_eq!(
            request_fields("resize_blend"),
            crate::doc::edit::RESIZE_REQUEST
        );
        assert!(budgeted(&r));
    }

    #[test]
    fn blend_refusals_point_at_the_query_and_the_way_round() {
        let n =
            ryukei::name::Name::derive(ryukei::name::OpKey(1), ryukei::name::Role::NewFace, &[]);
        let hint_of = |e: ryukei::TweakError| hint(&refusal(&e)).unwrap_or_default();
        assert!(hint_of(ryukei::TweakError::NotABlend { face: n }).contains("ryucad blends"));
        let lost = hint_of(ryukei::TweakError::BlendLost {
            face: n,
            reason: ryukei::BlendLoss::Overflow,
        });
        assert!(lost.contains("delete_blend"), "{lost}");
        assert!(hint_of(ryukei::TweakError::BadTolerance).contains("0.1 mm"));
    }
}
