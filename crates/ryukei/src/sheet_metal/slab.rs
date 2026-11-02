//! Whether a face or an edge lies between a flange's two side planes.
//!
//! The slab is `0 <= h <= t`, `h` the height above the first side along
//! its inward normal, grown by the tolerance. Edges have closed forms: a
//! line's ends, a circle's or an ellipse's `c + A cos s + B sin s` over its
//! interval; a NURBS's control points over its stretch (its convex hull)
//! certify it in, an end outside certifies it out, and a hull straddling a
//! side is halved until one does.
//! A planar face lies in the slab when its edges do (the slab and the
//! region its outer loop bounds are convex); a cylinder or cone whose axis
//! is along the normal by its height range (`v` from its pcurves), a
//! cylinder whose axis lies across it by its angles, a torus about the
//! normal by its tube; anything else by the height's mean value form over
//! its parameter box, cell by cell ([`super::cells`]).

use super::cells::{self, Form, Parts, Verdict, dot, iv};
use super::shape::{self, Flat, edges_of};
use crate::geom::{Curve, NurbsCurve, Point3, Surface, UnitVector3, polar_angle};
use crate::intersect::face_rect;
use crate::tol::ANGULAR_RESOLUTION;
use crate::topo::{Body, EdgeId, FaceId};
use core::f64::consts::{PI, TAU};

/// The space between a flange's sides.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Slab {
    /// A point on the first side.
    pub(super) origin: Point3,
    /// The first side's inward normal: towards the second.
    pub(super) up: UnitVector3,
    pub(super) thickness: f64,
}

impl Slab {
    /// The slab behind a side, `t` deep.
    pub(super) fn behind(side: &Flat, thickness: f64) -> Self {
        Self {
            origin: side.origin,
            up: -side.normal,
            thickness,
        }
    }

    fn height(&self, p: Point3) -> f64 {
        (p - self.origin).dot(self.up.get())
    }

    /// Whether a height range lies in the slab grown by `eps`.
    fn holds(&self, [lo, hi]: [f64; 2], eps: f64) -> bool {
        lo >= -eps && hi <= self.thickness + eps
    }

    /// Whether a height range certainly leaves it.
    fn leaves(&self, [lo, hi]: [f64; 2], eps: f64) -> bool {
        hi < -eps || lo > self.thickness + eps
    }
}

/// The range of `c + a cos s + b sin s` over `[s0, s1]`.
fn wave(c: f64, a: f64, b: f64, [s0, s1]: [f64; 2]) -> [f64; 2] {
    let f = |s: f64| {
        let (sn, cs) = crate::math::sin_cos(s);
        c + a * cs + b * sn
    };
    let (mut lo, mut hi) = (f(s0).min(f(s1)), f(s0).max(f(s1)));
    let peak = polar_angle(b, a);
    let amp = crate::math::hypot(a, b);
    for k in -2..=2 {
        for (at, value) in [(peak, c + amp), (peak + PI, c - amp)] {
            let s = at + TAU * f64::from(k);
            if s > s0 && s < s1 {
                lo = lo.min(value);
                hi = hi.max(value);
            }
        }
    }
    [lo, hi]
}

/// The height range of an edge.
fn edge_heights(b: &Body, e: EdgeId, slab: &Slab) -> Option<[f64; 2]> {
    let edge = &b.edges[e];
    let [t0, t1] = edge.interval;
    let up = slab.up.get();
    Some(match &b.curves[edge.curve] {
        Curve::Line(_) => {
            let [p, q] = [t0, t1].map(|t| slab.height(b.curves[edge.curve].point(t)));
            [p.min(q), p.max(q)]
        }
        Curve::Circle(c) => {
            let f = c.frame();
            let r = c.radius();
            wave(
                slab.height(f.origin()),
                r * f.x().get().dot(up),
                r * f.y().get().dot(up),
                [t0, t1],
            )
        }
        Curve::Ellipse(e) => {
            let f = e.frame();
            wave(
                slab.height(f.origin()),
                e.major() * f.x().get().dot(up),
                e.minor() * f.y().get().dot(up),
                [t0, t1],
            )
        }
        Curve::Nurbs(_) => return None,
    })
}

/// How many times a spline edge is halved before its hull settles it.
const SPLINE_HALVINGS: u32 = 6;

/// Whether a spline over `range` lies in the slab grown by `eps`. Its
/// clamped part's control points bound it (a hull wholly inside is
/// certified in) and its ends lie on it (an end outside certifies it
/// out); a hull straddling a side with both ends in settles nothing, so
/// the part is halved, up to [`SPLINE_HALVINGS`] times.
fn spline_within(n: &NurbsCurve, range: [f64; 2], slab: &Slab, eps: f64, depth: u32) -> Verdict {
    let Ok(part) = n.part(range) else {
        return Verdict::Unsure;
    };
    let hs: Vec<f64> = part.control_points().map(|p| slab.height(p)).collect();
    let hull = [
        hs.iter().copied().fold(f64::INFINITY, f64::min),
        hs.iter().copied().fold(f64::NEG_INFINITY, f64::max),
    ];
    let end_out = |h: Option<&f64>| h.is_some_and(|&h| slab.leaves([h, h], eps));
    if slab.holds(hull, eps) {
        Verdict::Yes
    } else if end_out(hs.first()) || end_out(hs.last()) {
        Verdict::No
    } else if depth == 0 {
        Verdict::Unsure
    } else {
        let mid = 0.5 * (range[0] + range[1]);
        let halves = [[range[0], mid], [mid, range[1]]];
        let mut out = Verdict::Yes;
        for h in halves {
            match spline_within(n, h, slab, eps, depth - 1) {
                Verdict::Yes => {}
                Verdict::No => return Verdict::No,
                Verdict::Unsure => out = Verdict::Unsure,
            }
        }
        out
    }
}

/// Whether an edge lies in the slab grown by `tol` and its tolerances.
pub(super) fn edge_within(b: &Body, e: EdgeId, slab: &Slab, tol: f64) -> Verdict {
    let eps = tol + shape::edge_tolerance(b, e);
    let edge = &b.edges[e];
    if let Curve::Nurbs(n) = &b.curves[edge.curve] {
        return spline_within(n, edge.interval, slab, eps, SPLINE_HALVINGS);
    }
    match edge_heights(b, e, slab) {
        Some(h) if slab.holds(h, eps) => Verdict::Yes,
        Some(_) => Verdict::No,
        None => Verdict::Unsure,
    }
}

/// Whether a face lies in the slab grown by `tol` and its tolerances.
pub(super) fn face_within(b: &Body, f: FaceId, slab: &Slab, tol: f64) -> Verdict {
    let eps = tol + shape::face_tolerance(b, f);
    let surface = &b.surfaces[b.faces[f].surface];
    let up = slab.up;
    let along = |z: UnitVector3| shape::parallel(z, up, ANGULAR_RESOLUTION);
    let across = |z: UnitVector3| z.get().dot(up.get()).abs() <= ANGULAR_RESOLUTION;
    let range = |k: usize| face_rect(b, f).map(|r| r[k]);
    let frame = surface
        .frame()
        .copied()
        .unwrap_or(crate::geom::Frame::WORLD);
    let closed = |h: Option<[f64; 2]>| match h {
        Some(h) if slab.holds(h, eps) => Verdict::Yes,
        Some(_) => Verdict::No,
        None => Verdict::Unsure,
    };
    match surface {
        Surface::Plane(_) => {
            let mut out = Verdict::Yes;
            for e in edges_of(b, f) {
                match edge_within(b, e, slab, tol) {
                    Verdict::Yes => {}
                    v => out = v,
                }
                if out == Verdict::No {
                    break;
                }
            }
            out
        }
        Surface::Cylinder(_) if along(frame.z()) => {
            let k = frame.z().get().dot(up.get());
            let h0 = slab.height(frame.origin());
            closed(range(1).map(|[v0, v1]| sorted(h0 + k * v0, h0 + k * v1)))
        }
        Surface::Cone(_) if along(frame.z()) => {
            let k = frame.z().get().dot(up.get());
            let h0 = slab.height(frame.origin());
            closed(range(1).map(|[v0, v1]| sorted(h0 + k * v0, h0 + k * v1)))
        }
        Surface::Cylinder(c) if across(frame.z()) => {
            let fr = frame;
            let r = c.radius();
            closed(shape::u_range(b, f).map(|us| {
                wave(
                    slab.height(fr.origin()),
                    r * fr.x().get().dot(up.get()),
                    r * fr.y().get().dot(up.get()),
                    us,
                )
            }))
        }
        Surface::Torus(t) if along(frame.z()) => {
            let h0 = slab.height(frame.origin());
            closed(Some([h0 - t.minor(), h0 + t.minor()]))
        }
        _ => {
            let Some(rect) = face_rect(b, f) else {
                return Verdict::Unsure;
            };
            let o = iv(slab.origin.to_vector());
            let n = iv(up.get());
            let value = |q: &Parts| dot(cells::sub(q.p, o), n);
            let gradient = |q: &Parts| [dot(q.u, n), dot(q.v, n)];
            let form = Form {
                value: &value,
                gradient: &gradient,
            };
            cells::judge(surface, rect, |sheet, cell| {
                let Some((h, _)) = form.over(sheet, cell) else {
                    return Verdict::Unsure;
                };
                let h = [h.lo(), h.hi()];
                if slab.holds(h, eps) {
                    Verdict::Yes
                } else if slab.leaves(h, eps) {
                    Verdict::No
                } else {
                    Verdict::Unsure
                }
            })
        }
    }
}

fn sorted(a: f64, b: f64) -> [f64; 2] {
    [a.min(b), a.max(b)]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A spline arc reaching 0.9 in a slab 1 deep, whose quarter-turn
    /// hull reaches 1.27: halved, it is certified in; raised past the
    /// side, an end leaves and it is certified out.
    #[test]
    fn a_spline_whose_hull_overshoots_is_halved_until_it_settles() {
        use crate::geom::{Circle, Frame, Reparam, Vector3};
        let slab = Slab {
            origin: Point3::ORIGIN,
            up: UnitVector3::Z,
            thickness: 1.0,
        };
        let arc = |lift: f64, r: f64| {
            let frame = Frame::new(
                Point3::new(0.0, 0.0, lift),
                Vector3::new(0.0, -1.0, 0.0),
                Vector3::new(1.0, 0.0, 1.0),
            )
            .unwrap();
            Circle::new(frame, r).unwrap().to_nurbs().unwrap().0
        };
        // A quarter turn about the top: one rational quadratic arc.
        let range = [
            Reparam::Angle.to_nurbs(0.0),
            Reparam::Angle.to_nurbs(0.5 * PI),
        ];
        let n = arc(0.0, 0.9);
        let hull = n.part(range).unwrap();
        let top = hull.control_points().map(|p| p.z).fold(0.0, f64::max);
        assert!(top > 1.2, "the hull overshoots: {top}");
        assert_eq!(spline_within(&n, range, &slab, 1e-7, 0), Verdict::Unsure);
        assert_eq!(
            spline_within(&n, range, &slab, 1e-7, SPLINE_HALVINGS),
            Verdict::Yes
        );
        // Its ends at 1.44 and its top at 1.7: out.
        let n = arc(0.8, 0.9);
        assert_eq!(
            spline_within(&n, range, &slab, 1e-7, SPLINE_HALVINGS),
            Verdict::No
        );
        // Ends in and its top past the side by a hundredth: the first
        // halving's end there leaves.
        let n = arc(0.0, 1.01);
        assert_eq!(spline_within(&n, range, &slab, 1e-7, 0), Verdict::Unsure);
        assert_eq!(
            spline_within(&n, range, &slab, 1e-7, SPLINE_HALVINGS),
            Verdict::No
        );
    }

    #[test]
    fn waves_reach_their_peaks_inside_the_range() {
        let w = wave(1.0, 2.0, 0.0, [-1.0, 1.0]);
        assert_eq!(w[1], 3.0);
        assert!((w[0] - (1.0 + 2.0 * crate::math::cos(1.0))).abs() < 1e-15);
        let w = wave(0.0, 0.0, 1.0, [0.0, TAU]);
        assert_eq!(w, [-1.0, 1.0]);
    }
}
