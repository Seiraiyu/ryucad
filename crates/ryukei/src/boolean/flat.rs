//! Flat pieces split off (design 551, plan 18.B, #585).
//!
//! A NURBS face may hold a Bézier span, between knots of full multiplicity,
//! whose control points all lie within half the resolution of one plane
//! ([`coincide::planar_rows`]). Where that plane is coincident with a
//! partner's surface (a face the check lists with it in a self-union, or a
//! face of another operand of a boolean), the face is split along the
//! knot's iso-line, which at a knot of full multiplicity is exactly a row
//! of control points: SEMV where the line crosses the face's loop, MEF
//! along the row ([`split_off`]). The flat piece is given the plane and the
//! curved piece keeps its spline, trimmed by the new edge. The flat piece
//! is then wholly coincident with its partner, and `FaceMeet`, the table,
//! split and classify are unchanged.
//!
//! Knots below full multiplicity aren't split (their iso-curve isn't a
//! control row), nor periodic directions, nor a face with more than one
//! loop, nor one the iso-line crosses anywhere but at two places on two
//! edges. Where anything withholds the split the face is left as it was,
//! so bodies without the case keep their bits.

// Used from Tasks 5 and 6 of plan 18.B on; until then only its tests call it.
#![allow(dead_code)]

use crate::geom::{Box3, Direction, NurbsSurface, Surface};
use crate::intersect::coincide::planar_rows;
use crate::tol::LINEAR_RESOLUTION;
use crate::topo::{Body, FaceId};

/// A flat Bézier span of a NURBS surface beside a knot of full
/// multiplicity: the direction, the knot, whether the span lies after it
/// (at greater parameters), the span's far knot, its plane and the box of
/// its control points.
#[derive(Clone, Debug)]
pub(crate) struct FlatSpan {
    pub(crate) dir: Direction,
    pub(crate) knot: f64,
    pub(crate) after: bool,
    pub(crate) far: f64,
    pub(crate) plane: Surface,
    pub(crate) bounds: Box3,
}

/// The flat spans of `n` beside a knot of full multiplicity strictly
/// inside `pb`, the face's parameter box: a span flat by
/// [`planar_rows`], its neighbour across the knot not. In `u` then `v`,
/// each by increasing knot.
pub(crate) fn flat_spans_of(n: &NurbsSurface, pb: [[f64; 2]; 2]) -> Vec<FlatSpan> {
    let mut out = Vec::new();
    for dir in [Direction::U, Direction::V] {
        let d = dir as usize;
        let kv = &n.knots()[d];
        let (k, m, p) = (kv.knots(), kv.mults(), kv.degree());
        let r = k.len() - 1;
        // Bodies without an interior knot of full multiplicity pay only this.
        if kv.is_periodic() || !(1..r).any(|i| m[i] == p) {
            continue;
        }
        // Each span's rows and plane.
        let spans: Vec<(usize, Option<Surface>)> = (0..r)
            .map(|i| {
                let s = kv.span(0.5 * (k[i] + k[i + 1]));
                (s, planar_rows(n, dir, s - p..=s))
            })
            .collect();
        for i in 1..r {
            if m[i] != p || !(pb[d][0] < k[i] && k[i] < pb[d][1]) {
                continue;
            }
            let (span, after, far) = match (&spans[i - 1].1, &spans[i].1) {
                (None, Some(_)) => (i, true, k[i + 1]),
                (Some(_), None) => (i - 1, false, k[i - 1]),
                _ => continue,
            };
            let (s, plane) = (spans[span].0, spans[span].1.clone().expect("flat"));
            let [nu, nv] = n.counts();
            let pts = (s - p..=s).flat_map(|row| {
                (0..match dir {
                    Direction::U => nv,
                    Direction::V => nu,
                })
                    .map(move |o| match dir {
                        Direction::U => n.control_point(row, o),
                        Direction::V => n.control_point(o, row),
                    })
            });
            let Some(bounds) = Box3::from_points(pts) else {
                continue;
            };
            out.push(FlatSpan {
                dir,
                knot: k[i],
                after,
                far,
                plane,
                bounds: bounds.grow(LINEAR_RESOLUTION),
            });
        }
    }
    out
}

/// The flat spans of face `f` of `body` ([`flat_spans_of`] over the face's
/// parameter box); none for a face that isn't a spline's.
pub(crate) fn flat_spans(body: &Body, f: FaceId) -> Vec<FlatSpan> {
    let Surface::Nurbs(n) = &body.surfaces[body.faces[f].surface] else {
        return Vec::new();
    };
    // Cheap first: no interior knot of full multiplicity in either way.
    let interior = |kv: &crate::geom::KnotVector| {
        let m = kv.mults();
        !kv.is_periodic() && m.len() > 2 && m[1..m.len() - 1].iter().any(|&x| x == kv.degree())
    };
    if !n.knots().iter().any(interior) {
        return Vec::new();
    }
    flat_spans_of(n, crate::intersect::parameter_box(body, f))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{KnotVector, NurbsSurface, Point3};
    use crate::tol::LINEAR_RESOLUTION;

    const MM: f64 = 1e-3;

    /// A rational bicubic-by-linear patch 1 mm deep in y over a profile in
    /// xz, as Voron's molex face #1789906 is: `knots`/`mults` along u, the
    /// profile's points `(x, z)` in mm, weights `w`.
    fn patch(knots: &[f64], mults: &[usize], xz: &[(f64, f64)], w: &[f64]) -> NurbsSurface {
        let ku = KnotVector::clamped(3, knots, mults).unwrap();
        let kv = KnotVector::clamped(1, &[0.0, 1.0], &[2, 2]).unwrap();
        let mut pts = Vec::new();
        let mut ws = Vec::new();
        for (i, &(x, z)) in xz.iter().enumerate() {
            for y in [0.0, MM] {
                pts.push(Point3::new(x * MM, y, z * MM));
                ws.push(w[i]);
            }
        }
        NurbsSurface::new([ku, kv], pts, Some(ws)).unwrap()
    }

    /// The molex shape: a curved span leaving z = 0 at an angle at u = 1,
    /// then a flat span on z = 0 to u = 1.01.
    const END: [(f64, f64); 7] = [
        (0.0, 1.0),
        (0.0, 0.5),
        (0.6, 0.3),
        (1.0, 0.0),
        (1.1, 0.0),
        (1.2, 0.0),
        (1.3, 0.0),
    ];
    const W: [f64; 7] = [1.0, 0.9, 0.9, 1.0, 1.0, 1.0, 1.0];
    const ALL: [[f64; 2]; 2] = [[0.0, 1.01], [0.0, 1.0]];

    #[test]
    fn a_flat_end_span_is_found_at_its_knot() {
        let n = patch(&[0.0, 1.0, 1.01], &[4, 3, 4], &END, &W);
        let spans = flat_spans_of(&n, ALL);
        assert_eq!(spans.len(), 1, "{spans:?}");
        let s = &spans[0];
        assert_eq!(
            (s.dir, s.knot, s.after, s.far),
            (Direction::U, 1.0, true, 1.01)
        );
        let f = s.plane.frame().unwrap();
        assert!(f.z().get().z.abs() > 1.0 - 1e-12, "{f:?}");
        assert!(f.origin().z.abs() <= LINEAR_RESOLUTION);
    }

    #[test]
    fn a_flat_middle_span_is_found_at_both_its_knots() {
        let mut xz = END.to_vec();
        xz.extend([(1.4, 0.3), (1.6, 0.6), (1.6, 1.0)]);
        let n = patch(&[0.0, 1.0, 1.01, 2.0], &[4, 3, 3, 4], &xz, &[1.0; 10]);
        let spans = flat_spans_of(&n, [[0.0, 2.0], [0.0, 1.0]]);
        let got: Vec<(f64, bool)> = spans.iter().map(|s| (s.knot, s.after)).collect();
        assert_eq!(got, [(1.0, true), (1.01, false)]);
    }

    #[test]
    fn a_span_flat_only_to_twice_the_resolution_is_not_split() {
        let mut xz = END;
        xz[5].1 = 2.0 * LINEAR_RESOLUTION / MM;
        let n = patch(&[0.0, 1.0, 1.01], &[4, 3, 4], &xz, &W);
        assert!(flat_spans_of(&n, ALL).is_empty());
    }

    #[test]
    fn a_flat_span_at_a_knot_below_full_multiplicity_is_not_split() {
        // Knot 1 of multiplicity 2: its iso-curve isn't a control row.
        let xz = [
            (0.0, 1.0),
            (0.0, 0.5),
            (1.0, 0.0),
            (1.1, 0.0),
            (1.2, 0.0),
            (1.3, 0.0),
        ];
        let n = patch(&[0.0, 1.0, 1.01], &[4, 2, 4], &xz, &[1.0; 6]);
        assert!(flat_spans_of(&n, ALL).is_empty());
    }

    #[test]
    fn a_knot_outside_the_face_is_not_split() {
        let n = patch(&[0.0, 1.0, 1.01], &[4, 3, 4], &END, &W);
        assert!(flat_spans_of(&n, [[0.0, 0.9], [0.0, 1.0]]).is_empty());
        assert!(flat_spans_of(&n, [[1.0, 1.01], [0.0, 1.0]]).is_empty());
    }

    #[test]
    fn planar_is_unchanged_by_its_split_into_rows() {
        // `planar` over the whole net and `planar_rows` over every row agree.
        let n = patch(&[0.0, 1.0, 1.01], &[4, 3, 4], &END, &W);
        assert_eq!(
            crate::intersect::coincide::planar(&n),
            crate::intersect::coincide::planar_rows(&n, Direction::U, 0..=6)
        );
        assert!(crate::intersect::coincide::planar_rows(&n, Direction::U, 3..=6).is_some());
        assert!(crate::intersect::coincide::planar_rows(&n, Direction::U, 0..=3).is_none());
    }
}
