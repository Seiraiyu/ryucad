//! The global offset (design 299): where the local build refuses, the raw
//! envelope at the offset distance, every face of it split against every
//! other it crosses, and the pieces at the distance from the original
//! kept.
//!
//! - **The envelope** is the local build's plan in [`Mode::Envelope`]:
//!   each face's offset (inverted where it passes its centre), a fill
//!   about every edge between faces at the distance that isn't smooth
//!   (inverted where their offsets close), spheres at vertices, and the
//!   local build's closing curves between a face at the distance and one
//!   at zero (a shell's open face). It is built through the layout with
//!   no check: its faces cross.
//! - **Self-intersection** is the self-union's pipeline
//!   ([`crate::repair::resolve`]) over the pairs
//!   [`crate::check::check_intersections`] finds crossing or overlapping.
//! - **Selection by distance**: every point of the envelope lies within
//!   `|d|` of the original's faces at the distance; the offset's boundary
//!   is where it lies exactly `|d|` from them. A group of pieces (joined
//!   across edges no crossing touches) is dropped where a point inside
//!   one of them has a point of such a face, or of one of its edges,
//!   nearer than `|d| - 2 reach`, and kept where every such face is
//!   certified at least `|d| - reach` away ([`face_off_ball`]) and the
//!   point lies on the offset's side of the original.
//!
//! One nonzero distance only: an offset whose faces move by different
//! nonzero distances isn't global (`None`).

use super::OffsetSpec;
use super::classify::{Mode, Plan};
use crate::boolean::build::Kept;
use crate::boolean::classify::inner_point;
use crate::boolean::table::Table;
use crate::boolean::{Fail, past_cap};
use crate::check::{Fault, check_intersections_in};
use crate::geom::{Point2, Point3};
use crate::intersect::{
    Inclusion, Place, face_box, face_off_ball, point_in_body_in, point_in_face,
};
use crate::name::{Journal, Name, OpKey};
use crate::split::Piece;
use crate::tol::LINEAR_RESOLUTION;
use crate::topo::{Body, FaceId};
use crate::work::Meter;
use crate::{Error, OffsetError};
use std::collections::{BTreeMap, BTreeSet};

/// The least reach of distance selection: a piece within it of `|d|` is
/// kept, one more than twice it nearer is dropped.
const REACH: f64 = 100.0 * LINEAR_RESOLUTION;

/// How many inner points of a group are tried before it is uncertain.
const ATTEMPTS: usize = 4;

/// `body` offset by `spec` globally, or `None` where the global offset
/// doesn't apply (faces at different nonzero distances).
pub(crate) fn global_in(
    key: OpKey,
    body: &Body,
    spec: &OffsetSpec,
    m: &Meter,
) -> Result<Option<(Body, Journal)>, Error> {
    let plan = Plan::with_mode(body, spec, Mode::Envelope)?;
    let mut ds: Vec<f64> = plan
        .distance
        .values()
        .copied()
        .filter(|&d| d != 0.0)
        .collect();
    ds.sort_by(f64::total_cmp);
    ds.dedup();
    let [d] = ds[..] else {
        return Ok(None);
    };
    let geo = super::edges::curves(&plan, m)?;
    let vgeo = super::vertices::vertices(&plan, &geo, m)?;
    let dbg = std::env::var_os("RYUKEI_14D").is_some();
    let (envelope, built) = super::build::build(key, &plan, &geo, &vgeo, m).inspect_err(|e| {
        if dbg {
            eprintln!("14d: envelope build: {e:?}")
        }
    })?;
    if dbg {
        eprintln!(
            "14d: envelope {:?} check {:?}",
            envelope.counts(),
            crate::check::check(&envelope)
        );
    }
    let mut pairs: BTreeSet<(FaceId, FaceId)> = BTreeSet::new();
    for fault in check_intersections_in(&envelope, m) {
        match fault {
            Fault::FacesCross { a, b } | Fault::FacesOverlap { a, b } => {
                pairs.insert((a.min(b), a.max(b)));
            }
            Fault::CannotCheck { a, b } => {
                if dbg {
                    eprintln!(
                        "14d: cannot check {:?} {:?}",
                        envelope.surfaces[envelope.faces[a].surface],
                        envelope.surfaces[envelope.faces[b].surface]
                    );
                }
                return Err(OffsetError::Uncertain {
                    at: envelope.faces[a].name,
                }
                .into());
            }
            _ => {}
        }
    }
    let moving: Vec<FaceId> = body.faces().filter(|&f| plan.distance[&f] != 0.0).collect();
    let still: BTreeSet<Name> = body
        .faces()
        .filter(|&f| plan.distance[&f] == 0.0)
        .map(|f| body.faces[f].name)
        .collect();
    // Faces of the envelope that are inverted: inverted offsets, fills
    // about edges whose offsets close, and spheres at vertices where only
    // such fills meet. Every point of them is nearer the original than
    // `|d|`: their pieces are dropped untested, and join no group.
    let mut inverted: BTreeSet<Name> = BTreeSet::new();
    for &f in plan.inversions.keys() {
        inverted.insert(body.faces[f].name);
    }
    for &e in &plan.inverted {
        inverted.insert(Name::derive(
            key,
            crate::name::Role::Offset(crate::name::OffsetRole::Edge),
            &[body.edges[e].name],
        ));
    }
    for (&v, g) in &vgeo {
        if g.sphere.is_some()
            && body
                .vertex_edges(v)
                .into_iter()
                .filter(|e| plan.class[e].0 == super::classify::Class::Opening)
                .all(|e| plan.inverted.contains(&e))
        {
            inverted.insert(Name::derive(
                key,
                crate::name::Role::Offset(crate::name::OffsetRole::Vertex),
                &[body.vertices[v].name],
            ));
        }
    }
    let alone = |f: FaceId| inverted.contains(&envelope.faces[f].name);
    let select = |ops: &[Body], table: &Table, pieces: &[Piece], groups: &[usize], m: &Meter| {
        by_distance(
            body,
            &moving,
            &still,
            d,
            &alone,
            (ops, table, pieces, groups),
            m,
        )
    };
    let fail = |f: Fail| -> Error {
        match f {
            Fail::Inconsistent => Error::Topology(crate::TopologyError::Inconsistent),
            Fail::Uncertain(a, _) | Fail::Unsupported(a, _) => OffsetError::Uncertain {
                at: envelope.faces[a.face].name,
            }
            .into(),
            Fail::NonManifold { faces, .. } => OffsetError::Uncertain {
                at: faces
                    .first()
                    .map_or(Name::from_u128(0), |f| envelope.faces[f.face].name),
            }
            .into(),
        }
    };
    if dbg {
        eprintln!("14d: {} pairs", pairs.len());
    }
    let Some((out, journal)) = crate::repair::resolve(key, &envelope, &pairs, &select, &alone, m)
        .inspect_err(|f| {
            if dbg {
                eprintln!("14d: resolve {f:?}");
                if let Fail::NonManifold { faces, .. } = f {
                    for x in faces {
                        let fc = &envelope.faces[x.face];
                        eprintln!(
                            "  face {:?} {:?} {:?}",
                            x.face, fc.sense, envelope.surfaces[fc.surface]
                        );
                    }
                }
                for f in envelope.faces() {
                    let fc = &envelope.faces[f];
                    eprintln!(
                        "  env {:?} {:?} {:?} box {:?}",
                        f,
                        fc.sense,
                        envelope.surfaces[fc.surface],
                        crate::intersect::face_box(&envelope, f)
                    );
                }
            }
        })
        .map_err(fail)?
    else {
        return Err(OffsetError::Empty.into());
    };
    if let Some((at, needed)) = past_cap(&out) {
        return Err(OffsetError::ToleranceTooLarge { at, needed }.into());
    }
    let mut all = built;
    all.append(journal);
    Ok(Some((out, all)))
}

/// What a group of pieces is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Verdict {
    Keep,
    Drop,
}

/// The pieces whose groups lie at `|d|` from the `moving` faces of
/// `original`, on the offset's side of it (`still` names the faces at
/// zero, which lie on it).
fn by_distance(
    original: &Body,
    moving: &[FaceId],
    still: &BTreeSet<Name>,
    d: f64,
    alone: &(dyn Fn(FaceId) -> bool + Sync),
    (ops, table, pieces, groups): (&[Body], &Table, &[Piece], &[usize]),
    m: &Meter,
) -> Result<Vec<Kept>, Fail> {
    let envelope = &ops[0];
    let mut members: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (i, &g) in groups.iter().enumerate() {
        members.entry(g).or_default().push(i);
    }
    let lists: Vec<Vec<usize>> = members.into_values().collect();
    let boxes: Vec<(FaceId, [Point3; 2])> = moving
        .iter()
        .map(|&f| {
            let b = face_box(original, f);
            (f, [b.min(), b.max()])
        })
        .collect();
    let verdicts: Vec<Vec<(usize, Verdict)>> = m
        .par_map(&lists, |m, list| {
            if alone(pieces[list[0]].face.face) {
                return Ok(list.iter().map(|&i| (i, Verdict::Drop)).collect());
            }
            let v = group_verdict(original, &boxes, still, d, (ops, table, pieces), list, m)?;
            if std::env::var_os("RYUKEI_14D").is_some() {
                let fs: Vec<_> = list.iter().map(|&i| pieces[i].face.face).collect();
                eprintln!("14d: group {fs:?} {v:?}");
            }
            Ok(list.iter().map(|&i| (i, v)).collect())
        })
        // The first failure in order, not the first a thread found.
        .into_iter()
        .collect::<Result<_, Fail>>()?;
    let _ = envelope;
    let mut kept = Vec::new();
    for (i, v) in verdicts.into_iter().flatten() {
        if v == Verdict::Keep {
            kept.push((i, pieces[i].clone()));
        }
    }
    kept.sort_by_key(|x| x.0);
    Ok(kept
        .into_iter()
        .map(|(_, piece)| Kept {
            piece,
            reversed: false,
        })
        .collect())
}

/// The distance from `q` to a box, `[min, max]`.
fn box_distance(q: Point3, [lo, hi]: [Point3; 2]) -> f64 {
    let gap = |x: f64, a: f64, b: f64| (a - x).max(0.0).max(x - b);
    let (x, y, z) = (
        gap(q.x, lo.x, hi.x),
        gap(q.y, lo.y, hi.y),
        gap(q.z, lo.z, hi.z),
    );
    (x * x + y * y + z * z).sqrt()
}

fn group_verdict(
    original: &Body,
    boxes: &[(FaceId, [Point3; 2])],
    still: &BTreeSet<Name>,
    d: f64,
    (ops, table, pieces): (&[Body], &Table, &[Piece]),
    list: &[usize],
    m: &Meter,
) -> Result<Verdict, Fail> {
    let envelope = &ops[0];
    let r = d.abs();
    for &i in list {
        let p = &pieces[i];
        let face = &envelope.faces[p.face.face];
        let reach = REACH.max(2.0 * crate::intersect::face_tolerance(envelope, p.face.face));
        let on_original = still.contains(&face.name);
        for attempt in 0..ATTEMPTS {
            let Some((q, _)) = inner_point(ops, table, p, attempt) else {
                continue;
            };
            m.charge(1).map_err(|_| Fail::Uncertain(p.face, p.face))?;
            let near: Vec<FaceId> = boxes
                .iter()
                .filter(|(_, b)| box_distance(q, *b) < r - reach)
                .map(|x| x.0)
                .collect();
            if std::env::var_os("RYUKEI_14D").is_some() {
                eprintln!("14d:   piece {i} face {:?} q {q:?}", p.face.face);
            }
            if nearer(original, &near, q, r - 2.0 * reach) {
                return Ok(Verdict::Drop);
            }
            let mut certified = true;
            for &f in &near {
                if face_off_ball(original, f, q, r - reach, m) != Ok(true) {
                    certified = false;
                    break;
                }
            }
            if !certified {
                continue;
            }
            if on_original {
                return Ok(Verdict::Keep);
            }
            let want = if d < 0.0 {
                Inclusion::Inside
            } else {
                Inclusion::Outside
            };
            match point_in_body_in(original, q, m) {
                Ok(x) if x == want => return Ok(Verdict::Keep),
                Ok(Inclusion::Boundary) | Err(_) => continue,
                Ok(_) => return Ok(Verdict::Drop),
            }
        }
    }
    let f = pieces[list[0]].face;
    Err(Fail::Uncertain(f, f))
}

/// Whether a point of one of `faces` (its surface's nearest point, inside
/// it) or of one of their edges lies nearer `q` than `limit`.
fn nearer(body: &Body, faces: &[FaceId], q: Point3, limit: f64) -> bool {
    if limit <= 0.0 {
        return false;
    }
    for &f in faces {
        let s = &body.surfaces[body.faces[f].surface];
        let foot = s.project(q);
        if foot.point.distance(q) < limit
            && matches!(
                point_in_face(body, f, Point2::new(foot.param.0, foot.param.1)),
                Ok(Place::Inside)
            )
        {
            return true;
        }
        for l in body.face_loops(f) {
            for fin in body.loop_fins(l) {
                let e = &body.edges[body.fins[fin].edge];
                let c = &body.curves[e.curve];
                let (_, x) = super::edges::project_on(c, e.interval, q);
                if x.distance(q) < limit {
                    return true;
                }
            }
        }
    }
    false
}
