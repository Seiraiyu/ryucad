//! Nesting loops into lumps.
//!
//! Separation guarantees that different loops are more than
//! [`crate::tol::LINEAR_RESOLUTION`] apart, so one point of a loop
//! classifies the whole loop against every other one, by its certified
//! winding number ([`crate::geom::winding2`]).

use crate::geom::Vector2;
use crate::geom::chain2::Chain2;
use crate::geom::intersect2::SLACK_2D;
use crate::geom::winding2::winding;
use rayon::prelude::*;

/// Where each loop sits: how many loops contain it, and the containing
/// loop of greatest depth.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Place {
    pub(super) depth: usize,
    pub(super) parent: Option<usize>,
}

/// The place of every loop, given each loop's chains and a point on it.
/// Fails with the index of the first loop, in input order, whose
/// classification could not be certified, or whose parent is not one
/// level up (which certified winding numbers rule out).
pub(super) fn places(loops: &[&[Chain2]], points: &[Vector2]) -> Result<Vec<Place>, usize> {
    let boxes: Vec<(Vector2, Vector2)> = loops.iter().map(|l| bounds(l)).collect();
    let containers: Vec<Option<Vec<usize>>> = (0..loops.len())
        .into_par_iter()
        .map(|i| {
            let p = points[i];
            let mut inside = Vec::new();
            for (j, chains) in loops.iter().enumerate() {
                if j == i || !contains(boxes[j], p) {
                    continue;
                }
                if winding(p, chains)? != 0 {
                    inside.push(j);
                }
            }
            Some(inside)
        })
        .collect();
    let mut found = Vec::with_capacity(loops.len());
    for (i, c) in containers.into_iter().enumerate() {
        found.push(c.ok_or(i)?);
    }
    let depths: Vec<usize> = found.iter().map(Vec::len).collect();
    let mut out = Vec::with_capacity(found.len());
    for (i, inside) in found.iter().enumerate() {
        let parent = inside
            .iter()
            .copied()
            .max_by_key(|&j| (depths[j], usize::MAX - j));
        if parent.map(|j| depths[j] + 1) != (depths[i] > 0).then_some(depths[i]) {
            return Err(i);
        }
        out.push(Place {
            depth: depths[i],
            parent,
        });
    }
    Ok(out)
}

/// The box of a loop's control hulls, grown by the rounding slack.
fn bounds(chains: &[Chain2]) -> (Vector2, Vector2) {
    let grow = Vector2::new(SLACK_2D, SLACK_2D);
    let (lo, hi) = chains
        .iter()
        .map(|c| (c.lo, c.hi))
        .reduce(|(a, b), (c, d)| {
            (
                Vector2::new(a.x.min(c.x), a.y.min(c.y)),
                Vector2::new(b.x.max(d.x), b.y.max(d.y)),
            )
        })
        .expect("a loop has segments");
    (lo - grow, hi + grow)
}

fn contains((lo, hi): (Vector2, Vector2), p: Vector2) -> bool {
    lo.x <= p.x && p.x <= hi.x && lo.y <= p.y && p.y <= hi.y
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::Point2;
    use crate::sweep::profile::tests::{arc, circle, polygon, profile, square};
    use crate::sweep::{Loop, LoopKey, ValidLoop, ValidProfile};
    use core::f64::consts::TAU;

    fn valid(loops: Vec<Loop>) -> ValidProfile {
        ValidProfile::new(profile(loops)).unwrap()
    }

    /// The loop keys of each lump: its outer loop, then its holes.
    fn shape(p: &ValidProfile) -> Vec<Vec<u128>> {
        p.lumps()
            .iter()
            .map(|l| {
                let mut keys = vec![l.outer.key.0];
                keys.extend(l.holes.iter().map(|h| h.key.0));
                keys
            })
            .collect()
    }

    fn chains(l: &Loop) -> Vec<Chain2> {
        l.segments
            .iter()
            .map(|s| Chain2::new(&s.curve, s.t0, s.t1))
            .collect()
    }

    #[test]
    fn winding_numbers() {
        let ccw = square(1, 10, (0.0, 0.0), 2.0);
        let cw = polygon(2, 20, &[(0.0, 0.0), (0.0, 2.0), (2.0, 2.0), (2.0, 0.0)]);
        let inside = Vector2::new(1.0, 0.5);
        assert_eq!(winding(inside, &chains(&ccw)), Some(1));
        assert_eq!(winding(inside, &chains(&cw)), Some(-1));
        assert_eq!(winding(Vector2::new(3.0, 1.0), &chains(&ccw)), Some(0));
        // Close to the boundary, but off it.
        assert_eq!(winding(Vector2::new(1.0, 2e-8), &chains(&ccw)), Some(1));
        assert_eq!(winding(Vector2::new(1.0, -2e-8), &chains(&ccw)), Some(0));
        let c = Loop {
            key: LoopKey(3),
            segments: vec![arc(30, circle((0.0, 0.0), 1.0), TAU, 0.0)],
        };
        assert_eq!(winding(Vector2::new(0.3, 0.9), &chains(&c)), Some(-1));
    }

    #[test]
    fn a_square_with_two_holes() {
        let p = valid(vec![
            square(1, 10, (0.0, 0.0), 10.0),
            square(2, 20, (1.0, 1.0), 2.0),
            square(3, 30, (5.0, 5.0), 2.0),
        ]);
        assert_eq!(shape(&p), [[1, 2, 3]]);
        let lump = &p.lumps()[0];
        assert_eq!(lump.outer.area, 100.0);
        assert_eq!(lump.holes[0].area, -4.0);
        assert_eq!(lump.holes[1].area, -4.0);
    }

    #[test]
    fn an_island_in_a_hole() {
        let island = square(1, 10, (3.0, 3.0), 3.0);
        let hole = square(2, 20, (1.0, 1.0), 8.0);
        let outer = square(3, 30, (0.0, 0.0), 10.0);
        let p = valid(vec![island, hole, outer]);
        // Lumps in the order of their outer loops in the input.
        assert_eq!(shape(&p), [vec![1], vec![3, 2]]);
        assert!(p.lumps()[0].outer.area > 0.0);
        // Four deep: a hole in the island too.
        let p = valid(vec![
            square(3, 30, (0.0, 0.0), 10.0),
            square(2, 20, (1.0, 1.0), 8.0),
            square(1, 10, (3.0, 3.0), 3.0),
            square(4, 40, (4.0, 4.0), 1.0),
        ]);
        assert_eq!(shape(&p), [[3, 2], [1, 4]]);
    }

    #[test]
    fn two_disjoint_squares_keep_input_order() {
        let p = valid(vec![
            square(7, 70, (5.0, 0.0), 1.0),
            square(3, 30, (0.0, 0.0), 1.0),
        ]);
        assert_eq!(shape(&p), [[7], [3]]);
    }

    /// The loop as given, or reversed, matches the oriented one.
    fn same_loop(given: &Loop, got: &ValidLoop, reversed: bool) -> bool {
        let segments: Vec<_> = if reversed {
            given.segments.iter().rev().map(|s| s.reversed()).collect()
        } else {
            given.segments.clone()
        };
        segments == got.segments
    }

    #[test]
    fn loops_are_turned_to_their_role() {
        // A clockwise outer loop and a counterclockwise hole: both turned.
        let outer = polygon(1, 10, &[(0.0, 0.0), (0.0, 4.0), (4.0, 4.0), (4.0, 0.0)]);
        let hole = square(2, 20, (1.0, 1.0), 2.0);
        let p = valid(vec![outer.clone(), hole.clone()]);
        let lump = &p.lumps()[0];
        assert!(same_loop(&outer, &lump.outer, true));
        assert!(same_loop(&hole, &lump.holes[0], true));
        assert_eq!((lump.outer.area, lump.holes[0].area), (16.0, -4.0));
        assert_eq!(lump.holes[0].vertices[1], Point2::new(1.0, 3.0));
        // A clockwise hole stays as it is.
        let cw = polygon(2, 20, &[(1.0, 1.0), (1.0, 3.0), (3.0, 3.0), (3.0, 1.0)]);
        let p = valid(vec![square(1, 10, (0.0, 0.0), 4.0), cw.clone()]);
        assert!(same_loop(&cw, &p.lumps()[0].holes[0], false));
    }

    #[test]
    fn a_circle_in_a_circle() {
        let outer = Loop {
            key: LoopKey(1),
            segments: vec![arc(1, circle((0.0, 0.0), 2.0), 0.0, TAU)],
        };
        let inner = Loop {
            key: LoopKey(2),
            segments: vec![arc(2, circle((0.5, 0.0), 1.0), 0.0, TAU)],
        };
        let p = valid(vec![inner, outer]);
        assert_eq!(shape(&p), [[1, 2]]);
        let hole = &p.lumps()[0].holes[0];
        assert_eq!((hole.segments[0].t0, hole.segments[0].t1), (TAU, 0.0));
        assert!((hole.area + core::f64::consts::PI).abs() <= 1e-13);
    }
}
