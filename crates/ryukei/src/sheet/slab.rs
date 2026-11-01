//! Whether a face or an edge lies between a flange's two side planes.
//!
//! The slab is `0 <= h <= t`, `h` the height above the first side along
//! its inward normal, grown by the tolerance. Edges have closed forms: a
//! line's ends, a circle's or an ellipse's `c + A cos s + B sin s` over its
//! interval, a NURBS's control points over its stretch (its convex hull).
//! A planar face lies in the slab when its edges do (the slab and the
//! region its outer loop bounds are convex); a cylinder or cone whose axis
//! is along the normal by its height range (`v` from its pcurves), a
//! cylinder whose axis lies across it by its angles, a torus about the
//! normal by its tube; anything else by the height's mean value form over
//! its parameter box, cell by cell ([`super::cells`]).

use super::cells::{self, Form, Parts, Verdict, dot, iv};
use super::shape::{self, Flat, edges_of};
use crate::geom::{Curve, Point3, Surface, UnitVector3, polar_angle};
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
        Curve::Nurbs(n) => {
            let part = n.part([t0, t1]).ok()?;
            let hs: Vec<f64> = part.control_points().map(|p| slab.height(p)).collect();
            [
                hs.iter().copied().fold(f64::INFINITY, f64::min),
                hs.iter().copied().fold(f64::NEG_INFINITY, f64::max),
            ]
        }
    })
}

/// Whether an edge lies in the slab grown by `tol` and its tolerances.
pub(super) fn edge_within(b: &Body, e: EdgeId, slab: &Slab, tol: f64) -> Verdict {
    let eps = tol + shape::edge_tolerance(b, e);
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

    #[test]
    fn waves_reach_their_peaks_inside_the_range() {
        let w = wave(1.0, 2.0, 0.0, [-1.0, 1.0]);
        assert_eq!(w[1], 3.0);
        assert!((w[0] - (1.0 + 2.0 * crate::math::cos(1.0))).abs() < 1e-15);
        let w = wave(0.0, 0.0, 1.0, [0.0, TAU]);
        assert_eq!(w, [-1.0, 1.0]);
    }
}
