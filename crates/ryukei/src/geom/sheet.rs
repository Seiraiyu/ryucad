//! Every surface written over jets.
//!
//! A [`Sheet`] gives a surface's point as jets in its two parameters (see
//! [`crate::geom::jet`]): in floating point at a point, for Newton's method
//! and directions, or over [`Interval`]s over a box of parameters, for
//! certified enclosures. The analytic surfaces are their closed forms
//! over `cos` and `sin` of their angles; a pipe is [`Pipe::jets`] over its
//! spine's derivatives; a NURBS is the hull of its Bézier patches'
//! enclosures ([`NurbsSurface::jet_box`]); an extrusion and a revolution
//! are their generating curve's enclosures ([`Curve::jet_box`]) moved
//! along a direction or turned about an axis.

use super::bound2::{IVec3, trig_box};
use super::jet::{self, JVec, Jet, Num};
use super::{
    Extrusion, NurbsSurface, Offset, Pipe, Point3, Revolution, Surface, SurfaceEval, Sweep, Vector3,
};
use crate::interval::Interval;
use crate::math;

mod nearest;
pub(crate) use nearest::{krawczyk2, nearest};

/// A surface over jets.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Sheet<'a> {
    Analytic(&'a Surface),
    Pipe(&'a Pipe),
    Nurbs(&'a NurbsSurface),
    Extrusion(&'a Extrusion),
    Revolution(&'a Revolution),
    Sweep(&'a Sweep),
    Offset(&'a Offset),
}

/// Jets from a value and its derivatives, one per coordinate:
/// `[S, Su, Sv, Suu, Suv, Svv]`.
fn from_parts<T: Copy>(e: [[T; 3]; 6]) -> JVec<T> {
    core::array::from_fn(|k| Jet {
        v: e[0][k],
        d: [e[1][k], e[2][k]],
        h: [e[3][k], e[4][k], e[5][k]],
    })
}

fn from_eval(e: &SurfaceEval) -> JVec<f64> {
    from_parts([
        e.point.to_vector().to_array(),
        e.du.to_array(),
        e.dv.to_array(),
        e.duu.to_array(),
        e.duv.to_array(),
        e.dvv.to_array(),
    ])
}

impl<'a> Sheet<'a> {
    /// The sheet of any surface.
    pub(crate) fn of(s: &'a Surface) -> Self {
        match s {
            Surface::Pipe(p) => Self::Pipe(p),
            Surface::Nurbs(n) => Self::Nurbs(n),
            Surface::Extrusion(e) => Self::Extrusion(e),
            Surface::Revolution(r) => Self::Revolution(r),
            Surface::Sweep(w) => Self::Sweep(w),
            Surface::Offset(o) => Self::Offset(o),
            Surface::Plane(_)
            | Surface::Cylinder(_)
            | Surface::Cone(_)
            | Surface::Sphere(_)
            | Surface::Torus(_) => Self::Analytic(s),
        }
    }

    /// Enclosures of the point's jets over a box of parameters; `None`
    /// where a pipe's frame can't be certified over it, or where a box of
    /// a line's parameter isn't finite.
    pub(crate) fn jet_box(&self, [ru, rv]: [[f64; 2]; 2]) -> Option<JVec<Interval>> {
        match self {
            Self::Pipe(p) => p.jet_box([ru, rv]).map(|[p, _]| p),
            Self::Analytic(s) => {
                let u = Interval::new(ru[0], ru[1])?;
                let v = Interval::new(rv[0], rv[1])?;
                analytic(s, [u, v], trig_box(ru), trig_box(rv))
            }
            Self::Nurbs(n) => Some(from_parts(n.jet_box([ru, rv]))),
            Self::Extrusion(e) => {
                if !(ru[0].is_finite() && ru[1].is_finite()) {
                    return None;
                }
                let c = e.curve().jet_box(ru);
                let v = Interval::new(rv[0], rv[1])?;
                let d = e.dir().get().to_array().map(Interval::point);
                let zero = [Interval::point(0.0); 3];
                let p: IVec3 = core::array::from_fn(|k| c[0][k] + v * d[k]);
                Some(from_parts([p, c[1], d, c[2], zero, zero]))
            }
            Self::Revolution(r) => {
                if !(rv[0].is_finite() && rv[1].is_finite()) {
                    return None;
                }
                let c = r.curve().jet_box(rv);
                Some(revolved(r, c, trig_box(ru)))
            }
            Self::Sweep(w) => w.jet_box([ru, rv]),
            Self::Offset(o) => o.jet_box([ru, rv]),
        }
    }

    /// The point's jets at `(u, v)`, in floating point.
    pub(crate) fn jets_at(&self, u: f64, v: f64) -> JVec<f64> {
        let swap = |(s, c): (f64, f64)| (c, s);
        match self {
            Self::Pipe(p) => p.jets_at(u, v)[0],
            Self::Analytic(s) => {
                analytic(s, [u, v], swap(math::sin_cos(u)), swap(math::sin_cos(v)))
                    .expect("an analytic surface")
            }
            Self::Nurbs(n) => from_eval(&n.eval(u, v)),
            Self::Extrusion(e) => from_eval(&e.eval(u, v)),
            Self::Revolution(r) => from_eval(&r.eval(u, v)),
            Self::Sweep(w) => w.jets_at(u, v),
            Self::Offset(o) => o.jets_at(u, v),
        }
    }

    pub(crate) fn point(&self, u: f64, v: f64) -> Point3 {
        match self {
            Self::Pipe(p) => p.point(u, v),
            Self::Analytic(s) => s.point(u, v),
            Self::Nurbs(n) => n.point(u, v),
            Self::Extrusion(e) => e.point(u, v),
            Self::Revolution(r) => r.point(u, v),
            Self::Sweep(w) => w.point(u, v),
            Self::Offset(o) => o.point(u, v),
        }
    }
}

impl Sheet<'_> {
    /// The point and its derivatives up to third order at `(u, v)`, by
    /// [`super::nurbs::third::ORDERS`], for the surfaces an
    /// [`super::Offset`] stands on: a NURBS, an extrusion, a revolution or
    /// a sweep. `None` for the others, which offset to their own kinds.
    pub(crate) fn jets3_at(&self, u: f64, v: f64) -> Option<[[f64; 3]; 10]> {
        match self {
            Self::Nurbs(n) => Some(n.jet3(u, v).map(Vector3::to_array)),
            Self::Extrusion(e) => {
                let c = e.curve().jet3(u).map(Vector3::to_array);
                Some(extruded3(e.dir().get(), c, v))
            }
            Self::Revolution(r) => {
                let c = r.curve().jet3(v).map(Vector3::to_array);
                let (sin, cos) = math::sin_cos(u);
                Some(revolved3(r, c, cos, sin))
            }
            Self::Sweep(w) => Some(w.jets3_at(u, v)),
            _ => None,
        }
    }

    /// Enclosures of [`Sheet::jets3_at`] over a box of parameters; `None`
    /// likewise, or where a box of an unbounded direction isn't finite.
    pub(crate) fn jet3_box(&self, [ru, rv]: [[f64; 2]; 2]) -> Option<[IVec3; 10]> {
        match self {
            Self::Nurbs(n) => Some(n.jet3_box([ru, rv])),
            Self::Extrusion(e) => {
                if !(ru[0].is_finite() && ru[1].is_finite()) {
                    return None;
                }
                let c = e.curve().jet_box(ru);
                Some(extruded3(e.dir().get(), c, Interval::new(rv[0], rv[1])?))
            }
            Self::Revolution(r) => {
                if !(rv[0].is_finite() && rv[1].is_finite()) {
                    return None;
                }
                let c = r.curve().jet_box(rv);
                let (cos, sin) = trig_box(ru);
                Some(revolved3(r, c, cos, sin))
            }
            Self::Sweep(w) => w.jet3_box([ru, rv]),
            _ => None,
        }
    }
}

/// An extrusion's derivatives up to third order from its curve's point
/// and first three derivatives at `u`: `S = C + v d`, `Sv = d`, and every
/// other derivative along `v` zero.
fn extruded3<T: Num>(d: Vector3, c: [[T; 3]; 4], v: T) -> [[T; 3]; 10] {
    let zero = [T::of(0.0); 3];
    let d = d.to_array().map(T::of);
    let p = core::array::from_fn(|k| c[0][k] + v * d[k]);
    [p, c[1], d, c[2], zero, zero, c[3], zero, zero, zero]
}

/// A revolution's derivatives up to third order, from its curve's point
/// and first three derivatives at `v` and `(cos u, sin u)`: with `ρ` and
/// `h` the curve's components along the frame's `x` and `z`, `S = o + ρ
/// radial + h z`, each `u` derivative turning `radial` a quarter.
fn revolved3<T: Num>(r: &Revolution, c: [[T; 3]; 4], cu: T, su: T) -> [[T; 3]; 10] {
    let f = r.frame();
    let o = f.origin().to_vector().to_array();
    let (x, y, z) = (
        f.x().get().to_array(),
        f.y().get().to_array(),
        f.z().get().to_array(),
    );
    let along = |w: [f64; 3], k: usize| {
        let rel = |i: usize| {
            if k == 0 {
                c[0][i] - T::of(o[i])
            } else {
                c[k][i]
            }
        };
        T::of(w[0]) * rel(0) + T::of(w[1]) * rel(1) + T::of(w[2]) * rel(2)
    };
    let rho: [T; 4] = core::array::from_fn(|k| along(x, k));
    let h: [T; 4] = core::array::from_fn(|k| along(z, k));
    let radial: [T; 3] = core::array::from_fn(|i| T::of(x[i]) * cu + T::of(y[i]) * su);
    let turn: [T; 3] = core::array::from_fn(|i| T::of(y[i]) * cu - T::of(x[i]) * su);
    let of = |a: T, w: [T; 3], b: T, zk: bool| -> [T; 3] {
        core::array::from_fn(|i| a * w[i] + if zk { b * T::of(z[i]) } else { T::of(0.0) })
    };
    let zero = T::of(0.0);
    let p = core::array::from_fn(|i| T::of(o[i]) + rho[0] * radial[i] + h[0] * T::of(z[i]));
    [
        p,
        of(rho[0], turn, zero, false),
        of(rho[1], radial, h[1], true),
        of(-rho[0], radial, zero, false),
        of(rho[1], turn, zero, false),
        of(rho[2], radial, h[2], true),
        of(-rho[0], turn, zero, false),
        of(-rho[1], radial, zero, false),
        of(rho[2], turn, zero, false),
        of(rho[3], radial, h[3], true),
    ]
}

/// A revolution's point as jets over a box: `o + ρ(v) radial(u) + h(v)
/// z`, with `ρ = (C - o) · x` and `h = (C - o) · z` from enclosures of
/// the curve's point and derivatives over the box's `v` and `(cos, sin)`
/// of its `u`.
fn revolved(r: &Revolution, c: [IVec3; 4], (cu, su): (Interval, Interval)) -> JVec<Interval> {
    let f = r.frame();
    let p = Interval::point;
    let o = f.origin().to_vector().to_array();
    let along = |w: Vector3, k: usize| {
        let w = w.to_array();
        let rel = |i: usize| if k == 0 { c[0][i] - p(o[i]) } else { c[k][i] };
        p(w[0]) * rel(0) + p(w[1]) * rel(1) + p(w[2]) * rel(2)
    };
    let (x, y, z) = (f.x().get(), f.y().get(), f.z().get());
    let rho = Jet::of_t(along(x, 0), along(x, 1), along(x, 2));
    let h = Jet::of_t(along(z, 0), along(z, 1), along(z, 2));
    let jcu = Jet::of_s(cu, -su, -cu);
    let jsu = Jet::of_s(su, cu, -su);
    let k = |w: Vector3| -> JVec<Interval> { w.to_array().map(|a| Jet::constant(p(a))) };
    let radial = jet::add(jet::times(k(x), jcu), jet::times(k(y), jsu));
    jet::add(
        k(f.origin().to_vector()),
        jet::add(jet::times(radial, rho), jet::times(k(z), h)),
    )
}

/// An analytic surface's point as jets in `(u, v)`, from the parameters
/// and `(cos, sin)` of each.
fn analytic<T: Num>(
    s: &Surface,
    [u, v]: [T; 2],
    (cu, su): (T, T),
    (cv, sv): (T, T),
) -> Option<JVec<T>> {
    let f = s.frame()?;
    let c = |w: Vector3| -> JVec<T> { w.to_array().map(|x| Jet::constant(T::of(x))) };
    let (o, x, y, z) = (
        c(f.origin().to_vector()),
        c(f.x().get()),
        c(f.y().get()),
        c(f.z().get()),
    );
    let one = T::of(1.0);
    let zero = T::of(0.0);
    let ju = Jet::of_s(u, one, zero);
    let jv = Jet::of_t(v, one, zero);
    let (jcu, jsu) = (Jet::of_s(cu, -su, -cu), Jet::of_s(su, cu, -su));
    let (jcv, jsv) = (Jet::of_t(cv, -sv, -cv), Jet::of_t(sv, cv, -sv));
    let radial = jet::add(jet::times(x, jcu), jet::times(y, jsu));
    let k = |a: f64| Jet::constant(T::of(a));
    Some(match s {
        Surface::Plane(_) => jet::add(o, jet::add(jet::times(x, ju), jet::times(y, jv))),
        Surface::Cylinder(cy) => jet::add(
            o,
            jet::add(jet::times(radial, k(cy.radius())), jet::times(z, jv)),
        ),
        Surface::Cone(cone) => {
            let (sin, cos) = math::sin_cos(cone.half_angle());
            let rho = k(cone.radius()) + jv * k(sin / cos);
            jet::add(o, jet::add(jet::times(radial, rho), jet::times(z, jv)))
        }
        Surface::Sphere(sp) => {
            let out = jet::add(jet::times(radial, jcv), jet::times(z, jsv));
            jet::add(o, jet::times(out, k(sp.radius())))
        }
        Surface::Torus(t) => {
            let ring = k(t.major()) + jcv * k(t.minor());
            jet::add(
                o,
                jet::add(jet::times(radial, ring), jet::times(z, jsv * k(t.minor()))),
            )
        }
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{Circle, Cone, Curve, Cylinder, Frame, KnotVector, Plane, Sphere, Torus};
    use core::f64::consts::TAU;
    use std::sync::Arc;

    fn tilted() -> Frame {
        Frame::new(
            Point3::new(0.3, -0.2, 0.5),
            Vector3::new(1.0, 0.4, 0.2),
            Vector3::new(0.0, 1.0, 0.0),
        )
        .unwrap()
    }

    fn analytic_surfaces() -> Vec<Surface> {
        let f = tilted();
        vec![
            Surface::Plane(Plane::new(f)),
            Surface::Cylinder(Cylinder::new(f, 0.7).unwrap()),
            Surface::Cone(Cone::new(f, 0.7, 0.4).unwrap()),
            Surface::Sphere(Sphere::new(f, 0.7).unwrap()),
            Surface::Torus(Torus::new(f, 1.5, 0.4).unwrap()),
        ]
    }

    /// A bicubic patch of two by three spans with uneven weights.
    fn spline() -> Surface {
        let ku = KnotVector::clamped(3, &[0.0, 0.4, 1.0], &[4, 1, 4]).unwrap();
        let kv = KnotVector::clamped(3, &[0.0, 0.3, 0.7, 1.0], &[4, 1, 1, 4]).unwrap();
        let (nu, nv) = (5, 6);
        let mut pts = Vec::new();
        let mut w = Vec::new();
        for i in 0..nu {
            for j in 0..nv {
                let (x, y) = (i as f64 * 0.5, j as f64 * 0.4);
                pts.push(Point3::new(
                    x,
                    y,
                    0.3 * math::sin(x * 1.3) * math::cos(y + 0.2),
                ));
                w.push(1.0 + 0.3 * ((i + 2 * j) % 3) as f64);
            }
        }
        let n = NurbsSurface::new([ku, kv], pts, Some(w)).unwrap();
        Surface::Nurbs(Arc::new(n))
    }

    /// The analytic surfaces' exact NURBS forms, a free-form patch, an
    /// extrusion of a NURBS circle and revolutions of a circle and a NURBS.
    fn swept_and_splines() -> Vec<Surface> {
        let mut out = vec![spline()];
        for s in analytic_surfaces().iter().skip(1) {
            let (n, _) = s.to_nurbs([[0.0, TAU], [-0.6, 0.4]]).unwrap();
            out.push(Surface::Nurbs(Arc::new(n)));
        }
        let circle = Circle::new(tilted(), 0.8).unwrap();
        let (nc, _) = circle.to_nurbs().unwrap();
        out.push(Surface::Extrusion(Arc::new(
            Extrusion::new(Curve::Nurbs(Arc::new(nc)), Vector3::new(0.2, 0.1, 1.0)).unwrap(),
        )));
        let meridian = Frame::new(
            Point3::new(2.0, 0.0, 0.3),
            Vector3::new(0.0, 1.0, 0.0),
            Vector3::new(1.0, 0.0, 0.0),
        )
        .unwrap();
        let ring = Circle::new(meridian, 0.5).unwrap();
        out.push(Surface::Revolution(Arc::new(
            Revolution::new(Frame::WORLD, Curve::Circle(ring)).unwrap(),
        )));
        let (nr, _) = ring.to_nurbs().unwrap();
        let part = nr.part([0.3, 2.5]).unwrap();
        out.push(Surface::Revolution(Arc::new(
            Revolution::new(Frame::WORLD, Curve::Nurbs(Arc::new(part))).unwrap(),
        )));
        out
    }

    #[test]
    fn jets_are_the_surfaces_derivatives() {
        for s in analytic_surfaces().into_iter().chain(swept_and_splines()) {
            let sheet = Sheet::of(&s);
            let [[u0, u1], [v0, v1]] = s.domain();
            let at = |a: f64, b: f64, x: f64| {
                if a.is_finite() && b.is_finite() {
                    a + (b - a) * x
                } else {
                    x * 4.0 - 2.0
                }
            };
            for (x, y) in [(0.0, 0.0), (0.3, 0.6), (0.71, 0.15), (0.9, 0.95)] {
                let (u, v) = (at(u0, u1, x), at(v0, v1, y));
                let j = sheet.jets_at(u, v);
                let e = s.eval(u, v);
                let part =
                    |f: &dyn Fn(&Jet<f64>) -> f64| Vector3::new(f(&j[0]), f(&j[1]), f(&j[2]));
                let close = |a: Vector3, b: Vector3| (a - b).length() <= 1e-12 * (1.0 + b.length());
                assert!(close(part(&|q| q.v), e.point.to_vector()), "{s:?}");
                assert!(close(part(&|q| q.d[0]), e.du) && close(part(&|q| q.d[1]), e.dv));
                assert!(close(part(&|q| q.h[0]), e.duu) && close(part(&|q| q.h[1]), e.duv));
                assert!(close(part(&|q| q.h[2]), e.dvv), "{s:?}");
                assert!(close(sheet.point(u, v).to_vector(), e.point.to_vector()));
            }
        }
    }

    /// Every value and derivative at a grid of points of each box lies in
    /// the box's enclosures.
    #[test]
    fn enclosures_hold_over_boxes() {
        for s in analytic_surfaces().into_iter().chain(swept_and_splines()) {
            let sheet = Sheet::of(&s);
            let [[u0, u1], [v0, v1]] = s.domain();
            let span = |a: f64, b: f64, x: f64, w: f64| {
                if a.is_finite() && b.is_finite() {
                    [a + (b - a) * x, a + (b - a) * (x + w)]
                } else {
                    [x * 4.0 - 2.0, (x + w) * 4.0 - 2.0]
                }
            };
            // Boxes inside one Bézier patch, across knots, and the whole
            // domain.
            for (x, y, w) in [(0.1, 0.2, 0.05), (0.35, 0.25, 0.3), (0.0, 0.0, 1.0)] {
                let cell = [span(u0, u1, x, w), span(v0, v1, y, w)];
                let p = sheet.jet_box(cell).unwrap();
                for i in 0..=6 {
                    for k in 0..=6 {
                        let u = cell[0][0] + (cell[0][1] - cell[0][0]) * f64::from(i) / 6.0;
                        let v = cell[1][0] + (cell[1][1] - cell[1][0]) * f64::from(k) / 6.0;
                        let e = s.eval(u, v);
                        let want = [e.point.to_vector(), e.du, e.dv, e.duu, e.duv, e.dvv];
                        for (c, pc) in p.iter().enumerate() {
                            let got = [pc.v, pc.d[0], pc.d[1], pc.h[0], pc.h[1], pc.h[2]];
                            for (g, w) in got.iter().zip(want) {
                                let x = w.to_array()[c];
                                let slack = 1e-12 * (1.0 + x.abs());
                                assert!(
                                    g.lo() - slack <= x && x <= g.hi() + slack,
                                    "{s:?} {cell:?} at {u}, {v}: {x} not in {g:?}"
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    /// A box past a periodic direction's end wraps round to its start.
    #[test]
    fn periodic_boxes_wrap() {
        let s = &swept_and_splines()[2];
        let Surface::Nurbs(n) = s else { unreachable!() };
        assert!(n.periods().0.is_some());
        let sheet = Sheet::of(s);
        let p = sheet
            .jet_box([[TAU - 0.2, TAU + 0.3], [-0.5, -0.4]])
            .unwrap();
        for u in [TAU - 0.2, TAU - 0.05, 0.1, 0.3] {
            let q = s.point(u, -0.45).to_vector().to_array();
            for (pc, qc) in p.iter().zip(q) {
                assert!(pc.v.lo() - 1e-12 <= qc && qc <= pc.v.hi() + 1e-12);
            }
        }
    }
}
