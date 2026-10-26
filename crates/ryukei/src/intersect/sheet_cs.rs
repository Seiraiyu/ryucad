//! A curve against any surface: certified subdivision.
//!
//! A curve point `C(t)` lies on a surface `S(u, v)` exactly when
//! `E(t, u, v) = C(t) - S(u, v)` vanishes: three equations in three
//! unknowns. Boxes of `(t, u, v)`, from sixteenths of the curve's
//! interval and the surface's first cells near the curve
//! ([`super::cells`]), are enclosed in interval arithmetic (the curve's
//! and the surface's jets over them, narrowed by the mean value form) and
//! halved along whichever parameter's width adds most to the enclosures
//! until each is excluded (a component of `E` of one certified sign),
//! holds exactly one solution (Krawczyk's test) where the curve crosses
//! the surface (`C' · (S_u × S_v)` of one certified sign), or is a
//! contact: the curve over its stretch of `t` certified within the
//! tolerance of the surface.
//!
//! A contact is certified along a path rather than over the box, so a
//! curve lying on the surface needs boxes only as short as its curvature
//! allows, not as short as the tolerance: from the foot `(u₀, v₀)` of the
//! stretch's middle `t₀`, the path `(u, v) = (u₀, v₀) + (a, b)(t - t₀)`
//! follows the curve's tangent (`(a, b)` its least-squares solution of
//! `S_u a + S_v b = C'`), and `g(t) = C(t) - S(u(t), v(t))` is within
//! `|g(t₀)| + |g'(t₀)| h + |g''| h² / 2` of zero, `h` the stretch's
//! half-width and `|g''|` enclosed over the stretch and the path's box.
//!
//! Hits follow [`super::curve_surface`]'s rules: contacts closer than the
//! tolerance are one, a contact shorter than 100 tolerances is a touch,
//! hits within the tolerance of an end of the interval are at it, and
//! sides (the natural normal's side is `Positive`) come from certified
//! signs of `(q - S) · (S_u × S_v)` at the foot of a point beside the hit.

use super::Failure;
use super::cells::cells;
use super::cs::{CurveHit, Side};
use crate::geom::bound2::IVec3;
use crate::geom::sheet::Sheet;
use crate::geom::{Curve, Point3, Surface};
use crate::interval::Interval;
use crate::predicates::Sign;
use crate::tol::LINEAR_RESOLUTION;

/// A contact shorter than this many tolerances is a touch.
const TOUCH: f64 = 100.0;

/// The most times a box of `(t, u, v)` is halved, one parameter at a
/// time.
const MAX_DEPTH: u32 = 120;

/// The most boxes a search may examine.
const MAX_BOXES: usize = 1 << 20;

/// The most boxes the search for a tangency in a contact may examine.
const TANGENT_BOXES: usize = 512;

type Iv = Interval;

/// A box of `[t, u, v]`.
pub(crate) type Box3p = [[f64; 2]; 3];

fn signed(x: Iv) -> bool {
    x.lo() > 0.0 || x.hi() < 0.0
}

fn meet(a: Iv, b: Iv) -> Iv {
    Iv::new(a.lo().max(b.lo()), a.hi().min(b.hi())).unwrap_or(a)
}

fn dot(a: IVec3, b: IVec3) -> Iv {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: IVec3, b: IVec3) -> IVec3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// An upper bound on the length of every vector in an interval box.
pub(crate) fn norm_hi(a: IVec3) -> f64 {
    let s = a.iter().fold(Iv::point(0.0), |acc, &x| {
        let m = Iv::point(x.mag());
        acc + m * m
    });
    s.sqrt().map_or(f64::INFINITY, Iv::hi)
}

/// Krawczyk's test for a zero of three functions over a box, from their
/// values at its middle and their Jacobian over it: `Some(true)` for
/// exactly one, `Some(false)` for none, `None` undecided.
pub(crate) fn krawczyk3(at: [Iv; 3], jac: [[Iv; 3]; 3], cell: Box3p) -> Option<bool> {
    let p = Iv::point;
    let mid = |x: Iv| (x.lo() + x.hi()) / 2.0;
    let m = jac.map(|r| r.map(mid));
    let cof =
        |r0: usize, r1: usize, c0: usize, c1: usize| m[r0][c0] * m[r1][c1] - m[r0][c1] * m[r1][c0];
    let det = m[0][0] * cof(1, 2, 1, 2) - m[0][1] * cof(1, 2, 0, 2) + m[0][2] * cof(1, 2, 0, 1);
    if det == 0.0 || !det.is_finite() {
        return None;
    }
    // The inverse of the middle Jacobian, by its adjugate.
    let y = [
        [cof(1, 2, 1, 2), -cof(0, 2, 1, 2), cof(0, 1, 1, 2)],
        [-cof(1, 2, 0, 2), cof(0, 2, 0, 2), -cof(0, 1, 0, 2)],
        [cof(1, 2, 0, 1), -cof(0, 2, 0, 1), cof(0, 1, 0, 1)],
    ]
    .map(|r| r.map(|x| x / det));
    if y.iter().flatten().any(|x| !x.is_finite()) {
        return None;
    }
    let x: [f64; 3] = cell.map(|[a, b]| (a + b) / 2.0);
    let boxes = [
        Iv::new(cell[0][0], cell[0][1])? - p(x[0]),
        Iv::new(cell[1][0], cell[1][1])? - p(x[1]),
        Iv::new(cell[2][0], cell[2][1])? - p(x[2]),
    ];
    let (mut inside, mut apart) = (true, false);
    for row in 0..3 {
        let mut k =
            p(x[row]) - (p(y[row][0]) * at[0] + p(y[row][1]) * at[1] + p(y[row][2]) * at[2]);
        for (col, &b) in boxes.iter().enumerate() {
            let ident = if row == col { 1.0 } else { 0.0 };
            let e = p(ident)
                - (p(y[row][0]) * jac[0][col]
                    + p(y[row][1]) * jac[1][col]
                    + p(y[row][2]) * jac[2][col]);
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

/// Every place where `curve` over `[t0, t1]` meets `surface`, in
/// increasing parameter, as [`super::curve_surface`] gives them for an
/// analytic surface. Fails with `Uncertain` where a stretch is neither
/// certified apart from the surface nor within the tolerance of it.
pub(crate) fn curve_sheet(
    curve: &Curve,
    [t0, t1]: [f64; 2],
    surface: &Surface,
    tolerance: f64,
) -> Result<Vec<CurveHit>, Failure> {
    let tolerance = tolerance.max(LINEAR_RESOLUTION);
    let s = Search {
        curve,
        surface,
        sheet: Sheet::of(surface),
        range: [t0, t1],
        tolerance,
    };
    let near = curve.bounds([t0, t1]).grow(tolerance);
    let mut found = Vec::new();
    let mut budget = MAX_BOXES;
    let uv = cells(surface, &near);
    for ts in s.t_cells() {
        for &[u, v] in &uv {
            s.search([ts, u, v], 0, &mut budget, &mut found)?;
        }
    }
    // An end within tolerance of the surface touches it.
    for t in [t0, t1] {
        if s.path_bound([t, t], None) <= tolerance {
            found.push(Candidate {
                t: [t, t],
                simple: None,
            });
        }
    }
    let merged = s.merge(found);
    let mut out = Vec::with_capacity(merged.len());
    for (k, c) in merged.iter().enumerate() {
        let lo = if k == 0 { t0 } else { merged[k - 1].t[1] };
        let hi = merged.get(k + 1).map_or(t1, |n| n.t[0]);
        out.push(s.classify(c, [lo, hi])?);
    }
    Ok(out)
}

/// A stretch of the curve's parameter holding a hit: one simple
/// crossing (`Some(rising)`, from the negative side to the positive when
/// rising), or a stretch within the tolerance.
#[derive(Clone, Copy, Debug)]
struct Candidate {
    t: [f64; 2],
    simple: Option<bool>,
}

/// A path on the surface following the curve about a parameter: its
/// foot, and its rates to first and second order.
#[derive(Clone, Copy, Debug)]
struct Path {
    uv: (f64, f64),
    first: (f64, f64),
    second: (f64, f64),
}

/// Enclosures of `E`, its Jacobian, and `C' · (S_u × S_v)`, over a box.
struct Enc {
    e: [Iv; 3],
    jac: [[Iv; 3]; 3],
    slope: Iv,
}

struct Search<'a> {
    curve: &'a Curve,
    surface: &'a Surface,
    sheet: Sheet<'a>,
    range: [f64; 2],
    tolerance: f64,
}

impl Search<'_> {
    /// The first cuts of the curve's interval: sixteenths.
    fn t_cells(&self) -> Vec<[f64; 2]> {
        let [t0, t1] = self.range;
        let at = |k: u32| {
            if k == 16 {
                t1
            } else {
                t0 + (t1 - t0) * f64::from(k) / 16.0
            }
        };
        (0..16).map(|i| [at(i), at(i + 1)]).collect()
    }

    /// The enclosures over a box, without the mean value form.
    fn raw(&self, [rt, ru, rv]: Box3p) -> Option<Enc> {
        let c = self.curve.jet_box(rt);
        let p = self.sheet.jet_box([ru, rv])?;
        let su = [p[0].d[0], p[1].d[0], p[2].d[0]];
        let sv = [p[0].d[1], p[1].d[1], p[2].d[1]];
        Some(Enc {
            e: core::array::from_fn(|k| c[0][k] - p[k].v),
            jac: core::array::from_fn(|k| [c[1][k], -p[k].d[0], -p[k].d[1]]),
            slope: dot(c[1], cross(su, sv)),
        })
    }

    /// The enclosures over a box, `E` narrowed by the mean value form
    /// about its middle.
    fn enclose(&self, cell: Box3p) -> Option<Enc> {
        let mut out = self.raw(cell)?;
        let m: [f64; 3] = cell.map(|[a, b]| (a + b) / 2.0);
        let at = self.raw(m.map(|x| [x, x]))?;
        let d: [Iv; 3] = [
            Iv::new(cell[0][0] - m[0], cell[0][1] - m[0])?,
            Iv::new(cell[1][0] - m[1], cell[1][1] - m[1])?,
            Iv::new(cell[2][0] - m[2], cell[2][1] - m[2])?,
        ];
        for k in 0..3 {
            let lin = at.e[k] + out.jac[k][0] * d[0] + out.jac[k][1] * d[1] + out.jac[k][2] * d[2];
            out.e[k] = meet(out.e[k], lin);
        }
        Some(out)
    }

    fn search(
        &self,
        cell: Box3p,
        depth: u32,
        budget: &mut usize,
        out: &mut Vec<Candidate>,
    ) -> Result<(), Failure> {
        *budget = budget.checked_sub(1).ok_or(Failure::Uncertain)?;
        let enc = self.enclose(cell);
        if let Some(c) = &enc {
            if c.e.iter().any(|&x| signed(x)) {
                return Ok(());
            }
            let m: [f64; 3] = cell.map(|[a, b]| (a + b) / 2.0);
            if let Some(at) = self.raw(m.map(|x| [x, x])) {
                match krawczyk3(at.e, c.jac, cell) {
                    Some(false) => return Ok(()),
                    Some(true) if signed(c.slope) => {
                        let t = self.root(cell);
                        out.push(Candidate {
                            t: [t, t],
                            simple: Some(c.slope.lo() > 0.0),
                        });
                        return Ok(());
                    }
                    _ => {}
                }
            }
            let mid = (
                (cell[1][0] + cell[1][1]) / 2.0,
                (cell[2][0] + cell[2][1]) / 2.0,
            );
            if self.path_bound(cell[0], Some(mid)) <= self.tolerance {
                out.push(Candidate {
                    t: cell[0],
                    simple: None,
                });
                return Ok(());
            }
        }
        if depth == MAX_DEPTH {
            return Err(Failure::Uncertain);
        }
        // Halve the parameter whose width adds most to the enclosures.
        let width = |k: usize| cell[k][1] - cell[k][0];
        let spread = |k: usize| match &enc {
            Some(c) => (0..3).map(|r| c.jac[r][k].mag()).fold(0.0, f64::max) * width(k),
            None => width(k),
        };
        let k = (0..3)
            .max_by(|&a, &b| spread(a).total_cmp(&spread(b)))
            .expect("three parameters");
        let [a, b] = cell[k];
        let m = (a + b) / 2.0;
        if m <= a || m >= b {
            return Err(Failure::Uncertain);
        }
        for half in [[a, m], [m, b]] {
            let mut c = cell;
            c[k] = half;
            self.search(c, depth + 1, budget, out)?;
        }
        Ok(())
    }

    /// The solution in a box certified to hold one, by Newton's method on
    /// `E` from its middle, kept inside the box.
    fn root(&self, cell: Box3p) -> f64 {
        let mut x: [f64; 3] = cell.map(|[a, b]| (a + b) / 2.0);
        for _ in 0..40 {
            let c = self.curve.jet3(x[0]);
            let j = self.sheet.jets_at(x[1], x[2]);
            let e: [f64; 3] = core::array::from_fn(|k| c[0].to_array()[k] - j[k].v);
            let m: [[f64; 3]; 3] =
                core::array::from_fn(|k| [c[1].to_array()[k], -j[k].d[0], -j[k].d[1]]);
            let Some(dx) = solve3(m, e) else { break };
            let next: [f64; 3] =
                core::array::from_fn(|k| (x[k] - dx[k]).clamp(cell[k][0], cell[k][1]));
            if next == x {
                break;
            }
            x = next;
        }
        x[0]
    }

    /// The path on the surface that follows the curve about `t`: the
    /// foot `(u, v)` of the curve's point, and rates chosen so that the
    /// path's point matches the curve to second order where it can,
    /// `(a, b)` the least-squares solution of `S_u a + S_v b = C'` and
    /// `(a₂, b₂)` of `S_u a₂ + S_v b₂ = C'' - (S_uu a² + 2 S_uv a b +
    /// S_vv b²)`. `None` where the surface has no tangent plane at the
    /// foot.
    fn foot_path(&self, t: f64, near: Option<(f64, f64)>) -> Option<Path> {
        let q = self.curve.jet3(t);
        let (um, vm) = match near {
            Some(uv) => self.newton_foot(Point3::ORIGIN + q[0], uv),
            None => self.surface.project(Point3::ORIGIN + q[0]).param,
        };
        let at = self.sheet.jets_at(um, vm);
        let su = at.map(|j| j.d[0]);
        let sv = at.map(|j| j.d[1]);
        let d3 = |a: [f64; 3], b: [f64; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
        let (g11, g12, g22) = (d3(su, su), d3(su, sv), d3(sv, sv));
        let det = g11 * g22 - g12 * g12;
        if !(det > 0.0 && det.is_finite()) {
            return None;
        }
        let fit = |r: [f64; 3]| {
            let (r1, r2) = (d3(su, r), d3(sv, r));
            ((g22 * r1 - g12 * r2) / det, (g11 * r2 - g12 * r1) / det)
        };
        let (a, b) = fit(q[1].to_array());
        let c2 = q[2].to_array();
        let r: [f64; 3] = core::array::from_fn(|k| {
            c2[k] - (at[k].h[0] * a * a + 2.0 * at[k].h[1] * a * b + at[k].h[2] * b * b)
        });
        let (a2, b2) = fit(r);
        let all = [um, vm, a, b, a2, b2];
        all.iter().all(|x| x.is_finite()).then_some(Path {
            uv: (um, vm),
            first: (a, b),
            second: (a2, b2),
        })
    }

    /// A foot of `p` near `(u, v)`: Gauss-Newton steps on `|p - S|`,
    /// kept inside a bounded domain. Any foot will do for a certificate;
    /// a near one makes its bound tight.
    fn newton_foot(&self, p: Point3, (mut u, mut v): (f64, f64)) -> (f64, f64) {
        let [du, dv] = self.surface.domain();
        let (pu, pv) = self.surface.periods();
        let keep = |x: f64, d: [f64; 2], period: Option<f64>| {
            if period.is_some() {
                x
            } else {
                x.clamp(d[0], d[1])
            }
        };
        for _ in 0..8 {
            let j = self.sheet.jets_at(u, v);
            let w: [f64; 3] = core::array::from_fn(|k| p.to_vector().to_array()[k] - j[k].v);
            let su = j.map(|x| x.d[0]);
            let sv = j.map(|x| x.d[1]);
            let d3 = |a: [f64; 3], b: [f64; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
            let (g11, g12, g22) = (d3(su, su), d3(su, sv), d3(sv, sv));
            let det = g11 * g22 - g12 * g12;
            if !(det > 0.0 && det.is_finite()) {
                break;
            }
            let (r1, r2) = (d3(su, w), d3(sv, w));
            let (a, b) = ((g22 * r1 - g12 * r2) / det, (g11 * r2 - g12 * r1) / det);
            let (nu, nv) = (keep(u + a, du, pu), keep(v + b, dv, pv));
            if !(nu.is_finite() && nv.is_finite()) || (nu == u && nv == v) {
                break;
            }
            (u, v) = (nu, nv);
        }
        (u, v)
    }

    /// The parameter of a tangency in a contact: the one place in the
    /// stretch (widened by itself each way) where the curve runs along the
    /// surface's tangent plane at its foot, `G = ((C - S) · S_u, (C - S) ·
    /// S_v, C' · (S_u × S_v)) = 0` (the derivative of the signed distance
    /// vanishes; the squared distance's would be a degenerate root where
    /// the curve touches the surface), certified by Krawczyk's test over
    /// boxes about the path the contact follows.
    fn tangent(&self, c: &Candidate) -> Option<f64> {
        let [lo, hi] = self.range;
        let w = c.t[1] - c.t[0];
        let rt = [(c.t[0] - w).max(lo), (c.t[1] + w).min(hi)];
        let tm = (c.t[0] + c.t[1]) / 2.0;
        let Path {
            uv: (um, vm),
            first: (da, db),
            ..
        } = self.foot_path(tm, None)?;
        let reach = (rt[1] - rt[0]).max(1e-300);
        let pad = |x: f64, k: f64| k.abs() * reach + 1e-9 * (1.0 + x.abs()) + reach;
        let (hu, hv) = (pad(um, da), pad(vm, db));
        let cell = [rt, [um - hu, um + hu], [vm - hv, vm + hv]];
        let enclose = |b: Box3p| -> Option<([Iv; 3], [[Iv; 3]; 3])> {
            let cv = self.curve.jet_box(b[0]);
            let p = self.sheet.jet_box([b[1], b[2]])?;
            let w: IVec3 = core::array::from_fn(|k| cv[0][k] - p[k].v);
            let su: IVec3 = core::array::from_fn(|k| p[k].d[0]);
            let sv: IVec3 = core::array::from_fn(|k| p[k].d[1]);
            let suu: IVec3 = core::array::from_fn(|k| p[k].h[0]);
            let suv: IVec3 = core::array::from_fn(|k| p[k].h[1]);
            let svv: IVec3 = core::array::from_fn(|k| p[k].h[2]);
            let (c1, c2) = (cv[1], cv[2]);
            let n = cross(su, sv);
            let nu: IVec3 = core::array::from_fn(|k| cross(suu, sv)[k] + cross(su, suv)[k]);
            let nv: IVec3 = core::array::from_fn(|k| cross(suv, sv)[k] + cross(su, svv)[k]);
            Some((
                [dot(w, su), dot(w, sv), dot(c1, n)],
                [
                    [
                        dot(c1, su),
                        dot(w, suu) - dot(su, su),
                        dot(w, suv) - dot(sv, su),
                    ],
                    [
                        dot(c1, sv),
                        dot(w, suv) - dot(su, sv),
                        dot(w, svv) - dot(sv, sv),
                    ],
                    [dot(c2, n), dot(c1, nu), dot(c1, nv)],
                ],
            ))
        };
        let mut stack = vec![(cell, 0u32)];
        let mut found = None;
        // A stretch lying on the surface has `G = 0` all along it and no
        // one root: the search gives up after a fixed number of boxes.
        let mut budget = TANGENT_BOXES;
        while let Some((b, depth)) = stack.pop() {
            budget = budget.checked_sub(1)?;
            let (g, jac) = enclose(b)?;
            if g.iter().any(|&x| signed(x)) {
                continue;
            }
            let m: [f64; 3] = b.map(|[x, y]| (x + y) / 2.0);
            let (at, _) = enclose(m.map(|x| [x, x]))?;
            match krawczyk3(at, jac, b) {
                Some(false) => continue,
                Some(true) if found.is_none() => found = Some(m[0]),
                Some(true) => return None,
                None if depth < 14 => {
                    let k = (0..3)
                        .max_by(|&x, &y| {
                            let wd = |i: usize| (b[i][1] - b[i][0]) / (cell[i][1] - cell[i][0]);
                            wd(x).total_cmp(&wd(y))
                        })
                        .expect("three parameters");
                    let mid = (b[k][0] + b[k][1]) / 2.0;
                    for half in [[b[k][0], mid], [mid, b[k][1]]] {
                        let mut nb = b;
                        nb[k] = half;
                        stack.push((nb, depth + 1));
                    }
                }
                None => return None,
            }
        }
        found
    }

    /// A certified bound on how far the curve over `[a, b]` strays from
    /// the surface along the path from the middle's foot (see the module
    /// notes), the foot found near `near` or else by projection; infinite
    /// where the path leaves a bounded domain or the surface has no
    /// tangent plane at the foot.
    fn path_bound(&self, [a, b]: [f64; 2], near: Option<(f64, f64)>) -> f64 {
        let tm = (a + b) / 2.0;
        let h = (b - a) / 2.0;
        let Some(path) = self.foot_path(tm, near) else {
            return f64::INFINITY;
        };
        let Some(delta) = Iv::new(-h, h) else {
            return f64::INFINITY;
        };
        let p = Iv::point;
        let ((um, vm), (da, db), (da2, db2)) = (path.uv, path.first, path.second);
        // The path's parameters and their rates over the stretch.
        let half = p(0.5) * delta.square();
        let (us, vs) = (
            p(um) + p(da) * delta + p(da2) * half,
            p(vm) + p(db) * delta + p(db2) * half,
        );
        let (u1, v1) = (p(da) + p(da2) * delta, p(db) + p(db2) * delta);
        let [du, dv] = self.surface.domain();
        let (pu, pv) = self.surface.periods();
        let inside = |x: Iv, d: [f64; 2], period: Option<f64>| {
            period.is_some() || (x.lo() >= d[0] && x.hi() <= d[1])
        };
        if !(inside(us, du, pu) && inside(vs, dv, pv)) {
            return f64::INFINITY;
        }
        let (Some(pt), Some(over)) = (
            self.sheet.jet_box([[um, um], [vm, vm]]),
            self.sheet.jet_box([[us.lo(), us.hi()], [vs.lo(), vs.hi()]]),
        ) else {
            return f64::INFINITY;
        };
        let c0 = self.curve.jet_box([tm, tm]);
        let cr = self.curve.jet_box([a, b]);
        let (ia, ib) = (p(da), p(db));
        // g = C - S(u(t), v(t)): g(t₀), g'(t₀), and g'' over the stretch.
        let g0: IVec3 = core::array::from_fn(|k| c0[0][k] - pt[k].v);
        let g1: IVec3 = core::array::from_fn(|k| c0[1][k] - pt[k].d[0] * ia - pt[k].d[1] * ib);
        let g2: IVec3 = core::array::from_fn(|k| {
            cr[2][k]
                - (over[k].h[0] * u1 * u1
                    + p(2.0) * over[k].h[1] * u1 * v1
                    + over[k].h[2] * v1 * v1
                    + over[k].d[0] * p(da2)
                    + over[k].d[1] * p(db2))
        });
        let bound = p(norm_hi(g0)) + p(norm_hi(g1)) * p(h) + p(0.5) * p(norm_hi(g2)) * p(h) * p(h);
        bound.hi() + super::SLACK
    }

    /// Whether the curve over `[a, b]` is certified within the tolerance
    /// of the surface, in sixteen stretches.
    fn gap_within(&self, [a, b]: [f64; 2]) -> bool {
        let steps = 16;
        (0..steps).all(|i| {
            let at = |k: u32| a + (b - a) * f64::from(k) / f64::from(steps);
            self.path_bound([at(i), at(i + 1)], None) <= self.tolerance
        })
    }

    /// Candidates in order, with overlapping ones, and ones the curve
    /// between stays within the tolerance of the surface, made one.
    fn merge(&self, mut found: Vec<Candidate>) -> Vec<Candidate> {
        found.sort_by(|a, b| a.t[0].total_cmp(&b.t[0]));
        let mut out: Vec<Candidate> = Vec::with_capacity(found.len());
        for c in found {
            if let Some(last) = out.last_mut() {
                let same = c.simple.is_some() && last.simple.is_some() && c.t == last.t;
                let touching = same
                    || c.t[0] <= last.t[1]
                    || self
                        .curve
                        .point(last.t[1])
                        .distance(self.curve.point(c.t[0]))
                        <= self.tolerance
                    || self.gap_within([last.t[1], c.t[0]]);
                if touching {
                    if !same {
                        last.t[1] = last.t[1].max(c.t[1]);
                        last.simple = None;
                    }
                    continue;
                }
            }
            out.push(c);
        }
        out
    }

    /// The certified side of the surface a point is on, against its foot.
    fn side_at(&self, q: Point3) -> Option<Sign> {
        let (u, v) = self.surface.project(q).param;
        let j = self.sheet.jet_box([[u, u], [v, v]])?;
        let qv = q.to_vector().to_array().map(Iv::point);
        let w: IVec3 = core::array::from_fn(|k| qv[k] - j[k].v);
        let su = [j[0].d[0], j[1].d[0], j[2].d[0]];
        let sv = [j[0].d[1], j[1].d[1], j[2].d[1]];
        dot(w, cross(su, sv)).sign().filter(|&x| x != Sign::Zero)
    }

    /// The side just past `from` towards `limit`: the first certified
    /// sign, stepping out by doubling steps.
    fn side(&self, from: f64, limit: f64) -> Result<Side, Failure> {
        let span = limit - from;
        let mut step = span / 1024.0;
        for _ in 0..10 {
            match self.side_at(self.curve.point(from + step)) {
                Some(Sign::Positive) => return Ok(Side::Positive),
                Some(Sign::Negative) => return Ok(Side::Negative),
                _ => step *= 2.0,
            }
        }
        Err(Failure::Uncertain)
    }

    fn classify(&self, c: &Candidate, room: [f64; 2]) -> Result<CurveHit, Failure> {
        let [t0, t1] = self.range;
        let near =
            |a: f64, b: f64| self.curve.point(a).distance(self.curve.point(b)) <= self.tolerance;
        let [a, b] = c.t;
        let short = |x: f64, y: f64| {
            self.curve.point(x).distance(self.curve.point(y)) <= TOUCH * self.tolerance
        };
        let tangent = match c.simple {
            Some(_) => None,
            None => self.tangent(c),
        };
        let mid = tangent.unwrap_or((a + b) / 2.0);
        let point_like = c.simple.is_some()
            || tangent.is_some()
            || (0..=8).all(|k| short(mid, a + (b - a) * f64::from(k) / 8.0));
        if !point_like {
            let lo = if near(a, t0) { t0 } else { a };
            let hi = if near(b, t1) { t1 } else { b };
            return Ok(CurveHit::On { t: [lo, hi] });
        }
        let (at_start, at_end) = (near(mid, t0) || a <= t0, near(mid, t1) || b >= t1);
        let t = if at_start {
            t0
        } else if at_end {
            t1
        } else {
            mid
        };
        let (before, after) = match c.simple {
            Some(rising) if !at_start && !at_end => {
                let (neg, pos) = (Side::Negative, Side::Positive);
                if rising { (neg, pos) } else { (pos, neg) }
            }
            _ => (
                if at_start {
                    Side::Beyond
                } else {
                    self.side(a, room[0])?
                },
                if at_end {
                    Side::Beyond
                } else {
                    self.side(b, room[1])?
                },
            ),
        };
        Ok(CurveHit::Point {
            t,
            point: self.curve.point(t),
            before,
            after,
        })
    }
}

/// The solution of `m x = r`, by Cramer's rule; `None` when `m` is
/// singular.
pub(crate) fn solve3(m: [[f64; 3]; 3], r: [f64; 3]) -> Option<[f64; 3]> {
    let det3 = |a: [[f64; 3]; 3]| {
        a[0][0] * (a[1][1] * a[2][2] - a[1][2] * a[2][1])
            - a[0][1] * (a[1][0] * a[2][2] - a[1][2] * a[2][0])
            + a[0][2] * (a[1][0] * a[2][1] - a[1][1] * a[2][0])
    };
    let d = det3(m);
    if d == 0.0 || !d.is_finite() {
        return None;
    }
    let mut x = [0.0; 3];
    for (col, xc) in x.iter_mut().enumerate() {
        let mut a = m;
        for row in 0..3 {
            a[row][col] = r[row];
        }
        *xc = det3(a) / d;
    }
    x.iter().all(|v| v.is_finite()).then_some(x)
}

#[cfg(test)]
mod tests;
