//! Certified bounds on derivatives over a parameter range, for meshing.
//!
//! A bound here is an upper bound on a derivative's length over every
//! parameter of the range, never an estimate from samples. The analytic
//! types use closed forms, inflated by [`SLACK`] to cover the rounding of
//! `math::sin_cos`; a NURBS uses interval arithmetic on the control nets of
//! its Bézier pieces cut down to the range (see `nurbs::bound`).

use super::{Curve, Frame, Surface, Vector3};
use crate::interval::Interval;
use crate::math;
use core::f64::consts::{FRAC_PI_2, PI, TAU};

/// Relative inflation of the closed-form bounds: far more than the few
/// ulps of rounding in them.
pub(crate) const SLACK: f64 = 1.0 + 1e-12;

/// Bounds on a curve's derivatives with respect to its parameter `t`, over
/// a range.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CurveBound {
    /// An upper bound on `|C''|`.
    pub d2: f64,
    /// An upper bound on how fast the unit tangent turns, in radians per
    /// unit of `t`: `|C' × C''| / |C'|²`. Infinite where `C'` may vanish.
    pub turn: f64,
}

/// Bounds on a surface's derivatives with respect to `(u, v)`, over a
/// parameter box.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SurfaceBound {
    /// Upper bounds on `|Suu|`, `|Suv|` and `|Svv|`.
    pub duu: f64,
    pub duv: f64,
    pub dvv: f64,
    /// Upper bounds on how fast the unit normal turns along `u` and along
    /// `v`, in radians per unit of parameter. Infinite where `Su × Sv` may
    /// vanish and no closed form is known.
    pub turn_u: f64,
    pub turn_v: f64,
}

/// No bound at all: for a box over which nothing can be enclosed.
pub(crate) const INFINITE_BOUND: SurfaceBound = SurfaceBound {
    duu: f64::INFINITY,
    duv: f64::INFINITY,
    dvv: f64::INFINITY,
    turn_u: f64::INFINITY,
    turn_v: f64::INFINITY,
};

/// Bounds from enclosures of a surface's jets over a box: the second
/// derivatives' lengths, and the unit normal's turning rates, `|N_u| <=
/// |(S_u × S_v)_u| / |S_u × S_v|` and likewise along `v` (a unit vector
/// `w / |w|` turns no faster than `|w'| / |w|`). Infinite turning where
/// `S_u × S_v` may vanish.
pub(crate) fn from_jets(q: &super::jet::JVec<Interval>) -> SurfaceBound {
    let part = |f: &dyn Fn(&super::jet::Jet<Interval>) -> Interval| -> IVec3 {
        [f(&q[0]), f(&q[1]), f(&q[2])]
    };
    let cross = |a: IVec3, b: IVec3| -> IVec3 {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    };
    let add = |a: IVec3, b: IVec3| -> IVec3 { core::array::from_fn(|k| a[k] + b[k]) };
    let square = |a: IVec3| a[0].square() + a[1].square() + a[2].square();
    let hi = |a: IVec3| math::sqrt(square(a).hi()) * SLACK;
    let (su, sv) = (part(&|j| j.d[0]), part(&|j| j.d[1]));
    let (suu, suv, svv) = (part(&|j| j.h[0]), part(&|j| j.h[1]), part(&|j| j.h[2]));
    let n = cross(su, sv);
    let low = square(n).lo();
    let turn = |w: IVec3| {
        if low > 0.0 {
            hi(w) / (math::sqrt(low) / SLACK)
        } else {
            f64::INFINITY
        }
    };
    SurfaceBound {
        duu: hi(suu),
        duv: hi(suv),
        dvv: hi(svv),
        turn_u: turn(add(cross(suu, sv), cross(su, suv))),
        turn_v: turn(add(cross(suv, sv), cross(su, svv))),
    }
}

/// The least and greatest of `cos` over `[a, b]`.
pub(crate) fn cos_range([a, b]: [f64; 2]) -> (f64, f64) {
    if !(a.is_finite() && b.is_finite()) {
        return (-1.0, 1.0);
    }
    let (ca, cb) = (math::cos(a), math::cos(b));
    let hi = if hits([a, b], 0.0, TAU) {
        1.0
    } else {
        ca.max(cb)
    };
    let lo = if hits([a, b], PI, TAU) {
        -1.0
    } else {
        ca.min(cb)
    };
    (lo, hi)
}

/// True when `[a, b]` holds `x + k step` for some whole `k`.
fn hits([a, b]: [f64; 2], x: f64, step: f64) -> bool {
    !(a.is_finite() && b.is_finite()) || x + ((a - x) / step).ceil() * step <= b
}

/// The greatest of `|sin|` over `[a, b]`.
pub(crate) fn sin_abs_max([a, b]: [f64; 2]) -> f64 {
    if hits([a, b], FRAC_PI_2, PI) {
        1.0
    } else {
        math::sin(a).abs().max(math::sin(b).abs())
    }
}

/// The greatest of `|cos|` over `[a, b]`.
pub(crate) fn cos_abs_max([a, b]: [f64; 2]) -> f64 {
    if hits([a, b], 0.0, PI) {
        1.0
    } else {
        math::cos(a).abs().max(math::cos(b).abs())
    }
}

/// Enclosures of `cos` and `sin` over `[a, b]`, widened by `1e-15` to
/// cover the rounding of `math::cos` at the ends (and of `a - π/2`, at an
/// extremum where the function is flat), and kept inside `[-1, 1]`.
pub(crate) fn trig_box([a, b]: [f64; 2]) -> (Interval, Interval) {
    let widen = |(lo, hi): (f64, f64)| {
        Interval::new((lo - 1e-15).max(-1.0), (hi + 1e-15).min(1.0)).expect("ordered")
    };
    (
        widen(cos_range([a, b])),
        widen(cos_range([a - FRAC_PI_2, b - FRAC_PI_2])),
    )
}

/// An interval vector: a box in model space.
pub(crate) type IVec3 = [Interval; 3];

/// `o + x a + y b` for points or directions `x`, `y` and intervals `a`,
/// `b`.
fn combine(o: Vector3, x: Vector3, a: Interval, y: Vector3, b: Interval) -> IVec3 {
    let p = Interval::point;
    let (o, x, y) = (o.to_array(), x.to_array(), y.to_array());
    core::array::from_fn(|k| p(o[k]) + p(x[k]) * a + p(y[k]) * b)
}

impl Curve {
    /// The point and its first three derivatives at `t`: closed forms for
    /// the analytic curves, [`NurbsCurve::jet3`] for a NURBS.
    pub(crate) fn jet3(&self, t: f64) -> [Vector3; 4] {
        match self {
            Self::Line(l) => [
                l.origin().to_vector() + l.dir().get() * t,
                l.dir().get(),
                Vector3::ZERO,
                Vector3::ZERO,
            ],
            Self::Circle(_) | Self::Ellipse(_) => {
                let e = self.eval(t);
                [e.point.to_vector(), e.d1, e.d2, -e.d1]
            }
            Self::Nurbs(n) => n.jet3(t),
        }
    }

    /// Enclosures of the point and its first three derivatives at every
    /// parameter of `[t0, t1]`: closed forms over enclosures of `cos t`
    /// and `sin t` for a circle or an ellipse, a NURBS as
    /// [`NurbsCurve::jet_box`] gives. A line's must be a finite range.
    pub(crate) fn jet_box(&self, [t0, t1]: [f64; 2]) -> [IVec3; 4] {
        let zero = [Interval::point(0.0); 3];
        let conic = |f: &Frame, a: f64, b: f64| {
            let (c, s) = trig_box([t0, t1]);
            let (x, y) = (f.x().get(), f.y().get());
            let o = f.origin().to_vector();
            let d1 = combine(
                Vector3::ZERO,
                x,
                -s * Interval::point(a),
                y,
                c * Interval::point(b),
            );
            [
                combine(o, x, c * Interval::point(a), y, s * Interval::point(b)),
                d1,
                combine(
                    Vector3::ZERO,
                    x,
                    -c * Interval::point(a),
                    y,
                    -s * Interval::point(b),
                ),
                d1.map(|v| -v),
            ]
        };
        match self {
            Self::Line(l) => {
                let t = Interval::new(t0, t1).expect("an ordered finite range");
                let d = l.dir().get();
                [
                    combine(
                        l.origin().to_vector(),
                        d,
                        t,
                        Vector3::ZERO,
                        Interval::point(0.0),
                    ),
                    d.to_array().map(Interval::point),
                    zero,
                    zero,
                ]
            }
            Self::Circle(c) => conic(c.frame(), c.radius(), c.radius()),
            Self::Ellipse(e) => conic(e.frame(), e.major(), e.minor()),
            Self::Nurbs(n) => n.jet_box([t0, t1]),
        }
    }

    /// Bounds on the derivatives over `[t0, t1]`. A NURBS is bounded over
    /// the part of the range inside its domain; a periodic one wraps.
    pub fn derivative_bound(&self, range: [f64; 2]) -> CurveBound {
        match self {
            Self::Line(_) => CurveBound { d2: 0.0, turn: 0.0 },
            Self::Circle(c) => CurveBound {
                d2: c.radius() * SLACK,
                turn: SLACK,
            },
            // |C''| = |a cos t x + b sin t y| <= a, and
            // |C' × C''| / |C'|² = ab / |C'|² <= a / b.
            Self::Ellipse(e) => CurveBound {
                d2: e.major() * SLACK,
                turn: e.major() / e.minor() * SLACK,
            },
            Self::Nurbs(n) => n.derivative_bound(range),
        }
    }
}

impl Surface {
    /// Bounds on the derivatives over the box `[[u0, u1], [v0, v1]]`. A
    /// NURBS is bounded over the part of the box inside its domain; a
    /// periodic direction wraps.
    pub fn derivative_bound(&self, [ru, rv]: [[f64; 2]; 2]) -> SurfaceBound {
        match self {
            Self::Plane(_) => SurfaceBound {
                duu: 0.0,
                duv: 0.0,
                dvv: 0.0,
                turn_u: 0.0,
                turn_v: 0.0,
            },
            // Suu = -r radial; the normal is the radial direction.
            Self::Cylinder(c) => SurfaceBound {
                duu: c.radius() * SLACK,
                duv: 0.0,
                dvv: 0.0,
                turn_u: SLACK,
                turn_v: 0.0,
            },
            // With ρ(v) = r + v tan α: Suu = -ρ radial, Suv = tan α turn;
            // the normal is radial cos α - z sin α.
            Self::Cone(c) => {
                let (sin, cos) = math::sin_cos(c.half_angle());
                let rho = |v: f64| (c.radius() + v * sin / cos).abs();
                SurfaceBound {
                    duu: rho(rv[0]).max(rho(rv[1])) * SLACK,
                    duv: sin / cos * SLACK,
                    dvv: 0.0,
                    turn_u: cos * SLACK,
                    turn_v: 0.0,
                }
            }
            // Suu = -r cos v radial, Suv = -r sin v turn, Svv = -r out; the
            // normal is out, turning by |cos v| along u and 1 along v.
            Self::Sphere(s) => {
                let r = s.radius();
                let c = cos_abs_max(rv);
                SurfaceBound {
                    duu: r * c * SLACK,
                    duv: r * sin_abs_max(rv) * SLACK,
                    dvv: r * SLACK,
                    turn_u: c * SLACK,
                    turn_v: SLACK,
                }
            }
            // As the sphere, with the ring radius R + r cos v along u.
            Self::Torus(t) => {
                let (_, cos_hi) = cos_range(rv);
                SurfaceBound {
                    duu: (t.major() + t.minor() * cos_hi) * SLACK,
                    duv: t.minor() * sin_abs_max(rv) * SLACK,
                    dvv: t.minor() * SLACK,
                    turn_u: cos_abs_max(rv) * SLACK,
                    turn_v: SLACK,
                }
            }
            Self::Nurbs(n) => n.derivative_bound([ru, rv]),
            Self::Extrusion(e) => e.derivative_bound([ru, rv]),
            Self::Revolution(r) => r.derivative_bound([ru, rv]),
            Self::Pipe(p) => p.derivative_bound([ru, rv]),
            Self::Sweep(s) => s.derivative_bound([ru, rv]),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{
        Circle, Cone, Cylinder, Ellipse, Frame, KnotVector, NurbsCurve, Plane, Point3, Sphere,
        Torus, Vector3,
    };
    use std::sync::Arc;

    #[test]
    fn trigonometric_ranges() {
        assert_eq!(cos_range([-0.5, 0.5]).1, 1.0);
        assert_eq!(cos_range([3.0, 3.5]).0, -1.0);
        let (lo, hi) = cos_range([0.5, 1.0]);
        assert_eq!((lo, hi), (math::cos(1.0), math::cos(0.5)));
        // A whole turn anywhere, and unbounded ranges.
        assert_eq!(cos_range([7.0, 7.0 + TAU]), (-1.0, 1.0));
        assert_eq!(cos_range([0.0, f64::INFINITY]), (-1.0, 1.0));
        // Shifted by whole turns.
        assert_eq!(cos_range([TAU - 0.1, TAU + 0.1]).1, 1.0);
        assert_eq!(sin_abs_max([0.0, 0.1]), math::sin(0.1));
        assert_eq!(sin_abs_max([1.0, 2.0]), 1.0);
        assert_eq!(sin_abs_max([-2.0, -1.0]), 1.0);
        assert_eq!(cos_abs_max([3.0, 3.2]), 1.0);
        assert_eq!(cos_abs_max([1.0, 1.2]), math::cos(1.0));
    }

    /// Checks a curve bound against the derivatives at many parameters.
    fn holds_for_curve(c: &Curve, [a, b]: [f64; 2]) {
        let bound = c.derivative_bound([a, b]);
        for i in 0..=400 {
            let t = a + (b - a) * f64::from(i) / 400.0;
            let e = c.eval(t);
            assert!(e.d2.length() <= bound.d2, "|C''| at {t}");
            let speed = e.d1.length_squared();
            if speed > 0.0 {
                let turn = e.d1.cross(e.d2).length() / speed;
                assert!(turn <= bound.turn, "turn at {t}: {turn} > {}", bound.turn);
            }
        }
    }

    #[test]
    fn analytic_curves() {
        let frame = Frame::new(
            Point3::new(1.0, 2.0, 3.0),
            Vector3::new(0.3, -0.2, 1.0),
            Vector3::new(1.0, 0.5, 0.0),
        )
        .unwrap();
        let circle = Curve::Circle(Circle::new(frame, 2.0).unwrap());
        assert_eq!(
            circle.derivative_bound([0.0, 1.0]),
            CurveBound {
                d2: 2.0 * SLACK,
                turn: SLACK
            }
        );
        holds_for_curve(&circle, [-1.0, 7.0]);
        let ellipse = Curve::Ellipse(Ellipse::new(frame, 3.0, 0.5).unwrap());
        holds_for_curve(&ellipse, [0.0, TAU]);
        assert_eq!(ellipse.derivative_bound([0.0, 1.0]).turn, 6.0 * SLACK);
    }

    /// A degree 3 curve with uneven weights over three spans.
    pub(crate) fn wavy() -> Curve {
        let k = KnotVector::clamped(3, &[0.0, 0.4, 1.0, 2.5], &[4, 1, 2, 4]).unwrap();
        let pts = vec![
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(1.0, 3.0, -1.0),
            Point3::new(2.0, -1.0, 0.5),
            Point3::new(4.0, 2.0, 2.0),
            Point3::new(5.0, 0.0, 1.0),
            Point3::new(6.0, 1.0, -2.0),
            Point3::new(8.0, 0.0, 0.0),
        ];
        let w = vec![1.0, 0.5, 2.0, 0.8, 1.5, 0.3, 1.0];
        Curve::Nurbs(Arc::new(NurbsCurve::new(k, pts, Some(w)).unwrap()))
    }

    #[test]
    fn nurbs_curves() {
        let c = wavy();
        for range in [[0.0, 2.5], [0.1, 0.3], [0.3, 1.7], [2.4, 2.5]] {
            holds_for_curve(&c, range);
        }
        // Narrower ranges give tighter bounds.
        let whole = c.derivative_bound([0.0, 2.5]);
        let part = c.derivative_bound([1.2, 1.3]);
        assert!(part.d2 < whole.d2);
        assert!(part.turn < whole.turn);
        // Close to the true value over a short range.
        let tight = c.derivative_bound([1.25, 1.25 + 1e-3]);
        let true_d2 = c.eval(1.25).d2.length();
        assert!(tight.d2 <= true_d2 * 1.01, "{} vs {true_d2}", tight.d2);
    }

    /// Checks a curve's jet enclosures against its derivatives at many
    /// parameters, allowing for their rounding.
    fn jets_hold_for_curve(c: &Curve, [a, b]: [f64; 2]) {
        let boxes = c.jet_box([a, b]);
        for i in 0..=200 {
            let t = a + (b - a) * f64::from(i) / 200.0;
            for (k, v) in c.jet3(t).iter().enumerate() {
                for (j, x) in v.to_array().iter().enumerate() {
                    let iv = boxes[k][j];
                    let slack = 1e-12 * (1.0 + x.abs());
                    assert!(
                        iv.lo() - slack <= *x && *x <= iv.hi() + slack,
                        "derivative {k} component {j} at {t}: {x} outside {iv:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn jet_enclosures() {
        let frame = Frame::new(
            Point3::new(1.0, 2.0, 3.0),
            Vector3::new(0.3, -0.2, 1.0),
            Vector3::new(1.0, 0.5, 0.0),
        )
        .unwrap();
        let ellipse = Curve::Ellipse(Ellipse::new(frame, 3.0, 0.5).unwrap());
        for range in [[0.0, TAU], [0.2, 0.3], [-1.0, 1.0], [3.0, 3.3]] {
            jets_hold_for_curve(&ellipse, range);
        }
        jets_hold_for_curve(&Curve::Circle(Circle::new(frame, 2.0).unwrap()), [1.0, 2.0]);
        let line = Curve::Line(
            crate::geom::Line::new(Point3::new(1.0, 0.0, 0.0), Vector3::new(1.0, 1.0, 0.0))
                .unwrap(),
        );
        jets_hold_for_curve(&line, [-2.0, 3.0]);
        let c = wavy();
        for range in [[0.0, 2.5], [0.1, 0.3], [0.3, 1.7], [2.4, 2.5]] {
            jets_hold_for_curve(&c, range);
        }
        // The third derivative of the ellipse against its closed form, and
        // tight over a short range.
        let [_, d1, _, d3] = ellipse.jet3(0.7);
        assert!((d1 + d3).length() == 0.0);
        let tight = c.jet_box([1.25, 1.25 + 1e-4])[3];
        let true_d3 = c.jet3(1.25)[3];
        for (k, x) in true_d3.to_array().iter().enumerate() {
            assert!(tight[k].hi() - tight[k].lo() <= 0.05 * (1.0 + x.abs()));
        }
        // A periodic NURBS across its seam.
        let circle = Curve::Circle(Circle::new(Frame::WORLD, 2.0).unwrap());
        let (n, _) = circle.to_nurbs([0.0, 0.0]).unwrap();
        jets_hold_for_curve(&Curve::Nurbs(Arc::new(n)), [5.5, 7.0]);
    }

    #[test]
    fn periodic_nurbs_ranges_wrap() {
        let circle = Curve::Circle(Circle::new(Frame::WORLD, 2.0).unwrap());
        let (n, _) = circle.to_nurbs([0.0, 0.0]).unwrap();
        let c = Curve::Nurbs(Arc::new(n));
        // Across the period's end, and past it.
        holds_for_curve(&c, [5.5, 7.0]);
        holds_for_curve(&c, [-1.0, 0.5]);
        holds_for_curve(&c, [0.0, 3.0 * TAU]);
    }

    /// Checks a surface bound against the derivatives on a grid.
    fn holds_for_surface(s: &Surface, [ru, rv]: [[f64; 2]; 2]) {
        let b = s.derivative_bound([ru, rv]);
        for i in 0..=40 {
            for j in 0..=40 {
                let u = ru[0] + (ru[1] - ru[0]) * f64::from(i) / 40.0;
                let v = rv[0] + (rv[1] - rv[0]) * f64::from(j) / 40.0;
                let e = s.eval(u, v);
                let at = format!("{s:?} at ({u}, {v})");
                assert!(e.duu.length() <= b.duu, "duu {at}");
                assert!(e.duv.length() <= b.duv, "duv {at}");
                assert!(e.dvv.length() <= b.dvv, "dvv {at}");
                let Some(n) = s.normal(u, v) else { continue };
                // Finite differences of the unit normal.
                let h = 1e-6;
                let turn = |du: f64, dv: f64| {
                    let Some(m) = s.normal(u + du, v + dv) else {
                        return 0.0;
                    };
                    (m.get() - n.get()).length() / h
                };
                let slack = 1.0 + 1e-4;
                assert!(turn(h, 0.0) <= b.turn_u * slack + 1e-6, "turn_u {at}");
                assert!(turn(0.0, h) <= b.turn_v * slack + 1e-6, "turn_v {at}");
            }
        }
    }

    #[test]
    fn analytic_surfaces() {
        let frame = Frame::new(
            Point3::new(1.0, 2.0, 3.0),
            Vector3::new(0.3, -0.2, 1.0),
            Vector3::new(1.0, 0.5, 0.0),
        )
        .unwrap();
        let surfaces = [
            Surface::Plane(Plane::new(frame)),
            Surface::Cylinder(Cylinder::new(frame, 1.5).unwrap()),
            Surface::Cone(Cone::new(frame, 1.5, 0.4).unwrap()),
            Surface::Sphere(Sphere::new(frame, 2.0).unwrap()),
            Surface::Torus(Torus::new(frame, 3.0, 1.0).unwrap()),
        ];
        for s in &surfaces {
            holds_for_surface(s, [[0.0, TAU], [-1.2, 1.4]]);
            holds_for_surface(s, [[2.0, 2.5], [0.3, 0.5]]);
        }
        // Near a sphere's pole the normal hardly turns along u.
        let near_pole = surfaces[3].derivative_bound([[0.0, TAU], [1.5, FRAC_PI_2]]);
        assert!(near_pole.turn_u <= math::cos(1.5) * SLACK);
        assert_eq!(near_pole.turn_v, SLACK);
    }

    #[test]
    fn nurbs_surfaces() {
        use crate::geom::{KnotVector, NurbsSurface};
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
        let wavy = Surface::Nurbs(Arc::new(
            NurbsSurface::new([ku, kv], points, Some(weights)).unwrap(),
        ));
        holds_for_surface(&wavy, [[0.0, 2.0], [0.0, 3.0]]);
        holds_for_surface(&wavy, [[0.4, 0.6], [2.5, 3.5]]);
        let part = wavy.derivative_bound([[1.0, 1.01], [0.2, 0.21]]);
        let whole = wavy.derivative_bound([[0.0, 2.0], [0.0, 3.0]]);
        assert!(part.duu < whole.duu && part.turn_u < whole.turn_u);
        // A NURBS sphere: the poles are collapsed rows, where the normal's
        // turning is unbounded.
        let sphere = Surface::Sphere(Sphere::new(Frame::WORLD, 2.0).unwrap());
        let (n, _) = sphere.to_nurbs([[0.0, 0.0], [0.0, 0.0]]).unwrap();
        let n = Surface::Nurbs(Arc::new(n));
        holds_for_surface(&n, [[0.3, 1.2], [-1.0, 1.0]]);
        // A box of no width on the domain's edge stays tight: the
        // derivatives are cut down after differencing, not before.
        // (v = 0 is a knot: the edge of two patches.)
        let edge = n.derivative_bound([[0.3, 0.4], [0.0, 0.0]]);
        let wider = n.derivative_bound([[0.3, 0.4], [0.0, 0.01]]);
        assert!(edge.duu <= wider.duu && edge.dvv <= wider.dvv, "{edge:?}");
        let pole = n.derivative_bound([[0.0, 1.0], [1.4, FRAC_PI_2]]);
        assert_eq!(pole.turn_u, f64::INFINITY);
        assert!(pole.duu.is_finite());
    }
}
