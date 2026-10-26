//! The first cells of a surface's parameter plane for the general
//! intersectors: boxes of parameters covering the part of the surface
//! near some box in model space, cut where the surface's own pieces meet.
//!
//! A NURBS starts from its Bézier patches; a swept surface or a pipe from
//! its curve's pieces one way and eighths of a turn (or a range along a
//! direction) the other; an analytic surface from eighths of a turn in a
//! periodic direction, quarters of a bounded one, and the range a box
//! reaches along an unbounded one. Cells whose enclosure can't reach the
//! box are left out. The intersectors subdivide from these.

use super::pipe::Cell;
use crate::geom::sheet::Sheet;
use crate::geom::{Box3, Curve, Point3, Surface, Vector3};
use core::f64::consts::TAU;

/// The cells of `s`'s parameter plane whose points may lie in `near`.
pub(crate) fn cells(s: &Surface, near: &Box3) -> Vec<Cell> {
    let [du, dv] = s.domain();
    let (pu, pv) = s.periods();
    let (us, vs): (Vec<[f64; 2]>, Vec<[f64; 2]>) = match s {
        Surface::Nurbs(n) => {
            let out = n.patches().iter().map(|p| p.range).collect::<Vec<Cell>>();
            return reaching(s, out, near);
        }
        Surface::Extrusion(e) => {
            let c = e.curve();
            let along = |b: &Box3| span(b, e.dir().get());
            let (cb, nb) = (along(&c.bounds(c.domain())), along(near));
            (pieces(c), vec![[nb[0] - cb[1], nb[1] - cb[0]]])
        }
        Surface::Revolution(r) => (parts([0.0, TAU], 8), pieces(r.curve())),
        Surface::Pipe(p) => (pieces(p.spine()), parts(dv, 8)),
        _ => {
            let f = s.frame().expect("an analytic surface");
            let o = f.origin().to_vector();
            let reach = |w: Vector3| {
                let [a, b] = span(near, w);
                let at = o.dot(w);
                [a - at, b - at]
            };
            let split = |d: [f64; 2], period: Option<f64>, w: Vector3| {
                if period.is_some() {
                    parts(d, 8)
                } else if d[0].is_finite() && d[1].is_finite() {
                    parts(d, 4)
                } else {
                    let [a, b] = reach(w);
                    let r = [a.max(d[0]), b.min(d[1])];
                    if r[0] < r[1] { vec![r] } else { Vec::new() }
                }
            };
            (
                split(du, pu, f.x().get()),
                split(
                    dv,
                    pv,
                    if matches!(s, Surface::Plane(_)) {
                        f.y().get()
                    } else {
                        f.z().get()
                    },
                ),
            )
        }
    };
    let out = us
        .iter()
        .flat_map(|&u| vs.iter().map(move |&v| [u, v]))
        .collect();
    reaching(s, out, near)
}

/// The least and greatest of `p · w` over a box's corners.
fn span(b: &Box3, w: Vector3) -> [f64; 2] {
    b.corners()
        .iter()
        .fold([f64::INFINITY, f64::NEG_INFINITY], |[lo, hi], p| {
            let x = p.to_vector().dot(w);
            [lo.min(x), hi.max(x)]
        })
}

/// `[a, b]` cut into `n` equal parts, the last ending at `b` exactly.
fn parts([a, b]: [f64; 2], n: u32) -> Vec<[f64; 2]> {
    let at = |k: u32| {
        if k == n {
            b
        } else {
            a + (b - a) * f64::from(k) / f64::from(n)
        }
    };
    (0..n).map(|k| [at(k), at(k + 1)]).collect()
}

/// A curve's domain cut where its pieces meet: a NURBS's Bézier pieces,
/// eighths of a closed conic's turn.
fn pieces(c: &Curve) -> Vec<[f64; 2]> {
    match c {
        Curve::Nurbs(n) => n.pieces().iter().map(|p| p.range).collect(),
        _ => parts(c.domain(), 8),
    }
}

/// The cells whose enclosure may reach `near`.
fn reaching(s: &Surface, cells: Vec<Cell>, near: &Box3) -> Vec<Cell> {
    let sheet = Sheet::of(s);
    cells
        .into_iter()
        .filter(|&c| match sheet.jet_box(c) {
            Some(p) => Box3::new(
                Point3::new(p[0].v.lo(), p[1].v.lo(), p[2].v.lo()),
                Point3::new(p[0].v.hi(), p[1].v.hi(), p[2].v.hi()),
            )
            .intersects(near),
            None => true,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{Circle, Cylinder, Extrusion, Frame, Plane, Sphere};
    use std::sync::Arc;

    fn covered(s: &Surface, near: &Box3, samples: &[(f64, f64)]) {
        let cs = cells(s, near);
        for &(u, v) in samples {
            let [du0, dv0] = s.domain();
            let (pu0, pv0) = s.periods();
            let off = |x: f64, p: Option<f64>, d: [f64; 2]| p.is_none() && (x < d[0] || x > d[1]);
            if off(u, pu0, du0) || off(v, pv0, dv0) {
                continue;
            }
            let p = s.point(u, v);
            let (lo, hi) = (near.min(), near.max());
            let inside = (lo.x..=hi.x).contains(&p.x)
                && (lo.y..=hi.y).contains(&p.y)
                && (lo.z..=hi.z).contains(&p.z);
            if !inside {
                continue;
            }
            let (pu, pv) = s.periods();
            let wrap = |x: f64, p: Option<f64>, d: [f64; 2]| match p {
                Some(p) => d[0] + (x - d[0]).rem_euclid(p),
                None => x,
            };
            let [du, dv] = s.domain();
            let outside =
                |x: f64, p: Option<f64>, d: [f64; 2]| p.is_none() && (x < d[0] || x > d[1]);
            if outside(u, pu, du) || outside(v, pv, dv) {
                continue;
            }
            let (u, v) = (wrap(u, pu, du), wrap(v, pv, dv));
            assert!(
                cs.iter()
                    .any(|c| c[0][0] <= u && u <= c[0][1] && c[1][0] <= v && v <= c[1][1]),
                "{s:?} at {u}, {v}"
            );
        }
    }

    #[test]
    fn cells_cover_the_surface_near_a_box() {
        let near = Box3::new(Point3::new(-1.0, -1.0, -1.0), Point3::new(1.5, 1.0, 2.0));
        let f = Frame::new(
            Point3::new(0.2, 0.1, -0.3),
            Vector3::new(0.3, 0.1, 1.0),
            Vector3::new(1.0, 0.0, 0.0),
        )
        .unwrap();
        let circle = Circle::new(f, 0.6).unwrap();
        let (nc, _) = circle.to_nurbs().unwrap();
        let surfaces = [
            Surface::Plane(Plane::new(f)),
            Surface::Cylinder(Cylinder::new(f, 0.6).unwrap()),
            Surface::Sphere(Sphere::new(f, 0.9).unwrap()),
            Surface::Extrusion(Arc::new(
                Extrusion::new(Curve::Nurbs(Arc::new(nc)), Vector3::new(0.0, 0.2, 1.0)).unwrap(),
            )),
        ];
        let samples: Vec<(f64, f64)> = (0..40)
            .flat_map(|i| {
                (0..40).map(move |j| (-3.0 + 0.17 * f64::from(i), -1.5 + 0.08 * f64::from(j)))
            })
            .collect();
        for s in &surfaces {
            covered(s, &near, &samples);
        }
        // A box far from a cylinder's axis keeps only the cells near it.
        let far = Box3::new(Point3::new(5.0, 5.0, 5.0), Point3::new(6.0, 6.0, 6.0));
        assert!(cells(&surfaces[1], &far).is_empty());
    }
}
