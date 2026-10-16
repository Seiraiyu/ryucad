//! Constrained Delaunay triangulation in the plane, on exact predicates.
//!
//! Points are taken as exact (doc 12): `orient2d` locates them and orders
//! every triangle counterclockwise, and `incircle` decides every flip, so
//! no decision rests on a rounded sign. The triangulation starts as a
//! rectangle that every later point must lie in. Points are inserted by
//! Lawson's method: split the triangle (or edge) the point falls in, then
//! flip edges that fail the empty-circle test. A segment is forced in by
//! Sloan's method ("A fast algorithm for generating constrained Delaunay
//! triangulations", Computers & Structures 47, 1993): flip the edges it
//! crosses until it appears, then restore the empty-circle test around
//! it. Constrained ("fixed") edges are never flipped.
//!
//! Triangles are only ever split or flipped, never removed, so a triangle
//! index stays valid for the life of the triangulation. Every operation is
//! a fixed sequence of exact decisions: the same inputs in the same order
//! give the same triangles.

use crate::geom::Point2;
use crate::predicates::{Sign, incircle, orient2d};
use std::collections::VecDeque;

/// No neighbour: a side of the rectangle.
pub(crate) const NONE: u32 = u32::MAX;

/// A triangle: its corners counterclockwise, and for each corner `i` the
/// neighbour across the edge opposite it (from corner `i + 1` to corner
/// `i + 2`) and whether that edge is fixed.
#[derive(Clone, Debug, PartialEq)]
struct Tri {
    v: [u32; 3],
    n: [u32; 3],
    fixed: [bool; 3],
}

/// Where a point lies in the triangulation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Located {
    /// Strictly inside a triangle.
    Inside(u32),
    /// On the edge opposite corner `i` of a triangle.
    Edge(u32, usize),
    /// On a vertex.
    Vertex(u32),
}

/// Why a segment could not be forced in: it crosses a fixed edge.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Crossing;

/// Where tracing a segment stops short of its end.
enum Stop {
    /// At a vertex on the segment.
    Vertex(u32),
    /// At a fixed edge the segment crosses.
    Fixed,
}

pub(crate) fn next(i: usize) -> usize {
    (i + 1) % 3
}

pub(crate) fn prev(i: usize) -> usize {
    (i + 2) % 3
}

#[derive(Clone, Debug)]
pub(crate) struct Cdt {
    points: Vec<Point2>,
    tris: Vec<Tri>,
    /// A triangle with each vertex as a corner.
    touch: Vec<u32>,
    /// Where point location starts: the last triangle made.
    hint: u32,
    /// Counts walk steps, so the walk varies which edge it tries first.
    turn: usize,
}

impl Cdt {
    /// The rectangle `[lo, hi]` as two triangles. Its corners are the
    /// vertices 0 to 3, counterclockwise from `lo`.
    pub(crate) fn new(lo: Point2, hi: Point2) -> Self {
        let points = vec![lo, Point2::new(hi.x, lo.y), hi, Point2::new(lo.x, hi.y)];
        let tris = vec![
            Tri {
                v: [0, 1, 2],
                n: [NONE, 1, NONE],
                fixed: [true, false, true],
            },
            Tri {
                v: [0, 2, 3],
                n: [NONE, NONE, 0],
                fixed: [true, true, false],
            },
        ];
        Self {
            points,
            tris,
            touch: vec![0, 0, 0, 1],
            hint: 0,
            turn: 0,
        }
    }

    pub(crate) fn point(&self, v: u32) -> Point2 {
        self.points[v as usize]
    }

    pub(crate) fn point_count(&self) -> usize {
        self.points.len()
    }

    pub(crate) fn triangle_count(&self) -> usize {
        self.tris.len()
    }

    /// The corners of a triangle, counterclockwise.
    pub(crate) fn corners(&self, t: u32) -> [u32; 3] {
        self.tris[t as usize].v
    }

    /// The neighbour across the edge opposite corner `i`, or [`NONE`].
    pub(crate) fn neighbour(&self, t: u32, i: usize) -> u32 {
        self.tris[t as usize].n[i]
    }

    /// True when the edge opposite corner `i` is fixed (a rectangle side
    /// or a forced segment).
    pub(crate) fn is_fixed(&self, t: u32, i: usize) -> bool {
        self.tris[t as usize].fixed[i]
    }

    fn orient(&self, a: u32, b: u32, c: u32) -> Sign {
        orient2d(self.point(a), self.point(b), self.point(c))
    }

    fn add_point(&mut self, p: Point2) -> u32 {
        self.points.push(p);
        self.touch.push(NONE);
        (self.points.len() - 1) as u32
    }

    fn set(&mut self, t: u32, tri: Tri) {
        for &v in &tri.v {
            self.touch[v as usize] = t;
        }
        if t as usize == self.tris.len() {
            self.tris.push(tri);
        } else {
            self.tris[t as usize] = tri;
        }
        self.hint = t;
    }

    /// Points the neighbour `nb`'s link to `old` at `new` instead.
    fn relink(&mut self, nb: u32, old: u32, new: u32) {
        if nb == NONE {
            return;
        }
        let tri = &mut self.tris[nb as usize];
        let j = tri.n.iter().position(|&x| x == old).expect("linked");
        tri.n[j] = new;
    }

    /// Where `p` lies, walking from the last triangle made. `None` when it
    /// is outside the rectangle.
    pub(crate) fn locate(&mut self, p: Point2) -> Option<Located> {
        self.locate_from(self.hint, p)
    }

    /// Where `p` lies, walking from triangle `t`.
    pub(crate) fn locate_from(&mut self, mut t: u32, p: Point2) -> Option<Located> {
        // A visibility walk can circle in a constrained triangulation; a
        // walk that runs long gives way to a scan.
        let limit = 4 * self.tris.len() + 16;
        for _ in 0..limit {
            let tri = &self.tris[t as usize];
            self.turn += 1;
            let start = self.turn % 3;
            let mut step = None;
            for k in 0..3 {
                let i = (start + k) % 3;
                let (b, c) = (tri.v[next(i)], tri.v[prev(i)]);
                if orient2d(self.point(b), self.point(c), p) == Sign::Negative {
                    step = Some(tri.n[i]);
                    break;
                }
            }
            match step {
                None => return Some(self.classify(t, p)),
                Some(NONE) => return None,
                Some(n) => t = n,
            }
        }
        (0..self.tris.len() as u32).find_map(|t| {
            let tri = &self.tris[t as usize];
            let outside = (0..3).any(|i| {
                orient2d(self.point(tri.v[next(i)]), self.point(tri.v[prev(i)]), p)
                    == Sign::Negative
            });
            (!outside).then(|| self.classify(t, p))
        })
    }

    /// Where `p` lies in triangle `t`, which holds it.
    fn classify(&self, t: u32, p: Point2) -> Located {
        let tri = &self.tris[t as usize];
        let zero: Vec<usize> = (0..3)
            .filter(|&i| {
                orient2d(self.point(tri.v[next(i)]), self.point(tri.v[prev(i)]), p) == Sign::Zero
            })
            .collect();
        match zero[..] {
            [] => Located::Inside(t),
            [i] => Located::Edge(t, i),
            [i, j] => Located::Vertex(tri.v[3 - i - j]),
            _ => unreachable!("a triangle with no area"),
        }
    }

    /// Inserts `p`, or returns the vertex already there. `None` when `p` is
    /// outside the rectangle.
    pub(crate) fn insert(&mut self, p: Point2) -> Option<u32> {
        self.insert_from(self.hint, p)
    }

    /// Inserts `p`, locating it from triangle `t`.
    pub(crate) fn insert_from(&mut self, t: u32, p: Point2) -> Option<u32> {
        Some(match self.locate_from(t, p)? {
            Located::Vertex(v) => v,
            Located::Inside(t) => {
                let v = self.add_point(p);
                self.split_triangle(t, v);
                v
            }
            Located::Edge(t, i) => {
                let v = self.add_point(p);
                self.split_edge_with(t, i, v);
                v
            }
        })
    }

    /// Inserts `p`, which must lie exactly on the edge opposite corner `i`
    /// of triangle `t`, splitting that edge; halves of a fixed edge stay
    /// fixed.
    pub(crate) fn split_edge(&mut self, t: u32, i: usize, p: Point2) -> u32 {
        let tri = &self.tris[t as usize];
        debug_assert_eq!(
            orient2d(self.point(tri.v[next(i)]), self.point(tri.v[prev(i)]), p),
            Sign::Zero
        );
        let v = self.add_point(p);
        self.split_edge_with(t, i, v);
        v
    }

    fn split_triangle(&mut self, t: u32, v: u32) {
        let Tri {
            v: [a, b, c],
            n,
            fixed,
        } = self.tris[t as usize].clone();
        let (t1, t2) = (self.tris.len() as u32, self.tris.len() as u32 + 1);
        self.set(
            t,
            Tri {
                v: [a, b, v],
                n: [t1, t2, n[2]],
                fixed: [false, false, fixed[2]],
            },
        );
        self.set(
            t1,
            Tri {
                v: [b, c, v],
                n: [t2, t, n[0]],
                fixed: [false, false, fixed[0]],
            },
        );
        self.set(
            t2,
            Tri {
                v: [c, a, v],
                n: [t, t1, n[1]],
                fixed: [false, false, fixed[1]],
            },
        );
        self.relink(n[0], t, t1);
        self.relink(n[1], t, t2);
        self.legalize(vec![(t, 2), (t1, 2), (t2, 2)]);
    }

    fn split_edge_with(&mut self, t: u32, i: usize, v: u32) {
        let tri = self.tris[t as usize].clone();
        let (a, b, c) = (tri.v[i], tri.v[next(i)], tri.v[prev(i)]);
        let (nb, nc) = (tri.n[next(i)], tri.n[prev(i)]);
        let (fb, fc) = (tri.fixed[next(i)], tri.fixed[prev(i)]);
        let f = tri.fixed[i];
        let u = tri.n[i];
        let t2 = self.tris.len() as u32;
        let u2 = if u == NONE { NONE } else { t2 + 1 };
        // t becomes [a, b, v] and t2 [a, v, c]; across the edge, u becomes
        // [d, c, v] and u2 [d, v, b].
        self.set(
            t,
            Tri {
                v: [a, b, v],
                n: [u2, t2, nc],
                fixed: [f, false, fc],
            },
        );
        self.set(
            t2,
            Tri {
                v: [a, v, c],
                n: [u, nb, t],
                fixed: [f, fb, false],
            },
        );
        self.relink(nb, t, t2);
        let mut todo = vec![(t, 2), (t2, 1)];
        if u != NONE {
            let ut = self.tris[u as usize].clone();
            let j = ut.n.iter().position(|&x| x == t).expect("linked");
            let d = ut.v[j];
            let (nuc, nub) = (ut.n[next(j)], ut.n[prev(j)]);
            let (fuc, fub) = (ut.fixed[next(j)], ut.fixed[prev(j)]);
            self.set(
                u,
                Tri {
                    v: [d, c, v],
                    n: [t2, u2, nub],
                    fixed: [f, false, fub],
                },
            );
            self.set(
                u2,
                Tri {
                    v: [d, v, b],
                    n: [t, nuc, u],
                    fixed: [f, fuc, false],
                },
            );
            self.relink(nuc, u, u2);
            todo.extend([(u, 2), (u2, 1)]);
        }
        self.legalize(todo);
    }

    /// Flips the edge opposite corner `i` of `t`, which must have a
    /// neighbour. With `t = [a, b, c]` (`a` at corner `i`) and `d` opposite
    /// across `bc`, the triangles become `[a, b, d]` (kept in `t`) and
    /// `[a, d, c]` (kept in the neighbour).
    fn flip(&mut self, t: u32, i: usize) -> u32 {
        let tri = self.tris[t as usize].clone();
        let (a, b, c) = (tri.v[i], tri.v[next(i)], tri.v[prev(i)]);
        let (ntb, ntc) = (tri.n[next(i)], tri.n[prev(i)]);
        let (ftb, ftc) = (tri.fixed[next(i)], tri.fixed[prev(i)]);
        let u = tri.n[i];
        let ut = self.tris[u as usize].clone();
        let j = ut.n.iter().position(|&x| x == t).expect("linked");
        let d = ut.v[j];
        // In u = [d, c, b]: across (b, d) is opposite c, across (d, c)
        // opposite b.
        let (nuc, nub) = (ut.n[next(j)], ut.n[prev(j)]);
        let (fuc, fub) = (ut.fixed[next(j)], ut.fixed[prev(j)]);
        self.set(
            t,
            Tri {
                v: [a, b, d],
                n: [nuc, u, ntc],
                fixed: [fuc, false, ftc],
            },
        );
        self.set(
            u,
            Tri {
                v: [a, d, c],
                n: [nub, ntb, t],
                fixed: [fub, ftb, false],
            },
        );
        self.relink(nuc, u, t);
        self.relink(ntb, t, u);
        u
    }

    /// The vertex across the edge opposite corner `i` of `t`.
    fn across(&self, t: u32, i: usize) -> Option<u32> {
        let u = self.tris[t as usize].n[i];
        if u == NONE {
            return None;
        }
        let ut = &self.tris[u as usize];
        let j = ut.n.iter().position(|&x| x == t).expect("linked");
        Some(ut.v[j])
    }

    /// True when the edge opposite corner `i` of `t` may and should be
    /// flipped: not fixed, and the vertex across it is inside `t`'s
    /// circumcircle.
    fn illegal(&self, t: u32, i: usize) -> bool {
        let tri = &self.tris[t as usize];
        if tri.fixed[i] {
            return false;
        }
        let Some(d) = self.across(t, i) else {
            return false;
        };
        let [a, b, c] = tri.v.map(|v| self.point(v));
        incircle(a, b, c, self.point(d)) == Sign::Positive
    }

    /// Lawson's flips after inserting a vertex: each entry is an edge
    /// opposite the new vertex.
    fn legalize(&mut self, mut todo: Vec<(u32, usize)>) {
        while let Some((t, i)) = todo.pop() {
            if self.illegal(t, i) {
                let u = self.flip(t, i);
                // The new vertex is corner 0 of both triangles.
                todo.push((t, 0));
                todo.push((u, 0));
            }
        }
    }

    /// Every triangle with `a` as a corner, and `a`'s corner index in it:
    /// round from one side of the rectangle (if `a` is on one) to the
    /// other.
    pub(crate) fn around(&self, a: u32) -> Vec<(u32, usize)> {
        let start = self.touch[a as usize];
        let index = |t: u32| {
            self.tris[t as usize]
                .v
                .iter()
                .position(|&x| x == a)
                .expect("touching")
        };
        let mut out = vec![(start, index(start))];
        // Counterclockwise: across the edge from a to the next corner.
        let mut t = start;
        loop {
            let i = index(t);
            let n = self.tris[t as usize].n[prev(i)];
            if n == NONE {
                break;
            }
            if n == start {
                return out;
            }
            out.push((n, index(n)));
            t = n;
        }
        // Reached a side: go clockwise from the start too.
        let mut t = start;
        loop {
            let i = index(t);
            let n = self.tris[t as usize].n[next(i)];
            if n == NONE {
                break;
            }
            out.push((n, index(n)));
            t = n;
        }
        out
    }

    /// The triangle with the directed edge `a → b`, and the corner the edge
    /// is opposite.
    pub(crate) fn find_edge(&self, a: u32, b: u32) -> Option<(u32, usize)> {
        self.around(a)
            .into_iter()
            .find(|&(t, i)| self.tris[t as usize].v[next(i)] == b)
            .map(|(t, i)| (t, prev(i)))
    }

    /// Marks the edge between `a` and `b` fixed on both sides.
    fn fix(&mut self, a: u32, b: u32) {
        for (x, y) in [(a, b), (b, a)] {
            if let Some((t, i)) = self.find_edge(x, y) {
                self.tris[t as usize].fixed[i] = true;
            }
        }
    }

    /// True when the segment `a b` crosses the edge `x y` at a point
    /// inside both.
    fn crosses(&self, [a, b]: [u32; 2], [x, y]: [u32; 2]) -> bool {
        let opposite = |s: Sign, t: Sign| {
            matches!(
                (s, t),
                (Sign::Positive, Sign::Negative) | (Sign::Negative, Sign::Positive)
            )
        };
        opposite(self.orient(a, b, x), self.orient(a, b, y))
            && opposite(self.orient(x, y, a), self.orient(x, y, b))
    }

    /// The edges the segment from `a` to `b` crosses, in order, as vertex
    /// pairs; or where it stops: at the first vertex strictly between `a`
    /// and `b` that lies on it, or at a fixed edge it crosses.
    fn trace(&self, a: u32, b: u32) -> Result<Vec<[u32; 2]>, Stop> {
        let pb = self.point(b);
        let ahead = |x: u32| (self.point(x) - self.point(a)).dot(pb - self.point(a)) > 0.0;
        // The triangle around a the segment leaves through.
        let mut first = None;
        for (t, i) in self.around(a) {
            let tri = &self.tris[t as usize];
            let (x, y) = (tri.v[next(i)], tri.v[prev(i)]);
            let (sx, sy) = (self.orient(a, x, b), self.orient(a, y, b));
            if sx == Sign::Zero && ahead(x) {
                return Err(Stop::Vertex(x));
            }
            if sy == Sign::Zero && ahead(y) {
                return Err(Stop::Vertex(y));
            }
            if sx == Sign::Positive && sy == Sign::Negative {
                first = Some((t, i));
                break;
            }
        }
        let (mut t, i) = first.expect("the segment leaves a");
        let tri = &self.tris[t as usize];
        if tri.fixed[i] {
            return Err(Stop::Fixed);
        }
        let mut edge = [tri.v[next(i)], tri.v[prev(i)]];
        let mut out = vec![edge];
        let side = |s: u32| self.orient(a, b, s);
        loop {
            let tri = &self.tris[t as usize];
            let k = (0..3)
                .find(|&k| {
                    let e = [tri.v[next(k)], tri.v[prev(k)]];
                    e == edge || e == [edge[1], edge[0]]
                })
                .expect("edge of t");
            let u = tri.n[k];
            let ut = &self.tris[u as usize];
            let j = ut.n.iter().position(|&x| x == t).expect("linked");
            let s = ut.v[j];
            if s == b {
                return Ok(out);
            }
            match side(s) {
                Sign::Zero => return Err(Stop::Vertex(s)),
                sign => {
                    // Leave through the edge from s to whichever end of the
                    // crossed edge lies on the other side.
                    edge = if side(edge[0]) == sign {
                        [s, edge[1]]
                    } else {
                        [edge[0], s]
                    };
                }
            }
            let k = (0..3)
                .find(|&k| {
                    let e = [ut.v[next(k)], ut.v[prev(k)]];
                    e == edge || e == [edge[1], edge[0]]
                })
                .expect("edge of u");
            if ut.fixed[k] {
                return Err(Stop::Fixed);
            }
            out.push(edge);
            t = u;
        }
    }

    /// Forces the segment between vertices `a` and `b` into the
    /// triangulation as fixed edges, split at any vertex lying on it.
    /// Returns its pieces in order from `a` to `b`. Fails when it crosses
    /// a fixed edge.
    pub(crate) fn insert_segment(&mut self, a: u32, b: u32) -> Result<Vec<[u32; 2]>, Crossing> {
        let mut todo = vec![(a, b)];
        let mut pieces = Vec::new();
        while let Some((a, b)) = todo.pop() {
            if a == b {
                continue;
            }
            if self.find_edge(a, b).is_some() || self.find_edge(b, a).is_some() {
                self.fix(a, b);
                pieces.push([a, b]);
                continue;
            }
            let crossed = match self.trace(a, b) {
                Ok(c) => c,
                Err(Stop::Vertex(v)) => {
                    todo.push((v, b));
                    todo.push((a, v));
                    continue;
                }
                Err(Stop::Fixed) => return Err(Crossing),
            };
            self.flip_out([a, b], crossed)?;
            self.fix(a, b);
            pieces.push([a, b]);
        }
        Ok(pieces)
    }

    /// Sloan's loop: flips each crossed edge whose quadrilateral is convex,
    /// queueing the new diagonal again while it still crosses, then
    /// restores the empty-circle test on the new edges.
    fn flip_out(&mut self, seg: [u32; 2], crossed: Vec<[u32; 2]>) -> Result<(), Crossing> {
        let mut queue: VecDeque<[u32; 2]> = crossed.into();
        let mut made = Vec::new();
        // Each pass over the queue without a flip means no progress.
        let mut stuck = 0;
        while let Some([x, y]) = queue.pop_front() {
            let (t, i) = self.find_edge(x, y).expect("edge exists");
            let p = self.tris[t as usize].v[i];
            let q = self.across(t, i).expect("interior edge");
            // Convex when the diagonal p q separates x and y strictly.
            let convex = matches!(
                (self.orient(p, q, x), self.orient(p, q, y)),
                (Sign::Positive, Sign::Negative) | (Sign::Negative, Sign::Positive)
            );
            if !convex {
                queue.push_back([x, y]);
                stuck += 1;
                if stuck > 2 * queue.len() + 2 {
                    return Err(Crossing);
                }
                continue;
            }
            stuck = 0;
            self.flip(t, i);
            if self.crosses(seg, [p, q]) {
                queue.push_back([p, q]);
            } else {
                made.push([p, q]);
            }
        }
        // Restore the empty-circle test on the new edges (but not on the
        // segment itself).
        loop {
            let mut flipped = false;
            for e in &mut made {
                if *e == seg || *e == [seg[1], seg[0]] {
                    continue;
                }
                let Some((t, i)) = self.find_edge(e[0], e[1]) else {
                    continue;
                };
                if self.illegal(t, i) {
                    let p = self.tris[t as usize].v[i];
                    let q = self.across(t, i).expect("interior edge");
                    self.flip(t, i);
                    *e = [p, q];
                    flipped = true;
                }
            }
            if !flipped {
                return Ok(());
            }
        }
    }

    /// Checks every invariant; for tests.
    #[cfg(test)]
    pub(crate) fn validate(&self) {
        for (t, tri) in self.tris.iter().enumerate() {
            let [a, b, c] = tri.v.map(|v| self.point(v));
            assert_eq!(orient2d(a, b, c), Sign::Positive, "triangle {t} not ccw");
            for i in 0..3 {
                let n = tri.n[i];
                let (x, y) = (tri.v[next(i)], tri.v[prev(i)]);
                if n == NONE {
                    assert!(tri.fixed[i], "an open side must be fixed");
                    continue;
                }
                let nt = &self.tris[n as usize];
                let j = nt.n.iter().position(|&m| m == t as u32).expect("symmetric");
                assert_eq!((nt.v[next(j)], nt.v[prev(j)]), (y, x), "shared edge");
                assert_eq!(nt.fixed[j], tri.fixed[i], "fixed on both sides");
            }
        }
        for (v, &t) in self.touch.iter().enumerate() {
            if t != NONE {
                assert!(self.tris[t as usize].v.contains(&(v as u32)));
            }
        }
    }

    /// True when no unfixed edge fails the empty-circle test; for tests.
    #[cfg(test)]
    pub(crate) fn is_delaunay(&self) -> bool {
        (0..self.tris.len() as u32).all(|t| (0..3).all(|i| !self.illegal(t, i)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u64) -> f64 {
        *seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (*seed >> 11) as f64 / (1u64 << 53) as f64
    }

    fn square() -> Cdt {
        Cdt::new(Point2::new(0.0, 0.0), Point2::new(1.0, 1.0))
    }

    #[test]
    fn a_rectangle_is_two_triangles() {
        let c = square();
        c.validate();
        assert_eq!(c.triangle_count(), 2);
        assert_eq!(c.point_count(), 4);
        assert!(c.is_delaunay());
    }

    #[test]
    fn random_points_are_delaunay() {
        let mut c = square();
        let mut seed = 7;
        for _ in 0..500 {
            let p = Point2::new(lcg(&mut seed), lcg(&mut seed));
            c.insert(p).unwrap();
        }
        c.validate();
        assert!(c.is_delaunay());
        // 504 points, 4 on the hull: 2n - h - 2 triangles.
        assert_eq!(c.triangle_count(), 2 * 504 - 4 - 2);
    }

    #[test]
    fn grid_points_are_cocircular_and_collinear() {
        let mut c = square();
        for i in 0..=10 {
            for j in 0..=10 {
                c.insert(Point2::new(f64::from(i) / 10.0, f64::from(j) / 10.0))
                    .unwrap();
            }
        }
        c.validate();
        assert!(c.is_delaunay());
        // The corners were already there: 121 points, 40 on the hull.
        assert_eq!(c.point_count(), 121);
        assert_eq!(c.triangle_count(), 2 * 121 - 40 - 2);
    }

    #[test]
    fn duplicates_and_outside_points() {
        let mut c = square();
        let a = c.insert(Point2::new(0.5, 0.5)).unwrap();
        assert_eq!(c.insert(Point2::new(0.5, 0.5)), Some(a));
        assert_eq!(c.insert(Point2::new(1.0, 1.0)), Some(2));
        assert_eq!(c.insert(Point2::new(1.5, 0.5)), None);
        // Distinct by one ulp is distinct.
        let b = c.insert(Point2::new(0.5f64.next_up(), 0.5)).unwrap();
        assert_ne!(a, b);
        c.validate();
    }

    #[test]
    fn segments_become_fixed_edges() {
        let mut c = square();
        let mut seed = 11;
        for _ in 0..200 {
            c.insert(Point2::new(lcg(&mut seed), lcg(&mut seed)))
                .unwrap();
        }
        // A polygon through some of the points: a star around the centre.
        let centre = Point2::new(0.5, 0.5);
        let mut ring: Vec<u32> = (0..12)
            .map(|k| {
                let a = f64::from(k) * core::f64::consts::TAU / 12.0;
                let r = if k % 2 == 0 { 0.4 } else { 0.15 };
                let (s, co) = crate::math::sin_cos(a);
                c.insert(Point2::new(centre.x + r * co, centre.y + r * s))
                    .unwrap()
            })
            .collect();
        ring.push(ring[0]);
        for w in ring.windows(2) {
            c.insert_segment(w[0], w[1]).unwrap();
        }
        c.validate();
        assert!(c.is_delaunay(), "constrained Delaunay");
        for w in ring.windows(2) {
            let (t, i) = c.find_edge(w[0], w[1]).expect("segment present");
            assert!(c.is_fixed(t, i));
        }
    }

    #[test]
    fn segments_split_at_vertices_on_them() {
        let mut c = square();
        let p = |x: f64, y: f64| Point2::new(x, y);
        let a = c.insert(p(0.1, 0.1)).unwrap();
        let mid = c.insert(p(0.5, 0.5)).unwrap();
        let b = c.insert(p(0.9, 0.9)).unwrap();
        c.insert(p(0.3, 0.7)).unwrap();
        c.insert(p(0.7, 0.3)).unwrap();
        assert_eq!(c.insert_segment(a, b).unwrap(), [[a, mid], [mid, b]]);
        c.validate();
        assert!(c.find_edge(a, mid).or(c.find_edge(mid, a)).is_some());
        assert!(c.find_edge(mid, b).or(c.find_edge(b, mid)).is_some());
        // And a segment along the rectangle's diagonal through the corners.
        c.insert_segment(0, 2).unwrap();
        c.validate();
    }

    #[test]
    fn crossing_a_fixed_edge_fails() {
        let mut c = square();
        let p = |x: f64, y: f64| Point2::new(x, y);
        let a = c.insert(p(0.1, 0.5)).unwrap();
        let b = c.insert(p(0.9, 0.5)).unwrap();
        let x = c.insert(p(0.5, 0.1)).unwrap();
        let y = c.insert(p(0.5, 0.9)).unwrap();
        c.insert_segment(a, b).unwrap();
        assert_eq!(c.insert_segment(x, y), Err(Crossing));
    }

    #[test]
    fn long_thin_holes_and_near_duplicates() {
        // A thin slot: two long parallel segments 1e-9 apart, with
        // points along them that are almost collinear.
        let mut c = Cdt::new(Point2::new(-1.0, -1.0), Point2::new(11.0, 1.0));
        let mut top = Vec::new();
        let mut bottom = Vec::new();
        for k in 0..=100 {
            let x = f64::from(k) / 10.0;
            let wobble = if k % 3 == 0 { 1e-15 } else { 0.0 };
            top.push(c.insert(Point2::new(x, 1e-9 + wobble)).unwrap());
            bottom.push(c.insert(Point2::new(x, -wobble)).unwrap());
        }
        for w in top.windows(2).chain(bottom.windows(2)) {
            c.insert_segment(w[0], w[1]).unwrap();
        }
        c.insert_segment(top[0], bottom[0]).unwrap();
        c.insert_segment(top[100], bottom[100]).unwrap();
        c.validate();
        assert!(c.is_delaunay());
        // A long segment across many points at once.
        let mut d = square();
        let mut seed = 3;
        for _ in 0..300 {
            d.insert(Point2::new(lcg(&mut seed), lcg(&mut seed)))
                .unwrap();
        }
        d.insert_segment(0, 2).unwrap();
        d.validate();
        assert!(d.is_delaunay());
        // Crossing it fails, and changes nothing.
        let before = d.clone();
        assert_eq!(d.insert_segment(1, 3), Err(Crossing));
        assert_eq!(d.tris, before.tris);
    }

    #[test]
    fn splitting_edges_keeps_fixed_halves() {
        let mut c = square();
        // The bottom side, 0 → 1, is fixed: split it in the middle.
        let (t, i) = c.find_edge(0, 1).unwrap();
        let m = c.split_edge(t, i, Point2::new(0.5, 0.0));
        c.validate();
        let (t, i) = c.find_edge(0, m).unwrap();
        assert!(c.is_fixed(t, i));
        // An interior edge: a fresh square's diagonal 0 → 2.
        let mut c = square();
        let (t, i) = c.find_edge(0, 2).unwrap();
        let d = c.split_edge(t, i, Point2::new(0.5, 0.5));
        c.validate();
        assert!(c.is_delaunay());
        assert_eq!(c.locate(Point2::new(0.5, 0.5)), Some(Located::Vertex(d)));
    }

    #[test]
    fn random_segments_keep_every_invariant() {
        let mut c = square();
        let mut seed = 5;
        let mut ids = Vec::new();
        // Coordinates on a coarse grid, so collinear and cocircular cases
        // are common.
        let mut grid = || f64::from((lcg(&mut seed) * 16.0) as u32) / 16.0;
        for _ in 0..150 {
            ids.push(c.insert(Point2::new(grid(), grid())).unwrap());
        }
        let mut fixed = 0;
        for k in 0..200 {
            let (a, b) = (ids[(k * 7) % ids.len()], ids[(k * 13 + 5) % ids.len()]);
            if c.insert_segment(a, b).is_ok() {
                fixed += 1;
            }
            c.validate();
        }
        assert!(fixed > 20);
        assert!(c.is_delaunay());
    }

    #[test]
    fn the_same_inputs_give_the_same_triangles() {
        let build = || {
            let mut c = square();
            let mut seed = 99;
            let mut ids = Vec::new();
            for _ in 0..300 {
                ids.push(
                    c.insert(Point2::new(lcg(&mut seed), lcg(&mut seed)))
                        .unwrap(),
                );
            }
            for w in ids.windows(2).step_by(7) {
                let _ = c.insert_segment(w[0], w[1]);
            }
            (0..c.triangle_count() as u32)
                .map(|t| c.corners(t))
                .collect::<Vec<_>>()
        };
        assert_eq!(build(), build());
    }
}
