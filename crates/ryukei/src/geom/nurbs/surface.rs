//! NURBS surfaces.

use super::KnotVector;
use super::bezier::{BezierPatch, patches};
use super::eval::check_weights;
use super::refine::{Hom, check_split, clamp, refine_flat, split_at};
use crate::geom::{Box3, Point3, PrincipalCurvatures, SurfaceEval, UnitVector3, Vector3};
use crate::{Error, NurbsError};
use std::sync::OnceLock;

/// One of a surface's two parameter directions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    U,
    V,
}

/// A rational B-spline surface, tensor product of a `u` and a `v` knot
/// vector. Control points are row-major: point `(i, j)` (the `i`-th along
/// `u`, the `j`-th along `v`) is at index `i * counts[1] + j`. Immutable
/// once built.
///
/// Like [`super::NurbsCurve`], it builds and keeps its Bézier patches the
/// first time they are needed; the cache takes no part in equality.
#[derive(Clone, Debug)]
pub struct NurbsSurface {
    knots: [KnotVector; 2],
    points: Box<[Vector3]>,
    weights: Box<[f64]>,
    patches: OnceLock<Box<[BezierPatch]>>,
}

impl PartialEq for NurbsSurface {
    fn eq(&self, other: &Self) -> bool {
        (&self.knots, &self.points, &self.weights) == (&other.knots, &other.points, &other.weights)
    }
}

impl NurbsSurface {
    /// A surface from its knots, row-major control points and weights
    /// (`None` for all 1). Fails like [`super::NurbsCurve::new`].
    pub fn new(
        knots: [KnotVector; 2],
        points: Vec<Point3>,
        weights: Option<Vec<f64>>,
    ) -> Result<Self, Error> {
        if points.len() != knots[0].control_count() * knots[1].control_count() {
            return Err(Error::InvalidNurbs(NurbsError::ControlCount));
        }
        let points = points
            .iter()
            .map(|p| Point3::try_new(p.x, p.y, p.z).map(Point3::to_vector))
            .collect::<Result<Box<[_]>, _>>()?;
        let weights = check_weights(points.len(), weights)?;
        Ok(Self {
            knots,
            points,
            weights,
            patches: OnceLock::new(),
        })
    }

    pub fn knots(&self) -> &[KnotVector; 2] {
        &self.knots
    }

    pub fn degrees(&self) -> [usize; 2] {
        [self.knots[0].degree(), self.knots[1].degree()]
    }

    /// The number of control points along `u` and `v`.
    pub fn counts(&self) -> [usize; 2] {
        [self.knots[0].control_count(), self.knots[1].control_count()]
    }

    /// The control point `(i, j)`.
    pub fn control_point(&self, i: usize, j: usize) -> Point3 {
        Point3::ORIGIN + self.points[i * self.counts()[1] + j]
    }

    /// Every control point, row-major.
    pub fn control_points(&self) -> impl ExactSizeIterator<Item = Point3> + '_ {
        self.points.iter().map(|&v| Point3::ORIGIN + v)
    }

    /// The weights, row-major.
    pub fn weights(&self) -> &[f64] {
        &self.weights
    }

    pub fn domain(&self) -> [[f64; 2]; 2] {
        [self.knots[0].domain(), self.knots[1].domain()]
    }

    pub fn periods(&self) -> (Option<f64>, Option<f64>) {
        (self.knots[0].period(), self.knots[1].period())
    }

    pub fn point(&self, u: f64, v: f64) -> Point3 {
        self.eval(u, v).point
    }

    /// The point and its partial derivatives up to second order (The NURBS
    /// Book, A3.6 and A4.4): weighted sums over the `v` basis first, then
    /// the `u` basis, then the quotient rule.
    pub fn eval(&self, u: f64, v: f64) -> SurfaceEval {
        let (bu, bv) = (self.knots[0].basis(u), self.knots[1].basis(v));
        let [p, q] = self.degrees();
        let [nu, nv] = self.counts();
        let mut a = [[Vector3::ZERO; 3]; 3];
        let mut w = [[0.0; 3]; 3];
        for i in 0..=p {
            let row = (bu.span() - p + i) % nu;
            let mut ta = [Vector3::ZERO; 3];
            let mut tw = [0.0; 3];
            for j in 0..=q {
                let idx = row * nv + (bv.span() - q + j) % nv;
                let wi = self.weights[idx];
                let pw = self.points[idx] * wi;
                for l in 0..3 {
                    let nl = bv.values(l)[j];
                    ta[l] += pw * nl;
                    tw[l] += wi * nl;
                }
            }
            for k in 0..3 {
                let nk = bu.values(k)[i];
                for l in 0..3 - k {
                    a[k][l] += ta[l] * nk;
                    w[k][l] += tw[l] * nk;
                }
            }
        }
        let w0 = w[0][0];
        let s = a[0][0] / w0;
        let su = (a[1][0] - s * w[1][0]) / w0;
        let sv = (a[0][1] - s * w[0][1]) / w0;
        let suu = (a[2][0] - su * (2.0 * w[1][0]) - s * w[2][0]) / w0;
        let svv = (a[0][2] - sv * (2.0 * w[0][1]) - s * w[0][2]) / w0;
        let suv = (a[1][1] - s * w[1][1] - sv * w[1][0] - su * w[0][1]) / w0;
        SurfaceEval {
            point: Point3::ORIGIN + s,
            du: su,
            dv: sv,
            duu: suu,
            duv: suv,
            dvv: svv,
        }
    }

    /// The derivatives against the domain normalized to `[0, 1]²`, so
    /// vanishing is judged in metres whatever the knots' units.
    fn unit_eval(&self, u: f64, v: f64) -> SurfaceEval {
        let [[u0, u1], [v0, v1]] = self.domain();
        self.eval(u, v).scaled(u1 - u0, v1 - v0)
    }

    /// The unit normal, along `du × dv`. Where that vanishes (a pole: a
    /// collapsed row of control points) it is the limit normal: the first
    /// of `du × duv`, `duv × dv`, `duu × dv` and `du × dvv` that does not
    /// vanish, turned to agree with the normal a short step into the
    /// domain. `None` if every candidate vanishes.
    pub fn normal(&self, u: f64, v: f64) -> Option<UnitVector3> {
        let e = self.unit_eval(u, v);
        if let Some(n) = e.normal() {
            return Some(n);
        }
        let n = [
            e.du.cross(e.duv),
            e.duv.cross(e.dv),
            e.duu.cross(e.dv),
            e.du.cross(e.dvv),
        ]
        .into_iter()
        .find_map(Vector3::normalize)?;
        let [[u0, u1], [v0, v1]] = self.domain();
        let inward = |t: f64, a: f64, b: f64| {
            let step = (b - a) * 1e-6;
            if t < (a + b) / 2.0 {
                t + step
            } else {
                t - step
            }
        };
        let near = self
            .unit_eval(inward(u, u0, u1), inward(v, v0, v1))
            .normal();
        Some(match near {
            Some(m) if m.dot(n) < 0.0 => -n,
            _ => n,
        })
    }

    /// The principal curvatures against `du × dv`; `None` where that
    /// vanishes.
    pub fn principal_curvatures(&self, u: f64, v: f64) -> Option<PrincipalCurvatures> {
        let e = self.unit_eval(u, v);
        e.principal_curvatures(e.normal()?)
    }

    /// The control lines running along `dir`: for `U`, column `j` for each
    /// `j`; for `V`, row `i` for each `i`.
    fn lines(&self, dir: Direction) -> Vec<Vec<Hom<Vector3>>> {
        let [nu, nv] = self.counts();
        let at = |i: usize, j: usize| Hom {
            p: self.points[i * nv + j],
            w: self.weights[i * nv + j],
        };
        match dir {
            Direction::U => (0..nv)
                .map(|j| (0..nu).map(|i| at(i, j)).collect())
                .collect(),
            Direction::V => (0..nu)
                .map(|i| (0..nv).map(|j| at(i, j)).collect())
                .collect(),
        }
    }

    /// A surface from new lines along `dir` with clamped flat knots `flat`;
    /// the other direction keeps its knots.
    fn with_lines(&self, dir: Direction, flat: &[f64], lines: &[Vec<Hom<Vector3>>]) -> Self {
        let d = dir as usize;
        let mut knots = self.knots.clone();
        knots[d] = KnotVector::clamped_flat(self.knots[d].degree(), flat);
        let (outer, inner) = (lines.len(), lines[0].len());
        let mut hom = Vec::with_capacity(outer * inner);
        match dir {
            Direction::U => {
                for i in 0..inner {
                    hom.extend(lines.iter().map(|line| line[i]));
                }
            }
            Direction::V => hom.extend(lines.iter().flatten().copied()),
        }
        Self {
            knots,
            points: hom.iter().map(|h| h.p).collect(),
            weights: hom.iter().map(|h| h.w).collect(),
            patches: OnceLock::new(),
        }
    }

    /// The same surface with clamped knots in both directions, over one
    /// period where it was periodic.
    pub fn to_clamped(&self) -> Self {
        let mut s = self.clone();
        for dir in [Direction::U, Direction::V] {
            let knots = &s.knots[dir as usize];
            if knots.is_periodic() {
                let lines: Vec<_> = s.lines(dir).iter().map(|l| clamp(knots, l)).collect();
                let new: Vec<_> = lines.iter().map(|(_, h)| h.clone()).collect();
                s = s.with_lines(dir, &lines[0].0, &new);
            }
        }
        s
    }

    /// The same surface with each of `knots` inserted once along `dir`,
    /// clamped in that direction. Fails like [`super::NurbsCurve::refine`].
    pub fn refine(&self, dir: Direction, knots: &[f64]) -> Result<Self, Error> {
        let kv = &self.knots[dir as usize];
        let lines = self
            .lines(dir)
            .iter()
            .map(|l| refine_flat(kv, l, knots))
            .collect::<Result<Vec<_>, _>>()?;
        let new: Vec<_> = lines.iter().map(|(_, h)| h.clone()).collect();
        Ok(self.with_lines(dir, &lines[0].0, &new))
    }

    /// The surface cut along `dir` at `t` into two pieces, clamped in that
    /// direction. Fails like [`super::NurbsCurve::split`].
    pub fn split(&self, dir: Direction, t: f64) -> Result<(Self, Self), Error> {
        let kv = &self.knots[dir as usize];
        check_split(t, kv.domain())?;
        let halves: Vec<_> = self
            .lines(dir)
            .iter()
            .map(|l| {
                let (flat, hom) = clamp(kv, l);
                split_at(&flat, kv.degree(), &hom, t)
            })
            .collect();
        let half = |k: usize| {
            let lines: Vec<_> = halves.iter().map(|h| h[k].1.clone()).collect();
            self.with_lines(dir, &halves[0][k].0, &lines)
        };
        Ok((half(0), half(1)))
    }

    /// The Bézier patches, row-major over the spans, built once.
    pub(crate) fn patches(&self) -> &[BezierPatch] {
        self.patches.get_or_init(|| {
            let hom: Vec<_> = self
                .points
                .iter()
                .zip(self.weights.iter())
                .map(|(&p, &w)| Hom { p, w })
                .collect();
            patches(&self.knots, self.counts(), &hom).into()
        })
    }

    /// A box holding the surface over its domain: the union of the
    /// control-hull boxes of its Bézier patches.
    pub fn bounds(&self) -> Box3 {
        let patches = self.patches();
        patches
            .iter()
            .map(BezierPatch::bounds)
            .fold(patches[0].bounds(), |a, b| a.union(&b))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NurbsError;
    use core::f64::consts::FRAC_1_SQRT_2;

    fn close(a: Vector3, b: Vector3, tol: f64) -> bool {
        (a - b).length() <= tol
    }

    fn line(n: usize) -> KnotVector {
        let mut mults = vec![1; n];
        mults[0] = 2;
        mults[n - 1] = 2;
        let knots: Vec<f64> = (0..n).map(|i| i as f64).collect();
        KnotVector::clamped(1, &knots, &mults).unwrap()
    }

    #[test]
    fn a_bilinear_patch() {
        let p = [
            Vector3::new(0.0, 0.0, 0.0),
            Vector3::new(0.0, 1.0, 0.0),
            Vector3::new(1.0, 0.0, 1.0),
            Vector3::new(1.0, 1.0, 0.0),
        ];
        let s = NurbsSurface::new(
            [line(2), line(2)],
            p.iter().map(|v| Point3::ORIGIN + *v).collect(),
            None,
        )
        .unwrap();
        assert_eq!(s.degrees(), [1, 1]);
        assert_eq!(s.counts(), [2, 2]);
        assert_eq!(s.domain(), [[0.0, 1.0], [0.0, 1.0]]);
        assert_eq!(s.periods(), (None, None));
        assert_eq!(s.control_point(1, 0), Point3::new(1.0, 0.0, 1.0));
        assert_eq!(s.control_points().count(), 4);
        assert_eq!(s.weights(), [1.0; 4]);
        let (u, v) = (0.25, 0.5);
        let expect = p[0] * ((1.0 - u) * (1.0 - v))
            + p[1] * ((1.0 - u) * v)
            + p[2] * (u * (1.0 - v))
            + p[3] * (u * v);
        let e = s.eval(u, v);
        assert!(close(e.point.to_vector(), expect, 1e-16));
        assert_eq!(e.point, s.point(u, v));
        let twist = p[0] - p[1] - p[2] + p[3];
        assert!(close(e.duv, twist, 1e-15));
        assert_eq!((e.duu, e.dvv), (Vector3::ZERO, Vector3::ZERO));
        assert!(close(
            e.du,
            (p[2] - p[0]) * (1.0 - v) + (p[3] - p[1]) * v,
            1e-15
        ));
    }

    /// A quarter of a cylinder of radius 2 and height 3 about z: an exact
    /// rational quadratic in u, linear in v.
    fn quarter_cylinder() -> NurbsSurface {
        let ku = KnotVector::clamped(2, &[0.0, 1.0], &[3, 3]).unwrap();
        let corner = [(2.0, 0.0), (2.0, 2.0), (0.0, 2.0)];
        let mut points = Vec::new();
        let mut weights = Vec::new();
        for (i, &(x, y)) in corner.iter().enumerate() {
            for z in [0.0, 3.0] {
                points.push(Point3::new(x, y, z));
                weights.push(if i == 1 { FRAC_1_SQRT_2 } else { 1.0 });
            }
        }
        NurbsSurface::new([ku, line(2)], points, Some(weights)).unwrap()
    }

    #[test]
    fn an_exact_quarter_cylinder() {
        let s = quarter_cylinder();
        for i in 0..=8 {
            for j in 0..=4 {
                let (u, v) = (f64::from(i) / 8.0, f64::from(j) / 4.0);
                let e = s.eval(u, v);
                let p = e.point;
                assert!((crate::math::hypot(p.x, p.y) - 2.0).abs() <= 1e-15);
                assert!((p.z - 3.0 * v).abs() <= 1e-15);
                // du is horizontal and tangent to the circle; dv is the axis.
                assert!(e.du.z.abs() <= 1e-15);
                assert!(e.du.dot(Vector3::new(p.x, p.y, 0.0)).abs() <= 1e-14);
                assert!(close(e.dv, Vector3::new(0.0, 0.0, 3.0), 1e-15));
            }
        }
    }

    #[test]
    fn rational_derivatives_match_finite_differences() {
        let ku = KnotVector::clamped(3, &[0.0, 0.5, 2.0], &[4, 1, 4]).unwrap();
        let kv = KnotVector::clamped(2, &[0.0, 1.0, 1.5, 3.0], &[3, 1, 2, 3]).unwrap();
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
        let s = NurbsSurface::new([ku, kv], points, Some(weights)).unwrap();
        let h = 1e-5;
        for (u, v) in [(0.1, 0.2), (0.6, 1.2), (1.9, 2.9), (1.0, 0.5)] {
            let e = s.eval(u, v);
            let p = |a: f64, b: f64| s.point(u + a, v + b);
            let m = p(0.0, 0.0);
            let tol1 = |d: Vector3| 1e-7 * d.length().max(1.0);
            let tol2 = |d: Vector3| 1e-3 * d.length().max(1.0);
            assert!(close(
                e.du,
                (p(h, 0.0) - p(-h, 0.0)) / (2.0 * h),
                tol1(e.du)
            ));
            assert!(close(
                e.dv,
                (p(0.0, h) - p(0.0, -h)) / (2.0 * h),
                tol1(e.dv)
            ));
            let second = |a: Point3, b: Point3| ((a - m) + (b - m)) / (h * h);
            assert!(close(e.duu, second(p(h, 0.0), p(-h, 0.0)), tol2(e.duu)));
            assert!(close(e.dvv, second(p(0.0, h), p(0.0, -h)), tol2(e.dvv)));
            let cross = ((p(h, h) - p(h, -h)) - (p(-h, h) - p(-h, -h))) / (4.0 * h * h);
            assert!(close(e.duv, cross, tol2(e.duv)), "({u}, {v})");
        }
    }

    #[test]
    fn periodic_in_u() {
        // A tube: a uniform periodic quadratic around a square, swept along z.
        let ku = KnotVector::periodic(2, &[0.0, 1.0, 2.0, 3.0, 4.0], &[1, 1, 1, 1, 1]).unwrap();
        let ring = [(1.0, 0.0), (0.0, 1.0), (-1.0, 0.0), (0.0, -1.0)];
        let mut points = Vec::new();
        for &(x, y) in &ring {
            for z in [0.0, 2.0] {
                points.push(Point3::new(x, y, z));
            }
        }
        let s = NurbsSurface::new([ku, line(2)], points, None).unwrap();
        assert_eq!(s.periods(), (Some(4.0), None));
        for (u, v) in [(0.0, 0.0), (0.7, 0.3), (3.9, 1.0)] {
            let (a, b) = (s.eval(u, v), s.eval(u + 4.0, v));
            assert!(a.point.distance(b.point) <= 1e-15);
            assert!(close(a.du, b.du, 1e-14) && close(a.duu, b.duu, 1e-13));
        }
    }

    /// The triangle (1,0,0), (0,1,0), (0,0,1) as a bilinear patch whose
    /// `v = 1` row collapses to the last corner: a pole.
    fn pole() -> NurbsSurface {
        let points = vec![
            Point3::new(1.0, 0.0, 0.0),
            Point3::new(0.0, 0.0, 1.0),
            Point3::new(0.0, 1.0, 0.0),
            Point3::new(0.0, 0.0, 1.0),
        ];
        NurbsSurface::new([line(2), line(2)], points, None).unwrap()
    }

    #[test]
    fn the_normal_at_a_pole_is_the_limit() {
        let s = pole();
        let n = 1.0 / 3.0f64.sqrt();
        let expect = Vector3::new(n, n, n);
        assert_eq!(s.eval(0.5, 1.0).normal(), None);
        for u in [0.0, 0.3, 1.0] {
            assert!(
                close(s.normal(u, 1.0).unwrap().get(), expect, 1e-15),
                "u={u}"
            );
            assert!(
                close(s.normal(u, 0.5).unwrap().get(), expect, 1e-15),
                "u={u}"
            );
        }
        assert_eq!(s.principal_curvatures(0.5, 1.0), None);
        let k = s.principal_curvatures(0.5, 0.5).unwrap();
        assert_eq!((k.k1, k.k2), (0.0, 0.0));
    }

    #[test]
    fn normals_do_not_depend_on_the_parameter_scale() {
        let k = KnotVector::clamped(1, &[0.0, 1e9], &[2, 2]).unwrap();
        let points = vec![
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(0.0, 1.0, 0.0),
            Point3::new(1.0, 0.0, 0.0),
            Point3::new(1.0, 1.0, 0.0),
        ];
        let s = NurbsSurface::new([k.clone(), k], points, None).unwrap();
        assert_eq!(s.normal(5e8, 5e8), Some(crate::geom::UnitVector3::Z));
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

    /// The two surfaces agree on a 21 × 21 grid over `[u0, u1] × [v0, v1]`.
    fn same_shape(a: &NurbsSurface, b: &NurbsSurface, [[u0, u1], [v0, v1]]: [[f64; 2]; 2]) -> bool {
        (0..=20).all(|i| {
            (0..=20).all(|j| {
                let u = u0 + (u1 - u0) * f64::from(i) / 20.0;
                let v = v0 + (v1 - v0) * f64::from(j) / 20.0;
                a.point(u, v).distance(b.point(u, v)) <= 1e-14
            })
        })
    }

    #[test]
    fn clamping_keeps_the_shape() {
        let s = wavy();
        let c = s.to_clamped();
        assert_eq!(s.periods(), (None, Some(3.0)));
        assert_eq!(c.periods(), (None, None));
        assert_eq!(c.domain(), s.domain());
        assert!(same_shape(&s, &c, s.domain()));
    }

    #[test]
    fn refining_in_either_direction_keeps_the_shape() {
        let s = wavy();
        let u = s.refine(Direction::U, &[0.25, 1.0, 1.0]).unwrap();
        assert_eq!(u.knots()[0].knots(), [0.0, 0.25, 0.5, 1.0, 2.0]);
        assert_eq!(u.counts(), [s.counts()[0] + 3, s.counts()[1]]);
        assert_eq!(u.periods(), s.periods());
        assert!(same_shape(&s, &u, s.domain()));
        let v = s.refine(Direction::V, &[2.0]).unwrap();
        assert_eq!(v.periods(), (None, None));
        assert!(same_shape(&s, &v, s.domain()));
        assert_eq!(
            s.refine(Direction::V, &[3.0]),
            Err(Error::InvalidNurbs(NurbsError::KnotOrder))
        );
    }

    #[test]
    fn splitting_in_either_direction() {
        let s = wavy();
        let (a, b) = s.split(Direction::U, 0.7).unwrap();
        assert_eq!(a.domain(), [[0.0, 0.7], [0.0, 3.0]]);
        assert_eq!(b.domain(), [[0.7, 2.0], [0.0, 3.0]]);
        assert!(same_shape(&s, &a, a.domain()));
        assert!(same_shape(&s, &b, b.domain()));
        let (a, b) = s.split(Direction::V, 1.5).unwrap();
        assert!(same_shape(&s, &a, a.domain()));
        assert!(same_shape(&s, &b, b.domain()));
        assert_eq!(
            s.split(Direction::U, 2.0),
            Err(Error::InvalidNurbs(NurbsError::ShortDomain))
        );
    }

    #[test]
    fn rejections() {
        let p = |x: f64| Point3::new(x, 0.0, 0.0);
        let four = || vec![p(0.0), p(1.0), p(2.0), p(3.0)];
        let invalid = |e| Err(Error::InvalidNurbs(e));
        assert_eq!(
            NurbsSurface::new([line(2), line(2)], four()[..3].to_vec(), None),
            invalid(NurbsError::ControlCount)
        );
        assert_eq!(
            NurbsSurface::new([line(2), line(2)], four(), Some(vec![1.0, 1.0, -1.0, 1.0])),
            invalid(NurbsError::Weight)
        );
        let mut far = four();
        far[3] = p(-501.0);
        assert_eq!(
            NurbsSurface::new([line(2), line(2)], far, None),
            Err(Error::OutsideSizeBox)
        );
    }
}
