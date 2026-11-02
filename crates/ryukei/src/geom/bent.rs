//! The bent surface: a surface in a sheet-metal bend's strip carried
//! through the bend's fold map, exact by its definition.

use super::bound2::{IVec3, trig_box};
use super::jet::{JVec, Jet, Num};
use super::offset::{cut, moved_box, point_box};
use super::sheet::{Sheet, nearest};
use super::{
    Box3, Frame, Mirror, ParamFlip, Point3, Projection, Rigid, Surface, SurfaceBound, SurfaceEval,
    UnitVector3, Vector3,
};
use crate::Error;
use crate::interval::Interval;
use crate::math;
use core::f64::consts::TAU;

/// The deepest a cell is halved while certifying a bent surface.
const MAX_DEPTH: u32 = 16;

/// The most cells certifying a bent surface looks at before it gives up.
const MAX_CELLS: usize = 1 << 14;

/// A box of parameters.
type Cell = [[f64; 2]; 2];

/// A scalar's value and derivatives up to third order in `(u, v)`, by
/// [`super::nurbs::third::ORDERS`].
type D3<T> = [T; 10];

/// `G(u, v) = F(B(u, v))` over a box of its base's parameters: the base
/// `B` lies in a bend's strip, in the strip's own coordinates (`x` across
/// the strip, `y` along the bend's axis, `z` through the sheet from its
/// inner side), and the fold map
///
/// ```text
/// F(x, y, z) = O + y a + (r + z)(cos(κ x) e₁ + sin(κ x) e₂)
/// ```
///
/// wraps the strip round the bend's axis: `O`, `e₁`, `e₂` and `a` are the
/// axis frame's origin and `x`, `y` and `z` axes, `r` the inner radius and
/// `κ` the neutral curvature (`θ / ba`, rounded once), so the strip's
/// sides `z = 0` and `z = t` go to the bend's cylinders and its bend lines
/// `x = 0` and `x = ba` to the lines at angles 0 and `θ`.
///
/// A bent surface keeps its base's parameters, so a pcurve on the base is
/// one on it. `F` is regular and keeps orientation wherever `r + z > 0`
/// (its Jacobian is orthogonal, with determinant `κ (r + z)`), and one to
/// one while `κ x` spans less than a turn, so the natural normal, along
/// `G_u × G_v`, is the base's carried by `F`. Bases are planes, cylinders,
/// extrusions and NURBS: the walls a cut in a flat leaves.
#[derive(Clone, Debug)]
pub struct Bent {
    base: Surface,
    axis: Frame,
    radius: f64,
    curvature: f64,
    domain: Cell,
    /// The base's own cells cut to the box: certifying, bounds and
    /// searches start from them.
    cells: Vec<Cell>,
    bounds: Box3,
}

impl PartialEq for Bent {
    fn eq(&self, o: &Self) -> bool {
        (
            &self.base,
            &self.axis,
            self.radius,
            self.curvature,
            self.domain,
        ) == (&o.base, &o.axis, o.radius, o.curvature, o.domain)
    }
}

/// The product of two scalars' jets, by Leibniz's rule.
fn mul3<T: Num>(f: D3<T>, g: D3<T>) -> D3<T> {
    use super::nurbs::third::{ORDERS, binomial, index};
    core::array::from_fn(|k| {
        let (i, j) = ORDERS[k];
        let mut s = T::of(0.0);
        for a in 0..=i {
            for b in 0..=j {
                let c = binomial(i, a) * binomial(j, b);
                s = s + T::of(c) * f[index(a, b)] * g[index(i - a, j - b)];
            }
        }
        s
    })
}

/// `g(w(u, v))` from `g` and its first three derivatives at `w`'s value,
/// by the chain rule.
fn compose3<T: Num>(g: [T; 4], w: D3<T>) -> D3<T> {
    let [_, wu, wv, wuu, wuv, wvv, wuuu, wuuv, wuvv, wvvv] = w;
    let three = T::of(3.0);
    let two = T::of(2.0);
    [
        g[0],
        g[1] * wu,
        g[1] * wv,
        g[2] * wu * wu + g[1] * wuu,
        g[2] * wu * wv + g[1] * wuv,
        g[2] * wv * wv + g[1] * wvv,
        g[3] * wu * wu * wu + three * g[2] * wu * wuu + g[1] * wuuu,
        g[3] * wu * wu * wv + g[2] * (wuu * wv + two * wuv * wu) + g[1] * wuuv,
        g[3] * wu * wv * wv + g[2] * (wvv * wu + two * wuv * wv) + g[1] * wuvv,
        g[3] * wv * wv * wv + three * g[2] * wv * wvv + g[1] * wvvv,
    ]
}

/// A plane's or a cylinder's point and derivatives up to third order, by
/// [`super::nurbs::third::ORDERS`], from its parameters and `(cos u, sin
/// u)`.
fn analytic3<T: Num>(s: &Surface, u: T, v: T, (cu, su): (T, T)) -> Option<[[T; 3]; 10]> {
    let f = s.frame()?;
    let o = f.origin().to_vector().to_array().map(T::of);
    let (x, y, z) = (
        f.x().get().to_array().map(T::of),
        f.y().get().to_array().map(T::of),
        f.z().get().to_array().map(T::of),
    );
    let zero = [T::of(0.0); 3];
    Some(match s {
        Surface::Plane(_) => {
            let p = core::array::from_fn(|k| o[k] + u * x[k] + v * y[k]);
            [p, x, y, zero, zero, zero, zero, zero, zero, zero]
        }
        Surface::Cylinder(c) => {
            let r = T::of(c.radius());
            let radial: [T; 3] = core::array::from_fn(|k| cu * x[k] + su * y[k]);
            let turn: [T; 3] = core::array::from_fn(|k| cu * y[k] - su * x[k]);
            let p = core::array::from_fn(|k| o[k] + r * radial[k] + v * z[k]);
            [
                p,
                turn.map(|t| r * t),
                z,
                radial.map(|t| -(r * t)),
                zero,
                zero,
                turn.map(|t| -(r * t)),
                zero,
                zero,
                zero,
            ]
        }
        _ => return None,
    })
}

/// Whether `B_u × B_v` is certified non-zero, and `r + z` positive, over
/// a cell, from the base's enclosures there.
fn regular_over(d: &[IVec3; 10], radius: f64) -> Option<bool> {
    let (su, sv) = (d[1], d[2]);
    let w = [
        su[1] * sv[2] - su[2] * sv[1],
        su[2] * sv[0] - su[0] * sv[2],
        su[0] * sv[1] - su[1] * sv[0],
    ];
    let a = w[0].square() + w[1].square() + w[2].square();
    let rr = d[0][2] + Interval::point(radius);
    if rr.hi() <= 0.0 {
        return Some(false);
    }
    (a.lo() > 0.0 && rr.lo() > 1e-9 * radius).then_some(true)
}

impl Bent {
    /// `base`, in a strip's coordinates, carried round the bend of inner
    /// radius `radius` on `axis` with neutral curvature `curvature`, over
    /// `domain`, a box of the base's parameters (cut to its domain; a
    /// periodic direction may run past its end and wraps).
    ///
    /// Fails with `NonFinite`, or `Degenerate`: for a base other than a
    /// plane, a cylinder, an extrusion or a NURBS, a radius or curvature
    /// that isn't positive, an empty box, or a surface that can't be
    /// certified regular and one to one over the box. That is checked
    /// over the base's own cells cut to the box, halved across the longer
    /// side up to 16 times, at most 16,384 cells, in interval arithmetic:
    /// `B_u × B_v` doesn't vanish, `r + z` stays positive, and `κ x` spans
    /// less than a turn over the whole box.
    pub fn new(
        base: Surface,
        axis: Frame,
        radius: f64,
        curvature: f64,
        domain: Cell,
    ) -> Result<Self, Error> {
        if !(radius.is_finite() && curvature.is_finite())
            || domain.iter().flatten().any(|x| x.is_nan())
        {
            return Err(Error::NonFinite);
        }
        if radius <= 0.0 || curvature <= 0.0 {
            return Err(Error::Degenerate);
        }
        if !matches!(
            base,
            Surface::Plane(_) | Surface::Cylinder(_) | Surface::Extrusion(_) | Surface::Nurbs(_)
        ) {
            return Err(Error::Degenerate);
        }
        let whole = base.domain();
        let (pu, pv) = base.periods();
        let periods = [pu, pv];
        let mut domain = domain;
        for k in 0..2 {
            let [a, b] = domain[k];
            domain[k] = match periods[k] {
                Some(p) if b - a >= p => whole[k],
                Some(p) => {
                    let a0 = whole[k][0] + (a - whole[k][0]).rem_euclid(p);
                    [a0, a0 + (b - a)]
                }
                None => [a.max(whole[k][0]), b.min(whole[k][1])],
            };
            if !domain[k].iter().all(|x| x.is_finite()) || domain[k][0] >= domain[k][1] {
                return Err(Error::Degenerate);
            }
        }
        let near = crate::tol::size_box();
        let mut cells = Vec::new();
        for c in crate::intersect::cells::cells(&base, &near) {
            for u in cut(c[0], domain[0], periods[0]) {
                for v in cut(c[1], domain[1], periods[1]) {
                    if u[0] < u[1] && v[0] < v[1] {
                        cells.push([u, v]);
                    }
                }
            }
        }
        cells.sort_by(|a, b| {
            a.iter()
                .flatten()
                .zip(b.iter().flatten())
                .map(|(x, y)| x.total_cmp(y))
                .find(|o| o.is_ne())
                .unwrap_or(core::cmp::Ordering::Equal)
        });
        cells.dedup();
        if cells.is_empty() {
            return Err(Error::Degenerate);
        }
        let mut s = Self {
            base,
            axis,
            radius,
            curvature,
            domain,
            cells,
            bounds: crate::tol::size_box(),
        };
        s.bounds = s.certify()?;
        Ok(s)
    }

    /// The base, in its strip's coordinates.
    pub fn base(&self) -> &Surface {
        &self.base
    }

    /// The bend's axis frame: `F`'s `O`, `e₁`, `e₂` and `a`.
    pub fn axis(&self) -> &Frame {
        &self.axis
    }

    /// The bend's inner radius `r`.
    pub fn radius(&self) -> f64 {
        self.radius
    }

    /// The neutral curvature `κ`.
    pub fn curvature(&self) -> f64 {
        self.curvature
    }

    /// The box of the base's parameters the surface is over.
    pub fn domain(&self) -> Cell {
        self.domain
    }

    pub(crate) fn cells(&self) -> &[Cell] {
        &self.cells
    }

    /// The fold map of a point in the strip's coordinates.
    pub fn fold(&self, q: Point3) -> Point3 {
        let (s, c) = math::sin_cos(q.x * self.curvature);
        let f = &self.axis;
        f.origin() + f.z().get() * q.y + (f.x().get() * c + f.y().get() * s) * (self.radius + q.z)
    }

    /// Certifies the surface regular and one to one over its box, and
    /// returns the box its cells' enclosures fill.
    fn certify(&self) -> Result<Box3, Error> {
        let mut stack: Vec<(Cell, u32)> = self.cells.iter().map(|&c| (c, 0)).collect();
        let mut seen = 0usize;
        let mut bounds: Option<Box3> = None;
        let mut across: Option<Interval> = None;
        while let Some((cell, depth)) = stack.pop() {
            seen += 1;
            if seen > MAX_CELLS {
                return Err(Error::Degenerate);
            }
            let d = self.base_box(cell);
            match d.as_ref().and_then(|d| regular_over(d, self.radius)) {
                Some(true) => {
                    let d = d.expect("an enclosure");
                    across = Some(across.map_or(d[0][0], |a| a.hull(d[0][0])));
                    let q = self.folded_box(d);
                    let b = point_box(&second(q));
                    bounds = Some(bounds.map_or(b, |o| o.union(&b)));
                    continue;
                }
                Some(false) => return Err(Error::Degenerate),
                None => {}
            }
            if depth == MAX_DEPTH {
                return Err(Error::Degenerate);
            }
            let len = |k: usize| {
                let w = cell[k][1] - cell[k][0];
                d.as_ref().map_or(w, |d| {
                    let v = d[1 + k];
                    let sq = v[0].square() + v[1].square() + v[2].square();
                    w * math::sqrt(sq.hi())
                })
            };
            let k = usize::from(len(1) > len(0) || len(0).is_nan());
            let m = (cell[k][0] + cell[k][1]) / 2.0;
            let (mut a, mut b) = (cell, cell);
            a[k][1] = m;
            b[k][0] = m;
            stack.push((a, depth + 1));
            stack.push((b, depth + 1));
        }
        let x = across.ok_or(Error::Degenerate)?;
        let turn = (x.hi() - x.lo()) * self.curvature;
        if turn.is_nan() || turn >= TAU * (1.0 - 1e-9) {
            return Err(Error::Degenerate);
        }
        let b = bounds.ok_or(Error::Degenerate)?;
        Ok(b.grow(1e-12 * (1.0 + b.max().to_vector().length())))
    }

    /// The base's point and derivatives up to third order at `(u, v)`.
    fn base_at(&self, u: f64, v: f64) -> [[f64; 3]; 10] {
        let (s, c) = math::sin_cos(u);
        analytic3(&self.base, u, v, (c, s))
            .or_else(|| Sheet::of(&self.base).jets3_at(u, v))
            .expect("a bent surface's base has third derivatives")
    }

    /// Enclosures of [`Bent::base_at`] over a cell; `None` where a box of
    /// an unbounded direction isn't finite.
    fn base_box(&self, [ru, rv]: Cell) -> Option<[IVec3; 10]> {
        match &self.base {
            Surface::Plane(_) | Surface::Cylinder(_) => {
                let u = Interval::new(ru[0], ru[1])?;
                let v = Interval::new(rv[0], rv[1])?;
                analytic3(&self.base, u, v, trig_box(ru))
            }
            b => Sheet::of(b).jet3_box([ru, rv]),
        }
    }

    /// `F` over a base's point and derivatives up to third order, given
    /// `(cos, sin)` of `κ x` at (or enclosing) `x`'s value.
    fn folded<T: Num>(&self, b: [[T; 3]; 10], (cw, sw): (T, T)) -> [[T; 3]; 10] {
        let k = T::of(self.curvature);
        let x: D3<T> = core::array::from_fn(|i| b[i][0] * k);
        let y: D3<T> = core::array::from_fn(|i| b[i][1]);
        let mut rr: D3<T> = core::array::from_fn(|i| b[i][2]);
        rr[0] = rr[0] + T::of(self.radius);
        let c = compose3([cw, -sw, -cw, sw], x);
        let s = compose3([sw, cw, -sw, -cw], x);
        let (m1, m2) = (mul3(rr, c), mul3(rr, s));
        let f = &self.axis;
        let (o, e1, e2, a) = (
            f.origin().to_vector().to_array(),
            f.x().get().to_array(),
            f.y().get().to_array(),
            f.z().get().to_array(),
        );
        core::array::from_fn(|i| {
            core::array::from_fn(|n| {
                let lin = T::of(a[n]) * y[i] + T::of(e1[n]) * m1[i] + T::of(e2[n]) * m2[i];
                if i == 0 { T::of(o[n]) + lin } else { lin }
            })
        })
    }

    /// [`Bent::folded`] over enclosures.
    fn folded_box(&self, d: [IVec3; 10]) -> [IVec3; 10] {
        let x = d[0][0] * Interval::point(self.curvature);
        self.folded(d, trig_box([x.lo(), x.hi()]))
    }

    /// The point and its derivatives up to third order at `(u, v)`.
    pub(crate) fn jets3_at(&self, u: f64, v: f64) -> [[f64; 3]; 10] {
        let b = self.base_at(u, v);
        let (s, c) = math::sin_cos(b[0][0] * self.curvature);
        self.folded(b, (c, s))
    }

    /// Enclosures of [`Bent::jets3_at`] over a box of parameters.
    pub(crate) fn jet3_box(&self, cell: Cell) -> Option<[IVec3; 10]> {
        Some(self.folded_box(self.base_box(cell)?))
    }

    /// The point's jets at `(u, v)`.
    pub(crate) fn jets_at(&self, u: f64, v: f64) -> JVec<f64> {
        second(self.jets3_at(u, v))
    }

    /// Enclosures of the point's jets over a box of parameters.
    pub(crate) fn jet_box(&self, cell: Cell) -> Option<JVec<Interval>> {
        self.jet3_box(cell).map(second)
    }

    pub(crate) fn point(&self, u: f64, v: f64) -> Point3 {
        let b = self.base.point(u, v);
        self.fold(b)
    }

    pub(crate) fn eval(&self, u: f64, v: f64) -> SurfaceEval {
        let d = self.jets3_at(u, v);
        let at = |k: usize| Vector3::new(d[k][0], d[k][1], d[k][2]);
        SurfaceEval {
            point: self.point(u, v),
            du: at(1),
            dv: at(2),
            duu: at(3),
            duv: at(4),
            dvv: at(5),
        }
    }

    /// The unit normal along `G_u × G_v`: the base's normal `n` carried
    /// by `F`'s cofactor, `n_x t + κ (r + z)(n_y a + n_z ρ̂)` with `t` and
    /// `ρ̂` the tangential and radial directions at `κ x`. `None` where the
    /// base has none.
    pub(crate) fn normal(&self, u: f64, v: f64) -> Option<UnitVector3> {
        let b = self.base.point(u, v);
        let n = self.base.normal(u, v)?.get();
        let f = &self.axis;
        let (s, c) = math::sin_cos(b.x * self.curvature);
        let (e1, e2) = (f.x().get(), f.y().get());
        let t = e2 * c - e1 * s;
        let radial = e1 * c + e2 * s;
        let lambda = self.curvature * (self.radius + b.z);
        (t * n.x + (f.z().get() * n.y + radial * n.z) * lambda).normalize()
    }

    /// The base's periods in the directions the box covers whole.
    pub(crate) fn periods(&self) -> (Option<f64>, Option<f64>) {
        let whole = self.base.domain();
        let (pu, pv) = self.base.periods();
        let keep = |p: Option<f64>, k: usize| p.filter(|_| self.domain[k] == whole[k]);
        (keep(pu, 0), keep(pv, 1))
    }

    pub(crate) fn bounds(&self) -> Box3 {
        self.bounds
    }

    /// The nearest point, as [`nearest`] finds it from the surface's
    /// cells.
    pub(crate) fn project(&self, p: Point3) -> Projection<(f64, f64), Point3> {
        nearest(
            &Sheet::Bent(self),
            self.domain,
            self.periods(),
            &self.cells,
            p,
        )
    }

    /// The surface moved by a rigid transform: its axis frame moves, its
    /// base stays in its strip's coordinates, and nothing is certified
    /// again.
    pub(crate) fn transformed(&self, r: &Rigid) -> Self {
        Self {
            axis: self.axis.transformed(r),
            bounds: moved_box(&self.bounds, |p| r.apply_point(p)),
            ..self.clone()
        }
    }

    /// The surface reflected by `m`. The reflected axes `(m e₁, m e₂, m
    /// a)` are left-handed, so the new frame's axis is `-m a` and the base
    /// is reflected across its strip's `y = 0`: then `F'(x, -y, z) =
    /// m F(x, y, z)`. Parameters map as the base's do.
    pub(crate) fn mirrored(&self, m: &Mirror) -> Result<(Self, ParamFlip), Error> {
        let f = &self.axis;
        let axis = Frame::from_axes(
            m.apply_point(f.origin()),
            -m.apply_unit(f.z()),
            m.apply_unit(f.x()),
        )?;
        let across = Mirror::new(Point3::ORIGIN, UnitVector3::Y);
        let (base, flip) = self.base.mirrored(&across)?;
        let map = |c: Cell| -> Cell {
            match flip {
                ParamFlip::Kept => c,
                ParamFlip::U(at) => [[at - c[0][1], at - c[0][0]], c[1]],
                ParamFlip::V(at) => [c[0], [at - c[1][1], at - c[1][0]]],
            }
        };
        let mut cells: Vec<Cell> = self.cells.iter().map(|&c| map(c)).collect();
        cells.reverse();
        Ok((
            Self {
                base,
                axis,
                radius: self.radius,
                curvature: self.curvature,
                domain: map(self.domain),
                cells,
                bounds: moved_box(&self.bounds, |p| m.apply_point(p)),
            },
            flip,
        ))
    }

    /// Bounds over a parameter box, the worst of those from the
    /// enclosures over its parts in the surface's cells (see
    /// [`super::bound2::from_jets`]).
    pub(crate) fn derivative_bound(&self, [ru, rv]: Cell) -> SurfaceBound {
        let mut out = SurfaceBound {
            duu: 0.0,
            duv: 0.0,
            dvv: 0.0,
            turn_u: 0.0,
            turn_v: 0.0,
        };
        let (pu, pv) = self.base.periods();
        let mut any = false;
        for c in &self.cells {
            for u in cut(c[0], ru, pu) {
                for v in cut(c[1], rv, pv) {
                    let Some(q) = self.jet_box([u, v]) else {
                        return super::bound2::INFINITE_BOUND;
                    };
                    any = true;
                    let b = super::bound2::from_jets(&q);
                    out.duu = out.duu.max(b.duu);
                    out.duv = out.duv.max(b.duv);
                    out.dvv = out.dvv.max(b.dvv);
                    out.turn_u = out.turn_u.max(b.turn_u);
                    out.turn_v = out.turn_v.max(b.turn_v);
                }
            }
        }
        if any {
            out
        } else {
            match self.jet_box([ru, rv]) {
                Some(q) => super::bound2::from_jets(&q),
                None => super::bound2::INFINITE_BOUND,
            }
        }
    }
}

impl super::offset::fit::Fitted for Bent {
    fn jets_at(&self, u: f64, v: f64) -> JVec<f64> {
        Bent::jets_at(self, u, v)
    }

    fn jet_box(&self, cell: Cell) -> Option<JVec<Interval>> {
        Bent::jet_box(self, cell)
    }

    fn cells(&self) -> &[Cell] {
        &self.cells
    }

    fn base_periods(&self) -> (Option<f64>, Option<f64>) {
        self.base.periods()
    }

    fn domain(&self) -> Cell {
        self.domain
    }
}

impl Bent {
    /// A NURBS within `tolerance` (at least
    /// [`crate::tol::LINEAR_RESOLUTION`]) of the surface over the box
    /// `[ru, rv]` of its parameters, on the same parameters, and the
    /// certified bound on how far it strays: the bicubic Hermite net
    /// [`super::Offset::to_nurbs_within`] fits, by the same rules and
    /// limits. A bent surface has no exact rational form
    /// ([`Surface::to_nurbs`] is `Degenerate`); this is for export.
    pub fn to_nurbs_within(
        &self,
        range: Cell,
        tolerance: f64,
    ) -> Result<(super::NurbsSurface, [super::Reparam; 2], f64), Error> {
        super::offset::fit::within(self, range, tolerance)
    }
}

/// The value and derivatives up to second order of third-order jets.
fn second<T: Copy>(d: [[T; 3]; 10]) -> JVec<T> {
    core::array::from_fn(|k| Jet {
        v: d[0][k],
        d: [d[1][k], d[2][k]],
        h: [d[3][k], d[4][k], d[5][k]],
    })
}

#[cfg(test)]
pub(crate) mod tests;
