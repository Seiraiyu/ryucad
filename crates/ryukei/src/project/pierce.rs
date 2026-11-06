//! Where a curve passes through another placed body (design 326, phase
//! 3): bodies placed to interfere (a press fit) change a curve's
//! visibility where it pierces another body's face, which needn't lie on
//! any image curve. Each curve is met with every face of every other
//! placed body whose box its own meets, and cut at each hit inside the
//! face; a stretch it lies along a face, or a search that can't be
//! certified, is flagged.

use crate::geom::{Box3, Curve, Point2, Point3};
use crate::intersect::broad::BoxTree;
use crate::intersect::{
    CurveHit, Place, curve_surface_in, face_box, face_tolerance, point_in_face,
};
use crate::tol::LINEAR_RESOLUTION;
use crate::topo::{Body, FaceId};
use crate::work::{Meter, Scope};

/// One stretch of a 3D curve to pierce: the curve, its stretch, the
/// placed body it lies on and its tolerance.
pub(super) struct Stretch<'a> {
    pub(super) curve: &'a Curve,
    pub(super) range: [f64; 2],
    pub(super) body: usize,
    pub(super) tolerance: f64,
}

/// What piercing one stretch found.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Pierced {
    /// 3D parameters to cut at.
    pub(super) cuts: Vec<f64>,
    /// 3D stretches whose crossings couldn't be certified.
    pub(super) unsure: Vec<[f64; 2]>,
}

/// Every face of the placed bodies, boxed in model space.
struct Faces<'a> {
    bodies: &'a [&'a Body],
    faces: Vec<(usize, FaceId)>,
    tree: BoxTree,
}

impl<'a> Faces<'a> {
    fn new(bodies: &'a [&'a Body]) -> Self {
        let mut faces = Vec::new();
        let mut boxes = Vec::new();
        for (b, &body) in bodies.iter().enumerate() {
            for f in body.faces() {
                boxes.push((face_box(body, f).grow(face_tolerance(body, f)), faces.len()));
                faces.push((b, f));
            }
        }
        Self {
            bodies,
            faces,
            tree: BoxTree::new(boxes),
        }
    }
}

/// Each stretch's piercings of the other placed bodies' faces, in order.
/// A single body passes through nothing of its own (a valid body's faces
/// meet only at their edges), so only other bodies are met.
pub(super) fn piercings(stretches: &[Stretch], bodies: &[&Body], m: &Meter) -> Vec<Pierced> {
    if bodies.len() < 2 {
        return vec![Pierced::default(); stretches.len()];
    }
    let faces = Faces::new(bodies);
    m.par_map(stretches, |m, s| pierce(s, &faces, m))
}

fn curve_box(curve: &Curve, range: [f64; 2], grow: f64) -> Box3 {
    let p = curve.jet_box(range)[0];
    Box3::new(
        Point3::new(p[0].lo(), p[1].lo(), p[2].lo()),
        Point3::new(p[0].hi(), p[1].hi(), p[2].hi()),
    )
    .grow(grow)
}

fn pierce(s: &Stretch, faces: &Faces, m: &Meter) -> Pierced {
    let mut out = Pierced::default();
    let [r0, r1] = s.range;
    if r1 <= r0 {
        return out;
    }
    let probe = BoxTree::new(vec![(curve_box(s.curve, s.range, s.tolerance), 0)]);
    let tol = s.tolerance.max(LINEAR_RESOLUTION);
    for (i, _) in faces.tree.pairs_with(&probe) {
        let (b, f) = faces.faces[i];
        if b == s.body {
            continue;
        }
        // A face a curve is met with. Halted, `work::run` reports it.
        if m.charge(1).is_err() {
            return out;
        }
        let body = faces.bodies[b];
        let surface = &body.surfaces[body.faces[f].surface];
        let hits = m.capped(Scope::Search, || {
            curve_surface_in(s.curve, s.range, surface, tol, m)
        });
        let Ok(hits) = hits else {
            // No certified crossings: the whole stretch needs agreeing
            // counts.
            out.unsure.push(s.range);
            continue;
        };
        for hit in hits {
            match hit {
                CurveHit::Point { t, point, .. } => {
                    let (u, v) = surface.project(point).param;
                    // A hit outside the face changes nothing. Anywhere
                    // else (inside, on its boundary, or unsure) the
                    // piece is cut there: each side is judged on its
                    // own, so a cut that changes nothing costs nothing.
                    if !matches!(
                        point_in_face(body, f, Point2::new(u, v)),
                        Ok(Place::Outside)
                    ) {
                        out.cuts.push(t);
                    }
                }
                CurveHit::On { t } => {
                    // Lying along the face, the curve may cross its
                    // boundary anywhere along the stretch.
                    out.cuts.extend(t);
                    out.unsure.push(t);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{Line, Rigid, Vector3};
    use crate::name::OpKey;
    use crate::primitive;

    #[test]
    fn a_line_through_another_block_is_cut_at_its_faces_and_not_by_its_own() {
        let (b, _) = primitive::block(OpKey(1), 0.01, 0.01, 0.01, &Rigid::IDENTITY).unwrap();
        let bodies = [&b, &b];
        let l = Curve::Line(
            Line::new(
                Point3::new(0.005, 0.004, -0.01),
                Vector3::new(0.0, 0.0, 1.0),
            )
            .unwrap(),
        );
        let s = |body| Stretch {
            curve: &l,
            range: [0.0, 0.03],
            body,
            tolerance: 1e-8,
        };
        // unmetered: a unit test
        let got = piercings(&[s(0)], &bodies, &Meter::free());
        let mut cuts = got[0].cuts.clone();
        cuts.sort_by(f64::total_cmp);
        assert_eq!(cuts.len(), 2, "{got:?}");
        assert!((cuts[0] - 0.01).abs() < 1e-12 && (cuts[1] - 0.02).abs() < 1e-12);
        assert!(got[0].unsure.is_empty());
        // One body alone: nothing.
        // unmetered: a unit test
        let alone = piercings(&[s(0)], &[&b], &Meter::free());
        assert_eq!(alone, vec![Pierced::default()]);
    }
}
