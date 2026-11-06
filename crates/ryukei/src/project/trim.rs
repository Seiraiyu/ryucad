//! Silhouette branches trimmed to their faces (design 326).

use super::closed::Raw;
use crate::geom::intersect2::{Meet2, crossings};
use crate::intersect::{Failure, Place, point_in_face, shifted, shifts};
use crate::tol::LINEAR_RESOLUTION;
use crate::topo::{Body, EdgeId, FaceId};
use crate::work::Meter;

/// The stretches of a branch inside its face, and the face's edges a
/// stretch of it runs along.
#[derive(Clone, Debug, Default)]
pub(super) struct Trimmed {
    pub(super) stretches: Vec<[f64; 2]>,
    pub(super) on_edges: Vec<EdgeId>,
}

pub(super) fn trim(body: &Body, face: FaceId, raw: &Raw, m: &Meter) -> Result<Trimmed, Failure> {
    let surface = &body.surfaces[body.faces[face].surface];
    let (pu, pv) = surface.periods();
    let (c2, map) = &raw.pcurve;
    let [r0, r1] = raw.range;
    let fwd = |t: f64| map[0] + map[1] * t;
    let back = |s: f64| (s - map[0]) / map[1];
    let ra = [fwd(r0), fwd(r1)];
    let mut cuts: Vec<f64> = Vec::new();
    for l in body.face_loops(face) {
        for fin in body.loop_fins(l) {
            // A fin met with the branch.
            m.charge(1)?;
            let f = &body.fins[fin];
            let Some(pc) = &f.pcurve else {
                return Err(Failure::Uncertain);
            };
            let edge = &body.edges[f.edge];
            let c = &body.curves2[pc.curve];
            let rb = edge.interval.map(|t| pc.map[0] + pc.map[1] * t);
            for shift in shifts(c2, ra, c, rb, [pu, pv]) {
                let moved = shifted(c, shift).ok_or(Failure::Uncertain)?;
                for meet in crossings((c2, ra), (&moved, rb), LINEAR_RESOLUTION)
                    .ok_or(Failure::Uncertain)?
                {
                    match meet {
                        Meet2::Point { ta, .. } => cuts.push(back(ta)),
                        Meet2::Overlap { ta, .. } => cuts.extend(ta.map(back)),
                    }
                }
            }
        }
    }
    cuts.retain(|&t| t > r0 && t < r1);
    cuts.sort_by(f64::total_cmp);
    cuts.dedup();
    let period = raw
        .curve
        .period()
        .filter(|&p| (r1 - r0 - p).abs() <= 1e-12 * p);
    let pieces: Vec<[f64; 2]> = match period {
        Some(p) if !cuts.is_empty() => (0..cuts.len())
            .map(|i| {
                [
                    cuts[i],
                    if i + 1 < cuts.len() {
                        cuts[i + 1]
                    } else {
                        cuts[0] + p
                    },
                ]
            })
            .collect(),
        _ => {
            let mut ends = vec![r0];
            ends.extend(cuts.iter().copied());
            ends.push(r1);
            ends.windows(2).map(|w| [w[0], w[1]]).collect()
        }
    };
    let short = 4.0 * raw.tolerance.max(LINEAR_RESOLUTION);
    let mut out = Trimmed::default();
    for [a, b] in pieces {
        // A piece placed in its face.
        m.charge(1)?;
        let mid = (a + b) / 2.0;
        let (pa, pb, pm) = (raw.curve.point(a), raw.curve.point(b), raw.curve.point(mid));
        if pa.distance(pb) <= short && pa.distance(pm) <= short {
            continue;
        }
        match point_in_face(body, face, c2.point(fwd(mid)))? {
            Place::Inside => out.stretches.push([a, b]),
            Place::OnEdge(e) => out.on_edges.push(e),
            Place::Outside | Place::OnVertex(_) => {}
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{Rigid, Surface, Vector3};
    use crate::name::OpKey;
    use crate::primitive;
    use crate::project::closed::{Closed, closed_form};

    #[test]
    fn a_cylinders_rulings_run_from_cap_to_cap() {
        let (body, _) = primitive::cylinder(OpKey(1), 0.01, 0.03, &Rigid::IDENTITY).unwrap();
        // unmetered: a unit test
        let m = Meter::free();
        let t = Vector3::new(0.0, -1.0, 0.0);
        let side = body
            .faces()
            .find(|&f| matches!(body.surfaces[body.faces[f].surface], Surface::Cylinder(_)))
            .unwrap();
        let rect = crate::intersect::face_rect(&body, side).unwrap();
        let surface = &body.surfaces[body.faces[side].surface];
        let Some(Closed::Branches(raws)) = closed_form(surface, t, rect, &m).unwrap() else {
            panic!()
        };
        for raw in &raws {
            let trimmed = trim(&body, side, raw, &m).unwrap();
            assert_eq!(trimmed.stretches.len(), 1);
            let [a, b] = trimmed.stretches[0];
            let (pa, pb) = (raw.curve.point(a), raw.curve.point(b));
            assert!(
                pa.z.abs() < 1e-12 && (pb.z - 0.03).abs() < 1e-12,
                "{pa:?} {pb:?}"
            );
            assert!(trimmed.on_edges.is_empty());
        }
    }
}
