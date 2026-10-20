//! Implicit forms of the analytic surfaces, and certified distances from
//! them.
//!
//! In its frame's local coordinates `(x, y, z)`, each analytic surface is
//! the zero set of a polynomial `f`: `z` (plane), `x² + y² - r²`
//! (cylinder), `x² + y² - (r + z tan α)²` (cone, both nappes),
//! `x² + y² + z² - r²` (sphere), and
//! `(x² + y² + z² + R² - r²)² - 4 R² (x² + y²)` (torus). Each factors so
//! that the distance from the surface is at most a known multiple of
//! `|f|` ([`Implicit::distance_bound`]), which turns a bound on `f` along
//! a curve into a certified distance.
//!
//! Local coordinates are computed in interval arithmetic from the frame's
//! axes as stored; that the axes are orthonormal only to rounding, and
//! that the surfaces evaluate their points in floating point, is covered
//! by [`super::SLACK`].

use super::SLACK;
use crate::geom::bernstein::Bernstein;
use crate::geom::bernstein2::Bernstein2;
use crate::geom::chain3::Piece3;
use crate::geom::jet::{self, JVec, Jet, Num};
use crate::geom::{BezierPatch, Frame, Point3, Surface, Vector3};
use crate::interval::Interval;
use crate::math;
use crate::predicates::Sign;

/// An analytic surface's implicit form.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Implicit {
    frame: Frame,
    kind: Kind,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    Plane,
    Cylinder { r: f64 },
    Cone { r: f64, sin: f64, cos: f64 },
    Sphere { r: f64 },
    Torus { major: f64, minor: f64 },
}

fn p(x: f64) -> Interval {
    Interval::point(x)
}

impl Implicit {
    /// The implicit form of an analytic surface; `None` for any other.
    pub(crate) fn of(s: &Surface) -> Option<Self> {
        let frame = *s.frame()?;
        let kind = match s {
            Surface::Plane(_) => Kind::Plane,
            Surface::Cylinder(c) => Kind::Cylinder { r: c.radius() },
            Surface::Cone(c) => {
                let (sin, cos) = math::sin_cos(c.half_angle());
                Kind::Cone {
                    r: c.radius(),
                    sin,
                    cos,
                }
            }
            Surface::Sphere(c) => Kind::Sphere { r: c.radius() },
            Surface::Torus(t) => Kind::Torus {
                major: t.major(),
                minor: t.minor(),
            },
            _ => return None,
        };
        Some(Self { frame, kind })
    }

    /// The degree of `f`: 1, 2 or 4.
    pub(crate) fn degree(&self) -> usize {
        match self.kind {
            Kind::Plane => 1,
            Kind::Torus { .. } => 4,
            _ => 2,
        }
    }

    /// The local coordinates of `v`, a point given as its offset from the
    /// model origin.
    pub(crate) fn local(&self, v: Vector3) -> [Interval; 3] {
        let o = self.frame.origin();
        let d = [p(v.x) - p(o.x), p(v.y) - p(o.y), p(v.z) - p(o.z)];
        let along = |a: Vector3| d[0] * p(a.x) + d[1] * p(a.y) + d[2] * p(a.z);
        [
            along(self.frame.x().get()),
            along(self.frame.y().get()),
            along(self.frame.z().get()),
        ]
    }

    /// `f` at local coordinates.
    pub(crate) fn value(&self, [x, y, z]: [Interval; 3]) -> Interval {
        let rho2 = x.square() + y.square();
        match self.kind {
            Kind::Plane => z,
            Kind::Cylinder { r } => rho2 - p(r).square(),
            Kind::Cone { r, sin, cos } => {
                let q = p(r) + z * tan(sin, cos);
                rho2 - q.square()
            }
            Kind::Sphere { r } => rho2 + z.square() - p(r).square(),
            Kind::Torus { major, minor } => {
                let s = rho2 + z.square() + p(major).square() - p(minor).square();
                s.square() - p(4.0) * p(major).square() * rho2
            }
        }
    }

    /// `f` at a point given as jets in model space: its value and its
    /// derivatives by the jets' two parameters, in floating point or in
    /// intervals. Constants are formed in the jets' own arithmetic (`r²`
    /// as `r · r`, `tan α` as `sin α / cos α`), so interval jets enclose
    /// the true values.
    pub(crate) fn over<T: Num>(&self, q: JVec<T>) -> Jet<T> {
        let c = |v: f64| Jet::constant(T::of(v));
        let axis = |a: Vector3| a.to_array().map(c);
        let d = jet::sub(q, axis(self.frame.origin().to_vector()));
        let x = jet::dot(d, axis(self.frame.x().get()));
        let y = jet::dot(d, axis(self.frame.y().get()));
        let z = jet::dot(d, axis(self.frame.z().get()));
        let rho2 = x * x + y * y;
        match self.kind {
            Kind::Plane => z,
            Kind::Cylinder { r } => rho2 - c(r) * c(r),
            Kind::Cone { r, sin, cos } => {
                let tan = T::of(cos).recip().expect("a cone's cosine is positive") * T::of(sin);
                let q = c(r) + z * Jet::constant(tan);
                rho2 - q * q
            }
            Kind::Sphere { r } => rho2 + z * z - c(r) * c(r),
            Kind::Torus { major, minor } => {
                let s = rho2 + z * z + c(major) * c(major) - c(minor) * c(minor);
                s * s - c(4.0) * c(major) * c(major) * rho2
            }
        }
    }

    /// `f` at a model point.
    pub(crate) fn value_at(&self, q: Point3) -> Interval {
        self.value(self.local(q.to_vector()))
    }

    /// `f` and its gradient at a model point, in floating point: for
    /// Newton's method, never for a decision.
    pub(crate) fn eval_grad(&self, q: Point3) -> (f64, Vector3) {
        let l = self.frame.to_local(q);
        let (x, y, z) = (l.x, l.y, l.z);
        let rho2 = x * x + y * y;
        let (f, g) = match self.kind {
            Kind::Plane => (z, [0.0, 0.0, 1.0]),
            Kind::Cylinder { r } => (rho2 - r * r, [2.0 * x, 2.0 * y, 0.0]),
            Kind::Cone { r, sin, cos } => {
                let t = sin / cos;
                let q = r + z * t;
                (rho2 - q * q, [2.0 * x, 2.0 * y, -2.0 * q * t])
            }
            Kind::Sphere { r } => (rho2 + z * z - r * r, [2.0 * x, 2.0 * y, 2.0 * z]),
            Kind::Torus { major, minor } => {
                let s = rho2 + z * z + major * major - minor * minor;
                let k = 8.0 * major * major;
                (
                    s * s - 4.0 * major * major * rho2,
                    [4.0 * s * x - k * x, 4.0 * s * y - k * y, 4.0 * s * z],
                )
            }
        };
        let (fx, fy, fz) = (
            self.frame.x().get(),
            self.frame.y().get(),
            self.frame.z().get(),
        );
        (f, fx * g[0] + fy * g[1] + fz * g[2])
    }

    /// The certified side of the surface a model point is on: the sign of
    /// `f`, except that a cone's (one nappe) is the sign of `ρ - q`, so
    /// the region of the other nappe is outside. `None` when rounding
    /// can't tell, or the point is on the surface.
    pub(crate) fn side_at(&self, q: Point3) -> Option<Sign> {
        let local = self.local(q.to_vector());
        let s = match self.kind {
            Kind::Cone { r, sin, cos } => {
                let [x, y, z] = local;
                let rho = (x.square() + y.square()).sqrt().expect("a sum of squares");
                (rho - (p(r) + z * tan(sin, cos))).sign()
            }
            _ => self.value(local).sign(),
        };
        s.filter(|&s| s != Sign::Zero)
    }

    /// True for a point certified to lie on the other nappe's side of a
    /// cone's apex, where `f` vanishes off the surface.
    pub(crate) fn beyond_apex(&self, q: Point3) -> bool {
        self.box_beyond_apex(self.local(q.to_vector()))
    }

    /// True for a local box certified to lie on the other nappe's side of
    /// a cone's apex.
    pub(crate) fn box_beyond_apex(&self, [_, _, z]: [Interval; 3]) -> bool {
        match self.kind {
            Kind::Cone { r, sin, cos } => (p(r) + z * tan(sin, cos)).hi() < 0.0,
            _ => false,
        }
    }

    /// `W^d f(A / W)` for a rational piece `A / W`, `d` the degree of
    /// `f`, as a Bernstein polynomial in the piece's local parameter, and
    /// the least weight, a lower bound on `W`. The curve's `f` at local
    /// parameter `s` is the polynomial's value over `W(s)^d`.
    pub(crate) fn on_piece(&self, piece: &Piece3) -> (Bernstein, f64) {
        let mut xs = Vec::with_capacity(piece.pts.len());
        let mut ys = Vec::with_capacity(piece.pts.len());
        let mut zs = Vec::with_capacity(piece.pts.len());
        let mut ws = Vec::with_capacity(piece.pts.len());
        for h in &piece.pts {
            let [x, y, z] = self.local(h.p);
            let w = p(h.w);
            xs.push(x * w);
            ys.push(y * w);
            zs.push(z * w);
            ws.push(w);
        }
        let w_min = piece.pts.iter().map(|h| h.w).fold(f64::INFINITY, f64::min);
        let (x, y, z, w) = (
            Bernstein::new(xs),
            Bernstein::new(ys),
            Bernstein::new(zs),
            Bernstein::new(ws),
        );
        let rho2 = || x.mul(&x).add(&y.mul(&y));
        let poly = match self.kind {
            Kind::Plane => z,
            Kind::Cylinder { r } => rho2().sub(&w.mul(&w).scale(p(r).square())),
            Kind::Cone { r, sin, cos } => {
                let q = w.scale(p(r)).add(&z.scale(tan(sin, cos)));
                rho2().sub(&q.mul(&q))
            }
            Kind::Sphere { r } => rho2().add(&z.mul(&z)).sub(&w.mul(&w).scale(p(r).square())),
            Kind::Torus { major, minor } => {
                let w2 = w.mul(&w);
                let s = rho2()
                    .add(&z.mul(&z))
                    .add(&w2.scale(p(major).square() - p(minor).square()));
                s.mul(&s)
                    .sub(&w2.mul(&rho2()).scale(p(4.0) * p(major).square()))
            }
        };
        (poly, w_min)
    }

    /// `W^d f(A / W)` for a rational patch `A / W`, as a polynomial in the
    /// patch's local parameters, and the least weight, as for
    /// [`Implicit::on_piece`].
    pub(crate) fn on_patch(&self, patch: &BezierPatch) -> (Bernstein2, f64) {
        let [m, n] = patch.degrees;
        let mut xs = Vec::with_capacity(patch.pts.len());
        let mut ys = Vec::with_capacity(patch.pts.len());
        let mut zs = Vec::with_capacity(patch.pts.len());
        let mut ws = Vec::with_capacity(patch.pts.len());
        for h in &patch.pts {
            let [x, y, z] = self.local(h.p);
            let w = p(h.w);
            xs.push(x * w);
            ys.push(y * w);
            zs.push(z * w);
            ws.push(w);
        }
        let w_min = patch.pts.iter().map(|h| h.w).fold(f64::INFINITY, f64::min);
        let (x, y, z, w) = (
            Bernstein2::new(m, n, xs),
            Bernstein2::new(m, n, ys),
            Bernstein2::new(m, n, zs),
            Bernstein2::new(m, n, ws),
        );
        let rho2 = || x.mul(&x).add(&y.mul(&y));
        let poly = match self.kind {
            Kind::Plane => z,
            Kind::Cylinder { r } => rho2().sub(&w.mul(&w).scale(p(r).square())),
            Kind::Cone { r, sin, cos } => {
                let q = w.scale(p(r)).add(&z.scale(tan(sin, cos)));
                rho2().sub(&q.mul(&q))
            }
            Kind::Sphere { r } => rho2().add(&z.mul(&z)).sub(&w.mul(&w).scale(p(r).square())),
            Kind::Torus { major, minor } => {
                let w2 = w.mul(&w);
                let s = rho2()
                    .add(&z.mul(&z))
                    .add(&w2.scale(p(major).square() - p(minor).square()));
                s.mul(&s)
                    .sub(&w2.mul(&rho2()).scale(p(4.0) * p(major).square()))
            }
        };
        (poly, w_min)
    }

    /// An upper bound on the distance from the surface of any point in
    /// the local box `[x, y, z]` where `|f| <= f_max`, plus [`SLACK`].
    ///
    /// The factors, with `ρ` the distance from the axis: the plane's `f`
    /// is its signed distance `d`; a cylinder's is `d (ρ + r)` and a
    /// sphere's `d (|p| + r)`; a torus's is `(D² - r²)((ρ + R)² + z² - r²)`
    /// with `D` the distance from its core circle and `d = D - r`, the
    /// second factor at least `R² - r²`. A cone's distance is at most
    /// `cos α |ρ - q|`, `q = r + z tan α`, and `|f| = |ρ - q| (ρ + q)`, so
    /// where `q > 0` it is at most `cos α min(|f| / (ρ + q), sqrt|f|)`; a
    /// box reaching the apex or past it is bounded by its distance from
    /// the apex, which is on the surface. Each denominator takes the
    /// least value the box allows.
    pub(crate) fn distance_bound(&self, f_max: f64, [x, y, z]: [Interval; 3]) -> f64 {
        let rho2 = x.square() + y.square();
        let rho = rho2.sqrt().expect("a sum of squares");
        let d = match self.kind {
            Kind::Plane => f_max,
            Kind::Cylinder { r } => f_max / (rho.lo() + r).next_down(),
            Kind::Sphere { r } => {
                let q = (rho2 + z.square()).sqrt().expect("a sum of squares");
                f_max / (q.lo() + r).next_down()
            }
            Kind::Torus { major, minor } => {
                let core = ((rho - p(major)).square() + z.square())
                    .sqrt()
                    .expect("a sum of squares");
                let far = (rho + p(major)).square() + z.square() - p(minor).square();
                let far = far.lo().max(major * major - minor * minor);
                f_max / ((core.lo() + minor).next_down() * far).next_down()
            }
            Kind::Cone { r, sin, cos } => {
                let q = p(r) + z * tan(sin, cos);
                if q.lo() > 0.0 {
                    cos * (f_max / (rho.lo() + q.lo()).next_down()).min(math::sqrt(f_max))
                } else {
                    let apex = -r * cos / sin;
                    let dz = z - p(apex);
                    math::sqrt((rho2 + dz.square()).hi())
                }
            }
        };
        (d * (1.0 + 1e-12)).next_up() + SLACK
    }
}

/// `tan α` as an interval, from the cone's own sine and cosine.
fn tan(sin: f64, cos: f64) -> Interval {
    p(sin)
        .checked_div(p(cos))
        .expect("a cone's cosine is positive")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::chain3::Chain3;
    use crate::geom::{Circle, Cone, Curve, Cylinder, Line, Plane, Sphere, Torus};
    use core::f64::consts::{FRAC_PI_4, PI, TAU};

    fn frame() -> Frame {
        Frame::new(
            Point3::new(1.0, 2.0, 3.0),
            Vector3::new(0.3, -0.4, 1.0),
            Vector3::new(1.0, 0.0, 0.0),
        )
        .unwrap()
    }

    fn all() -> [Surface; 5] {
        let f = frame();
        [
            Surface::Plane(Plane::new(f)),
            Surface::Cylinder(Cylinder::new(f, 2.0).unwrap()),
            Surface::Cone(Cone::new(f, 2.0, FRAC_PI_4 / 2.0).unwrap()),
            Surface::Sphere(Sphere::new(f, 2.0).unwrap()),
            Surface::Torus(Torus::new(f, 3.0, 1.0).unwrap()),
        ]
    }

    /// The box holding just `q`, in `s`'s local coordinates.
    fn at(i: &Implicit, q: Point3) -> (f64, [Interval; 3]) {
        let local = i.local(q.to_vector());
        (i.value(local).mag(), local)
    }

    #[test]
    fn points_on_the_surface_have_f_near_zero() {
        for s in all() {
            let i = Implicit::of(&s).unwrap();
            for (u, v) in [(0.0, 0.5), (1.0, 1.5), (4.0, -0.7), (5.5, 2.0)] {
                let (f, local) = at(&i, s.point(u, v));
                assert!(
                    i.distance_bound(f, local) <= 2.0 * SLACK,
                    "{s:?} at {u}, {v}: {f}"
                );
            }
        }
        let nurbs = Surface::Nurbs(std::sync::Arc::new(
            Surface::Plane(Plane::new(frame()))
                .to_nurbs([[0.0, 1.0], [0.0, 1.0]])
                .unwrap()
                .0,
        ));
        assert_eq!(Implicit::of(&nurbs), None);
    }

    #[test]
    fn the_distance_bound_holds_off_the_surface() {
        let mut seed = 7u64;
        let mut next = || {
            seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
            (seed >> 11) as f64 / (1u64 << 53) as f64
        };
        for s in all() {
            let i = Implicit::of(&s).unwrap();
            for _ in 0..500 {
                let q = Point3::new(
                    next() * 12.0 - 5.0,
                    next() * 12.0 - 4.0,
                    next() * 12.0 - 3.0,
                );
                let (f, local) = at(&i, q);
                let d = s.distance(q);
                assert!(i.distance_bound(f, local) >= d, "{s:?} at {q:?}");
            }
            // Close to the surface the bound is close to the distance.
            for (u, v) in [(0.5, 0.5), (2.0, 1.0)] {
                let e = s.eval(u, v);
                let n = s.normal(u, v).unwrap().get();
                let q = e.point + n * 1e-6;
                let (f, local) = at(&i, q);
                let b = i.distance_bound(f, local);
                assert!(b >= s.distance(q) && b <= 1e-5, "{s:?}: {b}");
            }
        }
    }

    #[test]
    fn a_cone_near_its_apex() {
        let s = Surface::Cone(Cone::new(Frame::WORLD, 1.0, FRAC_PI_4).unwrap());
        let i = Implicit::of(&s).unwrap();
        // The apex is at z = -1. Points on the other nappe have f = 0 but
        // are far from this one.
        let other = Point3::new(1.0, 0.0, -2.0);
        let (f, local) = at(&i, other);
        assert!(f <= 1e-12);
        assert!(i.distance_bound(f, local) >= s.distance(other));
        let apex = Point3::new(0.0, 0.0, -1.0);
        let (f, local) = at(&i, apex);
        assert!(i.distance_bound(f, local) <= 2.0 * SLACK);
    }

    #[test]
    fn curves_substitute_into_the_implicit_form() {
        // A circle round the sphere's equator lies on it; a line through
        // the centre crosses it twice.
        let s = Surface::Sphere(Sphere::new(Frame::WORLD, 2.0).unwrap());
        let i = Implicit::of(&s).unwrap();
        let equator = Curve::Circle(Circle::new(Frame::WORLD, 2.0).unwrap());
        for piece in &Chain3::new(&equator, 0.0, TAU).pieces {
            let (poly, w) = i.on_piece(piece);
            assert!(poly.range().mag() / (w * w) <= 1e-12);
        }
        let line = Curve::Line(
            Line::new(Point3::new(-5.0, 0.0, 0.0), Vector3::new(1.0, 0.0, 0.0)).unwrap(),
        );
        let chain = Chain3::new(&line, 0.0, 10.0);
        let (poly, _) = i.on_piece(&chain.pieces[0]);
        assert_eq!(poly.degree(), 2);
        for k in 0..=10 {
            let s = f64::from(k) / 10.0;
            let x = -5.0 + 10.0 * s;
            assert!(
                poly.eval(s).contains(x * x - 4.0)
                    || (poly.eval(s).mag() - (x * x - 4.0).abs()).abs() < 1e-12
            );
        }
        // A torus: degree 4 in a line's parameter.
        let t = Surface::Torus(Torus::new(Frame::WORLD, 3.0, 1.0).unwrap());
        let (poly, _) = Implicit::of(&t).unwrap().on_piece(&chain.pieces[0]);
        assert_eq!(poly.degree(), 4);
        let roots = poly.roots(1e-14);
        let xs: Vec<f64> = roots
            .iter()
            .map(|r| -5.0 + 10.0 * (r.range()[0] + r.range()[1]) / 2.0)
            .collect();
        for (x, want) in xs.iter().zip([-4.0, -2.0, 2.0, 4.0]) {
            assert!((x - want).abs() < 1e-12, "{xs:?}");
        }
        let _ = PI;
    }
}
