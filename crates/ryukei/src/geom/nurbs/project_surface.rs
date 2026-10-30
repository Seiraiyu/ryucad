//! Global closest-point projection onto NURBS surfaces.
//!
//! A nearest point inside the domain is a stationary point of the
//! distance: a common root of `gu = (S - p)·Su` and `gv = (S - p)·Sv`. On a
//! Bézier patch with homogeneous form `S = A / W`, `W³ gu` and `W³ gv` are
//! tensor-product polynomials whose Bernstein coefficients are computed
//! directly, as for curves. A part of a patch where either has one strict
//! sign holds no stationary point; parts that can't be excluded are split
//! four ways down to a depth limit, where Newton's method finds the point.
//! Parts farther from `p` than the best point so far are pruned, and the
//! nearest parts are searched first. Nearest
//! points on the domain's boundary are those of its boundary curves.

use super::KnotVector;
use super::eval::Spline;
use super::project::{Foot, Signs, binom, creases, distinct, in_period, signs};
use super::refine::Hom;
use super::surface::{Direction, NurbsSurface};
use crate::geom::{SurfaceEval, Vector3};
use crate::tol::LINEAR_RESOLUTION;
use core::ops::{Add, Mul, Sub};

/// The deepest split of a patch.
const MAX_DEPTH: u32 = 16;

/// The most parts the search of one patch visits. Only continua of
/// stationary points that the per-patch checks don't catch come near it (a
/// patch whose control net collapses to a curve, say); when one does, the
/// search of that patch stops there and the nearest corner seen stands for
/// it.
const MAX_NODES: usize = 1 << 14;

/// Newton steps per stationary point.
const MAX_STEPS: u32 = 32;

/// Below this fraction of the largest product term a coefficient counts
/// as zero.
const ZERO: f64 = 1e-12;

/// A tensor-product Bernstein polynomial, coefficients row-major: row `i`
/// along `u`, column `j` along `v`.
#[derive(Clone, Debug)]
struct Grid<T> {
    rows: usize,
    cols: usize,
    c: Vec<T>,
}

impl<T: Copy> Grid<T> {
    fn at(&self, i: usize, j: usize) -> T {
        self.c[i * self.cols + j]
    }

    fn map<U>(&self, f: impl Fn(T) -> U) -> Grid<U> {
        Grid {
            rows: self.rows,
            cols: self.cols,
            c: self.c.iter().map(|&x| f(x)).collect(),
        }
    }

    /// Row `i` (a polynomial in `v`).
    fn row(&self, i: usize) -> Vec<T> {
        self.c[i * self.cols..(i + 1) * self.cols].to_vec()
    }

    /// Column `j` (a polynomial in `u`).
    fn col(&self, j: usize) -> Vec<T> {
        (0..self.rows).map(|i| self.at(i, j)).collect()
    }

    /// Split at `s` along `u` (`along_u`) or `v`, by de Casteljau with
    /// `lerp`.
    fn split(&self, along_u: bool, s: f64, lerp: impl Fn(T, T, f64) -> T) -> (Self, Self) {
        let (lines, len, step, stride) = if along_u {
            (self.cols, self.rows, 1, self.cols)
        } else {
            (self.rows, self.cols, self.cols, 1)
        };
        let mut left = self.c.clone();
        let mut right = self.c.clone();
        let mut q = Vec::with_capacity(len);
        for line in 0..lines {
            let base = line * step;
            q.clear();
            q.extend((0..len).map(|k| self.c[base + k * stride]));
            left[base] = q[0];
            right[base + (len - 1) * stride] = q[len - 1];
            for k in 1..len {
                for i in 0..len - k {
                    q[i] = lerp(q[i], q[i + 1], s);
                }
                left[base + k * stride] = q[0];
                right[base + (len - 1 - k) * stride] = q[len - 1 - k];
            }
        }
        let make = |c| Self {
            rows: self.rows,
            cols: self.cols,
            c,
        };
        (make(left), make(right))
    }
}

impl<T: Copy + Sub<Output = T> + Mul<f64, Output = T>> Grid<T> {
    /// The partial derivative along `u` (`along_u`) or `v`.
    fn deriv(&self, along_u: bool) -> Self {
        let (rows, cols) = if along_u {
            (self.rows - 1, self.cols)
        } else {
            (self.rows, self.cols - 1)
        };
        let n = if along_u { rows } else { cols } as f64;
        let c = (0..rows)
            .flat_map(|i| (0..cols).map(move |j| (i, j)))
            .map(|(i, j)| {
                let next = if along_u {
                    self.at(i + 1, j)
                } else {
                    self.at(i, j + 1)
                };
                (next - self.at(i, j)) * n
            })
            .collect();
        Self { rows, cols, c }
    }
}

/// The product weights `C(m, i) C(n, j) / C(m + n, i + j)` of one
/// direction.
fn weights(m: usize, n: usize) -> Vec<Vec<f64>> {
    (0..=m)
        .map(|i| {
            (0..=n)
                .map(|j| binom(m, i) * binom(n, j) / binom(m + n, i + j))
                .collect()
        })
        .collect()
}

/// The product of two tensor-product polynomials, combined by `term`, and
/// the largest term's size (by `size`).
fn product<A: Copy, B: Copy, C: Copy + Add<Output = C> + Mul<f64, Output = C>>(
    a: &Grid<A>,
    b: &Grid<B>,
    zero: C,
    term: impl Fn(A, B) -> C,
    size: impl Fn(A, B) -> f64,
) -> (Grid<C>, f64) {
    let (wu, wv) = (
        weights(a.rows - 1, b.rows - 1),
        weights(a.cols - 1, b.cols - 1),
    );
    let (rows, cols) = (a.rows + b.rows - 1, a.cols + b.cols - 1);
    let mut c = vec![zero; rows * cols];
    let mut big: f64 = 0.0;
    for (i, wui) in wu.iter().enumerate() {
        for (j, wvj) in wv.iter().enumerate() {
            let x = a.at(i, j);
            for (k, &wuk) in wui.iter().enumerate() {
                for (l, &wvl) in wvj.iter().enumerate() {
                    let y = b.at(k, l);
                    let w = wuk * wvl;
                    let idx = (i + k) * cols + j + l;
                    c[idx] = c[idx] + term(x, y) * w;
                    big = big.max(size(x, y) * w);
                }
            }
        }
    }
    (Grid { rows, cols, c }, big)
}

/// `(A - pW)·(A_u W - A W_u)` and its `v` counterpart on a patch, each
/// with its zero threshold, scaled as for curves by the terms before they
/// cancel.
fn stationary(net: &Grid<Hom<Vector3>>, p: Vector3) -> [(Grid<f64>, f64); 2] {
    let a = net.map(|h| (h.p - p) * h.w);
    let w = net.map(|h| h.w);
    let scale = |x: Vector3, y: f64| x * y;
    let size = |x: Vector3, y: f64| x.length() * y;
    let amax = a.c.iter().map(|x| x.length()).fold(0.0, f64::max);
    [true, false].map(|along_u| {
        let (aw, big_aw) = product(&a.deriv(along_u), &w, Vector3::ZERO, scale, size);
        let (wa, big_wa) = product(&a, &w.deriv(along_u), Vector3::ZERO, scale, size);
        let b = Grid {
            rows: aw.rows,
            cols: aw.cols,
            c: aw.c.iter().zip(&wa.c).map(|(&x, &y)| x - y).collect(),
        };
        let (g, _) = product(&a, &b, 0.0, Vector3::dot, |_, _| 0.0);
        (g, ZERO * amax * big_aw.max(big_wa))
    })
}

/// The distance from `p` to a net's control-hull box: no point of the
/// patch is nearer.
fn hull_distance(net: &Grid<Hom<Vector3>>, p: Vector3) -> f64 {
    let first = net.c[0].p;
    let (lo, hi) = net
        .c
        .iter()
        .fold((first, first), |(lo, hi), h| (lo.min(h.p), hi.max(h.p)));
    (p.max(lo).min(hi) - p).length()
}

/// A part of a patch still being searched.
struct Node {
    range: [[f64; 2]; 2],
    net: Grid<Hom<Vector3>>,
    g: [Grid<f64>; 2],
    depth: u32,
}

impl Node {
    /// The four quarters, in parameter order.
    fn split(&self) -> [Node; 4] {
        let [[u0, u1], [v0, v1]] = self.range;
        let (um, vm) = (u0 + (u1 - u0) * 0.5, v0 + (v1 - v0) * 0.5);
        let lerp = |a: f64, b: f64, s: f64| a * (1.0 - s) + b * s;
        let (nl, nr) = self.net.split(true, 0.5, Hom::lerp);
        let (al, ar) = self.g[0].split(true, 0.5, lerp);
        let (bl, br) = self.g[1].split(true, 0.5, lerp);
        let quarter = |net: &Grid<Hom<Vector3>>, a: &Grid<f64>, b: &Grid<f64>, u: [f64; 2]| {
            let (n0, n1) = net.split(false, 0.5, Hom::lerp);
            let (a0, a1) = a.split(false, 0.5, lerp);
            let (b0, b1) = b.split(false, 0.5, lerp);
            [
                Node {
                    range: [u, [v0, vm]],
                    net: n0,
                    g: [a0, b0],
                    depth: self.depth + 1,
                },
                Node {
                    range: [u, [vm, v1]],
                    net: n1,
                    g: [a1, b1],
                    depth: self.depth + 1,
                },
            ]
        };
        let [q0, q1] = quarter(&nl, &al, &bl, [u0, um]);
        let [q2, q3] = quarter(&nr, &ar, &br, [um, u1]);
        [q0, q1, q2, q3]
    }

    fn lower_bound(&self, p: Vector3) -> f64 {
        hull_distance(&self.net, p)
    }

    /// The four corners, which are points of the surface.
    fn corners(&self) -> [(f64, f64, Vector3); 4] {
        let (r, c) = (self.net.rows - 1, self.net.cols - 1);
        let [[u0, u1], [v0, v1]] = self.range;
        [
            (u0, v0, self.net.at(0, 0).p),
            (u0, v1, self.net.at(0, c).p),
            (u1, v0, self.net.at(r, 0).p),
            (u1, v1, self.net.at(r, c).p),
        ]
    }
}

/// The parts of a patch still to search. Pruning, the nearest by hull
/// distance comes first (ties last in, first out), so the part holding the
/// nearest point is searched before the budget can run out elsewhere;
/// otherwise depth first.
struct Frontier {
    p: Option<Vector3>,
    nodes: Vec<Node>,
    heap: std::collections::BinaryHeap<Queued>,
    seq: u64,
}

struct Queued(f64, u64, Node);

impl PartialEq for Queued {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other).is_eq()
    }
}
impl Eq for Queued {}
impl PartialOrd for Queued {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Queued {
    /// The greatest is the nearest, then the latest.
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        other.0.total_cmp(&self.0).then(self.1.cmp(&other.1))
    }
}

impl Frontier {
    fn new(prune: bool, p: Vector3) -> Self {
        Self {
            p: prune.then_some(p),
            nodes: Vec::new(),
            heap: std::collections::BinaryHeap::new(),
            seq: 0,
        }
    }

    fn push(&mut self, node: Node) {
        match self.p {
            Some(p) => {
                self.seq += 1;
                self.heap.push(Queued(node.lower_bound(p), self.seq, node));
            }
            None => self.nodes.push(node),
        }
    }

    fn pop(&mut self) -> Option<Node> {
        match self.p {
            Some(_) => self.heap.pop().map(|q| q.2),
            None => self.nodes.pop(),
        }
    }
}

/// The signs of `g` on a node, leaving out an edge row or column that
/// lies on the domain's boundary when all of it rounds to zero: roots
/// there are boundary points, which the boundary curves find, and such an
/// edge (a pole) would otherwise keep every node along it alive.
fn interior_signs(g: &Grid<f64>, zero: f64, on_boundary: [bool; 4]) -> Signs {
    let (r, c) = (g.rows - 1, g.cols - 1);
    let small = |i0: usize, i1: usize, j0: usize, j1: usize| {
        (i0..=i1).all(|i| (j0..=j1).all(|j| g.at(i, j).abs() <= zero))
    };
    let skip = [
        on_boundary[0] && small(0, 0, 0, c),
        on_boundary[1] && small(r, r, 0, c),
        on_boundary[2] && small(0, r, 0, 0),
        on_boundary[3] && small(0, r, c, c),
    ];
    let kept = (0..=r)
        .flat_map(|i| (0..=c).map(move |j| (i, j)))
        .filter(|&(i, j)| {
            !(skip[0] && i == 0 || skip[1] && i == r || skip[2] && j == 0 || skip[3] && j == c)
        })
        .map(|(i, j)| g.at(i, j));
    signs(kept, zero)
}

/// The least and greatest coefficient of a grid's partial derivative along
/// `u` (`along_u`) or `v`: bounds on the derivative over the node.
fn slope(g: &Grid<f64>, along_u: bool) -> [f64; 2] {
    let n = if along_u { g.rows - 1 } else { g.cols - 1 } as f64;
    let (rows, cols) = if along_u {
        (g.rows - 1, g.cols)
    } else {
        (g.rows, g.cols - 1)
    };
    let mut out = [f64::INFINITY, f64::NEG_INFINITY];
    for i in 0..rows {
        for j in 0..cols {
            let next = if along_u {
                g.at(i + 1, j)
            } else {
                g.at(i, j + 1)
            };
            let d = (next - g.at(i, j)) * n;
            out = [out[0].min(d), out[1].max(d)];
        }
    }
    out
}

fn times([a, b]: [f64; 2], [c, d]: [f64; 2]) -> [f64; 2] {
    let p = [a * c, a * d, b * c, b * d];
    [
        p.iter().copied().fold(f64::INFINITY, f64::min),
        p.iter().copied().fold(f64::NEG_INFINITY, f64::max),
    ]
}

/// The value of a grid at local parameters `(s, t)`.
fn value(g: &Grid<f64>, s: f64, t: f64) -> f64 {
    let casteljau = |mut q: Vec<f64>, x: f64| {
        for k in 1..q.len() {
            for i in 0..q.len() - k {
                q[i] = q[i] * (1.0 - x) + q[i + 1] * x;
            }
        }
        q[0]
    };
    let column = (0..g.rows).map(|i| casteljau(g.row(i), t)).collect();
    casteljau(column, s)
}

/// What the stationary points in a node are, by one interval Newton step
/// on `(gu, gv)` in the node's local parameters `[0, 1]²`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    /// No stationary point.
    None,
    /// Exactly one, and it is a minimum.
    OneMinimum,
    /// At most one, and it isn't a minimum.
    NoMinimum,
    /// No conclusion.
    Unknown,
}

/// The interval Jacobian `J` of `(gu, gv)` over the node comes from the
/// derivative grids' coefficients. When no determinant in it is zero, `J`
/// is regular, so the node holds at most one root (by the mean value
/// theorem). At a root the Jacobian is `W³` times the Hessian of
/// `|S - p|² / 2`, so its signs tell a minimum (determinant and first entry
/// positive) from a saddle or a maximum. The Newton operator
/// `N = c - J⁻¹ g(c)` from the centre `c` then encloses every root: if it
/// misses the node there is none, and if it lies inside, there is exactly
/// one.
fn kind(g: &[Grid<f64>; 2]) -> Kind {
    let (a, b) = (slope(&g[0], true), slope(&g[0], false));
    let (c, d) = (slope(&g[1], true), slope(&g[1], false));
    let [ad0, ad1] = times(a, d);
    let [bc0, bc1] = times(b, c);
    let det = [ad0 - bc1, ad1 - bc0];
    if det[0] <= 0.0 && det[1] >= 0.0 {
        return Kind::Unknown;
    }
    let minimum = det[0] > 0.0 && a[0] > 0.0;
    let other = det[1] < 0.0 || a[1] < 0.0;
    let (gu, gv) = (value(&g[0], 0.5, 0.5), value(&g[1], 0.5, 0.5));
    let sub = |[x0, x1]: [f64; 2], [y0, y1]: [f64; 2]| [x0 - y1, x1 - y0];
    let inv = [1.0 / det[1], 1.0 / det[0]];
    let ns = sub(
        [0.5, 0.5],
        times(sub(times(d, [gu, gu]), times(b, [gv, gv])), inv),
    );
    let nt = sub(
        [0.5, 0.5],
        times(sub(times(a, [gv, gv]), times(c, [gu, gu])), inv),
    );
    let misses = |[x0, x1]: [f64; 2]| x1 < 0.0 || x0 > 1.0;
    let inside = |[x0, x1]: [f64; 2]| 0.0 < x0 && x1 < 1.0;
    if misses(ns) || misses(nt) {
        Kind::None
    } else if other {
        Kind::NoMinimum
    } else if minimum && inside(ns) && inside(nt) {
        Kind::OneMinimum
    } else {
        Kind::Unknown
    }
}

/// The rounding size of `r·d` where `r = S - p`: `r` carries rounding of
/// the size of the point `S` itself, which dominates when `p` is on the
/// surface.
fn slack(r: Vector3, s: Vector3, d: Vector3) -> f64 {
    ZERO * (r.length() + s.length()) * d.length()
}

/// A spline from knots and control points with weights.
fn spline(knots: &KnotVector, pts: Vec<Hom<Vector3>>) -> Spline<Vector3> {
    Spline {
        knots: knots.clone(),
        points: pts.iter().map(|h| h.p).collect(),
        weights: pts.iter().map(|h| h.w).collect(),
    }
}

/// A Bézier curve along one edge of a patch as a spline over `range`.
fn edge_spline(range: [f64; 2], pts: Vec<Hom<Vector3>>) -> Spline<Vector3> {
    let p = pts.len() - 1;
    let mut flat = vec![range[0]; p + 1];
    flat.extend(vec![range[1]; p + 1]);
    Spline::from_flat(p, &flat, &pts)
}

impl NurbsSurface {
    /// `(S - p)·Su`, `(S - p)·Sv`, and whether `(u, v)` is a minimum of the
    /// distance: the Hessian of `|S - p|² / 2` is positive semidefinite
    /// within rounding.
    fn gradient(&self, p: Vector3, u: f64, v: f64) -> (f64, f64, bool, Vector3) {
        let e = self.eval(u, v);
        let r = e.point.to_vector() - p;
        let a = e.du.dot(e.du) + r.dot(e.duu);
        let b = e.du.dot(e.dv) + r.dot(e.duv);
        let c = e.dv.dot(e.dv) + r.dot(e.dvv);
        let tol = 1e-9 * (e.du.dot(e.du) + e.dv.dot(e.dv));
        let minimum = a >= -tol && c >= -tol && a * c - b * b >= -tol * tol;
        (r.dot(e.du), r.dot(e.dv), minimum, r)
    }

    /// Newton's method for a stationary point from `(u, v)`, kept inside
    /// `bounds`, halving any step that doesn't bring the point closer. A
    /// step falls back to steepest descent where the Hessian isn't positive
    /// definite. Converged (`true`) when a step, and the full step it was
    /// cut from, both move the point by less than a sixteenth of
    /// [`LINEAR_RESOLUTION`], or when no step brings it closer; not when
    /// the steps run out. A step the line search cut short from a long one
    /// proves nothing, however little it moved the point, unless it was cut
    /// to nothing.
    fn descend(
        &self,
        p: Vector3,
        (mut u, mut v): (f64, f64),
        bounds: [[f64; 2]; 2],
    ) -> (f64, f64, SurfaceEval, bool) {
        let clamp = |x: f64, k: usize| x.max(bounds[k][0]).min(bounds[k][1]);
        let mut e = self.eval(u, v);
        let mut r = e.point.to_vector() - p;
        for _ in 0..MAX_STEPS {
            let (gu, gv) = (r.dot(e.du), r.dot(e.dv));
            let a = e.du.dot(e.du) + r.dot(e.duu);
            let b = e.du.dot(e.dv) + r.dot(e.duv);
            let c = e.dv.dot(e.dv) + r.dot(e.dvv);
            let det = a * c - b * b;
            let (su, sv) = if a > 0.0 && det > 0.0 {
                ((b * gv - c * gu) / det, (b * gu - a * gv) / det)
            } else {
                let k = e.du.dot(e.du) + e.dv.dot(e.dv);
                (-gu / k, -gv / k)
            };
            let mut t = 1.0;
            let mut next = None;
            let mut full = 0.0;
            for _ in 0..30 {
                let (nu, nv) = (clamp(u + t * su, 0), clamp(v + t * sv, 1));
                let ne = self.eval(nu, nv);
                if t == 1.0 {
                    full = (ne.point - e.point).length();
                }
                let nr = ne.point.to_vector() - p;
                if nr.dot(nr) <= r.dot(r) {
                    next = Some((nu, nv, ne, nr));
                    break;
                }
                t *= 0.5;
            }
            // A step cut down until it rounds to no step at all is no step
            // that brings the point closer: the full step is rounding noise,
            // which the derivatives can make longer than the stopping size.
            let Some((nu, nv, ne, nr)) = next.filter(|n| (n.0, n.1) != (u, v)) else {
                return (u, v, e, true);
            };
            let moved = (ne.point - e.point).length();
            (u, v, e, r) = (nu, nv, ne, nr);
            if moved < LINEAR_RESOLUTION / 16.0 && full < LINEAR_RESOLUTION / 16.0 {
                return (u, v, e, true);
            }
        }
        (u, v, e, false)
    }

    /// The domain as Newton's bounds: unbounded in periodic directions.
    fn newton_bounds(&self) -> [[f64; 2]; 2] {
        let domain = self.domain();
        let periods = self.periods();
        let open = [f64::NEG_INFINITY, f64::INFINITY];
        [
            if periods.0.is_some() { open } else { domain[0] },
            if periods.1.is_some() { open } else { domain[1] },
        ]
    }

    /// A minimum found by [`Self::descend`] from the middle of `range` and
    /// kept inside `range` grown by a thousandth of its size (and inside
    /// the domain where it is clamped). `None` unless the descent converged
    /// where the Hessian is positive semidefinite, strictly inside the
    /// grown range or on the domain's own boundary where the range was cut.
    /// Convergence, not a small gradient, is the test: where the surface
    /// folds over, `Su` vanishes and the gradient's direction is noise.
    fn newton(&self, p: Vector3, range: [[f64; 2]; 2]) -> Option<(f64, f64)> {
        let limits = self.newton_bounds();
        let bounds: [[f64; 2]; 2] = core::array::from_fn(|k| {
            let [a, b] = range[k];
            let m = (b - a) * 1e-3;
            [(a - m).max(limits[k][0]), (b + m).min(limits[k][1])]
        });
        let middle = (
            (range[0][0] + range[0][1]) / 2.0,
            (range[1][0] + range[1][1]) / 2.0,
        );
        let (u, v, _, converged) = self.descend(p, middle, bounds);
        let inside = |x: f64, k: usize| {
            bounds[k][0] < x && x < bounds[k][1] || x == limits[k][0] || x == limits[k][1]
        };
        let (.., minimum, _) = self.gradient(p, u, v);
        (converged && minimum && inside(u, 0) && inside(v, 1)).then_some((u, v))
    }

    /// The minima of the distance along the boundary curves of the clamped
    /// directions that are minima of the surface too: where moving into
    /// the domain doesn't bring the point closer, within rounding.
    fn boundary_minima(&self, p: Vector3, prune: bool, out: &mut Vec<(f64, f64)>) {
        let [nu, nv] = self.counts();
        let [ku, kv] = self.knots();
        let hom = |i: usize, j: usize| self.hom(i, j);
        let [[u0, u1], [v0, v1]] = self.domain();
        let mut edges = Vec::new();
        if !ku.is_periodic() {
            for (i, u, inward) in [(0, u0, 1.0), (nu - 1, u1, -1.0)] {
                let s = spline(kv, (0..nv).map(|j| hom(i, j)).collect());
                edges.push((s, Some(u), None, inward));
            }
        }
        if !kv.is_periodic() {
            for (j, v, inward) in [(0, v0, 1.0), (nv - 1, v1, -1.0)] {
                let s = spline(ku, (0..nu).map(|i| hom(i, j)).collect());
                edges.push((s, None, Some(v), inward));
            }
        }
        for (s, fixed_u, fixed_v, inward) in edges {
            for (t, ..) in s.minima(&s.pieces(), p, prune) {
                let (u, v) = (fixed_u.unwrap_or(t), fixed_v.unwrap_or(t));
                let e = self.eval(u, v);
                let r = e.point.to_vector() - p;
                let along = if fixed_u.is_some() { e.du } else { e.dv };
                let slack = slack(r, e.point.to_vector(), along);
                if r.dot(along) * inward >= -slack {
                    out.push((u, v));
                }
            }
        }
    }

    /// The minima along the crease lines, where the knot multiplicity is
    /// the degree and the surface may fold: a nearest point can sit on a
    /// fold without being a stationary point of either side. Each crease
    /// is a row of control points of the surface clamped across it. Only
    /// points where moving off the crease to either side doesn't bring the
    /// surface closer count, judged within rounding by the one-sided
    /// derivatives of the two halves.
    fn crease_minima(&self, p: Vector3, prune: bool, out: &mut Vec<(f64, f64)>) {
        // Each crease's feet first: a spline fitted to resolution has
        // hundreds of creases (knots of multiplicity the degree), and
        // judging a foot splits the surface there. With `prune`, a crease
        // whose feet all lie farther than the nearest point found so far
        // (another crease's foot, or a boundary minimum, each a point of
        // the surface) plus resolution can't hold a nearest point, and
        // isn't judged.
        let dist = |q: Vector3| (q - p).length();
        let mut best = out
            .iter()
            .map(|&(u, v)| dist(self.point(u, v).to_vector()))
            .fold(f64::INFINITY, f64::min);
        let mut found = Vec::new();
        for dir in [Direction::U, Direction::V] {
            let d = dir as usize;
            let creases = creases(&self.knots()[d]);
            if creases.is_empty() {
                continue;
            }
            let c = self.clamped_along(dir);
            let deg = c.knots()[d].degree();
            let [nu, nv] = c.counts();
            for k in creases {
                let Some(last) = c.knots()[d].flat().iter().rposition(|&x| x == k) else {
                    continue;
                };
                let row = last - deg;
                let pts = match dir {
                    Direction::U => (0..nv).map(|j| c.hom(row, j)).collect(),
                    Direction::V => (0..nu).map(|i| c.hom(i, row)).collect(),
                };
                let iso = spline(&c.knots()[1 - d], pts);
                let feet = iso.minima(&iso.pieces(), p, prune);
                if feet.is_empty() {
                    continue;
                }
                let near = feet.iter().map(|f| dist(f.1)).fold(f64::INFINITY, f64::min);
                best = best.min(near);
                found.push((dir, k, feet, near));
            }
        }
        let mut clamped: [Option<NurbsSurface>; 2] = [None, None];
        for (dir, k, feet, near) in found {
            if prune && near > best + LINEAR_RESOLUTION {
                continue;
            }
            let d = dir as usize;
            let c = clamped[d].get_or_insert_with(|| self.clamped_along(dir));
            let [lo, hi] = c.domain()[d];
            // The halves either side, for the one-sided derivatives: at
            // a periodic seam, the clamped surface's two ends.
            let halves = if k == lo {
                Some((c.clone(), hi, c.clone(), lo))
            } else {
                c.split(dir, k).ok().map(|(l, r)| (l, k, r, k))
            };
            let Some((l, a, r, b)) = halves else {
                continue;
            };
            for (t, ..) in feet {
                let at = |x: f64| match dir {
                    Direction::U => (x, t),
                    Direction::V => (t, x),
                };
                // `(S - p)·S'` off the crease, and its rounding size.
                let slope = |s: &NurbsSurface, x: f64| {
                    let (u, v) = at(x);
                    let e = s.eval(u, v);
                    let along = if d == 0 { e.du } else { e.dv };
                    let r = e.point.to_vector() - p;
                    (r.dot(along), slack(r, e.point.to_vector(), along))
                };
                let ((before, tb), (after, ta)) = (slope(&l, a), slope(&r, b));
                if before <= tb && after >= -ta {
                    out.push(at(k));
                }
            }
        }
    }

    /// The local minima of the distance from `p`, sorted by `(u, v)`, with
    /// duplicates (points within [`LINEAR_RESOLUTION`]) removed. With
    /// `prune`, parts whose hull box is farther than the best point so far
    /// plus [`LINEAR_RESOLUTION`] are skipped.
    pub(crate) fn minima(&self, p: Vector3, prune: bool) -> Vec<Foot<(f64, f64), Vector3>> {
        let mut params = Vec::new();
        self.boundary_minima(p, prune, &mut params);
        self.crease_minima(p, prune, &mut params);
        let dist = |q: Vector3| (q - p).length();
        let mut bound = params
            .iter()
            .map(|&(u, v)| dist(self.point(u, v).to_vector()))
            .fold(f64::INFINITY, f64::min);
        let domain = self.domain();
        let periods = self.periods();
        let clamped = [periods.0.is_none(), periods.1.is_none()];
        let mut best_corner = (f64::INFINITY, domain[0][0], domain[1][0]);
        let nets: Vec<Grid<Hom<Vector3>>> = self
            .patches()
            .iter()
            .map(|patch| Grid {
                rows: patch.degrees[0] + 1,
                cols: patch.degrees[1] + 1,
                c: patch.pts.clone(),
            })
            .collect();
        if prune {
            // Every patch corner is a point of the surface. Newton's method
            // from the nearest gives a tight bound before any patch is
            // searched, so that most are pruned at once.
            let mut seed = (f64::INFINITY, (0.0, 0.0));
            for (patch, net) in self.patches().iter().zip(&nets) {
                let [[u0, u1], [v0, v1]] = patch.range;
                for (i, u) in [(0, u0), (net.rows - 1, u1)] {
                    for (j, v) in [(0, v0), (net.cols - 1, v1)] {
                        let d = dist(net.at(i, j).p);
                        if d < seed.0 {
                            seed = (d, (u, v));
                        }
                    }
                }
            }
            let (.., e, _) = self.descend(p, seed.1, self.newton_bounds());
            bound = bound.min(seed.0).min(dist(e.point.to_vector()));
        }
        let mut exhausted = false;
        'patches: for (patch, net) in self.patches().iter().zip(nets) {
            let range = &patch.range;
            if prune && hull_distance(&net, p) > bound + LINEAR_RESOLUTION {
                continue;
            }
            let [(gu, zu), (gv, zv)] = stationary(&net, p);
            let root = Node {
                range: *range,
                net,
                g: [gu, gv],
                depth: 0,
            };
            // A whole patch on which the distance doesn't change along a
            // direction (a surface of revolution seen from its axis) is
            // searched along one edge only.
            match (
                signs(root.g[0].c.iter().copied(), zu),
                signs(root.g[1].c.iter().copied(), zv),
            ) {
                (Signs::Zero, Signs::Zero) => {
                    params.push((range[0][0], range[1][0]));
                    continue;
                }
                (Signs::Zero, _) => {
                    let s = edge_spline(range[1], root.net.row(0));
                    for (t, ..) in s.minima(&s.pieces(), p, prune) {
                        params.push((range[0][0], t));
                    }
                    continue;
                }
                (_, Signs::Zero) => {
                    let s = edge_spline(range[0], root.net.col(0));
                    for (t, ..) in s.minima(&s.pieces(), p, prune) {
                        params.push((t, range[1][0]));
                    }
                    continue;
                }
                _ => {}
            }
            let mut stack = Frontier::new(prune, p);
            stack.push(root);
            let mut visits = 0;
            while let Some(node) = stack.pop() {
                visits += 1;
                if visits > MAX_NODES {
                    exhausted = true;
                    continue 'patches;
                }
                if prune {
                    if node.lower_bound(p) > bound + LINEAR_RESOLUTION {
                        continue;
                    }
                    for (u, v, q) in node.corners() {
                        let d = dist(q);
                        bound = bound.min(d);
                        if d < best_corner.0 {
                            best_corner = (d, u, v);
                        }
                    }
                }
                let [[a0, a1], [b0, b1]] = node.range;
                let on_boundary = [
                    clamped[0] && a0 == domain[0][0],
                    clamped[0] && a1 == domain[0][1],
                    clamped[1] && b0 == domain[1][0],
                    clamped[1] && b1 == domain[1][1],
                ];
                let excluded = [(0, zu), (1, zv)]
                    .iter()
                    .any(|&(k, z)| interior_signs(&node.g[k], z, on_boundary) == Signs::Excluded);
                if excluded {
                    continue;
                }
                match kind(&node.g) {
                    Kind::None | Kind::NoMinimum => continue,
                    Kind::OneMinimum => {
                        // Exactly one stationary point, a minimum: Newton's
                        // method finds it, and the node is done.
                        if let Some((u, v)) = self.newton(p, node.range) {
                            bound = bound.min(dist(self.point(u, v).to_vector()));
                            params.push((u, v));
                            continue;
                        }
                    }
                    Kind::Unknown => {}
                }
                if node.depth >= MAX_DEPTH {
                    if let Some((u, v)) = self.newton(p, node.range) {
                        bound = bound.min(dist(self.point(u, v).to_vector()));
                        params.push((u, v));
                    }
                    continue;
                }
                let [q0, q1, q2, q3] = node.split();
                for q in [q3, q2, q1, q0] {
                    stack.push(q);
                }
            }
        }
        if exhausted {
            params.push((best_corner.1, best_corner.2));
        }
        if params.is_empty() {
            params.push((domain[0][0], domain[1][0]));
        }
        let [ku, kv] = self.knots();
        let mut feet: Vec<Foot<(f64, f64), Vector3>> = params
            .into_iter()
            .map(|(u, v)| {
                let (u, v) = (in_period(ku, u), in_period(kv, v));
                let q = self.point(u, v).to_vector();
                ((u, v), q, dist(q))
            })
            .collect();
        feet.sort_by(|x, y| x.0.0.total_cmp(&y.0.0).then(x.0.1.total_cmp(&y.0.1)));
        distinct(feet)
    }
}

#[cfg(test)]
mod tests {
    use crate::geom::{Frame, KnotVector, NurbsSurface, Plane, Point3, Sphere, Surface, Torus};
    use core::f64::consts::{FRAC_PI_2, TAU};

    fn sphere() -> NurbsSurface {
        let s = Surface::Sphere(Sphere::new(Frame::WORLD, 2.0).unwrap());
        s.to_nurbs([[0.0; 2]; 2]).unwrap().0
    }

    fn wavy() -> NurbsSurface {
        let ku = KnotVector::clamped(3, &[0.0, 0.5, 2.0], &[4, 1, 4]).unwrap();
        let kv = KnotVector::periodic(2, &[0.0, 1.0, 1.5, 3.0], &[1, 2, 1, 1]).unwrap();
        let (nu, nv) = (ku.control_count(), kv.control_count());
        let mut points = Vec::new();
        let mut weights = Vec::new();
        for i in 0..nu {
            for j in 0..nv {
                let (x, y) = (i as f64, j as f64);
                points.push(Point3::new(x, y, ((x * 1.7 + y * 0.9) % 2.0) - 1.0));
                weights.push(0.5 + ((i * 7 + j * 3) % 5) as f64 * 0.3);
            }
        }
        NurbsSurface::new([ku, kv], points, Some(weights)).unwrap()
    }

    /// The nearest of an `n × n` grid of samples.
    fn sampled(s: &NurbsSurface, p: Point3, n: u32) -> f64 {
        let [[u0, u1], [v0, v1]] = s.domain();
        let mut best = f64::INFINITY;
        for i in 0..=n {
            for j in 0..=n {
                let u = u0 + (u1 - u0) * f64::from(i) / f64::from(n);
                let v = v0 + (v1 - v0) * f64::from(j) / f64::from(n);
                best = best.min(s.point(u, v).distance(p));
            }
        }
        best
    }

    #[test]
    fn a_sphere() {
        let s = sphere();
        for p in [
            Point3::new(3.0, 1.0, 0.5),
            Point3::new(-0.3, 0.2, 0.1),
            Point3::new(0.5, -4.0, -3.0),
        ] {
            let foot = s.project(p);
            let expect = (p.to_vector().length() - 2.0).abs();
            assert!(
                (foot.distance - expect).abs() <= 1e-14,
                "{p:?}: {}",
                foot.distance
            );
            assert_eq!(foot.point, s.point(foot.param.0, foot.param.1));
            assert_eq!(s.distance(p), foot.distance);
        }
    }

    #[test]
    fn poles_and_the_centre() {
        let s = sphere();
        // Above the north pole: the pole itself, at the end of v.
        let foot = s.project(Point3::new(0.0, 0.0, 5.0));
        assert!((foot.distance - 3.0).abs() <= 1e-15);
        assert!((foot.param.1 - FRAC_PI_2).abs() <= 1e-9, "{:?}", foot.param);
        // The centre: every point is nearest. The lowest parameter wins.
        let foot = s.project(Point3::ORIGIN);
        assert!((foot.distance - 2.0).abs() <= 1e-15);
        assert_eq!(foot.param, (0.0, -FRAC_PI_2));
    }

    #[test]
    fn a_torus_from_its_axis() {
        // Every point of a circle of the torus is nearest.
        let t = Surface::Torus(Torus::new(Frame::WORLD, 3.0, 1.0).unwrap());
        let (s, _) = t.to_nurbs([[0.0; 2]; 2]).unwrap();
        let foot = s.project(Point3::new(0.0, 0.0, 1.0));
        assert!((foot.distance - (10.0f64.sqrt() - 1.0)).abs() <= 1e-14);
        assert!(foot.param.0 < 1e-6, "{:?}", foot.param);
    }

    #[test]
    fn beyond_a_corner_of_a_patch() {
        let plane = Surface::Plane(Plane::new(Frame::WORLD));
        let (s, _) = plane.to_nurbs([[0.0, 1.0], [0.0, 2.0]]).unwrap();
        let foot = s.project(Point3::new(3.0, -1.0, 1.0));
        assert_eq!(foot.param, (1.0, 0.0));
        assert!((foot.distance - 6.0f64.sqrt()).abs() <= 1e-15);
        let foot = s.project(Point3::new(0.5, 3.0, -2.0));
        assert_eq!(foot.param, (0.5, 2.0));
        let foot = s.project(Point3::new(0.25, 0.5, 2.0));
        assert!((foot.param.0 - 0.25).abs() <= 1e-15 && (foot.param.1 - 0.5).abs() <= 1e-15);
    }

    #[test]
    fn the_ridge_of_a_roof() {
        // A roof: an inverted V across u, with its ridge a crease at u = 1
        // (a knot of multiplicity 2 on a quadratic), straight along v.
        let ku = KnotVector::clamped(2, &[0.0, 1.0, 2.0], &[3, 2, 3]).unwrap();
        let kv = KnotVector::clamped(1, &[0.0, 1.0], &[2, 2]).unwrap();
        let mut points = Vec::new();
        for (x, z) in [(-1.0, 0.0), (-0.5, 0.5), (0.0, 1.0), (0.5, 0.5), (1.0, 0.0)] {
            for y in [0.0, 1.0] {
                points.push(Point3::new(x, y, z));
            }
        }
        let s = NurbsSurface::new([ku, kv], points, None).unwrap();
        let above = Point3::new(0.0, 0.5, 3.0);
        let foot = s.project(above);
        assert_eq!(foot.param.0, 1.0);
        assert!((foot.param.1 - 0.5).abs() <= 1e-12);
        assert!((foot.distance - 2.0).abs() <= 1e-15);
        assert_eq!(s.local_minima(above).len(), 1);
        // Under the roof the ridge is a maximum across it: the nearest
        // points are on the slopes.
        let under = Point3::new(0.0, 0.5, 0.5);
        assert!(s.local_minima(under).iter().all(|m| m.param.0 != 1.0));
    }

    #[test]
    fn never_worse_than_dense_sampling() {
        let s = wavy();
        for p in [
            Point3::new(1.5, 1.0, 2.0),
            Point3::new(0.2, 3.5, -1.5),
            Point3::new(4.0, -1.0, 0.0),
            Point3::new(2.0, 2.0, 0.1),
        ] {
            let foot = s.project(p);
            let d = sampled(&s, p, 300);
            assert!(foot.distance <= d + 1e-12, "{p:?}: {} > {d}", foot.distance);
            assert!(foot.distance >= d - 1e-2, "{p:?}");
            let minima = s.local_minima(p);
            assert!(minima.iter().any(|m| m.param == foot.param), "{p:?}");
            assert!(minima.windows(2).all(|w| w[0].param < w[1].param));
        }
    }

    #[test]
    fn points_on_the_surface_far_from_the_origin() {
        // Rounding in S - p grows with the coordinates: a point on the
        // surface must still be found, once, even on the seam.
        use crate::geom::{Cylinder, Vector3};
        let o = Point3::new(9.102873694699264, -4.551436847349632, 3.034291231566421);
        let f = Frame::new(o, Vector3::new(0.0, 0.0, 1.0), Vector3::new(1.0, 0.0, 0.0)).unwrap();
        let c = Surface::Cylinder(Cylinder::new(f, 1.808922322312536).unwrap());
        let pad = 1e-3 * 0.20000000000000018 + 1e-6;
        let (s, _) = c
            .to_nurbs([[0.0; 2], [-pad, 0.20000000000000018 + pad]])
            .unwrap();
        for k in 0..8 {
            let p = c.point(f64::from(k) * TAU / 8.0, 0.0);
            assert!(s.project(p).distance <= 1e-14, "k={k}");
        }
        let seam = c.point(0.0, 0.0);
        assert_eq!(s.local_minima(seam).len(), 2);
    }

    #[test]
    fn a_surface_that_folds_over() {
        // Both u ends collapse to one point, and the u lines fold back on
        // themselves: Su vanishes along a line inside the patch, and the
        // nearest point sits on it. Found by fuzzing.
        let ku = KnotVector::clamped(2, &[2.458823529411765, 2.5686274509803924], &[3, 3]).unwrap();
        let kv = KnotVector::clamped(1, &[2.6235294117647054, 4.623529411764705], &[2, 2]).unwrap();
        let q = Point3::new(6.0, 6.0, 6.0);
        let points = vec![
            q,
            q,
            Point3::new(4.91764705882353, -6.0, -5.247058823529412),
            Point3::new(6.0, 6.0, -5.952941176470588),
            q,
            q,
        ];
        let w = vec![5.0, 4.689411764705882, 5.0, 2.5152941176470587, 5.0, 5.0];
        let s = NurbsSurface::new([ku, kv], points, Some(w)).unwrap();
        let p = Point3::new(6.0, 1.9058823529411768, -6.0);
        let foot = s.project(p);
        assert!(foot.distance <= sampled(&s, p, 300) + 1e-12, "{foot:?}");
    }

    #[test]
    fn a_point_on_a_corner() {
        // p is the corner (u1, v1). Newton from the parts beside it wants
        // a step out of the part, and the line search cuts it so short
        // that the point barely moves. That isn't convergence: the
        // shortened steps left a chain of false minima creeping towards
        // the corner, and the corner, within resolution of the last of
        // them, was dropped as its duplicate. Found by fuzzing.
        let ku = KnotVector::clamped(1, &[1.0, 2.333333333333333], &[2, 2]).unwrap();
        let kv =
            KnotVector::clamped(1, &[-2.764705882352941, -2.6862745098039214], &[2, 2]).unwrap();
        let c = -5.529411764705882;
        let p = Point3::new(c, c, c);
        let points = vec![
            Point3::new(c, -5.670588235294118, c),
            Point3::new(c, c, 6.0),
            Point3::new(c, c, -5.623529411764705),
            p,
        ];
        let w = vec![
            5.0,
            0.24411764705882355,
            0.20529411764705885,
            0.24411764705882355,
        ];
        let s = NurbsSurface::new([ku, kv], points, Some(w)).unwrap();
        let foot = s.project(p);
        assert_eq!(foot.distance, 0.0, "{foot:?}");
        assert_eq!(foot.param, (2.333333333333333, -2.6862745098039214));
    }

    #[test]
    fn a_patch_that_collapses_to_a_segment() {
        // The first of three bilinear patches has both v edges collapsed:
        // it is a line segment, with a whole curve of stationary points.
        // Searching it must not starve the patch that holds the nearest
        // point. Found by fuzzing.
        let ku = KnotVector::clamped(1, &[3.0, 5.0], &[2, 2]).unwrap();
        let kv = KnotVector::clamped(
            1,
            &[
                -2.8588235294117648,
                -2.811764705882353,
                -2.7647058823529416,
                -2.71764705882353,
            ],
            &[2, 1, 1, 2],
        )
        .unwrap();
        let (a, b) = (
            Point3::new(
                -5.7176470588235295,
                -5.7176470588235295,
                -5.7176470588235295,
            ),
            Point3::new(6.0, 6.0, 6.0),
        );
        let points = vec![
            a,
            a,
            Point3::new(-5.7176470588235295, 6.0, 6.0),
            b,
            b,
            b,
            b,
            Point3::new(2.5176470588235293, 2.5176470588235293, 2.5176470588235293),
        ];
        let w = vec![
            5.0,
            4.689411764705882,
            5.0,
            5.0,
            0.9817647058823531,
            5.0,
            1.5058823529411764,
            1.5058823529411764,
        ];
        let s = NurbsSurface::new([ku, kv], points, Some(w)).unwrap();
        let p = Point3::new(-2.4705882352941178, -3.8823529411764706, 6.0);
        let foot = s.project(p);
        assert!(foot.distance <= sampled(&s, p, 300) + 1e-12, "{foot:?}");
    }

    /// A bilinear patch with one far corner and weights a hundredfold
    /// apart: the parametrisation is crushed against the edges, where the
    /// derivatives reach thousands.
    fn crushed(v: [f64; 2], last: Point3, w: [f64; 4]) -> NurbsSurface {
        let ku = KnotVector::clamped(1, &[-3.0, -2.8745098039215686], &[2, 2]).unwrap();
        let kv = KnotVector::clamped(1, &v, &[2, 2]).unwrap();
        let far = 472.9411764705883;
        let points = vec![
            Point3::new(6.0, 6.0, -3.223529411764706),
            Point3::new(far, far, far),
            Point3::new(1.670588235294118, 4.7294117647058815, 6.0),
            last,
        ];
        NurbsSurface::new([ku, kv], points, Some(w.to_vec())).unwrap()
    }

    #[test]
    fn newton_cut_to_nothing_has_converged() {
        // The nearest point is just inside the v0 edge, where |Sv| is
        // about 1600. There Newton's full step is rounding noise longer
        // than the stopping size, and the line search cuts it until it
        // rounds to no step at all: that must count as converged, or the
        // point is lost and a far corner stands in. Found by fuzzing.
        let s = crushed(
            [-2.6941176470588237, -0.6941176470588237],
            Point3::new(-3.223529411764706, -6.0, 6.0),
            [
                0.05,
                1.1758823529411766,
                4.475882352941177,
                4.475882352941177,
            ],
        );
        let p = Point3::new(4.7294117647058815, 4.7294117647058815, 6.0);
        let foot = s.project(p);
        assert!(foot.distance <= sampled(&s, p, 300) + 1e-12, "{foot:?}");
    }

    #[test]
    fn a_crushed_edge_does_not_starve_the_patch() {
        // Along the v0 edge |Su| is about 20000, so parts at the depth
        // limit are still centimetres across and can't be pruned near a
        // local minimum there. Searched depth first, they used up the
        // patch's budget before the part holding the nearest point (by the
        // u1 edge) was reached. Found by fuzzing.
        let s = crushed(
            [-0.12941176470588234, 1.8705882352941177],
            Point3::new(6.0, 6.0, 6.0),
            [0.05, 5.0, 0.36058823529411765, 1.1758823529411766],
        );
        let p = Point3::new(4.7294117647058815, 6.0, 6.0);
        let foot = s.project(p);
        assert!(foot.distance <= sampled(&s, p, 300) + 1e-12, "{foot:?}");
    }

    #[test]
    fn periodic_parameters_stay_in_one_period() {
        let c = Surface::Torus(Torus::new(Frame::WORLD, 3.0, 1.0).unwrap());
        let (s, _) = c.to_nurbs([[0.0; 2]; 2]).unwrap();
        // Just below u = 0 on the outer equator.
        let p = Point3::new(5.0, -1e-3, 0.0);
        let foot = s.project(p);
        assert!(
            foot.param.0 < TAU && foot.param.0 > TAU - 1e-3,
            "{:?}",
            foot.param
        );
        assert!(
            foot.param.1.abs() <= 1e-9 || foot.param.1 > TAU - 1e-9,
            "{:?}",
            foot.param
        );
    }
}
