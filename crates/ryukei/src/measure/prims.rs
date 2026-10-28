//! Distances between points, segments and triangles, with the closest
//! points: the mesh side of [`super::distance`]. Plain f64; the caller
//! keeps a margin for rounding.

use crate::geom::{Point3, Vector3};

/// A piece of an entity's mesh.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Prim {
    Point(Point3),
    Segment([Point3; 2]),
    Triangle([Point3; 3]),
}

impl Prim {
    pub(crate) fn points(&self) -> &[Point3] {
        match self {
            Self::Point(p) => core::slice::from_ref(p),
            Self::Segment(s) => s,
            Self::Triangle(t) => t,
        }
    }
}

/// The distance between two pieces and the closest points on each.
pub(crate) fn between(a: &Prim, b: &Prim) -> (f64, Point3, Point3) {
    use Prim::*;
    let flip = |(d, p, q): (f64, Point3, Point3)| (d, q, p);
    match (a, b) {
        (Point(p), Point(q)) => (p.distance(*q), *p, *q),
        (Point(p), Segment(s)) => {
            let q = point_segment(*p, s);
            (p.distance(q), *p, q)
        }
        (Segment(_), Point(_)) => flip(between(b, a)),
        (Point(p), Triangle(t)) => {
            let q = point_triangle(*p, t);
            (p.distance(q), *p, q)
        }
        (Triangle(_), Point(_)) => flip(between(b, a)),
        (Segment(s), Segment(r)) => {
            let (p, q) = segment_segment(s, r);
            (p.distance(q), p, q)
        }
        (Segment(s), Triangle(t)) => segment_triangle(s, t),
        (Triangle(_), Segment(_)) => flip(between(b, a)),
        (Triangle(t), Triangle(r)) => {
            let mut best = (f64::INFINITY, t[0], r[0]);
            for k in 0..3 {
                let e = [t[k], t[(k + 1) % 3]];
                let c = segment_triangle(&e, r);
                if c.0 < best.0 {
                    best = c;
                }
                let e = [r[k], r[(k + 1) % 3]];
                let c = flip(segment_triangle(&e, t));
                if c.0 < best.0 {
                    best = c;
                }
            }
            best
        }
    }
}

/// The nearest point of segment `s` to `p`.
pub(crate) fn point_segment(p: Point3, s: &[Point3; 2]) -> Point3 {
    let d = s[1] - s[0];
    let len = d.length_squared();
    if len == 0.0 {
        return s[0];
    }
    let t = ((p - s[0]).dot(d) / len).clamp(0.0, 1.0);
    s[0] + d * t
}

/// The nearest point of triangle `t` to `p` (Ericson, Real-Time Collision
/// Detection, 5.1.5).
pub(crate) fn point_triangle(p: Point3, t: &[Point3; 3]) -> Point3 {
    let [a, b, c] = *t;
    let (ab, ac, ap) = (b - a, c - a, p - a);
    let (d1, d2) = (ab.dot(ap), ac.dot(ap));
    if d1 <= 0.0 && d2 <= 0.0 {
        return a;
    }
    let bp = p - b;
    let (d3, d4) = (ab.dot(bp), ac.dot(bp));
    if d3 >= 0.0 && d4 <= d3 {
        return b;
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        return a + ab * (d1 / (d1 - d3));
    }
    let cp = p - c;
    let (d5, d6) = (ab.dot(cp), ac.dot(cp));
    if d6 >= 0.0 && d5 <= d6 {
        return c;
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        return a + ac * (d2 / (d2 - d6));
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        return b + (c - b) * ((d4 - d3) / ((d4 - d3) + (d5 - d6)));
    }
    let denom = va + vb + vc;
    if denom == 0.0 || !denom.is_finite() {
        // A degenerate triangle: its sides.
        return [[a, b], [b, c], [c, a]]
            .iter()
            .map(|s| point_segment(p, s))
            .min_by(|x, y| x.distance(p).total_cmp(&y.distance(p)))
            .expect("three sides");
    }
    let (v, w) = (vb / denom, vc / denom);
    a + ab * v + ac * w
}

/// The closest points of two segments (Ericson 5.1.9).
pub(crate) fn segment_segment(s: &[Point3; 2], r: &[Point3; 2]) -> (Point3, Point3) {
    let (d1, d2) = (s[1] - s[0], r[1] - r[0]);
    let w = s[0] - r[0];
    let (a, e, f) = (d1.length_squared(), d2.length_squared(), d2.dot(w));
    if a == 0.0 && e == 0.0 {
        return (s[0], r[0]);
    }
    let (sc, tc);
    if a == 0.0 {
        sc = 0.0;
        tc = (f / e).clamp(0.0, 1.0);
    } else {
        let c = d1.dot(w);
        if e == 0.0 {
            tc = 0.0;
            sc = (-c / a).clamp(0.0, 1.0);
        } else {
            let b = d1.dot(d2);
            let denom = a * e - b * b;
            let mut s0 = if denom > 0.0 {
                ((b * f - c * e) / denom).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let mut t0 = (b * s0 + f) / e;
            if t0 < 0.0 {
                t0 = 0.0;
                s0 = (-c / a).clamp(0.0, 1.0);
            } else if t0 > 1.0 {
                t0 = 1.0;
                s0 = ((b - c) / a).clamp(0.0, 1.0);
            }
            sc = s0;
            tc = t0;
        }
    }
    (s[0] + d1 * sc, r[0] + d2 * tc)
}

/// The distance between a segment and a triangle, and the closest points:
/// zero where the segment crosses it.
pub(crate) fn segment_triangle(s: &[Point3; 2], t: &[Point3; 3]) -> (f64, Point3, Point3) {
    if let Some(x) = crossing(s, t) {
        return (0.0, x, x);
    }
    let mut best = (f64::INFINITY, s[0], t[0]);
    let mut take = |p: Point3, q: Point3| {
        let d = p.distance(q);
        if d < best.0 {
            best = (d, p, q);
        }
    };
    for &p in s {
        take(p, point_triangle(p, t));
    }
    for k in 0..3 {
        let (p, q) = segment_segment(s, &[t[k], t[(k + 1) % 3]]);
        take(p, q);
    }
    best
}

/// Where segment `s` passes through triangle `t`, if it does (Möller and
/// Trumbore, on the segment's own length).
fn crossing(s: &[Point3; 2], t: &[Point3; 3]) -> Option<Point3> {
    let d = s[1] - s[0];
    let (e1, e2) = (t[1] - t[0], t[2] - t[0]);
    let p = d.cross(e2);
    let det = e1.dot(p);
    if det == 0.0 || !det.is_finite() {
        return None;
    }
    let inv = 1.0 / det;
    let tv = s[0] - t[0];
    let u = tv.dot(p) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = tv.cross(e1);
    let v = d.dot(q) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let along = e2.dot(q) * inv;
    (0.0..=1.0).contains(&along).then(|| s[0] + d * along)
}

/// The smallest distance between two boxes given by their corners.
pub(crate) fn box_gap(a: [Point3; 2], b: [Point3; 2]) -> f64 {
    let gap = |lo1: f64, hi1: f64, lo2: f64, hi2: f64| (lo2 - hi1).max(lo1 - hi2).max(0.0);
    let v = Vector3::new(
        gap(a[0].x, a[1].x, b[0].x, b[1].x),
        gap(a[0].y, a[1].y, b[0].y, b[1].y),
        gap(a[0].z, a[1].z, b[0].z, b[1].z),
    );
    v.length()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(x: f64, y: f64, z: f64) -> Point3 {
        Point3::new(x, y, z)
    }

    #[test]
    fn point_to_triangle_regions() {
        let t = [p(0.0, 0.0, 0.0), p(1.0, 0.0, 0.0), p(0.0, 1.0, 0.0)];
        assert!(point_triangle(p(0.2, 0.2, 3.0), &t).distance(p(0.2, 0.2, 0.0)) < 1e-15);
        assert_eq!(point_triangle(p(-1.0, -1.0, 0.0), &t), t[0]);
        assert_eq!(point_triangle(p(2.0, -0.5, 0.0), &t), t[1]);
        assert_eq!(point_triangle(p(0.5, -2.0, 1.0), &t), p(0.5, 0.0, 0.0));
        assert_eq!(point_triangle(p(1.0, 1.0, 0.0), &t), p(0.5, 0.5, 0.0));
    }

    #[test]
    fn segments_skew_parallel_and_crossing() {
        let (a, b) = segment_segment(
            &[p(0.0, 0.0, 0.0), p(1.0, 0.0, 0.0)],
            &[p(0.5, -1.0, 2.0), p(0.5, 1.0, 2.0)],
        );
        assert_eq!((a, b), (p(0.5, 0.0, 0.0), p(0.5, 0.0, 2.0)));
        let (a, b) = segment_segment(
            &[p(0.0, 0.0, 0.0), p(1.0, 0.0, 0.0)],
            &[p(2.0, 1.0, 0.0), p(3.0, 1.0, 0.0)],
        );
        assert_eq!((a, b), (p(1.0, 0.0, 0.0), p(2.0, 1.0, 0.0)));
        let t = [p(0.0, 0.0, 0.0), p(1.0, 0.0, 0.0), p(0.0, 1.0, 0.0)];
        let (d, x, _) = segment_triangle(&[p(0.2, 0.2, -1.0), p(0.2, 0.2, 1.0)], &t);
        assert_eq!((d, x), (0.0, p(0.2, 0.2, 0.0)));
        let (d, _, _) = segment_triangle(&[p(0.2, 0.2, 1.0), p(0.2, 0.2, 2.0)], &t);
        assert_eq!(d, 1.0);
    }

    #[test]
    fn triangles_apart_and_through() {
        let t = Prim::Triangle([p(0.0, 0.0, 0.0), p(1.0, 0.0, 0.0), p(0.0, 1.0, 0.0)]);
        let up = Prim::Triangle([p(0.0, 0.0, 2.0), p(1.0, 0.0, 2.0), p(0.0, 1.0, 2.0)]);
        assert_eq!(between(&t, &up).0, 2.0);
        let through = Prim::Triangle([p(0.1, 0.1, -1.0), p(0.2, 0.1, 1.0), p(0.1, 0.2, 1.0)]);
        assert_eq!(between(&t, &through).0, 0.0);
        assert_eq!(
            box_gap(
                [p(0.0, 0.0, 0.0), p(1.0, 1.0, 1.0)],
                [p(4.0, 5.0, 0.5), p(6.0, 6.0, 0.6)]
            ),
            5.0
        );
    }
}
