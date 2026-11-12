//! Faces narrower than the chord (#598). A face whose one loop, cut at
//! two of its vertices, makes two chains each within the chord of the
//! other is drawn as a strip of triangles between them, each passing
//! the triangle test. Its loops needn't make a region of its parameters:
//! KiCad's Coilcraft coils' wire ends are slivers 0.2 to 0.6 µm wide
//! whose sides cross by up to 84 nm, and are drawn all the same.

use super::bound::{Targets, Verdict, triangle_test};
use super::domain::{Anchor, LoopPoint, Loops};
use super::uv::FaceGeom;
use crate::geom::{Point2, Point3};

/// A face's strip: its points by anchor with their parameters, and its
/// triangles as indices into them, wound as the loop runs.
#[derive(Clone, Debug)]
pub(crate) struct Strip {
    pub(crate) points: Vec<(Anchor, Point2)>,
    pub(crate) triangles: Vec<[u32; 3]>,
}

/// The distance from `p` to the segment `a b`.
fn to_segment(p: Point3, a: Point3, b: Point3) -> f64 {
    let ab = b - a;
    let len2 = ab.dot(ab);
    let s = if len2 > 0.0 {
        ((p - a).dot(ab) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    p.distance(a + ab * s)
}

/// Whether every point of `a` lies within `reach` of the polyline `b`.
fn within(a: &[Point3], b: &[Point3], reach: f64) -> bool {
    a.iter()
        .all(|&p| b.windows(2).any(|w| to_segment(p, w[0], w[1]) <= reach))
}

/// The strip of a face with one loop and no lone vertex, from the first
/// pair of its vertices (in loop order) whose chains are each within the
/// chord of the other and whose triangles all pass, or `None`.
pub(crate) fn strip(
    g: &FaceGeom,
    loops: &Loops,
    fixed: &dyn Fn(Anchor) -> Point3,
    t: &Targets,
) -> Option<Strip> {
    let [ring] = &loops.rings[..] else {
        return None;
    };
    if !loops.lone.is_empty() || ring.len() < 3 {
        return None;
    }
    let n = ring.len();
    let xyz: Vec<Point3> = ring.iter().map(|p| fixed(p.anchor)).collect();
    let corners: Vec<usize> = (0..n)
        .filter(|&i| matches!(ring[i].anchor, Anchor::Vertex(_)))
        .collect();
    for (k, &i) in corners.iter().enumerate() {
        for &j in &corners[k + 1..] {
            // `a` runs from `i` to `j` with the loop, `b` from `i` to `j`
            // against it.
            let a: Vec<usize> = (i..=j).collect();
            let b: Vec<usize> = (0..=n - (j - i)).map(|s| (i + n - s) % n).collect();
            let pa: Vec<Point3> = a.iter().map(|&x| xyz[x]).collect();
            let pb: Vec<Point3> = b.iter().map(|&x| xyz[x]).collect();
            if !(within(&pa, &pb, t.chord) && within(&pb, &pa, t.chord)) {
                continue;
            }
            if let Some(s) = zip(g, ring, &xyz, &a, &b, t) {
                return Some(s);
            }
        }
    }
    None
}

/// The triangles between chains `a` and `b` (indices into `ring`, both
/// from one end to the other), advancing along the shorter diagonal, or
/// `None` when one fails the triangle test. Each is wound as the loop
/// runs: `a` with it and `b` against it, the face to the left of both.
fn zip(
    g: &FaceGeom,
    ring: &[LoopPoint],
    xyz: &[Point3],
    a: &[usize],
    b: &[usize],
    t: &Targets,
) -> Option<Strip> {
    let mut tris: Vec<[usize; 3]> = Vec::new();
    let (mut i, mut k) = (0, 0);
    let (m, n) = (a.len() - 1, b.len() - 1);
    while i < m || k < n {
        let along_a = if i == m {
            false
        } else if k == n {
            true
        } else {
            xyz[a[i + 1]].distance(xyz[b[k]]) <= xyz[a[i]].distance(xyz[b[k + 1]])
        };
        let tri = if along_a {
            i += 1;
            [a[i - 1], a[i], b[k]]
        } else {
            k += 1;
            [a[i], b[k], b[k - 1]]
        };
        if tri[0] != tri[1] && tri[1] != tri[2] && tri[2] != tri[0] {
            tris.push(tri);
        }
    }
    for tri in &tris {
        let uv = tri.map(|x| ring[x].uv);
        let p = tri.map(|x| xyz[x]);
        if triangle_test(g.surface, g.metric, g.stretched(), uv, p, t) != Verdict::Pass {
            return None;
        }
    }
    let mut used: Vec<usize> = tris.iter().flatten().copied().collect();
    used.sort_unstable();
    used.dedup();
    let at = |x: usize| used.binary_search(&x).expect("used") as u32;
    Some(Strip {
        points: used.iter().map(|&x| (ring[x].anchor, ring[x].uv)).collect(),
        triangles: tris.iter().map(|t| t.map(at)).collect(),
    })
}
