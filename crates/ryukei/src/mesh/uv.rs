//! A face's parameter plane: how it is scaled, where its seams are, which
//! of its boundary rows collapse to a point, and how model-space points
//! are inverted into it.

use super::bound::Metric;
use crate::geom::{Point2, Point3, Surface};
use crate::tol::LINEAR_RESOLUTION;
use crate::topo::{Body, FaceId, Sense};

/// The two parameter directions, as indices: 0 is `u`, 1 is `v`.
pub(crate) const U: usize = 0;
pub(crate) const V: usize = 1;

/// What meshing needs to know about one face's parameter plane.
#[derive(Clone, Debug)]
pub(crate) struct FaceGeom<'a> {
    pub(crate) face: FaceId,
    pub(crate) surface: &'a Surface,
    /// The face's normal is the surface's natural normal.
    pub(crate) forward: bool,
    pub(crate) metric: Metric,
    /// The period of each direction, when it is periodic.
    pub(crate) periods: [Option<f64>; 2],
    /// For a direction the surface is periodic in but the face is not (see
    /// `open_unwound`), where the period holding the face starts.
    pub(crate) wrap: [Option<f64>; 2],
    /// The surface's own periods.
    pub(crate) full: [Option<f64>; 2],
    pub(crate) domain: [[f64; 2]; 2],
    /// Whether the boundary row at each end (`[direction][end]`) of a
    /// direction that is not periodic shrinks to a single point: a
    /// sphere's poles, a cone's apex, a NURBS's collapsed control row.
    pub(crate) collapsed: [[bool; 2]; 2],
}

/// The scale of a parameter direction: its speed at the first sample
/// where that is more than resolution, or 1 where it never is, rounded to
/// the nearest power of two. Scaling by a power of two is exact, so a
/// point and its parameters always agree on which way a triangle winds.
fn scale(speeds: impl Iterator<Item = f64>) -> f64 {
    let s = speeds
        .into_iter()
        .find(|&s| s > LINEAR_RESOLUTION && s.is_finite())
        .unwrap_or(1.0);
    power_of_two(s)
}

/// The power of two nearest `x > 0` (by ratio).
pub(crate) fn power_of_two(x: f64) -> f64 {
    let e = ((x.to_bits() >> 52) & 0x7ff) as i64 - 1023;
    let below = f64::from_bits(((e + 1023) as u64) << 52);
    if x >= below * core::f64::consts::SQRT_2 {
        below * 2.0
    } else {
        below
    }
}

impl<'a> FaceGeom<'a> {
    pub(crate) fn new(b: &'a Body, f: FaceId) -> Self {
        let face = &b.faces[f];
        let surface = &b.surfaces[face.surface];
        let (pu, pv) = surface.periods();
        let domain = surface.domain();
        let forward = face.sense == Sense::Forward;
        let (periods, wrap) = open_unwound(b, f, surface, [pu, pv], forward);
        let collapsed = collapsed_rows(surface);
        // The parameter box of the face's vertices (or of its domain when
        // it has none), within the domain's finite part.
        let mut lo = [f64::INFINITY; 2];
        let mut hi = [f64::NEG_INFINITY; 2];
        for l in b.face_loops(f) {
            let lp = &b.loops[l];
            let corners = lp
                .vertex
                .into_iter()
                .chain(b.loop_fins(l).map(|fin| b.start_of(fin)));
            for v in corners {
                let (u, w) = surface.project(b.vertices[v].point).param;
                for (k, x) in [u, w].into_iter().enumerate() {
                    lo[k] = lo[k].min(x);
                    hi[k] = hi[k].max(x);
                }
            }
        }
        let range: [[f64; 2]; 2] = core::array::from_fn(|k| {
            let [d0, d1] = domain[k];
            let finite = |x: f64, y: f64| if x.is_finite() { x } else { y };
            if lo[k] <= hi[k] {
                [lo[k], hi[k]]
            } else {
                [finite(d0, -1.0), finite(d1, 1.0)]
            }
        });
        // The middle of the box first, then the centres of a 3 × 3 grid of
        // cells over it.
        let at = |k: usize, f: f64| range[k][0] + (range[k][1] - range[k][0]) * f;
        let cells = [1.0 / 6.0, 0.5, 5.0 / 6.0];
        let evals: Vec<_> = core::iter::once((0.5, 0.5))
            .chain(
                cells
                    .iter()
                    .flat_map(|&x| cells.iter().map(move |&y| (x, y))),
            )
            .map(|(x, y)| surface.eval(at(U, x), at(V, y)))
            .collect();
        let metric = Metric {
            a: scale(evals.iter().map(|e| e.du.length())),
            b: scale(evals.iter().map(|e| e.dv.length())),
        };
        Self {
            face: f,
            surface,
            forward,
            metric,
            periods,
            wrap,
            full: [pu, pv],
            domain,
            collapsed,
        }
    }

    /// The scale of direction `k`.
    fn scale_of(&self, k: usize) -> f64 {
        if k == U { self.metric.a } else { self.metric.b }
    }

    /// The seam of a periodic direction: the start of its domain.
    pub(crate) fn seam(&self, k: usize) -> Option<f64> {
        self.periods[k].map(|_| self.domain[k][0])
    }

    /// The parameters of the nearest point of the surface to `p`, and its
    /// distance. Periodic parameters are in `[seam, seam + period)`, and
    /// parameters within resolution of a seam or of a collapsed row are
    /// moved exactly onto it.
    pub(crate) fn invert(&self, p: Point3) -> (Point2, f64) {
        let foot = self.surface.project(p);
        let (u, v) = foot.param;
        (self.snap(Point2::new(u, v)), foot.distance)
    }

    /// Moves parameters within resolution of a seam or collapsed row onto
    /// it; periodic ones into `[seam, seam + period)`.
    pub(crate) fn snap(&self, p: Point2) -> Point2 {
        let mut x = [p.x, p.y];
        for (k, x) in x.iter_mut().enumerate() {
            let close = |a: f64, b: f64| (a - b).abs() * self.scale_of(k) <= LINEAR_RESOLUTION;
            if let (Some(period), Some(seam)) = (self.periods[k], self.seam(k)) {
                *x = seam + (*x - seam).rem_euclid(period);
                if close(*x, seam) || close(*x, seam + period) {
                    *x = seam;
                }
            } else if let (Some(base), Some(period)) = (self.wrap[k], self.full[k]) {
                *x = base + (*x - base).rem_euclid(period);
            } else {
                for end in 0..2 {
                    if self.collapsed[k][end] && close(*x, self.domain[k][end]) {
                        *x = self.domain[k][end];
                    }
                }
            }
        }
        Point2::new(x[0], x[1])
    }

    /// The collapsed row `(direction, end)` that parameters lie exactly
    /// on, if any: there the other parameter is meaningless.
    pub(crate) fn pole(&self, p: Point2) -> Option<(usize, usize)> {
        let x = [p.x, p.y];
        (0..2)
            .flat_map(|k| (0..2).map(move |e| (k, e)))
            .find(|&(k, e)| self.collapsed[k][e] && x[k] == self.domain[k][e])
    }
}

/// A face on a surface periodic both ways has a domain only when its
/// loops cut it open in one direction: a torus face between two circles
/// (a ring turned whole, or a pipe's elbow) is a strip periodic only the
/// way its loops run. A direction no loop winds round is meshed as open,
/// its parameters wrapped into the one period that holds the face, which
/// starts just below where the face does: below the loop the face lies
/// above (on the left of a loop winding round, by the face's sense), or
/// below the face's single loop. The windings and extents are read from
/// the face's edges, sampled, inverted and unwrapped. A face whose loops
/// wind both ways keeps both periods (and is refused, as before).
///
/// Returns the periods to mesh with, and for an opened direction the start
/// of its period.
fn open_unwound(
    b: &Body,
    f: FaceId,
    surface: &Surface,
    periods: [Option<f64>; 2],
    forward: bool,
) -> ([Option<f64>; 2], [Option<f64>; 2]) {
    const SAMPLES: usize = 16;
    let none = (periods, [None; 2]);
    let [Some(pu), Some(pv)] = periods else {
        return none;
    };
    let period = [pu, pv];
    // Each loop: its winding both ways, and its unwrapped extent.
    let mut loops: Vec<([i64; 2], [[f64; 2]; 2])> = Vec::new();
    for l in b.face_loops(f) {
        let mut x: Vec<[f64; 2]> = Vec::new();
        for fin in b.loop_fins(l) {
            let e = &b.edges[b.fins[fin].edge];
            let curve = &b.curves[e.curve];
            let [t0, t1] = e.interval;
            for i in 0..SAMPLES {
                let s = i as f64 / SAMPLES as f64;
                let s = if b.fins[fin].sense == Sense::Forward {
                    s
                } else {
                    1.0 - s
                };
                let (u, v) = surface.project(curve.point(t0 + (t1 - t0) * s)).param;
                let raw = [u, v];
                let next = match x.last() {
                    Some(p) => {
                        core::array::from_fn(|k| super::edge::unwrap(raw[k], p[k], Some(period[k])))
                    }
                    None => raw,
                };
                x.push(next);
            }
        }
        let (Some(first), Some(last)) = (x.first(), x.last()) else {
            continue;
        };
        let wind: [i64; 2] = core::array::from_fn(|k| {
            let close = super::edge::unwrap(first[k], last[k], Some(period[k]));
            ((close - first[k]) / period[k]).round() as i64
        });
        let extent: [[f64; 2]; 2] = core::array::from_fn(|k| {
            let lo = x.iter().map(|p| p[k]).fold(f64::INFINITY, f64::min);
            let hi = x.iter().map(|p| p[k]).fold(f64::NEG_INFINITY, f64::max);
            [lo, hi]
        });
        loops.push((wind, extent));
    }
    if loops.is_empty() {
        return none;
    }
    let Some(k) = (0..2).find(|&k| loops.iter().all(|(w, _)| w[k] == 0)) else {
        return none;
    };
    let j = 1 - k;
    // The face is on the left of each loop when it uses the surface's
    // normal: above (in v) a loop running +u, and above (in u) a loop
    // running -v.
    let up = if k == V { 1 } else { -1 };
    let up = if forward { up } else { -up };
    let below = loops.iter().find(|(w, _)| w[j] == up);
    let lo = match below {
        Some((_, extent)) => extent[k][0],
        None => {
            let widest = loops
                .iter()
                .max_by(|a, b| (a.1[k][1] - a.1[k][0]).total_cmp(&(b.1[k][1] - b.1[k][0])))
                .expect("a loop");
            widest.1[k][0]
        }
    };
    let mut open = periods;
    open[k] = None;
    let mut wrap = [None; 2];
    wrap[k] = Some(lo - 1e-9 * period[k]);
    (open, wrap)
}

/// Which boundary rows of a surface collapse to a point, `[direction][end]`.
/// An extrusion has none; a revolution's are its `v` ends on the axis.
fn collapsed_rows(s: &Surface) -> [[bool; 2]; 2] {
    match s {
        Surface::Sphere(_) => [[false; 2], [true, true]],
        Surface::Cone(_) => [[false; 2], [true, false]],
        Surface::Revolution(r) => [[false; 2], r.poles()],
        Surface::Nurbs(n) => {
            let [nu, nv] = n.counts();
            let (pu, pv) = n.periods();
            let row =
                |pts: Vec<Point3>| pts.iter().all(|p| p.distance(pts[0]) <= LINEAR_RESOLUTION);
            let u_row =
                |i: usize| pu.is_none() && row((0..nv).map(|j| n.control_point(i, j)).collect());
            let v_row =
                |j: usize| pv.is_none() && row((0..nu).map(|i| n.control_point(i, j)).collect());
            [[u_row(0), u_row(nu - 1)], [v_row(0), v_row(nv - 1)]]
        }
        _ => [[false; 2]; 2],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::Rigid;
    use crate::name::OpKey;
    use crate::primitive;
    use core::f64::consts::{FRAC_PI_2, TAU};
    use std::sync::Arc;

    fn side(b: &Body) -> FaceId {
        b.faces().next().unwrap()
    }

    #[test]
    fn a_cylinder_side() {
        let (b, _) = primitive::cylinder(OpKey(1), 2.0, 3.0, &Rigid::IDENTITY).unwrap();
        let g = FaceGeom::new(&b, side(&b));
        assert!(matches!(g.surface, Surface::Cylinder(_)));
        assert!(g.forward);
        assert_eq!(g.periods, [Some(TAU), None]);
        assert_eq!(g.seam(U), Some(0.0));
        assert_eq!(g.seam(V), None);
        assert_eq!(g.collapsed, [[false; 2]; 2]);
        assert_eq!((g.metric.a, g.metric.b), (2.0, 1.0));
        // A point just short of a full turn lands on the seam.
        let p = Point3::new(2.0, -1e-10, 1.0);
        let (uv, d) = g.invert(p);
        assert_eq!(uv, Point2::new(0.0, 1.0));
        assert!(d <= 1e-15);
        let (uv, _) = g.invert(Point3::new(0.0, -2.0, 1.0));
        assert!((uv.x - 1.5 * core::f64::consts::PI).abs() <= 1e-15);
    }

    #[test]
    fn scales_are_powers_of_two() {
        assert_eq!(power_of_two(1.0), 1.0);
        assert_eq!(power_of_two(1.4), 1.0);
        assert_eq!(power_of_two(1.5), 2.0);
        assert_eq!(power_of_two(0.3), 0.25);
        assert_eq!(power_of_two(700.0), 512.0);
    }

    #[test]
    fn a_sphere_has_poles() {
        let (b, _) = primitive::sphere(OpKey(1), 2.0, &Rigid::IDENTITY).unwrap();
        let g = FaceGeom::new(&b, side(&b));
        assert_eq!(g.collapsed, [[false; 2], [true, true]]);
        assert_eq!((g.metric.a, g.metric.b), (2.0, 2.0));
        let (uv, _) = g.invert(Point3::new(0.0, 0.0, 2.0 + 1e-12));
        assert_eq!(uv.y, FRAC_PI_2);
        assert_eq!(g.pole(uv), Some((V, 1)));
        assert_eq!(g.pole(Point2::new(1.0, 0.3)), None);
    }

    #[test]
    fn a_nurbs_cone_has_a_collapsed_row() {
        let (mut b, _) = primitive::cone(OpKey(1), 1.0, 2.0, &Rigid::IDENTITY).unwrap();
        let f = side(&b);
        let s = b.faces[f].surface;
        let (n, _) = b.surfaces[s].to_nurbs([[0.0, 0.0], [-2.0, 0.0]]).unwrap();
        let id = b.add_surface(Surface::Nurbs(Arc::new(n)));
        crate::euler::set_surface(&mut b, f, id, Sense::Forward).unwrap();
        let g = FaceGeom::new(&b, f);
        assert_eq!(g.periods[V], None);
        // The cone runs from its apex at v = -2 to its base at v = 0.
        assert_eq!(g.collapsed, [[false; 2], [true, false]]);
    }

    #[test]
    fn an_extruded_side_is_like_a_cylinder() {
        use crate::geom::{Circle, Curve, Extrusion, Frame, Vector3};
        let (mut b, _) = primitive::cylinder(OpKey(1), 2.0, 3.0, &Rigid::IDENTITY).unwrap();
        let f = side(&b);
        let circle = Curve::Circle(Circle::new(Frame::WORLD, 2.0).unwrap());
        let e = Extrusion::new(circle, Vector3::new(0.0, 0.0, 1.0)).unwrap();
        let id = b.add_surface(Surface::Extrusion(Arc::new(e)));
        crate::euler::set_surface(&mut b, f, id, Sense::Forward).unwrap();
        let g = FaceGeom::new(&b, f);
        assert_eq!(g.periods, [Some(TAU), None]);
        assert_eq!(g.collapsed, [[false; 2]; 2]);
        assert_eq!((g.metric.a, g.metric.b), (2.0, 1.0));
        let (uv, _) = g.invert(Point3::new(2.0, -1e-10, 1.0));
        assert_eq!(uv, Point2::new(0.0, 1.0));
    }
}
