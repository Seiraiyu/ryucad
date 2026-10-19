//! Certified separation of profile segments.
//!
//! Every pair of segments that are not neighbours in a loop must be farther
//! apart than [`crate::tol::LINEAR_RESOLUTION`]; neighbours may meet only
//! at their shared end. Both are decided by the certified tests of
//! [`crate::geom`] on the segments' Bézier pieces: a test that can't pass
//! counts as touching, never as a guess.

use crate::geom::chain2::Chain2;
use crate::geom::intersect2::{Joints, NO_JOINT, chain_simple, chains_apart, overlapping_pairs};
use rayon::prelude::*;

/// The first pair of segments, in input order, that touch: `(i, i)` for a
/// segment that touches itself, else `(i, j)` with `i < j`. `loops` gives
/// each loop's first segment and length in `chains`, which holds every
/// segment in input order.
pub(super) fn first_touching(
    chains: &[Chain2],
    loops: &[(usize, usize)],
) -> Option<(usize, usize)> {
    let mut owner = vec![(0, 0); chains.len()];
    for &(first, len) in loops {
        for o in &mut owner[first..first + len] {
            *o = (first, len);
        }
    }
    let boxes: Vec<_> = chains.iter().map(|c| (c.lo, c.hi)).collect();
    let mut pairs = overlapping_pairs(&boxes);
    pairs.extend(
        (0..chains.len())
            .filter(|&i| chains[i].free_form)
            .map(|i| (i, i)),
    );
    pairs.sort_unstable();
    pairs
        .par_iter()
        .find_first(|&&(i, j)| {
            if i == j {
                return !chain_simple(&chains[i]);
            }
            let joints = neighbours(owner[i], i, j);
            !chains_apart(&chains[i], &chains[j], joints)
        })
        .copied()
}

/// How segments `i < j` meet, given `i`'s loop as its first segment and
/// length. Segments of different loops don't meet.
fn neighbours((first, len): (usize, usize), i: usize, j: usize) -> Joints {
    if j >= first + len {
        return NO_JOINT;
    }
    let (a, b) = (i - first, j - first);
    Joints {
        end_start: (a + 1) % len == b,
        start_end: (b + 1) % len == a,
    }
}

#[cfg(test)]
mod tests {
    use crate::geom::{Circle2, Curve2, Frame2, KnotVector, NurbsCurve2, Point2, Vector2};
    use crate::math;
    use crate::sweep::profile::tests::{arc, circle, fault, line, polygon, profile, square};
    use crate::sweep::{Loop, LoopKey, SegmentKey, ValidProfile};
    use crate::{Error, ProfileError};
    use core::f64::consts::{FRAC_PI_2, FRAC_PI_4, PI, TAU};
    use std::sync::Arc;

    fn touching(a: u128, b: u128) -> ProfileError {
        ProfileError::Touching {
            a: SegmentKey(a),
            b: SegmentKey(b),
        }
    }

    fn valid(loops: Vec<Loop>) -> ValidProfile {
        ValidProfile::new(profile(loops)).unwrap()
    }

    fn is_valid(loops: Vec<Loop>) -> bool {
        match ValidProfile::new(profile(loops)) {
            Ok(_) => true,
            Err(Error::InvalidProfile(ProfileError::Touching { .. })) => false,
            Err(e) => panic!("unexpected {e:?}"),
        }
    }

    /// A loop of one closed segment, keyed `key` like its loop.
    fn lone(key: u128, curve: Curve2, t0: f64, t1: f64) -> Loop {
        Loop {
            key: LoopKey(key),
            segments: vec![arc(key, curve, t0, t1)],
        }
    }

    /// A periodic cubic with uniform knots `0, 1, ..., n` and these
    /// control points.
    fn periodic(pts: &[(f64, f64)]) -> Curve2 {
        let n = pts.len();
        let knots: Vec<f64> = (0..=n).map(|i| i as f64).collect();
        let k = KnotVector::periodic(3, &knots, &vec![1; n + 1]).unwrap();
        let pts = pts.iter().map(|&(x, y)| Point2::new(x, y)).collect();
        Curve2::Nurbs2(Arc::new(NurbsCurve2::new(k, pts, None).unwrap()))
    }

    #[test]
    fn crossing_lines() {
        let bowtie = polygon(1, 10, &[(0.0, 0.0), (1.0, 1.0), (1.0, 0.0), (0.0, 1.0)]);
        assert_eq!(fault(vec![bowtie]), touching(10, 12));
        let a = square(1, 10, (0.0, 0.0), 1.0);
        let b = square(2, 20, (0.5, 0.5), 1.0);
        assert_eq!(fault(vec![a, b]), touching(11, 20));
    }

    #[test]
    fn touching_at_an_end() {
        let a = square(1, 10, (0.0, 0.0), 1.0);
        let b = polygon(2, 20, &[(1.0, 0.5), (2.0, 0.0), (2.0, 1.0)]);
        assert_eq!(fault(vec![a, b]), touching(11, 20));
        // A loop that comes back through one of its own vertices.
        let pinched = polygon(
            1,
            10,
            &[
                (0.0, 0.0),
                (2.0, 0.0),
                (1.0, 1.0),
                (2.0, 2.0),
                (0.0, 2.0),
                (1.0, 1.0),
            ],
        );
        assert_eq!(fault(vec![pinched]), touching(11, 14));
    }

    #[test]
    fn parallel_lines_either_side_of_resolution() {
        for (gap, ok) in [(0.5e-8, false), (2e-8, true)] {
            let a = square(1, 10, (0.0, 0.0), 1.0);
            let b = square(2, 20, (1.0 + gap, 0.25), 1.0);
            assert_eq!(is_valid(vec![a.clone(), b.clone()]), ok, "{gap}");
            if !ok {
                assert_eq!(fault(vec![a, b]), touching(11, 20));
            }
        }
    }

    #[test]
    fn a_circle_tangent_to_a_line() {
        let a = square(1, 10, (0.0, 0.0), 1.0);
        let c = lone(2, circle((0.5, 1.5), 0.5), 0.0, TAU);
        assert_eq!(fault(vec![a.clone(), c]), touching(12, 2));
        let c = lone(2, circle((0.5, 1.5 + 0.5e-8), 0.5), 0.0, TAU);
        assert_eq!(fault(vec![a.clone(), c]), touching(12, 2));
        let c = lone(2, circle((0.5, 1.5 + 2e-8), 0.5), 0.0, TAU);
        valid(vec![a, c]);
    }

    #[test]
    fn arcs_nearly_touching() {
        for (gap, ok) in [(2e-8, true), (0.5e-8, false)] {
            // Two circles nearly touching on a diagonal.
            let d = (2.0 + gap) / 2f64.sqrt();
            let a = lone(1, circle((0.0, 0.0), 1.0), 0.0, TAU);
            let b = lone(2, circle((d, d), 1.0), 1.0, 1.0 - TAU);
            assert_eq!(is_valid(vec![a, b]), ok, "{gap}");
            // A circle nearly touching a ring around it from inside.
            let outer = lone(3, circle((0.0, 0.0), 2.0), 0.0, TAU);
            let inner = lone(4, circle((1.0 - gap, 0.0), 1.0), 0.0, TAU);
            assert_eq!(is_valid(vec![outer, inner]), ok, "{gap}");
        }
    }

    /// A slot: two lines joined by semicircles, tangent at every joint.
    fn slot(key: u128, first: u128, (x, y): (f64, f64), len: f64, r: f64) -> Loop {
        Loop {
            key: LoopKey(key),
            segments: vec![
                line(first, (x, y - r), (x + len, y - r)),
                arc(first + 1, circle((x + len, y), r), -FRAC_PI_2, FRAC_PI_2),
                line(first + 2, (x + len, y + r), (x, y + r)),
                arc(first + 3, circle((x, y), r), FRAC_PI_2, 3.0 * FRAC_PI_2),
            ],
        }
    }

    #[test]
    fn slots_with_tangent_joints() {
        let p = valid(vec![slot(1, 10, (0.0, 0.0), 3.0, 1.0)]);
        let area = p.lumps()[0].outer.area;
        assert!((area - (6.0 + PI)).abs() <= 1e-12, "{area}");
        // Two slots side by side, close but apart.
        valid(vec![
            slot(1, 10, (0.0, 0.0), 3.0, 1.0),
            slot(2, 20, (0.0, 2.0 + 2e-8), 3.0, 1.0),
        ]);
        // A thin slot: its straight sides 4e-8 apart.
        valid(vec![slot(1, 10, (0.0, 0.0), 3.0, 2e-8)]);
    }

    #[test]
    fn sharp_valid_corners() {
        // A wedge with a one-degree tip.
        let (s, c) = math::sin_cos(PI / 360.0);
        valid(vec![polygon(1, 10, &[(0.0, 0.0), (c, s), (c, -s)])]);
        // A line and an arc of two degrees over it: one-degree corners at
        // both ends.
        let one = PI / 180.0;
        let r = 0.5 / math::sin(one);
        let over = circle((0.5, -r * math::cos(one)), r);
        let horn = Loop {
            key: LoopKey(2),
            segments: vec![
                line(20, (0.0, 0.0), (1.0, 0.0)),
                arc(21, over, FRAC_PI_2 - one, FRAC_PI_2 + one),
            ],
        };
        valid(vec![horn]);
    }

    #[test]
    fn a_cusp_is_rejected() {
        // A line, then an arc leaving back along it.
        let f = Frame2::new(Point2::new(1.0, 0.5), Vector2::new(0.0, -1.0), false).unwrap();
        let back = Curve2::Circle2(Circle2::new(f, 0.5).unwrap());
        let cusp = Loop {
            key: LoopKey(1),
            segments: vec![
                line(10, (0.0, 0.0), (1.0, 0.0)),
                arc(11, back, 0.0, PI),
                line(12, (1.0, 1.0), (0.0, 1.0)),
                line(13, (0.0, 1.0), (0.0, 0.0)),
            ],
        };
        assert_eq!(fault(vec![cusp]), touching(10, 11));
    }

    #[test]
    fn a_nurbs_figure_eight_touches_itself() {
        let eight = periodic(&[
            (1.0, 1.0),
            (2.0, 0.0),
            (1.0, -1.0),
            (-1.0, 1.0),
            (-2.0, 0.0),
            (-1.0, -1.0),
        ]);
        assert_eq!(fault(vec![lone(5, eight, 0.0, 6.0)]), touching(5, 5));
    }

    #[test]
    fn a_nurbs_segment_with_a_loop_in_it() {
        let k = KnotVector::clamped(3, &[0.0, 1.0], &[4, 4]).unwrap();
        let pts = vec![
            Point2::new(0.0, 0.0),
            Point2::new(3.0, 2.0),
            Point2::new(-1.0, 2.0),
            Point2::new(2.0, 0.0),
        ];
        let n = Curve2::Nurbs2(Arc::new(NurbsCurve2::new(k, pts, None).unwrap()));
        let l = Loop {
            key: LoopKey(1),
            segments: vec![arc(10, n, 0.0, 1.0), line(11, (2.0, 0.0), (0.0, 0.0))],
        };
        assert_eq!(fault(vec![l]), touching(10, 10));
    }

    /// A blob: a periodic cubic on control points around a circle.
    pub(crate) fn blob((x, y): (f64, f64), r: f64) -> Curve2 {
        let pts: Vec<_> = [1.0, 0.8, 1.1, 0.9, 1.0, 0.7, 1.2, 0.95]
            .iter()
            .enumerate()
            .map(|(i, k)| {
                let (s, c) = math::sin_cos(TAU * i as f64 / 8.0);
                (x + r * k * c, y + r * k * s)
            })
            .collect();
        periodic(&pts)
    }

    #[test]
    fn nurbs_blobs_are_accepted() {
        let p = valid(vec![
            lone(1, blob((0.0, 0.0), 1.0), 0.0, 8.0),
            lone(2, blob((3.0, 0.0), 1.0), 8.0, 0.0),
        ]);
        assert_eq!(p.lumps().len(), 2);
        // The same blob as two segments.
        let b = blob((0.0, 0.0), 1.0);
        let halves = Loop {
            key: LoopKey(3),
            segments: vec![arc(30, b.clone(), 2.5, 6.0), arc(31, b, 6.0, 10.5)],
        };
        valid(vec![halves]);
    }

    #[test]
    fn two_segment_loops() {
        let half = Loop {
            key: LoopKey(1),
            segments: vec![
                arc(10, circle((0.0, 0.0), 1.0), 0.0, PI),
                line(11, (-1.0, 0.0), (1.0, 0.0)),
            ],
        };
        valid(vec![half]);
        // Two arcs meeting at both ends at right angles.
        let r = 2f64.sqrt();
        let lens = Loop {
            key: LoopKey(2),
            segments: vec![
                arc(20, circle((0.0, -1.0), r), FRAC_PI_4, 3.0 * FRAC_PI_4),
                arc(21, circle((0.0, 1.0), r), 5.0 * FRAC_PI_4, 7.0 * FRAC_PI_4),
            ],
        };
        valid(vec![lens]);
        // A line there and back: no area, all overlap.
        let flat = Loop {
            key: LoopKey(3),
            segments: vec![
                line(30, (0.0, 0.0), (1.0, 0.0)),
                line(31, (1.0, 0.0), (0.0, 0.0)),
            ],
        };
        assert_eq!(fault(vec![flat]), touching(30, 31));
    }

    #[test]
    fn ten_thousand_sides() {
        let n = 10_000;
        let pts: Vec<_> = (0..n)
            .map(|i| {
                let (s, c) = math::sin_cos(TAU * f64::from(i) / f64::from(n));
                (10.0 * c, 10.0 * s)
            })
            .collect();
        let p = valid(vec![polygon(1, 0, &pts)]);
        assert_eq!(p.lumps()[0].outer.segments.len(), 10_000);
    }
}
