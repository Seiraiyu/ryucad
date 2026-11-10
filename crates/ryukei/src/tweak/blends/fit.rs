//! A NURBS face read as a fillet: the cylinder or torus its larger
//! curvature's centres lie on, certified within the tolerance of it. A
//! healed import's fillets are often splines; a NURBS form of a pipe
//! along a curve that is no line or circle is no fillet here.

use super::recognise::Canal;
use crate::geom::{
    Circle, Curve, Cylinder, Frame, Line, NurbsSurface, Point3, Surface, Torus, Vector3,
};
use crate::intersect::coincide::within_implicit;
use crate::intersect::implicit::Implicit;
use crate::topo::{Body, FaceId};

/// Points of the face's parameter box a side, for its centres.
const GRID: usize = 3;

/// The centre of the larger curvature at `(u, v)` and its radius.
fn centre(s: &Surface, u: f64, v: f64) -> Option<(Point3, f64)> {
    let e = s.eval(u, v);
    let n = s.normal(u, v)?;
    let k = e.principal_curvatures(n)?;
    let big = if k.k1.abs() >= k.k2.abs() { k.k1 } else { k.k2 };
    if big.abs() < 1e-9 {
        return None;
    }
    Some((e.point + n.get() / big, 1.0 / big.abs()))
}

/// The circle through three points: its centre, unit axis and radius.
fn circle(a: Point3, b: Point3, c: Point3) -> Option<(Point3, Vector3, f64)> {
    let (u, w) = (b - a, c - a);
    let n = u.cross(w);
    let n2 = n.length_squared();
    if n2 < 1e-24 {
        return None;
    }
    let x = (w * u.length_squared() - u * w.length_squared()).cross(n) / (2.0 * n2);
    let o = a + x;
    Some((o, n / crate::math::sqrt(n2), x.length()))
}

/// Face `f` on spline `n` read as a canal surface of one radius about a
/// line or a circle, certified within `tol`.
pub(crate) fn fit(
    body: &Body,
    f: FaceId,
    n: &std::sync::Arc<NurbsSurface>,
    tol: f64,
) -> Option<(Canal, f64)> {
    let s = Surface::Nurbs(n.clone());
    let rect = crate::intersect::face_rect(body, f)?;
    let at = |k: usize, r: [f64; 2]| r[0] + (r[1] - r[0]) * (k as f64 + 0.5) / GRID as f64;
    let mut cs = Vec::new();
    for i in 0..GRID {
        for j in 0..GRID {
            cs.push(centre(&s, at(i, rect[0]), at(j, rect[1]))?);
        }
    }
    let r = cs[GRID * GRID / 2].1;
    if cs.iter().any(|&(_, x)| (x - r).abs() > tol) {
        return None;
    }
    let c0 = cs[0].0;
    let c2 = cs
        .iter()
        .map(|&(c, _)| c)
        .max_by(|a, b| a.distance(c0).total_cmp(&b.distance(c0)))?;
    if c2.distance(c0) <= tol {
        return None;
    }
    let dir = (c2 - c0) / c2.distance(c0);
    let off = |c: Point3| {
        let w = c - c0;
        (w - dir * w.dot(dir)).length()
    };
    let c1 = cs
        .iter()
        .map(|&(c, _)| c)
        .max_by(|a, b| off(*a).total_cmp(&off(*b)))?;
    let (candidate, canal) = if off(c1) <= tol {
        let hint = if dir.x.abs() < 0.9 {
            Vector3::new(1.0, 0.0, 0.0)
        } else {
            Vector3::new(0.0, 1.0, 0.0)
        };
        let fr = Frame::new(c0, dir, hint).ok()?;
        (
            Surface::Cylinder(Cylinder::new(fr, r).ok()?),
            Canal {
                spine: Curve::Line(Line::new(c0, dir).ok()?),
            },
        )
    } else {
        let (o, z, big) = circle(c0, c1, c2)?;
        let fr = Frame::new(o, z, c0 - o).ok()?;
        (
            Surface::Torus(Torus::new(fr, big, r).ok()?),
            Canal {
                spine: Curve::Circle(Circle::new(fr, big).ok()?),
            },
        )
    };
    let imp = Implicit::of(&candidate)?;
    within_implicit(&imp, n, tol).then_some((canal, r))
}
