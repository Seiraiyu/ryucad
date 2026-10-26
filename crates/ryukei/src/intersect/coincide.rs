//! The coincidence rule: when two surfaces are the same surface.
//!
//! Directions agree within [`ANGULAR_RESOLUTION`] (measured by the sine
//! of the angle between them) and lengths and positions within
//! [`LINEAR_RESOLUTION`]:
//!
//! - planes with parallel or opposite normals, each origin within
//!   resolution of the other plane;
//! - cylinders with parallel axes, each origin within resolution of the
//!   other axis, and equal radii;
//! - cones with axes pointing the same way, apexes within resolution and
//!   equal half-angles;
//! - spheres with centres and radii within resolution;
//! - tori with parallel axes, centres within resolution and equal major
//!   and minor radii.
//!
//! The natural normals of coincident analytic surfaces agree, except for
//! planes, whose normals may be opposite.
//!
//! Surfaces with no implicit form are coincident by rule too, `same` read
//! from their natural normals at a point:
//!
//! - equal surfaces (one curve and direction, one curve and axis, one
//!   spine, radius and anchor; the same NURBS);
//! - a NURBS, or a revolution by its exact NURBS form, and an analytic
//!   surface whose implicit form certifies every Bézier patch within
//!   resolution of it (and none past a cone's apex);
//! - an extrusion along a cylinder's axis whose curve the cylinder's
//!   implicit form certifies within resolution of it;
//! - two NURBS whose control nets, after one is turned (each direction
//!   reversed, the two swapped), mapped affinely onto the other's domain
//!   and both given the knots of either, have equal degrees, weights in
//!   one ratio and control points within resolution of each other (their
//!   points at the same parameters then are too), or where one is such a
//!   part of the other, cut where the first's corners project.
//!
//! Anything else is not coincident: overlapping surfaces the rules don't
//! name are left to the intersectors, which can't separate them and say
//! so.

use super::implicit::Implicit;
use crate::geom::Direction;
use crate::geom::chain3::Chain3;
use crate::geom::{Frame, KnotVector, NurbsSurface, Point3, Surface, UnitVector3};
use crate::interval::Interval;
use crate::tol::{ANGULAR_RESOLUTION, LINEAR_RESOLUTION};

/// `Some(same)` when the surfaces are coincident, `same` when their
/// natural normals agree; `None` otherwise.
pub(crate) fn coincident(a: &Surface, b: &Surface) -> Option<bool> {
    let close = |x: f64, y: f64| (x - y).abs() <= LINEAR_RESOLUTION;
    let near = |p: Point3, q: Point3| p.distance(q) <= LINEAR_RESOLUTION;
    match (a, b) {
        (Surface::Plane(_), Surface::Plane(_)) => {
            let (f, g) = (a.frame()?, b.frame()?);
            let same = f.z().dot(g.z()) > 0.0;
            (parallel(f.z(), g.z())
                && off_plane(f, g.origin()) <= LINEAR_RESOLUTION
                && off_plane(g, f.origin()) <= LINEAR_RESOLUTION)
                .then_some(same)
        }
        (Surface::Cylinder(x), Surface::Cylinder(y)) => {
            let (f, g) = (a.frame()?, b.frame()?);
            (parallel(f.z(), g.z())
                && off_axis(f, g.origin()) <= LINEAR_RESOLUTION
                && off_axis(g, f.origin()) <= LINEAR_RESOLUTION
                && close(x.radius(), y.radius()))
            .then_some(true)
        }
        (Surface::Cone(x), Surface::Cone(y)) => {
            let (f, g) = (a.frame()?, b.frame()?);
            let apex = |f: &Frame, v: f64| f.origin() + f.z().get() * v;
            (f.z().dot(g.z()) > 0.0
                && parallel(f.z(), g.z())
                && near(apex(f, x.apex_v()), apex(g, y.apex_v()))
                && (x.half_angle() - y.half_angle()).abs() <= ANGULAR_RESOLUTION)
                .then_some(true)
        }
        (Surface::Sphere(x), Surface::Sphere(y)) => {
            let (f, g) = (a.frame()?, b.frame()?);
            (near(f.origin(), g.origin()) && close(x.radius(), y.radius())).then_some(true)
        }
        (Surface::Torus(x), Surface::Torus(y)) => {
            let (f, g) = (a.frame()?, b.frame()?);
            (parallel(f.z(), g.z())
                && near(f.origin(), g.origin())
                && close(x.major(), y.major())
                && close(x.minor(), y.minor()))
            .then_some(true)
        }
        _ => general(a, b),
    }
}

/// The rule for pairs where a surface has no implicit form.
fn general(a: &Surface, b: &Surface) -> Option<bool> {
    if a == b {
        return Some(true);
    }
    if let (Surface::Nurbs(x), Surface::Nurbs(y)) = (a, b) {
        return splines(x, y)
            .or_else(|| part_of(x, y))
            .or_else(|| part_of(y, x));
    }
    let (s, other) = match (Implicit::of(a).is_some(), Implicit::of(b).is_some()) {
        (false, true) => (a, b),
        (true, false) => (b, a),
        _ => return None,
    };
    let imp = Implicit::of(other)?;
    let on = match s {
        Surface::Nurbs(n) => on_implicit(&imp, n),
        Surface::Revolution(_) => s
            .to_nurbs(s.domain())
            .is_ok_and(|(n, _)| on_implicit(&imp, &n)),
        Surface::Extrusion(e) => match other {
            Surface::Cylinder(_) => {
                let axis = other.frame()?.z();
                parallel(e.dir(), axis) && curve_on(&imp, e.curve())
            }
            _ => false,
        },
        _ => false,
    };
    if !on {
        return None;
    }
    // The natural normals at the middle of the surface's domain (an
    // extrusion's at height 0).
    let [du, dv] = s.domain();
    let mid = |d: [f64; 2]| {
        if d[0].is_finite() && d[1].is_finite() {
            (d[0] + d[1]) / 2.0
        } else {
            0.0
        }
    };
    let (u, v) = (mid(du), mid(dv));
    let n = s.normal(u, v)?;
    let (ou, ov) = other.project(s.point(u, v)).param;
    let m = other.normal(ou, ov)?;
    Some(n.dot(m) > 0.0)
}

/// Whether every Bézier patch of a NURBS is certified within resolution of
/// an implicit form's surface, and none reaches past a cone's apex.
fn on_implicit(imp: &Implicit, n: &NurbsSurface) -> bool {
    n.patches().iter().all(|patch| {
        let (f, _) = imp.on_patch(patch);
        let mut local: Option<[Interval; 3]> = None;
        for h in &patch.pts {
            let l = imp.local(h.p);
            local = Some(match local {
                None => l,
                Some(o) => core::array::from_fn(|k| o[k].hull(l[k])),
            });
        }
        let Some(local) = local else {
            return false;
        };
        !imp.box_beyond_apex(local)
            && imp.distance_bound(f.range().mag(), local) <= LINEAR_RESOLUTION
    })
}

/// Whether a curve over its domain is certified within resolution of an
/// implicit form's surface, piece by exact rational Bézier piece.
fn curve_on(imp: &Implicit, c: &crate::geom::Curve) -> bool {
    let [t0, t1] = c.domain();
    if !(t0.is_finite() && t1.is_finite()) {
        return false;
    }
    Chain3::new(c, t0, t1).pieces.iter().all(|piece| {
        let (f, _) = imp.on_piece(piece);
        let mut local: Option<[Interval; 3]> = None;
        for h in &piece.pts {
            let l = imp.local(h.p);
            local = Some(match local {
                None => l,
                Some(o) => core::array::from_fn(|k| o[k].hull(l[k])),
            });
        }
        local.is_some_and(|local| {
            !imp.box_beyond_apex(local)
                && imp.distance_bound(f.range().mag(), local) <= LINEAR_RESOLUTION
        })
    })
}

/// Two NURBS by their nets, `y` turned every way (each direction
/// reversed, the two swapped): `Some(same)` for the first turn whose net
/// matches `x`'s, `same` when it turned `y`'s normal round an even number
/// of times.
fn splines(x: &NurbsSurface, y: &NurbsSurface) -> Option<bool> {
    let (x, y) = (x.to_clamped(), y.to_clamped());
    for swap in [false, true] {
        for flip_u in [false, true] {
            for flip_v in [false, true] {
                let Some(t) = turned(&y, swap, flip_u, flip_v) else {
                    continue;
                };
                if same_net(&x, &t) {
                    return Some(!(swap ^ flip_u ^ flip_v));
                }
            }
        }
    }
    None
}

/// A clamped NURBS with its directions swapped and each reversed as
/// asked.
fn turned(s: &NurbsSurface, swap: bool, flip_u: bool, flip_v: bool) -> Option<NurbsSurface> {
    let [ku, kv] = s.knots().clone();
    let [nu, nv] = s.counts();
    let flip = |k: &KnotVector| -> Option<KnotVector> {
        let [d0, d1] = k.domain();
        let knots: Vec<f64> = k.knots().iter().rev().map(|&t| d0 + d1 - t).collect();
        let mults: Vec<usize> = k.mults().iter().rev().copied().collect();
        KnotVector::clamped(k.degree(), &knots, &mults).ok()
    };
    let ku = if flip_u { flip(&ku)? } else { ku };
    let kv = if flip_v { flip(&kv)? } else { kv };
    let at = |i: usize, j: usize| {
        let i = if flip_u { nu - 1 - i } else { i };
        let j = if flip_v { nv - 1 - j } else { j };
        (s.control_point(i, j), s.weights()[i * nv + j])
    };
    let (rows, cols, knots) = if swap {
        (nv, nu, [kv, ku])
    } else {
        (nu, nv, [ku, kv])
    };
    let mut points = Vec::with_capacity(nu * nv);
    let mut weights = Vec::with_capacity(nu * nv);
    for i in 0..rows {
        for j in 0..cols {
            let (p, w) = if swap { at(j, i) } else { at(i, j) };
            points.push(p);
            weights.push(w);
        }
    }
    NurbsSurface::new(knots, points, Some(weights)).ok()
}

/// Whether two clamped NURBS of the same degrees have one net: `y`'s
/// domain mapped affinely onto `x`'s, both given every knot of either
/// (knots within `1e-12` of the domain's length taken as one), weights in
/// one ratio within `1e-12`, and control points within resolution.
fn same_net(x: &NurbsSurface, y: &NurbsSurface) -> bool {
    if x.degrees() != y.degrees() {
        return false;
    }
    let mut a = x.clone();
    let mut b = y.clone();
    for dir in [Direction::U, Direction::V] {
        let k = dir as usize;
        let (da, db) = (a.knots()[k].domain(), b.knots()[k].domain());
        let len = da[1] - da[0];
        // y's knots in x's parameter.
        let map = |t: f64| da[0] + (t - db[0]) * len / (db[1] - db[0]);
        let inner = |kv: &KnotVector, f: &dyn Fn(f64) -> f64| -> Vec<(f64, usize)> {
            let n = kv.knots().len();
            kv.knots()
                .iter()
                .zip(kv.mults())
                .enumerate()
                .filter(|&(i, _)| i > 0 && i + 1 < n)
                .map(|(_, (&t, &m))| (f(t), m))
                .collect()
        };
        let ka = inner(&a.knots()[k], &|t| t);
        let kb = inner(&b.knots()[k], &map);
        let near =
            |t: f64, list: &[(f64, usize)]| list.iter().any(|&(s, _)| (s - t).abs() <= 1e-12 * len);
        // Each side gains the knots only the other has, as many times.
        let missing = |from: &[(f64, usize)], to: &[(f64, usize)]| -> Vec<f64> {
            from.iter()
                .filter(|&&(t, _)| !near(t, to))
                .flat_map(|&(t, m)| core::iter::repeat_n(t, m))
                .collect()
        };
        let (add_a, add_b) = (missing(&kb, &ka), missing(&ka, &kb));
        let unmap = |t: f64| db[0] + (t - da[0]) * (db[1] - db[0]) / len;
        if !add_a.is_empty() {
            let Ok(r) = a.refine(dir, &add_a) else {
                return false;
            };
            a = r;
        }
        if !add_b.is_empty() {
            let back: Vec<f64> = add_b.iter().map(|&t| unmap(t)).collect();
            let Ok(r) = b.refine(dir, &back) else {
                return false;
            };
            b = r;
        }
    }
    if a.counts() != b.counts() {
        return false;
    }
    let (wa, wb) = (a.weights(), b.weights());
    let ratio = wa[0] / wb[0];
    if !(ratio.is_finite() && ratio > 0.0) {
        return false;
    }
    wa.iter()
        .zip(wb)
        .all(|(&p, &q)| (p - ratio * q).abs() <= 1e-12 * p.abs())
        && a.control_points()
            .zip(b.control_points())
            .all(|(p, q)| p.distance(q) <= LINEAR_RESOLUTION)
}

/// `x` as a part of `y`: `y` cut to the box of the parameters `x`'s
/// corners project to there (all of a direction both close up in),
/// compared by [`splines`].
fn part_of(x: &NurbsSurface, y: &NurbsSurface) -> Option<bool> {
    let [du, dv] = x.domain();
    let mut lo = [f64::INFINITY; 2];
    let mut hi = [f64::NEG_INFINITY; 2];
    for u in du {
        for v in dv {
            let foot = y.project(x.point(u, v));
            if foot.distance > LINEAR_RESOLUTION {
                return None;
            }
            let (a, b) = foot.param;
            lo = [lo[0].min(a), lo[1].min(b)];
            hi = [hi[0].max(a), hi[1].max(b)];
        }
    }
    let [yu, yv] = y.domain();
    // Round a direction where `x` closes up, its corners all project to
    // one seam: it is the whole of `y` that way, if `y` closes up too.
    let (xp, yp) = (x.periods(), y.periods());
    for (k, (xk, yk), d) in [(0, (xp.0, yp.0), yu), (1, (xp.1, yp.1), yv)] {
        if xk.is_some() {
            yk?;
            (lo[k], hi[k]) = (d[0], d[1]);
        }
    }
    if (lo[0] <= yu[0] && hi[0] >= yu[1] && lo[1] <= yv[0] && hi[1] >= yv[1])
        || !(lo[0] < hi[0] && lo[1] < hi[1])
    {
        return None;
    }
    let mut part = y.to_clamped();
    for (dir, (a, b)) in [
        (Direction::U, (lo[0], hi[0])),
        (Direction::V, (lo[1], hi[1])),
    ] {
        let d = part.domain()[dir as usize];
        if b < d[1] {
            part = part.split(dir, b).ok()?.0;
        }
        if a > d[0] {
            part = part.split(dir, a).ok()?.1;
        }
    }
    splines(x, &part)
}

/// Parallel or opposite within [`ANGULAR_RESOLUTION`].
pub(crate) fn parallel(u: UnitVector3, v: UnitVector3) -> bool {
    crate::tol::directions_parallel(u, v)
}

/// The distance of `p` from the plane of `f`'s xy axes.
pub(crate) fn off_plane(f: &Frame, p: Point3) -> f64 {
    (p - f.origin()).dot(f.z().get()).abs()
}

/// The distance of `p` from the line through `f`'s origin along its `z`.
pub(crate) fn off_axis(f: &Frame, p: Point3) -> f64 {
    let d = p - f.origin();
    (d - f.z().get() * d.dot(f.z().get())).length()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{Cone, Cylinder, Plane, Sphere, Torus, Vector3};
    use core::f64::consts::FRAC_PI_4;

    fn frame(o: (f64, f64, f64), z: (f64, f64, f64)) -> Frame {
        Frame::new(
            Point3::new(o.0, o.1, o.2),
            Vector3::new(z.0, z.1, z.2),
            Vector3::new(1.0, 0.3, 0.0),
        )
        .unwrap()
    }

    #[test]
    fn planes() {
        let p = |o, z| Surface::Plane(Plane::new(frame(o, z)));
        let a = p((0.0, 0.0, 1.0), (0.0, 0.0, 1.0));
        assert_eq!(
            coincident(&a, &p((5.0, 2.0, 1.0), (0.0, 0.0, 1.0))),
            Some(true)
        );
        assert_eq!(
            coincident(&a, &p((5.0, 2.0, 1.0), (0.0, 0.0, -1.0))),
            Some(false)
        );
        assert_eq!(
            coincident(&a, &p((5.0, 2.0, 1.0 + 0.5e-8), (0.0, 0.0, 1.0))),
            Some(true)
        );
        assert_eq!(
            coincident(&a, &p((5.0, 2.0, 1.0 + 2e-8), (0.0, 0.0, 1.0))),
            None
        );
        assert_eq!(coincident(&a, &p((0.0, 0.0, 1.0), (0.0, 1e-10, 1.0))), None);
    }

    #[test]
    fn surfaces_of_revolution() {
        let z = (0.0, 0.0, 1.0);
        let cyl = |o, z, r| Surface::Cylinder(Cylinder::new(frame(o, z), r).unwrap());
        let a = cyl((0.0, 0.0, 0.0), z, 1.0);
        // Shifted along the axis and turned round: the same cylinder.
        assert_eq!(
            coincident(&a, &cyl((0.0, 0.0, 7.0), (0.0, 0.0, -1.0), 1.0)),
            Some(true)
        );
        assert_eq!(coincident(&a, &cyl((0.0, 0.0, 0.0), z, 1.0 + 2e-8)), None);
        assert_eq!(coincident(&a, &cyl((2e-8, 0.0, 0.0), z, 1.0)), None);
        // Cones: the same apex and angle, whatever the reference radius.
        let c1 = Surface::Cone(Cone::new(frame((0.0, 0.0, 0.0), z), 1.0, FRAC_PI_4).unwrap());
        let c2 = Surface::Cone(Cone::new(frame((0.0, 0.0, 1.0), z), 2.0, FRAC_PI_4).unwrap());
        assert_eq!(coincident(&c1, &c2), Some(true));
        let flipped = Surface::Cone(
            Cone::new(frame((0.0, 0.0, -2.0), (0.0, 0.0, -1.0)), 1.0, FRAC_PI_4).unwrap(),
        );
        assert_eq!(coincident(&c1, &flipped), None);
        let s1 = Surface::Sphere(Sphere::new(frame((1.0, 2.0, 3.0), z), 2.0).unwrap());
        let s2 =
            Surface::Sphere(Sphere::new(frame((1.0, 2.0, 3.0), (1.0, 0.0, 0.0)), 2.0).unwrap());
        assert_eq!(coincident(&s1, &s2), Some(true));
        let t1 = Surface::Torus(Torus::new(frame((0.0, 0.0, 0.0), z), 3.0, 1.0).unwrap());
        let t2 =
            Surface::Torus(Torus::new(frame((0.0, 0.0, 0.0), (0.0, 0.0, -1.0)), 3.0, 1.0).unwrap());
        assert_eq!(coincident(&t1, &t2), Some(true));
        assert_eq!(coincident(&t1, &s1), None);
    }

    fn spline(s: &Surface, range: [[f64; 2]; 2]) -> Surface {
        let (n, _) = s.to_nurbs(range).unwrap();
        Surface::Nurbs(std::sync::Arc::new(n))
    }

    #[test]
    fn splines_on_analytic_surfaces() {
        use core::f64::consts::TAU;
        let z = (0.2, 0.1, 1.0);
        let plane = Surface::Plane(Plane::new(frame((0.0, 0.0, 1.0), z)));
        let cyl = Surface::Cylinder(Cylinder::new(frame((0.0, 0.0, 0.0), z), 1.0).unwrap());
        let torus = Surface::Torus(Torus::new(frame((1.0, 0.0, 0.0), z), 3.0, 1.0).unwrap());
        for s in [&plane, &cyl, &torus] {
            let range = match s {
                Surface::Plane(_) => [[-1.0, 2.0], [-1.0, 1.0]],
                _ => [[0.0, TAU], [0.0, 1.5]],
            };
            let n = spline(s, range);
            assert_eq!(coincident(&n, s), Some(true), "{s:?}");
            assert_eq!(coincident(s, &n), Some(true), "{s:?}");
        }
        // Moved by twice the resolution, no longer.
        let off = Surface::Cylinder(Cylinder::new(frame((2e-8, 0.0, 0.0), z), 1.0).unwrap());
        assert_eq!(
            coincident(&spline(&cyl, [[0.0, TAU], [0.0, 1.0]]), &off),
            None
        );
        // The other way round, a plane's spline faces the other way.
        let flipped = Surface::Plane(Plane::new(frame((0.0, 0.0, 1.0), (-0.2, -0.1, -1.0))));
        assert_eq!(
            coincident(&spline(&plane, [[-1.0, 1.0], [-1.0, 1.0]]), &flipped),
            Some(false)
        );
    }

    #[test]
    fn splines_with_one_net() {
        use crate::geom::{Direction, NurbsSurface};
        use core::f64::consts::TAU;
        use std::sync::Arc;
        let cyl =
            Surface::Cylinder(Cylinder::new(frame((0.0, 0.0, 0.0), (0.0, 0.0, 1.0)), 1.0).unwrap());
        let (n, _) = cyl.to_nurbs([[0.0, TAU], [-1.0, 1.0]]).unwrap();
        let s = |n: NurbsSurface| Surface::Nurbs(Arc::new(n));
        // Refined, and reparameterized: the same net.
        let refined = n.refine(Direction::V, &[0.25, -0.5]).unwrap();
        assert_eq!(coincident(&s(n.clone()), &s(refined.clone())), Some(true));
        // Reversed in one direction faces the other way; in both, the same.
        let rev = super::turned(&n.to_clamped(), false, true, false).unwrap();
        assert_eq!(coincident(&s(n.clone()), &s(rev)), Some(false));
        let both = super::turned(&refined.to_clamped(), true, true, false).unwrap();
        assert_eq!(coincident(&s(n.clone()), &s(both)), Some(true));
        // A part of it, cut where its corners project.
        let (lo, _) = n.split(Direction::U, 2.0).unwrap();
        let (_, part) = lo.split(Direction::U, 0.5).unwrap();
        let (part, _) = part.split(Direction::V, 0.3).unwrap();
        assert_eq!(coincident(&s(part.clone()), &s(n.clone())), Some(true));
        assert_eq!(coincident(&s(n.clone()), &s(part)), Some(true));
        // A cylinder of another radius has another net.
        let other =
            Surface::Cylinder(Cylinder::new(frame((0.0, 0.0, 0.0), (0.0, 0.0, 1.0)), 1.1).unwrap());
        let (m, _) = other.to_nurbs([[0.0, TAU], [-1.0, 1.0]]).unwrap();
        assert_eq!(coincident(&s(n), &s(m)), None);
    }

    #[test]
    fn swept_surfaces() {
        use crate::geom::{Circle, Curve, Extrusion, Revolution};
        use std::sync::Arc;
        let f = frame((0.0, 0.0, 0.0), (0.0, 0.0, 1.0));
        let circle = Circle::new(f, 1.0).unwrap();
        let cyl = Surface::Cylinder(Cylinder::new(f, 1.0).unwrap());
        let ext = Surface::Extrusion(Arc::new(
            Extrusion::new(Curve::Circle(circle), Vector3::new(0.0, 0.0, 1.0)).unwrap(),
        ));
        assert_eq!(coincident(&ext, &cyl), Some(true));
        let leaning = Surface::Extrusion(Arc::new(
            Extrusion::new(Curve::Circle(circle), Vector3::new(0.1, 0.0, 1.0)).unwrap(),
        ));
        assert_eq!(coincident(&leaning, &cyl), None);
        let meridian = Frame::new(
            Point3::new(3.0, 0.0, 0.0),
            Vector3::new(0.0, -1.0, 0.0),
            Vector3::new(1.0, 0.0, 0.0),
        )
        .unwrap();
        let rev = Surface::Revolution(Arc::new(
            Revolution::new(
                Frame::WORLD,
                Curve::Circle(Circle::new(meridian, 1.0).unwrap()),
            )
            .unwrap(),
        ));
        let torus = Surface::Torus(Torus::new(Frame::WORLD, 3.0, 1.0).unwrap());
        assert_eq!(coincident(&rev, &torus), Some(true));
        assert_eq!(coincident(&rev, &rev.clone()), Some(true));
    }
}
