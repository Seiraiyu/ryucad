//! Surfaces written over jets: the analytic surfaces and the pipe.
//!
//! A [`Sheet`] gives a surface's point as jets in its two parameters (see
//! [`crate::geom::jet`]): in floating point at a point, for Newton's method
//! and directions, or over [`Interval`]s over a box of parameters, for
//! certified enclosures. The analytic surfaces are their closed forms
//! over `cos` and `sin` of their angles; a pipe is
//! [`Pipe::jets`] over its spine's derivatives.

use super::bound2::trig_box;
use super::jet::{self, JVec, Jet, Num};
use super::{Pipe, Point3, Surface, Vector3};
use crate::interval::Interval;
use crate::math;

/// A surface with a closed form over jets.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Sheet<'a> {
    Analytic(&'a Surface),
    Pipe(&'a Pipe),
}

impl<'a> Sheet<'a> {
    /// The sheet of a plane, cylinder, cone, sphere, torus or pipe.
    pub(crate) fn of(s: &'a Surface) -> Option<Self> {
        match s {
            Surface::Pipe(p) => Some(Self::Pipe(p)),
            Surface::Plane(_)
            | Surface::Cylinder(_)
            | Surface::Cone(_)
            | Surface::Sphere(_)
            | Surface::Torus(_) => Some(Self::Analytic(s)),
            _ => None,
        }
    }

    /// Enclosures of the point's jets over a box of parameters; `None`
    /// where a pipe's frame can't be certified over it.
    pub(crate) fn jet_box(&self, [ru, rv]: [[f64; 2]; 2]) -> Option<JVec<Interval>> {
        match self {
            Self::Pipe(p) => p.jet_box([ru, rv]).map(|[p, _]| p),
            Self::Analytic(s) => {
                let u = Interval::new(ru[0], ru[1])?;
                let v = Interval::new(rv[0], rv[1])?;
                analytic(s, [u, v], trig_box(ru), trig_box(rv))
            }
        }
    }

    /// The point's jets at `(u, v)`, in floating point.
    pub(crate) fn jets_at(&self, u: f64, v: f64) -> JVec<f64> {
        let swap = |(s, c): (f64, f64)| (c, s);
        match self {
            Self::Pipe(p) => {
                let d = p.spine().jet3(u).map(Vector3::to_array);
                let (sin, cos) = math::sin_cos(v);
                let [p, _] = p
                    .jets(d, cos, sin)
                    .expect("a certified pipe has a frame on its spine");
                p
            }
            Self::Analytic(s) => {
                analytic(s, [u, v], swap(math::sin_cos(u)), swap(math::sin_cos(v)))
                    .expect("an analytic surface")
            }
        }
    }

    pub(crate) fn point(&self, u: f64, v: f64) -> Point3 {
        match self {
            Self::Pipe(p) => p.point(u, v),
            Self::Analytic(s) => s.point(u, v),
        }
    }
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
    use crate::geom::{Cone, Cylinder, Frame, Plane, Sphere, Torus};

    fn surfaces() -> Vec<Surface> {
        let f = Frame::new(
            Point3::new(0.3, -0.2, 0.5),
            Vector3::new(1.0, 0.4, 0.2),
            Vector3::new(0.0, 1.0, 0.0),
        )
        .unwrap();
        vec![
            Surface::Plane(Plane::new(f)),
            Surface::Cylinder(Cylinder::new(f, 0.7).unwrap()),
            Surface::Cone(Cone::new(f, 0.7, 0.4).unwrap()),
            Surface::Sphere(Sphere::new(f, 0.7).unwrap()),
            Surface::Torus(Torus::new(f, 1.5, 0.4).unwrap()),
        ]
    }

    #[test]
    fn jets_are_the_surfaces_derivatives() {
        for s in surfaces() {
            let sheet = Sheet::of(&s).unwrap();
            for (u, v) in [(0.0, 0.0), (1.2, -0.4), (-2.5, 0.9), (3.0, 0.3)] {
                let j = sheet.jets_at(u, v);
                let e = s.eval(u, v);
                let part =
                    |f: &dyn Fn(&Jet<f64>) -> f64| Vector3::new(f(&j[0]), f(&j[1]), f(&j[2]));
                let close = |a: Vector3, b: Vector3| (a - b).length() <= 1e-12 * (1.0 + b.length());
                assert!(close(part(&|q| q.v), e.point.to_vector()), "{s:?}");
                assert!(close(part(&|q| q.d[0]), e.du) && close(part(&|q| q.d[1]), e.dv));
                assert!(close(part(&|q| q.h[0]), e.duu) && close(part(&|q| q.h[1]), e.duv));
                assert!(close(part(&|q| q.h[2]), e.dvv), "{s:?}");
            }
        }
    }

    #[test]
    fn enclosures_hold_over_boxes() {
        for s in surfaces() {
            let sheet = Sheet::of(&s).unwrap();
            let cell = [[0.4, 0.9], [-0.3, 0.2]];
            let p = sheet.jet_box(cell).unwrap();
            for i in 0..=6 {
                for k in 0..=6 {
                    let u = 0.4 + 0.5 * f64::from(i) / 6.0;
                    let v = -0.3 + 0.5 * f64::from(k) / 6.0;
                    let q = s.point(u, v).to_vector().to_array();
                    for c in 0..3 {
                        let slack = 1e-12;
                        assert!(p[c].v.lo() - slack <= q[c] && q[c] <= p[c].v.hi() + slack);
                    }
                }
            }
        }
    }
}
