//! Extrude: a profile swept along a straight line.

use super::build::Keys;
use super::capped::{self, Motion};
use super::names::Namer;
use super::{Segment, SweepError, ValidProfile, place};
use crate::Error;
use crate::geom::{
    Curve, Curve2, Cylinder, Extrusion, Frame, Line, Plane, Point2, Rigid, Surface, UnitVector3,
    Vector3,
};
use crate::name::{Journal, OpKey};
use crate::tol::{ANGULAR_RESOLUTION, LINEAR_RESOLUTION};
use crate::topo::Body;

/// Extrudes `profile` along `dir` from `start` to `end` (distances along
/// `dir` from the profile's plane): one solid per lump of the profile.
///
/// `dir` may be oblique, but must be more than [`ANGULAR_RESOLUTION`] from
/// the profile's plane (`DirectionInPlane`); within that of the plane's
/// normal it is taken to be the normal. The extrusion must be at least
/// `10 × LINEAR_RESOLUTION` thick, measured along the normal (`ShortSpan`),
/// and stay in the size box (`OutsideSizeBox`). `dir` with no direction is
/// `Degenerate`; non-finite input is `NonFinite`.
///
/// Every face is analytic where the segment allows it (a line sweeps a
/// plane, a circle along the normal a cylinder) and an [`Extrusion`]
/// otherwise. Names come from `key` and the profile's keys:
/// [`crate::name::SweepRole`] lists them.
pub fn extrude(
    key: OpKey,
    profile: &ValidProfile,
    dir: Vector3,
    start: f64,
    end: f64,
) -> Result<(Body, Journal), Error> {
    let motion = Straight::new(profile.frame(), dir, start, end)?;
    check_extent(profile, &motion)?;
    let mut b = Body::new();
    let mut keys = Keys::new(key);
    let namer = Namer::new("extrude", key);
    for lump in profile.lumps() {
        capped::sweep(&mut b, &mut keys, &namer, profile.frame(), lump, &motion)?;
    }
    let journal = namer.finish(&mut b)?;
    Ok((b, journal))
}

/// An extrude's motion: along a unit direction from `start` to `end`.
pub(super) struct Straight {
    dir: UnitVector3,
    start: f64,
    end: f64,
    sign: f64,
    /// The direction is the profile's normal, or its reverse.
    normal: bool,
}

impl Straight {
    pub(super) fn new(frame: &Frame, dir: Vector3, start: f64, end: f64) -> Result<Self, Error> {
        Vector3::try_new(dir.x, dir.y, dir.z)?;
        if !(start.is_finite() && end.is_finite()) {
            return Err(Error::NonFinite);
        }
        let dir = dir.normalize().ok_or(Error::Degenerate)?;
        let n = frame.z();
        let along = dir.dot(n);
        if along.abs() <= ANGULAR_RESOLUTION {
            return Err(SweepError::DirectionInPlane.into());
        }
        let across = dir.get().cross(n.get()).length();
        let (dir, normal) = if across <= ANGULAR_RESOLUTION {
            (if along > 0.0 { n } else { -n }, true)
        } else {
            (dir, false)
        };
        if (end - start) * along.abs() < 10.0 * LINEAR_RESOLUTION {
            return Err(SweepError::ShortSpan.into());
        }
        Ok(Self {
            dir,
            start,
            end,
            sign: along.signum(),
            normal,
        })
    }
}

impl Motion for Straight {
    fn at(&self, end: bool) -> Rigid {
        let d = if end { self.end } else { self.start };
        Rigid::translation(self.dir.get() * d)
    }

    fn sign(&self) -> f64 {
        self.sign
    }

    fn fixed(&self, _: Point2) -> bool {
        false
    }

    fn lateral(
        &self,
        frame: &Frame,
        p: Point2,
        from_end: bool,
    ) -> Result<(Curve, [f64; 2]), Error> {
        let at = place::point(frame, p);
        let (a, z) = (
            self.at(from_end).apply_point(at),
            self.at(!from_end).apply_point(at),
        );
        Ok((Curve::Line(Line::through(a, z)?), [0.0, a.distance(z)]))
    }

    fn side(&self, frame: &Frame, s: &Segment) -> Result<Option<Surface>, Error> {
        let d = self.dir.get();
        Ok(Some(match &s.curve {
            Curve2::Line2(l) => {
                let along = place::vector(frame, l.dir());
                let f = Frame::new(place::point(frame, l.origin()), along.cross(d), along)?;
                Surface::Plane(Plane::new(f))
            }
            Curve2::Circle2(c) if self.normal => {
                let centre = place::point(frame, c.frame().origin());
                let f = Frame::new(centre, d, place::vector(frame, c.frame().x()))?;
                Surface::Cylinder(Cylinder::new(f, c.radius())?)
            }
            c => {
                let curve = place::curve(frame, c)?;
                Surface::Extrusion(std::sync::Arc::new(Extrusion::new(curve, d)?))
            }
        }))
    }
}

/// Fails unless the profile's box, on both caps, is inside the size box.
/// The swept solid lies in the hull of those, which is convex.
fn check_extent(profile: &ValidProfile, motion: &Straight) -> Result<(), Error> {
    let frame = profile.frame();
    for lump in profile.lumps() {
        for s in &lump.outer.segments {
            let bx = s.curve.bounds([s.t0.min(s.t1), s.t0.max(s.t1)]);
            let (lo, hi) = (bx.min(), bx.max());
            for (x, y) in [(lo.x, lo.y), (hi.x, lo.y), (lo.x, hi.y), (hi.x, hi.y)] {
                let p = place::point(frame, Point2::new(x, y));
                for end in [false, true] {
                    let q = motion.at(end).apply_point(p);
                    crate::geom::Point3::try_new(q.x, q.y, q.z)
                        .map_err(|_| Error::from(SweepError::OutsideSizeBox))?;
                }
            }
        }
    }
    Ok(())
}
