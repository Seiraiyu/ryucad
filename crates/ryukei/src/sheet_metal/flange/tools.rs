//! An edge flange's tools, exact sweeps in the edge's frame:
//!
//! - the bend, the sheet's section across the flange's span (`s` from
//!   `span[0]` to `span[1]`, `h` from `-(r + t)` to `-r` about the axis
//!   through the bend line, `r` along the side's normal) revolved through
//!   the bend's angle, so it leaves the bend line along the wall's outward
//!   normal and turns towards the side's;
//! - the flange, the bend's end section extruded along the way the bend
//!   leaves it by the straight length;
//! - the cut, where the position trims the base: the side's region from
//!   past the wall to the bend line, and each relief beside it, extruded
//!   through the sheet.
//!
//! Segment keys say what each segment is, never where it falls in a list,
//! so a flange's sizes, setbacks and reliefs rename nothing they don't
//! make or take away.

use super::plan::Planned;
use crate::Error;
use crate::geom::{Axis, Circle2, Curve2, Frame, Frame2, Line2, Point2, Vector2, Vector3};
use crate::name::OpKey;
use crate::sweep::{Loop, LoopKey, Profile, Segment, SegmentKey, ValidProfile, extrude, revolve};
use crate::topo::Body;
use core::f64::consts::PI;

/// The bend's and the flange's section: from the outer side (or the inner
/// one, for the flange) at the start, round the section.
const SECTION: [SegmentKey; 4] = [SegmentKey(1), SegmentKey(2), SegmentKey(3), SegmentKey(4)];

/// The cut's segments by what they are.
const WALL: SegmentKey = SegmentKey(11);
const START: SegmentKey = SegmentKey(12);
const START_FLOOR: SegmentKey = SegmentKey(13);
const START_SIDE: SegmentKey = SegmentKey(14);
const LINE: SegmentKey = SegmentKey(15);
const END_SIDE: SegmentKey = SegmentKey(16);
const END_FLOOR: SegmentKey = SegmentKey(17);
const END: SegmentKey = SegmentKey(18);

/// One loop of segments, those of no length left out.
fn profile(frame: Frame, segments: Vec<Segment>) -> Result<ValidProfile, Error> {
    let segments = segments
        .into_iter()
        .filter(|s| (s.t1 - s.t0).abs() > 0.0)
        .collect();
    ValidProfile::new(Profile {
        frame,
        loops: vec![Loop {
            key: LoopKey(1),
            segments,
        }],
    })
}

/// The straight segment from `a` to `b`, of no length when they are one.
fn line(key: SegmentKey, a: (f64, f64), b: (f64, f64)) -> Result<Segment, Error> {
    let (a, b) = (Point2::new(a.0, a.1), Point2::new(b.0, b.1));
    let l = a.distance(b);
    let curve = if l > 0.0 {
        Line2::through(a, b)?
    } else {
        Line2::new(a, Vector2::new(1.0, 0.0))?
    };
    Ok(Segment {
        key,
        curve: Curve2::Line2(curve),
        t0: 0.0,
        t1: l,
    })
}

/// The half circle from `(s, x)` to `(s + w, x)` through `(s + w / 2, x +
/// w / 2)`.
fn half(key: SegmentKey, s: f64, x: f64, w: f64) -> Result<Segment, Error> {
    let frame = Frame2::new(Point2::new(s + 0.5 * w, x), Vector2::new(-1.0, 0.0), false)?;
    Ok(Segment {
        key,
        curve: Curve2::Circle2(Circle2::new(frame, 0.5 * w)?),
        t0: 0.0,
        t1: PI,
    })
}

/// A rectangle's segments in `(s, h)`, keyed by [`SECTION`] from `(s0, h0)`.
fn rectangle(s: [f64; 2], h: [f64; 2]) -> Result<Vec<Segment>, Error> {
    let p = [(s[0], h[0]), (s[1], h[0]), (s[1], h[1]), (s[0], h[1])];
    (0..4)
        .map(|k| line(SECTION[k], p[k], p[(k + 1) % 4]))
        .collect()
}

/// The direction the bend turns about, so the section leaves along the
/// wall's outward normal towards the side's.
fn turn(p: &Planned) -> Vector3 {
    let a = p.along.get();
    if a.cross(p.normal.get()).dot(p.out.get()) < 0.0 {
        a
    } else {
        -a
    }
}

/// The axis's point at the edge's start.
fn axis_origin(p: &Planned) -> crate::geom::Point3 {
    p.at(0.0, p.trim, p.radius)
}

/// The bend: the section revolved through the bend's angle.
pub(crate) fn bend(key: OpKey, p: &Planned) -> Result<Body, Error> {
    let (r, t) = (p.radius, p.thickness);
    let a = p.along.get();
    let o = axis_origin(p);
    let frame = Frame::new(o, a.cross(p.normal.get()), a)?;
    let section = profile(frame, rectangle(p.span, [-(r + t), -r])?)?;
    Ok(revolve(key, &section, Axis::new(o, turn(p))?, 0.0, p.angle)?.0)
}

/// The radial direction at the bend's end and the way it leaves.
pub(crate) fn leaving(p: &Planned) -> (Vector3, Vector3) {
    let (sn, cs) = crate::math::sin_cos(p.angle);
    let (n, w) = (p.normal.get(), p.out.get());
    (w * sn - n * cs, n * sn + w * cs)
}

/// The flange: the bend's end section extruded the straight length.
pub(crate) fn flange(key: OpKey, p: &Planned) -> Result<Body, Error> {
    let (r, t) = (p.radius, p.thickness);
    let a = p.along.get();
    let (radial, dir) = leaving(p);
    let frame = Frame::new(axis_origin(p), a.cross(radial), a)?;
    let section = profile(frame, rectangle(p.span, [r, r + t])?)?;
    Ok(extrude(key, &section, dir, 0.0, p.straight)?.0)
}

/// The cut: the base from past the wall to the bend line across the span,
/// with the reliefs, through the sheet and past both sides by its
/// thickness. `None` where nothing is cut.
pub(crate) fn cut(key: OpKey, p: &Planned) -> Result<Option<Body>, Error> {
    if p.trim == 0.0 {
        return Ok(None);
    }
    let (d, m) = (p.trim, p.thickness);
    let [s0, s1] = p.span;
    let size = super::plan::relief_size;
    let (lo, x0) = match size(p.reliefs[0]) {
        Some((w, dep)) => (s0 - w, d + dep),
        None => (s0, d),
    };
    let (hi, x1) = match size(p.reliefs[1]) {
        Some((w, dep)) => (s1 + w, d + dep),
        None => (s1, d),
    };
    let mut segs = vec![line(WALL, (hi, -m), (lo, -m))?];
    match p.reliefs[0] {
        crate::sheet_metal::Relief::Obround { width, .. } => {
            let x = x0 - 0.5 * width;
            segs.push(line(START, (lo, -m), (lo, x))?);
            segs.push(half(START_FLOOR, lo, x, width)?);
            segs.push(line(START_SIDE, (s0, x), (s0, d))?);
        }
        _ => {
            segs.push(line(START, (lo, -m), (lo, x0))?);
            segs.push(line(START_FLOOR, (lo, x0), (s0, x0))?);
            segs.push(line(START_SIDE, (s0, x0), (s0, d))?);
        }
    }
    segs.push(line(LINE, (s0, d), (s1, d))?);
    match p.reliefs[1] {
        crate::sheet_metal::Relief::Obround { width, .. } => {
            let x = x1 - 0.5 * width;
            segs.push(line(END_SIDE, (s1, d), (s1, x))?);
            segs.push(half(END_FLOOR, s1, x, width)?);
            segs.push(line(END, (hi, x), (hi, -m))?);
        }
        _ => {
            segs.push(line(END_SIDE, (s1, d), (s1, x1))?);
            segs.push(line(END_FLOOR, (s1, x1), (hi, x1))?);
            segs.push(line(END, (hi, x1), (hi, -m))?);
        }
    }
    let a = p.along.get();
    let frame = Frame::new(p.at(0.0, 0.0, m), a.cross(-p.out.get()), a)?;
    let section = profile(frame, segs)?;
    let dir = -p.normal.get();
    Ok(Some(
        extrude(key, &section, dir, 0.0, p.thickness + 2.0 * m)?.0,
    ))
}
