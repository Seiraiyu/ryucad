//! The bend graph: flanges as nodes, bends as edges.
//!
//! A breadth-first walk from the fixed flange, bends taken in name order
//! at each flange, says which end of each bend is nearer the fixed one, the
//! tree it unfolds along and the first bend closing a loop. Unfolding a
//! bend turns the flanges beyond it flat about its axis and moves them on
//! by its developed length less nothing: the strip takes its place
//! ([`unbend`]).

use crate::geom::{Frame, Rigid};
use std::collections::VecDeque;

/// What a walk of the graph found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Walk {
    /// Each bend's flange nearer the start, or `None` where the walk
    /// never reached it.
    pub(super) near: Vec<Option<usize>>,
    /// Whether each flange was reached.
    pub(super) reached: Vec<bool>,
    /// The bend the walk reached each flange through.
    pub(super) parent: Vec<Option<usize>>,
    /// The bends of the tree in the order the walk took them.
    pub(super) order: Vec<usize>,
    /// The first bend that closes a loop, in the walk's order.
    pub(super) closing: Option<usize>,
}

/// Walks `flanges` flanges joined by bends with these `ends` from `start`.
pub(super) fn walk(flanges: usize, ends: &[[usize; 2]], start: usize) -> Walk {
    let mut adjacent: Vec<Vec<usize>> = vec![Vec::new(); flanges];
    for (k, [a, c]) in ends.iter().enumerate() {
        adjacent[*a].push(k);
        if c != a {
            adjacent[*c].push(k);
        }
    }
    let mut w = Walk {
        near: vec![None; ends.len()],
        reached: vec![false; flanges],
        parent: vec![None; flanges],
        order: Vec::new(),
        closing: None,
    };
    let mut queue = VecDeque::from([start]);
    w.reached[start] = true;
    while let Some(x) = queue.pop_front() {
        for &k in &adjacent[x] {
            if w.near[k].is_some() {
                continue;
            }
            w.near[k] = Some(x);
            let [a, c] = ends[k];
            let y = if a == x { c } else { a };
            if y == x || w.reached[y] {
                w.closing.get_or_insert(k);
                continue;
            }
            w.reached[y] = true;
            w.parent[y] = Some(k);
            w.order.push(k);
            queue.push_back(y);
        }
    }
    w
}

/// The bends of the loop a closing bend makes with the tree: the tree's
/// path between its ends, then it.
pub(super) fn cycle(w: &Walk, ends: &[[usize; 2]], closing: usize) -> Vec<usize> {
    let up = |mut f: usize| {
        let mut path = vec![f];
        while let Some(k) = w.parent[f] {
            let [a, c] = ends[k];
            f = if a == f { c } else { a };
            path.push(f);
        }
        path
    };
    let [a, c] = ends[closing];
    let (pa, pc) = (up(a), up(c));
    let meet = *pa.iter().find(|f| pc.contains(f)).expect("both reached");
    let mut out = vec![closing];
    for path in [pa, pc] {
        for &f in path.iter().take_while(|&&f| f != meet) {
            out.extend(w.parent[f]);
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

/// The motion that lays a bend's far side flat beside its near one: a
/// turn by `-angle` about the axis (`z` of `axis`, oriented so the angle
/// runs from the near flange to the far one), then a move of `length`
/// along the strip, `axis`'s `y`.
pub(super) fn unbend(axis: &Frame, angle: f64, length: f64) -> Rigid {
    let o = axis.origin().to_vector();
    let turn = Rigid::rotation(axis.z(), -angle);
    Rigid::translation(o + axis.y().get() * length)
        .then_after(&turn)
        .then_after(&Rigid::translation(-o))
}

/// Each flange's motion from its folded place to the flat, along the
/// walk's tree, each bend's `unbend` from `motion` (`None` for one left
/// folded).
pub(super) fn motions(
    flanges: usize,
    ends: &[[usize; 2]],
    w: &Walk,
    motion: impl Fn(usize) -> Option<Rigid>,
) -> Vec<Rigid> {
    let mut out = vec![Rigid::IDENTITY; flanges];
    for &k in &w.order {
        let near = w.near[k].expect("a tree bend");
        let [a, c] = ends[k];
        let far = if a == near { c } else { a };
        out[far] = match motion(k) {
            Some(u) => out[near].then_after(&u),
            None => out[near],
        };
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{Point3, Vector3};
    use core::f64::consts::FRAC_PI_2;

    #[test]
    fn a_walk_finds_the_tree_and_the_loop() {
        // 0 - 1 - 2 - 0, and 2 - 3.
        let ends = [[0, 1], [1, 2], [2, 0], [2, 3]];
        let w = walk(4, &ends, 0);
        assert_eq!(w.order, [0, 2, 3]);
        assert_eq!(w.closing, Some(1));
        assert_eq!(w.near, [Some(0), Some(1), Some(0), Some(2)]);
        assert_eq!(cycle(&w, &ends, 1), [0, 1, 2]);
        let w = walk(4, &[[0, 1], [2, 3]], 0);
        assert_eq!(w.reached, [true, true, false, false]);
    }

    #[test]
    fn unbending_a_right_angle_lays_the_far_side_flat() {
        // Axis along z through the origin, the near flange's bend line at
        // x = 1 (radius 1), the far one at y = 1.
        let axis = Frame::WORLD;
        let u = unbend(&axis, FRAC_PI_2, 2.0);
        // The far bend line's point (0, 1, 0) goes to (1, 2, 0).
        let p = u.apply_point(Point3::new(0.0, 1.0, 0.0));
        assert!(p.distance(Point3::new(1.0, 2.0, 0.0)) < 1e-15);
        // The far flange runs along -x from there; flat, it runs along +y.
        let d = u.apply_vector(Vector3::new(-1.0, 0.0, 0.0));
        assert!((d - Vector3::new(0.0, 1.0, 0.0)).length() < 1e-15);
    }
}
