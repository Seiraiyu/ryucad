//! An offset's NURBS form within a certified tolerance, for export: a
//! bicubic Hermite net over the offset's own parameters.

use super::{Cell, Offset, cut};
use crate::Error;
use crate::geom::{KnotVector, NurbsSurface, Point3, Reparam, Vector3};
use crate::interval::Interval;

/// The most rounds of halving a fit takes.
const MAX_ROUNDS: u32 = 16;

/// The most spans a fit takes along either direction.
const MAX_SPANS: usize = 1 << 10;

/// The parts a patch is first cut into each way for Taylor's bound.
const PIECES: usize = 2;

/// The deepest a part is quartered while bounding a patch.
const MAX_DEPTH: u32 = 6;

/// The most parts one patch is bounded over.
const MAX_PARTS: usize = 1 << 12;

/// The offset's value and first derivatives at a corner of the grid, and
/// its mixed derivative.
#[derive(Clone, Copy)]
struct Corner {
    p: Vector3,
    du: Vector3,
    dv: Vector3,
    duv: Vector3,
}

impl Offset {
    /// A NURBS within `tolerance` (at least
    /// [`crate::tol::LINEAR_RESOLUTION`]) of the offset over the box `[ru,
    /// rv]` of its parameters, on the same parameters (both
    /// [`Reparam::Identity`]), and the certified bound on how far it
    /// strays. An offset has no exact rational form
    /// ([`crate::geom::Surface::to_nurbs`] is `Degenerate`); this is for
    /// export.
    ///
    /// The NURBS is bicubic, with knots of multiplicity three at a grid of
    /// parameters (the offset's own cells' sides, halved): on each cell,
    /// the Hermite patch through the offset's value, first derivatives and
    /// mixed derivative at its corners, so neighbouring patches share
    /// their sides and meet with matching tangents. The error `e` of a
    /// patch is bounded over parts of it by Taylor's formula about each
    /// part's middle, `|e(m)| + |e_u(m)| r_u + |e_v(m)| r_v + (|e_uu| r_u²
    /// + 2 |e_uv| r_u r_v + |e_vv| r_v²) / 2`, the second derivatives
    /// enclosed over the part (the offset's by its jets, the patch's from
    /// its second differences). A part whose remainder alone keeps it past
    /// the tolerance is quartered, up to six times and 4,096 parts a patch;
    /// a patch whose error at a part's middle passes half the tolerance, or
    /// that runs out of parts, is halved (its row or column of the grid),
    /// up to 16 rounds and 1,024 spans each way. Past that, or for a box
    /// outside a bounded direction's domain or wider than a period, the
    /// call fails with `Degenerate`. Its cost grows with how tightly the
    /// offset bends: a waved spline offset by a tenth of its size takes
    /// seconds at a relative `1e-5`.
    pub fn to_nurbs_within(
        &self,
        [ru, rv]: Cell,
        tolerance: f64,
    ) -> Result<(NurbsSurface, [Reparam; 2], f64), Error> {
        let tolerance = tolerance.max(crate::tol::LINEAR_RESOLUTION);
        let (pu, pv) = self.base.periods();
        let [du, dv] = self.domain;
        let fits = |r: [f64; 2], d: [f64; 2], p: Option<f64>| {
            r[0] < r[1]
                && match p {
                    Some(p) if d[1] - d[0] >= p => r[1] - r[0] <= p,
                    _ => d[0] <= r[0] && r[1] <= d[1],
                }
        };
        if !(fits(ru, du, pu) && fits(rv, dv, pv)) {
            return Err(Error::Degenerate);
        }
        // The offset's own cells' sides inside the box start the grid.
        let mut us = vec![ru[0], ru[1]];
        let mut vs = vec![rv[0], rv[1]];
        for c in &self.cells {
            for u in cut(c[0], ru, pu) {
                us.extend(u);
            }
            for v in cut(c[1], rv, pv) {
                vs.extend(v);
            }
        }
        let tidy = |x: &mut Vec<f64>, [a, b]: [f64; 2]| {
            x.retain(|&t| a <= t && t <= b);
            x.sort_by(f64::total_cmp);
            x.dedup();
        };
        tidy(&mut us, ru);
        tidy(&mut vs, rv);
        // Cells certified in an earlier round, by their sides: a patch
        // depends only on its corners, so one whose sides didn't move is
        // the same patch.
        let mut known: std::collections::BTreeMap<[u64; 4], f64> =
            std::collections::BTreeMap::new();
        for _ in 0..MAX_ROUNDS {
            let net = Net::new(self, &us, &vs);
            let mut halve_u = vec![false; us.len() - 1];
            let mut halve_v = vec![false; vs.len() - 1];
            let mut worst = 0.0f64;
            let mut ok = true;
            for i in 0..us.len() - 1 {
                for j in 0..vs.len() - 1 {
                    let key = [us[i], us[i + 1], vs[j], vs[j + 1]].map(f64::to_bits);
                    if let Some(&b) = known.get(&key) {
                        worst = worst.max(b);
                        continue;
                    }
                    match net.bound(self, i, j, tolerance) {
                        Some(Ok(b)) => {
                            known.insert(key, b);
                            worst = worst.max(b);
                        }
                        Some(Err(k)) => {
                            ok = false;
                            if k == 0 {
                                halve_u[i] = true;
                            } else {
                                halve_v[j] = true;
                            }
                        }
                        None => return Err(Error::Degenerate),
                    }
                }
            }
            if ok {
                return Ok((net.surface()?, [Reparam::Identity; 2], worst));
            }
            us = halved(&us, &halve_u);
            vs = halved(&vs, &halve_v);
            if us.len() > MAX_SPANS || vs.len() > MAX_SPANS {
                break;
            }
        }
        Err(Error::Degenerate)
    }

    /// The value, first derivatives and mixed derivative at `(u, v)`.
    fn corner(&self, u: f64, v: f64) -> Corner {
        let j = self.jets_at(u, v);
        let part = |f: &dyn Fn(usize) -> f64| Vector3::new(f(0), f(1), f(2));
        Corner {
            p: part(&|k| j[k].v),
            du: part(&|k| j[k].d[0]),
            dv: part(&|k| j[k].d[1]),
            duv: part(&|k| j[k].h[1]),
        }
    }
}

/// The grid's parameters with every flagged span halved.
fn halved(x: &[f64], flags: &[bool]) -> Vec<f64> {
    let mut out = vec![x[0]];
    for (k, w) in x.windows(2).enumerate() {
        if flags[k] {
            out.push((w[0] + w[1]) / 2.0);
        }
        out.push(w[1]);
    }
    out
}

/// A Hermite net over a grid: the offset's data at every corner and the
/// bicubic control points they give.
struct Net<'a> {
    us: &'a [f64],
    vs: &'a [f64],
    /// Control points, `3 × spans + 1` each way, row-major along `u`.
    points: Vec<Vector3>,
}

impl<'a> Net<'a> {
    fn new(o: &Offset, us: &'a [f64], vs: &'a [f64]) -> Self {
        let corners: Vec<Corner> = us
            .iter()
            .flat_map(|&u| vs.iter().map(move |&v| (u, v)))
            .map(|(u, v)| o.corner(u, v))
            .collect();
        let (nu, nv) = (3 * (us.len() - 1) + 1, 3 * (vs.len() - 1) + 1);
        // A control point is its nearest corner's value moved a third of
        // the span towards it along each direction it is off the corner,
        // with the mixed derivative for the product.
        let off = |x: &[f64], i: usize| -> (usize, f64) {
            let (c, r) = (i / 3, i % 3);
            match r {
                0 => (c, 0.0),
                1 => (c, (x[c + 1] - x[c]) / 3.0),
                _ => (c + 1, -(x[c + 1] - x[c]) / 3.0),
            }
        };
        let mut points = Vec::with_capacity(nu * nv);
        for i in 0..nu {
            let (cu, a) = off(us, i);
            for j in 0..nv {
                let (cv, b) = off(vs, j);
                let k = &corners[cu * vs.len() + cv];
                points.push(k.p + k.du * a + k.dv * b + k.duv * (a * b));
            }
        }
        Self { us, vs, points }
    }

    fn nv(&self) -> usize {
        3 * (self.vs.len() - 1) + 1
    }

    /// The control points of the patch over cell `(i, j)`.
    fn patch(&self, i: usize, j: usize) -> [[Vector3; 4]; 4] {
        let nv = self.nv();
        core::array::from_fn(|a| {
            core::array::from_fn(|b| self.points[(3 * i + a) * nv + 3 * j + b])
        })
    }

    /// The certified bound on the patch's distance from the offset over
    /// cell `(i, j)`, if it is within `tolerance`: `Ok` with the bound, or
    /// `Err` with the direction (0 along `u`) to halve the cell in: where
    /// the patch itself strays too far (the error and its gradient at a
    /// part's middle alone pass half the tolerance), the way its derivative
    /// strays more; where its parts can't be bounded within the budget, its
    /// longer side in model space. Parts start as a 2 × 2 grid and are
    /// quartered where only Taylor's remainder, which shrinks with them,
    /// keeps the bound past the tolerance. `None` where the offset can't
    /// be enclosed.
    fn bound(&self, o: &Offset, i: usize, j: usize, tolerance: f64) -> Option<Result<f64, usize>> {
        let p = self.patch(i, j);
        let (u0, u1) = (self.us[i], self.us[i + 1]);
        let (v0, v1) = (self.vs[j], self.vs[j + 1]);
        let (hu, hv) = (u1 - u0, v1 - v0);
        // The longer side in model space, from the patch's middle.
        let longer = {
            let (_, d) = patch_at(&p, 0.5, 0.5);
            let len = |v: [f64; 3]| crate::math::sqrt(v[0] * v[0] + v[1] * v[1] + v[2] * v[2]);
            usize::from(len(d[1]) > len(d[0]))
        };
        let k = Interval::point;
        let step = 1.0 / PIECES as f64;
        let mut stack: Vec<([f64; 2], [f64; 2], u32)> = (0..PIECES * PIECES)
            .map(|n| {
                let (a, b) = ((n / PIECES) as f64, (n % PIECES) as f64);
                (
                    [a * step, (a + 1.0) * step],
                    [b * step, (b + 1.0) * step],
                    0,
                )
            })
            .collect();
        let mut seen = 0usize;
        let mut worst = 0.0f64;
        while let Some((x, y, depth)) = stack.pop() {
            seen += 1;
            if seen > MAX_PARTS {
                return Some(Err(longer));
            }
            let (xm, ym) = ((x[0] + x[1]) / 2.0, (y[0] + y[1]) / 2.0);
            let (ru, rv) = (hu * (x[1] - x[0]) / 2.0, hv * (y[1] - y[0]) / 2.0);
            let truth = o.jets_at(u0 + hu * xm, v0 + hv * ym);
            let (h, hd) = patch_at(&p, xm, ym);
            // The error and its gradient at the middle, in floating point
            // (their rounding is far below any tolerance), then Taylor's
            // remainder over the part.
            let mut near = [0.0f64; 3];
            let mut strays = [0.0f64; 2];
            for c in 0..3 {
                let e0 = (h[c] - truth[c].v).abs();
                let eu = (hd[0][c] / hu - truth[c].d[0]).abs();
                let ev = (hd[1][c] / hv - truth[c].d[1]).abs();
                near[c] = e0 + eu * ru + ev * rv;
                strays[0] += eu * hu;
                strays[1] += ev * hv;
            }
            let near_len = crate::math::sqrt(near.iter().map(|x| x * x).sum());
            if near_len > tolerance / 2.0 {
                // Halved the way its derivative strays more over the span.
                return Some(Err(usize::from(strays[1] > strays[0])));
            }
            let part = [
                [u0 + hu * x[0], u0 + hu * x[1]],
                [v0 + hv * y[0], v0 + hv * y[1]],
            ];
            let q = o.jet_box(part)?;
            let h2 = patch_second(&p, Interval::new(x[0], x[1])?, Interval::new(y[0], y[1])?);
            let mut sum = 0.0;
            for c in 0..3 {
                let euu = mag(h2[0][c] * k(1.0 / (hu * hu)) - q[c].h[0]);
                let euv = mag(h2[1][c] * k(1.0 / (hu * hv)) - q[c].h[1]);
                let evv = mag(h2[2][c] * k(1.0 / (hv * hv)) - q[c].h[2]);
                let t = k(near[c])
                    + k(0.5)
                        * (k(euu) * k(ru * ru) + k(2.0 * euv) * k(ru * rv) + k(evv) * k(rv * rv));
                sum += t.hi() * t.hi();
            }
            // The rounding of the floating point terms, generously.
            let bound = crate::math::sqrt(sum) * (1.0 + 1e-12) + 1e-15;
            if bound <= tolerance {
                worst = worst.max(bound);
                continue;
            }
            if depth == MAX_DEPTH {
                return Some(Err(longer));
            }
            let (xs, ys) = ([x[0], xm, x[1]], [y[0], ym, y[1]]);
            for a in 0..2 {
                for b in 0..2 {
                    stack.push(([xs[a], xs[a + 1]], [ys[b], ys[b + 1]], depth + 1));
                }
            }
        }
        Some(Ok(worst))
    }

    /// The NURBS: degree 3 each way, knots at the grid, of multiplicity
    /// three inside and four at the ends.
    fn surface(&self) -> Result<NurbsSurface, Error> {
        let knots = |x: &[f64]| {
            let mut m = vec![3; x.len()];
            m[0] = 4;
            *m.last_mut().expect("two parameters") = 4;
            KnotVector::clamped(3, x, &m)
        };
        let points = self
            .points
            .iter()
            .map(|v| Point3::new(v.x, v.y, v.z))
            .collect();
        NurbsSurface::new([knots(self.us)?, knots(self.vs)?], points, None)
    }
}

/// The greatest magnitude of an interval.
fn mag(x: Interval) -> f64 {
    x.lo().abs().max(x.hi().abs())
}

/// The cubic Bernstein basis and its derivative at `t`.
fn basis(t: f64) -> ([f64; 4], [f64; 4]) {
    let s = 1.0 - t;
    (
        [s * s * s, 3.0 * t * s * s, 3.0 * t * t * s, t * t * t],
        [
            -3.0 * s * s,
            3.0 * s * s - 6.0 * t * s,
            6.0 * t * s - 3.0 * t * t,
            3.0 * t * t,
        ],
    )
}

/// A bicubic patch's point and its derivatives by its local parameters at
/// `(x, y)` of `[0, 1]²`.
fn patch_at(p: &[[Vector3; 4]; 4], x: f64, y: f64) -> ([f64; 3], [[f64; 3]; 2]) {
    let (bx, dx) = basis(x);
    let (by, dy) = basis(y);
    let mut v = Vector3::ZERO;
    let (mut du, mut dv) = (Vector3::ZERO, Vector3::ZERO);
    for a in 0..4 {
        for b in 0..4 {
            v = v + p[a][b] * (bx[a] * by[b]);
            du = du + p[a][b] * (dx[a] * by[b]);
            dv = dv + p[a][b] * (bx[a] * dy[b]);
        }
    }
    (v.to_array(), [du.to_array(), dv.to_array()])
}

/// The Bernstein basis of degree `n` (up to 3), enclosed over an interval
/// of `t`, written in `t` and `s = 1 - t`.
fn bernstein(n: usize, t: Interval) -> Vec<Interval> {
    let k = Interval::point;
    let s = k(1.0) - t;
    match n {
        1 => vec![s, t],
        2 => vec![s * s, k(2.0) * t * s, t * t],
        _ => vec![s * s * s, k(3.0) * t * s * s, k(3.0) * t * t * s, t * t * t],
    }
}

/// Enclosures of a bicubic patch's second derivatives by its local
/// parameters, `[uu, uv, vv]`, over a box of them: each the Bézier form
/// of lower degree on the patch's second differences, whose small
/// coefficients keep the enclosure as narrow as the derivative is.
fn patch_second(p: &[[Vector3; 4]; 4], x: Interval, y: Interval) -> [[Interval; 3]; 3] {
    let (b1x, b2x, b3x) = (bernstein(1, x), bernstein(2, x), bernstein(3, x));
    let (b1y, b2y, b3y) = (bernstein(1, y), bernstein(2, y), bernstein(3, y));
    let mut out = [[Interval::point(0.0); 3]; 3];
    let mut add = |m: usize, c: Vector3, w: Interval| {
        let c = c.to_array();
        for k in 0..3 {
            out[m][k] = out[m][k] + Interval::point(c[k]) * w;
        }
    };
    for b in 0..4 {
        for a in 0..2 {
            let d = (p[a + 2][b] - p[a + 1][b] * 2.0 + p[a][b]) * 6.0;
            add(0, d, b1x[a] * b3y[b]);
        }
    }
    for a in 0..3 {
        for b in 0..3 {
            let d = (p[a + 1][b + 1] - p[a + 1][b] - p[a][b + 1] + p[a][b]) * 9.0;
            add(1, d, b2x[a] * b2y[b]);
        }
    }
    for a in 0..4 {
        for b in 0..2 {
            let d = (p[a][b + 2] - p[a][b + 1] * 2.0 + p[a][b]) * 6.0;
            add(2, d, b3x[a] * b1y[b]);
        }
    }
    out
}
