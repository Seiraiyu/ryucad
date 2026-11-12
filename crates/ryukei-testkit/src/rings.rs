//! Rings turned from rounded rectangles (plan 17.A, #421): their corners
//! are tori, or with `spline` revolutions of their arcs' exact NURBS
//! forms. Every edge is smooth, so a global offset past the corners'
//! radius fills nothing: the corners' offsets pass their focal surfaces
//! whole and are dropped, and what is left is the ring of the inset
//! rectangle, sharp, whose volume Pappus gives.

use core::f64::consts::{FRAC_PI_2, PI, TAU};
use ryukei::geom::{Axis, Circle2, Curve2, Frame, Frame2, Line2, Point2, Point3, Vector2, Vector3};
use ryukei::name::OpKey;
use ryukei::sweep::{Loop, LoopKey, Profile, Segment, SegmentKey, ValidProfile, revolve};
use ryukei::topo::Body;
use std::sync::Arc;

/// A rectangle `[inner, inner + width] × [0, height]` in the `xy` plane,
/// its corners rounded to `radius`, turned a whole turn about `y`.
#[derive(Clone, Copy, Debug)]
pub struct Ring {
    pub inner: f64,
    pub width: f64,
    pub height: f64,
    pub radius: f64,
}

impl Ring {
    /// The volume of the ring shrunk by `d`, at least the corners' radius
    /// and less than half the rectangle's either side: the inset
    /// rectangle, sharp, turned about its centroid's radius.
    pub fn shrunk_volume(&self, d: f64) -> f64 {
        TAU * (self.inner + self.width / 2.0) * (self.width - 2.0 * d) * (self.height - 2.0 * d)
    }
}

/// The ring as a solid; `None` where the profile or the turn refuses.
pub fn ring(key: OpKey, r: &Ring, spline: bool) -> Option<Body> {
    let (x0, x1, y1, q) = (r.inner, r.inner + r.width, r.height, r.radius);
    let line = |a: (f64, f64), b: (f64, f64)| -> Option<(Curve2, f64, f64)> {
        let (p, s) = (Point2::new(a.0, a.1), Point2::new(b.0, b.1));
        Some((
            Curve2::Line2(Line2::through(p, s).ok()?),
            0.0,
            p.distance(s),
        ))
    };
    let arc = |c: (f64, f64), a0: f64, a1: f64| -> Option<(Curve2, f64, f64)> {
        let f = Frame2::new(Point2::new(c.0, c.1), Vector2::new(1.0, 0.0), true).ok()?;
        let c = Curve2::Circle2(Circle2::new(f, q).ok()?);
        if !spline {
            return Some((c, a0, a1));
        }
        let (n, m) = c.to_nurbs([a0, a1]).ok()?;
        Some((Curve2::Nurbs2(Arc::new(n)), m.to_nurbs(a0), m.to_nurbs(a1)))
    };
    let parts = [
        line((x0 + q, 0.0), (x1 - q, 0.0))?,
        arc((x1 - q, q), -FRAC_PI_2, 0.0)?,
        line((x1, q), (x1, y1 - q))?,
        arc((x1 - q, y1 - q), 0.0, FRAC_PI_2)?,
        line((x1 - q, y1), (x0 + q, y1))?,
        arc((x0 + q, y1 - q), FRAC_PI_2, PI)?,
        line((x0, y1 - q), (x0, q))?,
        arc((x0 + q, q), PI, 1.5 * PI)?,
    ];
    let segments = parts
        .into_iter()
        .enumerate()
        .map(|(i, (curve, t0, t1))| Segment {
            key: SegmentKey(10 + i as u128),
            curve,
            t0,
            t1,
        })
        .collect();
    let profile = ValidProfile::new(Profile {
        frame: Frame::WORLD,
        loops: vec![Loop {
            key: LoopKey(1),
            segments,
        }],
    })
    .ok()?;
    let y = Axis::new(Point3::ORIGIN, Vector3::new(0.0, 1.0, 0.0)).ok()?;
    Some(revolve(key, &profile, y, 0.0, TAU).ok()?.0)
}
