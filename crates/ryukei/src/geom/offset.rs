//! The offset surface: a surface moved along its natural normal, exact by
//! its definition.

use super::bound2::IVec3;
use super::jet::{self, JVec, Jet, Num};
use super::sheet::{Sheet, nearest};
use super::{
    Box3, Frame, Mirror, ParamFlip, Point3, Projection, Rigid, Surface, SurfaceBound, SurfaceEval,
    UnitVector3, Vector3,
};
use crate::Error;
use crate::interval::Interval;

/// The deepest a cell is halved while certifying an offset regular.
const MAX_DEPTH: u32 = 16;

/// The most cells certifying an offset looks at before it gives up.
const MAX_CELLS: usize = 1 << 14;

/// How many times [`Offset::jet_box`] halves a part of a box.
const ENCLOSE_DEPTH: u32 = 6;

/// A box of parameters.
type Cell = [[f64; 2]; 2];

/// `O(u, v) = S(u, v) + d N(u, v)` over a box of its base's parameters,
/// `N` the base's natural unit normal.
///
/// The base is a NURBS, an extrusion or a revolution: the analytic
/// surfaces and pipes offset to their own kinds ([`Surface::offset`]), and
/// an offset of an offset is one offset by the distances' sum. The offset
/// keeps its base's parameters, so a pcurve on the base is one on the
/// offset, and the normal lines of both are the same lines. An offset is
/// built only where it is regular, certified over its whole box (see
/// [`Offset::new`]): then its natural normal, along `O_u × O_v`, is its
/// base's.
#[derive(Clone, Debug)]
pub struct Offset {
    base: Surface,
    distance: f64,
    domain: Cell,
    /// The base's own cells cut to the box: certifying, bounds and
    /// searches start from them.
    cells: Vec<Cell>,
    bounds: Box3,
}

impl PartialEq for Offset {
    fn eq(&self, o: &Self) -> bool {
        (&self.base, self.distance, self.domain) == (&o.base, o.distance, o.domain)
    }
}

/// The parts of `[c0, c1]`, moved by whole periods where the direction
/// has one, that lie within `[a, b]`; where `[a, b]` is a single value (a
/// parameter line), the cells holding it.
fn cut([c0, c1]: [f64; 2], [a, b]: [f64; 2], period: Option<f64>) -> Vec<[f64; 2]> {
    let shifts: Vec<f64> = match period {
        Some(p) => {
            let lo = ((a - c1) / p).floor() as i64;
            let hi = ((b - c0) / p).ceil() as i64;
            (lo..=hi).map(|k| k as f64 * p).collect()
        }
        None => vec![0.0],
    };
    shifts
        .into_iter()
        .map(|k| [(c0 + k).max(a), (c1 + k).min(b)])
        .filter(|r| r[0] < r[1] || (a == b && r[0] == r[1]))
        .collect()
}

/// The offset's point as jets from its base's derivatives up to third
/// order (by [`super::nurbs::third::ORDERS`]): `S_u` and `S_v` are jets
/// of their own, whose derivatives are the base's second and third, so
/// `N = (S_u × S_v) / |S_u × S_v|` comes out with its first and second
/// derivatives. `None` where `S_u × S_v` may vanish.
pub(crate) fn offset_jets<T: Num>(d: [[T; 3]; 10], distance: f64) -> Option<JVec<T>> {
    let p: JVec<T> = core::array::from_fn(|k| Jet {
        v: d[0][k],
        d: [d[1][k], d[2][k]],
        h: [d[3][k], d[4][k], d[5][k]],
    });
    let su: JVec<T> = core::array::from_fn(|k| Jet {
        v: d[1][k],
        d: [d[3][k], d[4][k]],
        h: [d[6][k], d[7][k], d[8][k]],
    });
    let sv: JVec<T> = core::array::from_fn(|k| Jet {
        v: d[2][k],
        d: [d[4][k], d[5][k]],
        h: [d[7][k], d[8][k], d[9][k]],
    });
    let n = jet::unit(jet::cross(su, sv))?;
    Some(jet::add(p, n.map(|j| j.scale(distance))))
}

fn idot(a: IVec3, b: IVec3) -> Interval {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn icross(a: IVec3, b: IVec3) -> IVec3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// Whether the offset by `distance` is regular over a cell, from the
/// base's derivative enclosures there: `Some(true)` where certified,
/// `Some(false)` where certified to fold or turn inside out somewhere,
/// `None` where rounding can't tell.
///
/// With `κ₁`, `κ₂` the principal curvatures against the natural normal
/// (`N_u = -A S_u`, `A` the shape operator), `O_u × O_v = (1 - d κ₁)(1 -
/// d κ₂) (S_u × S_v)`: the offset is regular, its normal its base's,
/// exactly when both factors are positive, that is when their product and
/// their sum are. From the fundamental forms (`E`, `F`, `G`; `L'`, `M'`,
/// `N'` against `W = S_u × S_v`, unnormalized; `a = |W|²`), with `T = E
/// N' - 2 F M' + G L'`: the product times `a²` is `a² - d T √a + d² (L'
/// N' - M'²)`, and the sum times `a √a` is `2 a √a - d T`.
fn regular_over(d: &[IVec3; 10], distance: f64) -> Option<bool> {
    let (su, sv) = (d[1], d[2]);
    let w = icross(su, sv);
    let a = idot(w, w);
    if a.lo() <= 0.0 {
        return None;
    }
    let root = a.sqrt()?;
    let (e, f, g) = (idot(su, su), idot(su, sv), idot(sv, sv));
    let (l, m, n) = (idot(d[3], w), idot(d[4], w), idot(d[5], w));
    let k = Interval::point(distance);
    let t = e * n - Interval::point(2.0) * f * m + g * l;
    let product = a * a - k * t * root + k * k * (l * n - m * m);
    let sum = Interval::point(2.0) * a * root - k * t;
    // A margin far above the rounding of the moved control points.
    let scale = a.hi() * a.hi();
    if product.lo() > 1e-9 * scale && sum.lo() > 1e-9 * a.hi() * root.hi() {
        Some(true)
    } else if product.hi() < 0.0 || sum.hi() < 0.0 {
        Some(false)
    } else {
        None
    }
}

impl Offset {
    /// The offset of `base` by `distance` along its natural normal over
    /// `domain`, a box of the base's parameters (cut to its domain; a
    /// periodic direction may run past its end and wraps).
    ///
    /// An offset of an offset is one offset of the first's base by the
    /// distances' sum, over the boxes' overlap. Fails with `NonFinite`,
    /// or `Degenerate`: for a base that is analytic or a pipe (use
    /// [`Surface::offset`]) or a sweep, an empty box, or an offset that
    /// can't be certified regular over the box. That is checked over the
    /// base's own cells cut to the box (Bézier patches, curve pieces,
    /// eighths of a turn), halved across the longer side up to 16 times,
    /// at most 16,384 cells, in interval arithmetic: the base's normal is
    /// defined (no pole, no fold) and `1 - d κ` is positive for both
    /// principal curvatures, so the offset never folds or turns inside
    /// out.
    pub fn new(base: Surface, distance: f64, domain: Cell) -> Result<Self, Error> {
        if !distance.is_finite() || domain.iter().flatten().any(|x| x.is_nan()) {
            return Err(Error::NonFinite);
        }
        if let Surface::Offset(o) = &base {
            let cut: Cell = core::array::from_fn(|k| {
                [
                    domain[k][0].max(o.domain[k][0]),
                    domain[k][1].min(o.domain[k][1]),
                ]
            });
            return Self::new(o.base.clone(), o.distance + distance, cut);
        }
        if !matches!(
            base,
            Surface::Nurbs(_) | Surface::Extrusion(_) | Surface::Revolution(_)
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
            if domain[k][0] >= domain[k][1] || domain[k].iter().any(|x| x.is_nan()) {
                return Err(Error::Degenerate);
            }
        }
        let near = crate::tol::size_box();
        let mut cells = Vec::new();
        for c in crate::intersect::cells::cells(&base, &near) {
            let us = cut(c[0], domain[0], periods[0]);
            let vs = cut(c[1], domain[1], periods[1]);
            for &u in &us {
                for &v in &vs {
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
            distance,
            domain,
            cells,
            bounds: crate::tol::size_box(),
        };
        s.bounds = s.certify()?;
        Ok(s)
    }

    pub fn base(&self) -> &Surface {
        &self.base
    }

    pub fn distance(&self) -> f64 {
        self.distance
    }

    /// The box of the base's parameters the offset is over.
    pub fn domain(&self) -> Cell {
        self.domain
    }

    pub(crate) fn cells(&self) -> &[Cell] {
        &self.cells
    }

    /// Certifies the offset regular over its box and returns the box its
    /// cells' enclosures fill.
    fn certify(&self) -> Result<Box3, Error> {
        let sheet = Sheet::of(&self.base);
        let mut stack: Vec<(Cell, u32)> = self.cells.iter().map(|&c| (c, 0)).collect();
        let mut seen = 0usize;
        let mut bounds: Option<Box3> = None;
        while let Some((cell, depth)) = stack.pop() {
            seen += 1;
            if seen > MAX_CELLS {
                return Err(Error::Degenerate);
            }
            let d = sheet.jet3_box(cell);
            match self
                .local_box(cell)
                .and_then(|d| regular_over(&d, self.distance))
            {
                Some(true) => {
                    if let Some(q) = d.and_then(|d| offset_jets(d, self.distance)) {
                        let b = point_box(&q);
                        bounds = Some(bounds.map_or(b, |o| o.union(&b)));
                        continue;
                    }
                }
                Some(false) => return Err(Error::Degenerate),
                None => {}
            }
            if depth == MAX_DEPTH {
                return Err(Error::Degenerate);
            }
            // Halve across the longer side in model space; never along a
            // direction the curvatures don't depend on.
            let len = |k: usize| {
                if self.invariant() == Some(k) {
                    return 0.0;
                }
                let w = cell[k][1] - cell[k][0];
                d.as_ref().map_or(w, |d| {
                    let v = d[1 + k];
                    w * crate::math::sqrt(idot(v, v).hi())
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
        let b = bounds.ok_or(Error::Degenerate)?;
        Ok(b.grow(1e-12 * (1.0 + b.max().to_vector().length())))
    }

    /// The direction along which the base's fundamental forms don't
    /// change: a revolution's angle, an extrusion's height.
    fn invariant(&self) -> Option<usize> {
        match self.base {
            Surface::Revolution(_) => Some(0),
            Surface::Extrusion(_) => Some(1),
            _ => None,
        }
    }

    /// The base's derivative enclosures over a cell, in the frame of its
    /// tangent plane at the cell's middle (`x` along `S_u`, `z` along the
    /// normal): there the derivatives' main components barely turn across
    /// the cell, so the fundamental forms come out far tighter than from
    /// world axes. `None` where the base has no normal at the middle.
    fn local_box(&self, cell: Cell) -> Option<[IVec3; 10]> {
        let (u, v) = (
            (cell[0][0] + cell[0][1]) / 2.0,
            (cell[1][0] + cell[1][1]) / 2.0,
        );
        // Turning a revolution about its axis, or moving an extrusion
        // along its direction, moves the base onto itself: its curvatures
        // are the same all along that way, so one line across is enough.
        let cell = match self.invariant() {
            Some(0) => [[u, u], cell[1]],
            Some(_) => [cell[0], [v, v]],
            None => cell,
        };
        let e = self.base.eval(u, v);
        let n = self.base.normal(u, v)?;
        let f = Frame::new(e.point, n.get(), e.du).ok()?;
        let moved = self.base.transformed(&Rigid::from_frame(&f).inverse());
        Sheet::of(&moved).jet3_box(cell)
    }

    /// The point's jets at `(u, v)`.
    pub(crate) fn jets_at(&self, u: f64, v: f64) -> JVec<f64> {
        let d = Sheet::of(&self.base)
            .jets3_at(u, v)
            .expect("an offset's base has third derivatives");
        offset_jets(d, self.distance).expect("a certified offset has a normal")
    }

    /// Enclosures of the point's jets over a box of parameters: over the
    /// box at once, else the hull of those over its parts in the offset's
    /// cells, each halved up to [`ENCLOSE_DEPTH`] times where the base's
    /// normal can't be certified over it whole. `None` where a part still
    /// has no certified normal.
    pub(crate) fn jet_box(&self, [ru, rv]: Cell) -> Option<JVec<Interval>> {
        let sheet = Sheet::of(&self.base);
        let at = |c: Cell| offset_jets(sheet.jet3_box(c)?, self.distance);
        if let Some(q) = at([ru, rv]) {
            return Some(q);
        }
        let (pu, pv) = self.base.periods();
        let mut out: Option<JVec<Interval>> = None;
        let mut stack: Vec<(Cell, u32)> = Vec::new();
        for c in &self.cells {
            for u in cut(c[0], ru, pu) {
                for v in cut(c[1], rv, pv) {
                    stack.push(([u, v], 0));
                }
            }
        }
        while let Some((c, depth)) = stack.pop() {
            let Some(q) = at(c) else {
                if depth == ENCLOSE_DEPTH {
                    return None;
                }
                let k = usize::from(c[1][1] - c[1][0] > c[0][1] - c[0][0]);
                let m = (c[k][0] + c[k][1]) / 2.0;
                let (mut a, mut b) = (c, c);
                a[k][1] = m;
                b[k][0] = m;
                stack.push((a, depth + 1));
                stack.push((b, depth + 1));
                continue;
            };
            out = Some(match out {
                None => q,
                Some(o) => core::array::from_fn(|k| Jet {
                    v: o[k].v.hull(q[k].v),
                    d: core::array::from_fn(|i| o[k].d[i].hull(q[k].d[i])),
                    h: core::array::from_fn(|i| o[k].h[i].hull(q[k].h[i])),
                }),
            });
        }
        out
    }

    pub(crate) fn point(&self, u: f64, v: f64) -> Point3 {
        let p = self.jets_at(u, v);
        Point3::new(p[0].v, p[1].v, p[2].v)
    }

    pub(crate) fn eval(&self, u: f64, v: f64) -> SurfaceEval {
        let p = self.jets_at(u, v);
        let part = |f: &dyn Fn(&Jet<f64>) -> f64| Vector3::new(f(&p[0]), f(&p[1]), f(&p[2]));
        SurfaceEval {
            point: Point3::ORIGIN + part(&|j| j.v),
            du: part(&|j| j.d[0]),
            dv: part(&|j| j.d[1]),
            duu: part(&|j| j.h[0]),
            duv: part(&|j| j.h[1]),
            dvv: part(&|j| j.h[2]),
        }
    }

    /// The base's normal, which is the offset's (see [`Offset::new`]).
    pub(crate) fn normal(&self, u: f64, v: f64) -> Option<UnitVector3> {
        self.base.normal(u, v)
    }

    /// The base's periods in the directions the box covers whole.
    pub(crate) fn periods(&self) -> (Option<f64>, Option<f64>) {
        let whole = self.base.domain();
        let (pu, pv) = self.base.periods();
        let keep = |p: Option<f64>, k: usize| p.filter(|_| self.domain[k] == whole[k]);
        (keep(pu, 0), keep(pv, 1))
    }

    /// The base's genus when the box covers its whole domain.
    pub(crate) fn closed_genus(&self) -> Option<u32> {
        (self.domain == self.base.domain())
            .then(|| self.base.closed_genus())
            .flatten()
    }

    pub(crate) fn bounds(&self) -> Box3 {
        self.bounds
    }

    /// The nearest point, as [`nearest`] finds it from the offset's cells.
    pub(crate) fn project(&self, p: Point3) -> Projection<(f64, f64), Point3> {
        nearest(
            &Sheet::Offset(self),
            self.domain,
            self.periods(),
            &self.cells,
            p,
        )
    }

    /// The offset moved by a rigid transform; parameters are unchanged,
    /// and nothing is certified again.
    pub(crate) fn transformed(&self, r: &Rigid) -> Self {
        Self {
            base: self.base.transformed(r),
            distance: self.distance,
            domain: self.domain,
            cells: self.cells.clone(),
            bounds: moved_box(&self.bounds, |p| r.apply_point(p)),
        }
    }

    /// The offset reflected by `m`: its base reflected, the box mapped as
    /// the base's parameters are, and the distance negated where the
    /// base's natural normal turns against the mirrored geometry.
    pub(crate) fn mirrored(&self, m: &Mirror) -> Result<(Self, ParamFlip), Error> {
        let (base, flip) = self.base.mirrored(m)?;
        let map = |c: Cell| -> Cell {
            match flip {
                ParamFlip::Kept => c,
                ParamFlip::U(at) => [[at - c[0][1], at - c[0][0]], c[1]],
                ParamFlip::V(at) => [c[0], [at - c[1][1], at - c[1][0]]],
            }
        };
        let distance = if flip.flips_normal() {
            -self.distance
        } else {
            self.distance
        };
        let mut cells: Vec<Cell> = self.cells.iter().map(|&c| map(c)).collect();
        cells.reverse();
        Ok((
            Self {
                base,
                distance,
                domain: map(self.domain),
                cells,
                bounds: moved_box(&self.bounds, |p| m.apply_point(p)),
            },
            flip,
        ))
    }

    /// Bounds over a parameter box, the worst of those from the
    /// enclosures over its parts in the offset's cells (see
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

/// The box of a point's enclosure.
fn point_box(q: &JVec<Interval>) -> Box3 {
    Box3::new(
        Point3::new(q[0].v.lo(), q[1].v.lo(), q[2].v.lo()),
        Point3::new(q[0].v.hi(), q[1].v.hi(), q[2].v.hi()),
    )
}

/// A box holding the image of `b` under a rigid motion or a reflection.
fn moved_box(b: &Box3, f: impl Fn(Point3) -> Point3) -> Box3 {
    let (lo, hi) = (b.min(), b.max());
    let corners = (0..8).map(|i| {
        f(Point3::new(
            if i & 1 == 0 { lo.x } else { hi.x },
            if i & 2 == 0 { lo.y } else { hi.y },
            if i & 4 == 0 { lo.z } else { hi.z },
        ))
    });
    let mut out: Option<Box3> = None;
    for c in corners {
        let p = Box3::new(c, c);
        out = Some(out.map_or(p, |o| o.union(&p)));
    }
    let o = out.expect("eight corners");
    o.grow(1e-12 * (1.0 + o.max().to_vector().length()))
}

#[cfg(test)]
pub(crate) mod tests;
