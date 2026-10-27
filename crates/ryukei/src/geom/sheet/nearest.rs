//! The nearest point of any sheet, by subdivision of its parameters.
//!
//! The squared distance `f = |S - p|² / 2` is searched over cells of the
//! parameter domain: a cell whose enclosure lies farther from `p` than
//! the best point found so far (by more than [`LINEAR_RESOLUTION`]) is
//! dropped; so is one over which a component of the gradient
//! `((S - p) · S_u, (S - p) · S_v)` keeps one sign, which holds no
//! stationary point; a cell where Krawczyk's test certifies exactly one
//! stationary point gives it by Newton's method and is done. Others are
//! halved across their longer side in model space, down to a quarter of
//! [`LINEAR_RESOLUTION`] across, where the middle's foot will do. The
//! domain's own
//! edges (in a direction that doesn't wrap) are searched the same way in
//! one parameter, and its corners taken as they are, since the nearest
//! point may lie there without being stationary.
//!
//! The search is capped at [`MAX_CELLS`] cells: past that, each cell
//! still open gives the point Newton's method finds from its middle, so
//! the answer is a local minimum that may not be the global one. That
//! happens only where very many points are nearly equally near (the
//! centre of a sphere-like spline), where any of them is as good.

use super::super::jet::JVec;
use super::super::{Point3, Projection};
use super::Sheet;
use crate::interval::Interval;
use crate::tol::LINEAR_RESOLUTION;

type Iv = Interval;

/// The most cells (in two parameters and along edges together) a search
/// looks at before it settles for local minima.
pub(crate) const MAX_CELLS: usize = 4096;

/// A cell no longer than this across both parameters (in model space)
/// isn't halved again: every point of it is within it of its nearest.
const SMALL: f64 = LINEAR_RESOLUTION / 4.0;

/// The most Newton steps from one start.
const NEWTON_STEPS: usize = 48;

/// A cell of parameters: `[[u0, u1], [v0, v1]]`.
type Cell = [[f64; 2]; 2];

/// Krawczyk's test for a zero of two functions over a cell, from their
/// values at its middle and their Jacobian over it: `Some(true)` for
/// exactly one, `Some(false)` for none, `None` undecided.
pub(crate) fn krawczyk2(at: [Iv; 2], jac: [[Iv; 2]; 2], cell: Cell) -> Option<bool> {
    let p = Iv::point;
    let mid = |x: Iv| (x.lo() + x.hi()) / 2.0;
    let [[t0, t1], [s0, s1]] = cell;
    let m = jac.map(|r| r.map(mid));
    let det = m[0][0] * m[1][1] - m[0][1] * m[1][0];
    if det == 0.0 || !det.is_finite() {
        return None;
    }
    let y = [
        [m[1][1] / det, -m[0][1] / det],
        [-m[1][0] / det, m[0][0] / det],
    ];
    let x = [(t0 + t1) / 2.0, (s0 + s1) / 2.0];
    let boxes = [Iv::new(t0, t1)? - p(x[0]), Iv::new(s0, s1)? - p(x[1])];
    let (mut inside, mut apart) = (true, false);
    for row in 0..2 {
        let mut k = p(x[row]) - (p(y[row][0]) * at[0] + p(y[row][1]) * at[1]);
        for (col, &b) in boxes.iter().enumerate() {
            let ident = if row == col { 1.0 } else { 0.0 };
            let e = p(ident) - (p(y[row][0]) * jac[0][col] + p(y[row][1]) * jac[1][col]);
            k = k + e * b;
        }
        let [lo, hi] = cell[row];
        inside &= k.lo() > lo && k.hi() < hi;
        apart |= k.hi() < lo || k.lo() > hi;
    }
    if apart {
        Some(false)
    } else if inside {
        Some(true)
    } else {
        None
    }
}

fn vec<T: Copy>(j: &JVec<T>, f: impl Fn(&super::super::jet::Jet<T>) -> T) -> [T; 3] {
    [f(&j[0]), f(&j[1]), f(&j[2])]
}

fn dot<T: Copy + core::ops::Add<Output = T> + core::ops::Mul<Output = T>>(
    a: [T; 3],
    b: [T; 3],
) -> T {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// What the search keeps: the nearest point's parameters and distance.
#[derive(Clone, Copy, Debug)]
struct Foot {
    uv: (f64, f64),
    point: Point3,
    distance: f64,
}

struct Search<'a> {
    sheet: &'a Sheet<'a>,
    domain: Cell,
    periods: (Option<f64>, Option<f64>),
    p: Point3,
    feet: Vec<Foot>,
    best: f64,
    cells: usize,
}

impl Search<'_> {
    fn foot(&self, (u, v): (f64, f64)) -> Foot {
        let point = self.sheet.point(u, v);
        Foot {
            uv: (u, v),
            point,
            distance: point.distance(self.p),
        }
    }

    fn keep(&mut self, f: Foot) {
        if f.distance.is_finite() {
            self.best = self.best.min(f.distance);
            self.feet.push(f);
        }
    }

    /// `x` kept in the domain along direction `k`: clamped where it ends,
    /// as it is where it wraps.
    fn inside(&self, k: usize, x: f64) -> f64 {
        let period = if k == 0 {
            self.periods.0
        } else {
            self.periods.1
        };
        let [lo, hi] = self.domain[k];
        if period.is_some() { x } else { x.clamp(lo, hi) }
    }

    /// Newton's method on the gradient of `f` from `(u, v)`, the
    /// parameters free where `free` says (a domain edge holds one), each
    /// step halved until `f` falls; Gauss-Newton where the Hessian isn't
    /// positive definite.
    fn newton(&self, (mut u, mut v): (f64, f64), free: [bool; 2]) -> Foot {
        let f_at = |u: f64, v: f64| {
            let q = self.sheet.point(u, v);
            (q - self.p).length_squared()
        };
        let mut f = f_at(u, v);
        for _ in 0..NEWTON_STEPS {
            let j = self.sheet.jets_at(u, v);
            let e = (Point3::new(j[0].v, j[1].v, j[2].v) - self.p).to_array();
            let su = vec(&j, |x| x.d[0]);
            let sv = vec(&j, |x| x.d[1]);
            let suu = vec(&j, |x| x.h[0]);
            let suv = vec(&j, |x| x.h[1]);
            let svv = vec(&j, |x| x.h[2]);
            let g = [dot(e, su), dot(e, sv)];
            let gn = [[dot(su, su), dot(su, sv)], [dot(su, sv), dot(sv, sv)]];
            let h = [
                [gn[0][0] + dot(e, suu), gn[0][1] + dot(e, suv)],
                [gn[1][0] + dot(e, suv), gn[1][1] + dot(e, svv)],
            ];
            let solve = |m: [[f64; 2]; 2]| -> Option<[f64; 2]> {
                match free {
                    [true, true] => {
                        let det = m[0][0] * m[1][1] - m[0][1] * m[1][0];
                        (m[0][0] > 0.0 && det > 0.0).then(|| {
                            [
                                -(m[1][1] * g[0] - m[0][1] * g[1]) / det,
                                -(m[0][0] * g[1] - m[1][0] * g[0]) / det,
                            ]
                        })
                    }
                    [true, false] => (m[0][0] > 0.0).then(|| [-g[0] / m[0][0], 0.0]),
                    [false, true] => (m[1][1] > 0.0).then(|| [0.0, -g[1] / m[1][1]]),
                    [false, false] => None,
                }
            };
            let Some(mut step) = solve(h).or_else(|| solve(gn)) else {
                break;
            };
            let mut moved = false;
            for _ in 0..30 {
                let (nu, nv) = (self.inside(0, u + step[0]), self.inside(1, v + step[1]));
                let nf = f_at(nu, nv);
                if nf < f {
                    (u, v, f) = (nu, nv, nf);
                    moved = true;
                    break;
                }
                step = [step[0] / 2.0, step[1] / 2.0];
            }
            if !moved {
                break;
            }
        }
        self.foot((u, v))
    }

    /// Enclosures of the point's jets over a cell.
    fn enclose(&self, cell: Cell) -> Option<JVec<Iv>> {
        self.sheet.jet_box(cell)
    }

    /// The least distance from `p` to the box of the point's enclosure.
    fn lower(&self, q: &JVec<Iv>) -> f64 {
        let p = self.p.to_vector().to_array();
        let mut sum = 0.0;
        for k in 0..3 {
            let x = q[k].v;
            let gap = (x.lo() - p[k]).max(p[k] - x.hi()).max(0.0);
            sum += gap * gap;
        }
        crate::math::sqrt(sum) * (1.0 - 1e-12)
    }

    /// The gradient of `f` and its Jacobian over a cell, in the cell's
    /// enclosures.
    fn gradient(&self, q: &JVec<Iv>) -> ([Iv; 2], [[Iv; 2]; 2]) {
        let p = self.p.to_vector().to_array();
        let e: [Iv; 3] = core::array::from_fn(|k| q[k].v - Iv::point(p[k]));
        let su = vec(q, |x| x.d[0]);
        let sv = vec(q, |x| x.d[1]);
        let g = [dot(e, su), dot(e, sv)];
        let h = [
            [
                dot(su, su) + dot(e, vec(q, |x| x.h[0])),
                dot(su, sv) + dot(e, vec(q, |x| x.h[1])),
            ],
            [
                dot(su, sv) + dot(e, vec(q, |x| x.h[1])),
                dot(sv, sv) + dot(e, vec(q, |x| x.h[2])),
            ],
        ];
        (g, h)
    }

    /// The cell's length across each parameter in model space, from its
    /// enclosures (infinite where they fail).
    fn lengths(&self, cell: Cell, q: Option<&JVec<Iv>>) -> [f64; 2] {
        let w = [cell[0][1] - cell[0][0], cell[1][1] - cell[1][0]];
        match q {
            Some(q) => {
                let mag = |k: usize| {
                    let d = vec(q, |x| x.d[k]);
                    crate::math::sqrt(d.iter().map(|x| x.mag() * x.mag()).sum::<f64>())
                };
                [w[0] * mag(0), w[1] * mag(1)]
            }
            None => w,
        }
    }

    /// The search over cells of both parameters.
    fn cells(&mut self, starts: &[Cell]) {
        let mut stack: Vec<Cell> = starts.to_vec();
        stack.reverse();
        while let Some(cell) = stack.pop() {
            let q = self.enclose(cell);
            if let Some(q) = &q {
                if self.lower(q) > self.best + LINEAR_RESOLUTION {
                    continue;
                }
                let (g, h) = self.gradient(q);
                if g.iter().any(|x| x.lo() > 0.0 || x.hi() < 0.0) {
                    continue;
                }
                let mid = [
                    (cell[0][0] + cell[0][1]) / 2.0,
                    (cell[1][0] + cell[1][1]) / 2.0,
                ];
                let at = self
                    .enclose([[mid[0]; 2], [mid[1]; 2]])
                    .map(|m| self.gradient(&m).0);
                if let Some(at) = at {
                    match krawczyk2(at, h, cell) {
                        Some(false) => continue,
                        Some(true) => {
                            let f = self.newton((mid[0], mid[1]), [true, true]);
                            self.keep(f);
                            continue;
                        }
                        None => {}
                    }
                }
            }
            self.cells += 1;
            let len = self.lengths(cell, q.as_ref());
            // A cell this small can't hold a point much nearer than its
            // middle; nor can one past the cap be searched further.
            if self.cells >= MAX_CELLS || len[0] + len[1] <= SMALL {
                let mid = (
                    (cell[0][0] + cell[0][1]) / 2.0,
                    (cell[1][0] + cell[1][1]) / 2.0,
                );
                let f = self.newton(mid, [true, true]);
                self.keep(f);
                continue;
            }
            let k = usize::from(len[1] > len[0]);
            let m = (cell[k][0] + cell[k][1]) / 2.0;
            let (mut a, mut b) = (cell, cell);
            a[k][1] = m;
            b[k][0] = m;
            stack.push(b);
            stack.push(a);
        }
    }

    /// The search along the domain's edge where parameter `fixed` is `at`,
    /// over the given stretches of the other.
    fn edge(&mut self, fixed: usize, at: f64, starts: &[[f64; 2]]) {
        let free = 1 - fixed;
        let cell_of = |r: [f64; 2]| -> Cell {
            let mut c = [[at; 2]; 2];
            c[free] = r;
            c
        };
        let mut stack: Vec<[f64; 2]> = starts.to_vec();
        stack.reverse();
        while let Some(r) = stack.pop() {
            let cell = cell_of(r);
            let q = self.enclose(cell);
            if let Some(q) = &q {
                if self.lower(q) > self.best + LINEAR_RESOLUTION {
                    continue;
                }
                let (g, h) = self.gradient(q);
                let (g, h) = (g[free], h[free][free]);
                if g.lo() > 0.0 || g.hi() < 0.0 {
                    continue;
                }
                let m = (r[0] + r[1]) / 2.0;
                let mid = self
                    .enclose(cell_of([m, m]))
                    .map(|q| self.gradient(&q).0[free]);
                if let Some(gm) = mid {
                    // Krawczyk's test in one parameter.
                    let y = 1.0 / ((h.lo() + h.hi()) / 2.0);
                    if y.is_finite() {
                        let x = Iv::new(r[0], r[1]).expect("ordered") - Iv::point(m);
                        let k = Iv::point(m) - Iv::point(y) * gm
                            + (Iv::point(1.0) - Iv::point(y) * h) * x;
                        if k.hi() < r[0] || k.lo() > r[1] {
                            continue;
                        }
                        if k.lo() > r[0] && k.hi() < r[1] {
                            let mut start = [at; 2];
                            start[free] = m;
                            let mut free_ = [false; 2];
                            free_[free] = true;
                            let f = self.newton((start[0], start[1]), free_);
                            self.keep(f);
                            continue;
                        }
                    }
                }
            }
            self.cells += 1;
            let m = (r[0] + r[1]) / 2.0;
            let len = self.lengths(cell, q.as_ref())[free];
            if self.cells >= MAX_CELLS || len <= SMALL {
                let mut start = [at; 2];
                start[free] = m;
                let mut free_ = [false; 2];
                free_[free] = true;
                let f = self.newton((start[0], start[1]), free_);
                self.keep(f);
                continue;
            }
            stack.push([m, r[1]]);
            stack.push([r[0], m]);
        }
    }
}

/// The nearest point of `sheet` to `p` over `domain`, searched from the
/// cells `starts` (which cover the domain), with the guarantee described
/// in the module's notes: within [`LINEAR_RESOLUTION`] of the least
/// distance unless the search was capped, and among points that near the
/// lowest parameters. `periods` says which directions wrap.
pub(crate) fn nearest(
    sheet: &Sheet,
    domain: Cell,
    periods: (Option<f64>, Option<f64>),
    starts: &[Cell],
    p: Point3,
) -> Projection<(f64, f64), Point3> {
    let mut s = Search {
        sheet,
        domain,
        periods,
        p,
        feet: Vec::new(),
        best: f64::INFINITY,
        cells: 0,
    };
    for c in starts {
        let mid = ((c[0][0] + c[0][1]) / 2.0, (c[1][0] + c[1][1]) / 2.0);
        let f = s.newton(mid, [true, true]);
        s.keep(f);
    }
    // The domain's edges and corners where a direction ends.
    let ends = [periods.0.is_none(), periods.1.is_none()];
    for k in 0..2 {
        if !ends[k] {
            continue;
        }
        let other = 1 - k;
        let mut stretches: Vec<[f64; 2]> = starts.iter().map(|c| c[other]).collect();
        stretches.sort_by(|a, b| a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1])));
        stretches.dedup();
        for at in domain[k] {
            if !at.is_finite() {
                continue;
            }
            for &r in &stretches {
                for x in r {
                    let mut uv = [at; 2];
                    uv[other] = x;
                    let f = s.foot((uv[0], uv[1]));
                    s.keep(f);
                }
            }
            s.edge(k, at, &stretches);
        }
    }
    s.cells(starts);
    let feet = core::mem::take(&mut s.feet);
    let best = s.best;
    let foot = feet
        .into_iter()
        .filter(|f| f.distance <= best + LINEAR_RESOLUTION)
        .min_by(|a, b| a.uv.0.total_cmp(&b.uv.0).then(a.uv.1.total_cmp(&b.uv.1)))
        .unwrap_or_else(|| {
            let [[u0, u1], [v0, v1]] = domain;
            s.foot(((u0 + u1) / 2.0, (v0 + v1) / 2.0))
        });
    Projection {
        param: foot.uv,
        point: foot.point,
        distance: foot.distance,
    }
}

#[cfg(test)]
mod tests;
