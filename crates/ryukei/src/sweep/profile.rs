//! Profiles and their validation.
//!
//! Checks run in a fixed order and the first fault found is reported, in
//! input order: the structure (loops, segments, keys), each segment
//! (range, length, closed segments), the closure of each loop, the size
//! box, then orientation.

use super::area;
use crate::geom::{Box2, Curve2, Frame, Point2};
use crate::tol::{self, LINEAR_RESOLUTION};
use crate::{Error, ProfileError};
use std::collections::BTreeSet;

/// A caller's key for a segment (ryucad hashes sketch entity IDs into
/// them). Unique across the profile.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SegmentKey(pub u128);

/// A caller's key for a loop. Unique across the profile.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LoopKey(pub u128);

/// A trimmed curve: it runs from `curve(t0)` to `curve(t1)`, backwards
/// along the curve when `t1 < t0`. Coordinates are meters in the profile
/// frame's `xy` plane.
#[derive(Clone, Debug, PartialEq)]
pub struct Segment {
    pub key: SegmentKey,
    pub curve: Curve2,
    pub t0: f64,
    pub t1: f64,
}

/// A closed loop of segments, each ending where the next one starts
/// (cyclically).
#[derive(Clone, Debug, PartialEq)]
pub struct Loop {
    pub key: LoopKey,
    pub segments: Vec<Segment>,
}

/// A planar region: closed loops in the `xy` plane of `frame`, in any
/// orientation and any nesting. See [`ValidProfile::new`].
#[derive(Clone, Debug, PartialEq)]
pub struct Profile {
    pub frame: Frame,
    pub loops: Vec<Loop>,
}

/// A validated profile: loops oriented and nested into lumps.
#[derive(Clone, Debug, PartialEq)]
pub struct ValidProfile {
    frame: Frame,
    lumps: Vec<Lump>,
}

/// One connected region: an outer loop and the holes directly inside it.
#[derive(Clone, Debug, PartialEq)]
pub struct Lump {
    pub outer: ValidLoop,
    pub holes: Vec<ValidLoop>,
}

/// A loop of a validated profile.
#[derive(Clone, Debug, PartialEq)]
pub struct ValidLoop {
    pub key: LoopKey,
    /// Oriented: counterclockwise in the frame's xy plane for an outer
    /// loop, clockwise for a hole. Reversing a loop reverses the segment
    /// order and swaps each segment's t0 and t1.
    pub segments: Vec<Segment>,
    /// vertices[i] is where segments[i] starts: segments[i].curve.point(t0)
    /// after orientation.
    pub vertices: Vec<Point2>,
    /// The tolerance of vertices[i]: the larger of LINEAR_RESOLUTION and
    /// the gap between the previous segment's end and vertices[i].
    pub tolerances: Vec<f64>,
    /// Signed area in the frame (positive for outer loops, negative for
    /// holes).
    pub area: f64,
}

impl Segment {
    pub(super) fn start(&self) -> Point2 {
        self.curve.point(self.t0)
    }

    pub(super) fn end(&self) -> Point2 {
        self.curve.point(self.t1)
    }

    /// The same segment run the other way.
    pub(super) fn reversed(&self) -> Self {
        Self {
            t0: self.t1,
            t1: self.t0,
            ..self.clone()
        }
    }

    /// True when the segment spans exactly one period.
    pub(super) fn is_closed(&self) -> bool {
        self.curve.period() == Some((self.t1 - self.t0).abs())
    }

    /// A box holding the segment.
    pub(super) fn bounds(&self) -> Box2 {
        self.curve
            .bounds([self.t0.min(self.t1), self.t0.max(self.t1)])
    }
}

fn invalid(e: ProfileError) -> Error {
    Error::InvalidProfile(e)
}

impl ValidProfile {
    /// Validates a profile. For now every loop becomes a lump of its own,
    /// turned counterclockwise.
    ///
    /// Fails with [`Error::InvalidProfile`], reporting the first fault in
    /// this order: no loops, an empty loop, a repeated key; then for each
    /// segment in input order a bad range, a segment shorter than
    /// resolution, a whole period sharing its loop, a lone segment that is
    /// not a whole period; then each loop's closure; the size box; then a
    /// loop whose area is too small to orient.
    pub fn new(profile: Profile) -> Result<ValidProfile, Error> {
        check_structure(&profile)?;
        for l in &profile.loops {
            check_segments(l)?;
        }
        for l in &profile.loops {
            check_closed(l)?;
        }
        check_size_box(&profile)?;
        let mut areas = Vec::with_capacity(profile.loops.len());
        for l in &profile.loops {
            let a = area::signed_area(&l.segments);
            areas.push(a.ok_or(invalid(ProfileError::Degenerate { loop_: l.key }))?);
        }
        let lumps = profile
            .loops
            .into_iter()
            .zip(areas)
            .map(|(l, a)| Lump {
                outer: ValidLoop::oriented(l, a, true),
                holes: Vec::new(),
            })
            .collect();
        Ok(Self {
            frame: profile.frame,
            lumps,
        })
    }

    pub fn frame(&self) -> &Frame {
        &self.frame
    }

    /// The lumps, in the order of their outer loops in the input.
    pub fn lumps(&self) -> &[Lump] {
        &self.lumps
    }
}

impl ValidLoop {
    /// `l` turned counterclockwise (`ccw`) or clockwise, given its signed
    /// area `area`.
    fn oriented(l: Loop, area: f64, ccw: bool) -> Self {
        let (segments, area) = if (area > 0.0) == ccw {
            (l.segments, area)
        } else {
            (
                l.segments.iter().rev().map(Segment::reversed).collect(),
                -area,
            )
        };
        let n = segments.len();
        let vertices: Vec<Point2> = segments.iter().map(Segment::start).collect();
        let tolerances = (0..n)
            .map(|i| {
                let gap = segments[(i + n - 1) % n].end().distance(vertices[i]);
                gap.max(LINEAR_RESOLUTION)
            })
            .collect();
        Self {
            key: l.key,
            segments,
            vertices,
            tolerances,
            area,
        }
    }
}

/// Loops present, no empty loop, every key used once.
fn check_structure(p: &Profile) -> Result<(), Error> {
    if p.loops.is_empty() {
        return Err(invalid(ProfileError::Empty));
    }
    if let Some(l) = p.loops.iter().find(|l| l.segments.is_empty()) {
        return Err(invalid(ProfileError::EmptyLoop { loop_: l.key }));
    }
    let mut loops = BTreeSet::new();
    let mut segments = BTreeSet::new();
    for l in &p.loops {
        let fresh = loops.insert(l.key);
        if !(fresh && l.segments.iter().all(|s| segments.insert(s.key))) {
            return Err(invalid(ProfileError::DuplicateKey));
        }
    }
    Ok(())
}

/// Each segment's range and length, and the closed-segment rules.
fn check_segments(l: &Loop) -> Result<(), Error> {
    let alone = l.segments.len() == 1;
    for s in &l.segments {
        let segment = s.key;
        if !range_ok(s) {
            return Err(invalid(ProfileError::BadRange { segment }));
        }
        if is_short(s) {
            return Err(invalid(ProfileError::ShortSegment { segment }));
        }
        match (s.is_closed(), alone) {
            (true, false) => {
                return Err(invalid(ProfileError::ClosedSegmentNotAlone { segment }));
            }
            (false, true) => return Err(invalid(ProfileError::SeamLoop { segment })),
            _ => {}
        }
    }
    Ok(())
}

/// Finite, distinct ends, inside a clamped curve's domain and no longer
/// than a period.
fn range_ok(s: &Segment) -> bool {
    let (t0, t1) = (s.t0, s.t1);
    if !(t0.is_finite() && t1.is_finite()) || t0 == t1 {
        return false;
    }
    match s.curve.period() {
        Some(p) => (t1 - t0).abs() <= p,
        None => {
            let [a, b] = s.curve.domain();
            a <= t0.min(t1) && t0.max(t1) <= b
        }
    }
}

/// The explicit rule for a short segment: the points at nine evenly
/// spaced parameters all lie within [`LINEAR_RESOLUTION`] of its start.
fn is_short(s: &Segment) -> bool {
    let start = s.start();
    (1..=8).all(|i| {
        let t = s.t0 + (s.t1 - s.t0) * f64::from(i) / 8.0;
        s.curve.point(t).distance(start) <= LINEAR_RESOLUTION
    })
}

/// Each segment ends within resolution of the next one's start.
fn check_closed(l: &Loop) -> Result<(), Error> {
    let n = l.segments.len();
    for (i, s) in l.segments.iter().enumerate() {
        let next = &l.segments[(i + 1) % n];
        if s.end().distance(next.start()) > LINEAR_RESOLUTION {
            return Err(invalid(ProfileError::NotClosed {
                loop_: l.key,
                after: s.key,
            }));
        }
    }
    Ok(())
}

/// Every segment's box, carried into model space by the frame, lies in the
/// size box.
fn check_size_box(p: &Profile) -> Result<(), Error> {
    let segments = p.loops.iter().flat_map(|l| &l.segments);
    for s in segments {
        let b = s.bounds();
        let (lo, hi) = (b.min(), b.max());
        for (x, y) in [(lo.x, lo.y), (hi.x, lo.y), (lo.x, hi.y), (hi.x, hi.y)] {
            if !tol::in_size_box(p.frame.to_world(x, y, 0.0)) {
                return Err(invalid(ProfileError::OutsideSizeBox));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::geom::{Circle2, Ellipse2, Frame2, KnotVector, Line2, NurbsCurve2, Vector2};
    use core::f64::consts::{PI, TAU};
    use std::sync::Arc;

    /// A line segment from `a` to `b`, keyed `key`.
    pub(crate) fn line(key: u128, a: (f64, f64), b: (f64, f64)) -> Segment {
        let (a, b) = (Point2::new(a.0, a.1), Point2::new(b.0, b.1));
        Segment {
            key: SegmentKey(key),
            curve: Curve2::Line2(Line2::through(a, b).unwrap()),
            t0: 0.0,
            t1: a.distance(b),
        }
    }

    /// A circle around `c` of radius `r`, counterclockwise.
    pub(crate) fn circle(c: (f64, f64), r: f64) -> Curve2 {
        let f = Frame2::new(Point2::new(c.0, c.1), Vector2::new(1.0, 0.0), true).unwrap();
        Curve2::Circle2(Circle2::new(f, r).unwrap())
    }

    pub(crate) fn arc(key: u128, curve: Curve2, t0: f64, t1: f64) -> Segment {
        Segment {
            key: SegmentKey(key),
            curve,
            t0,
            t1,
        }
    }

    /// A closed polygon through `pts`, segment keys from `first`.
    pub(crate) fn polygon(key: u128, first: u128, pts: &[(f64, f64)]) -> Loop {
        let n = pts.len();
        Loop {
            key: LoopKey(key),
            segments: (0..n)
                .map(|i| line(first + i as u128, pts[i], pts[(i + 1) % n]))
                .collect(),
        }
    }

    /// An axis-aligned square of side `s` with its low corner at `(x, y)`.
    pub(crate) fn square(key: u128, first: u128, (x, y): (f64, f64), s: f64) -> Loop {
        polygon(
            key,
            first,
            &[(x, y), (x + s, y), (x + s, y + s), (x, y + s)],
        )
    }

    pub(crate) fn profile(loops: Vec<Loop>) -> Profile {
        Profile {
            frame: Frame::WORLD,
            loops,
        }
    }

    pub(crate) fn fault(loops: Vec<Loop>) -> ProfileError {
        match ValidProfile::new(profile(loops)) {
            Err(Error::InvalidProfile(e)) => e,
            other => panic!("expected a profile error, got {other:?}"),
        }
    }

    #[test]
    fn a_square() {
        let p = ValidProfile::new(profile(vec![square(1, 10, (0.0, 0.0), 2.0)])).unwrap();
        assert_eq!(p.frame(), &Frame::WORLD);
        assert_eq!(p.lumps().len(), 1);
        let outer = &p.lumps()[0].outer;
        assert_eq!(outer.key, LoopKey(1));
        assert_eq!(outer.area, 4.0);
        assert_eq!(outer.vertices[2], Point2::new(2.0, 2.0));
        assert_eq!(outer.tolerances, [LINEAR_RESOLUTION; 4]);
        assert!(p.lumps()[0].holes.is_empty());
    }

    #[test]
    fn a_clockwise_square_is_turned_round() {
        let cw = polygon(1, 10, &[(0.0, 0.0), (0.0, 1.0), (1.0, 1.0), (1.0, 0.0)]);
        let p = ValidProfile::new(profile(vec![cw.clone()])).unwrap();
        let outer = &p.lumps()[0].outer;
        assert_eq!(outer.area, 1.0);
        let keys: Vec<_> = outer.segments.iter().map(|s| s.key.0).collect();
        assert_eq!(keys, [13, 12, 11, 10]);
        assert_eq!(outer.segments[0], cw.segments[3].reversed());
        assert_eq!(outer.vertices[0], Point2::ORIGIN);
    }

    #[test]
    fn gaps_are_covered_by_vertex_tolerances() {
        let mut l = square(1, 10, (0.0, 0.0), 1.0);
        l.segments[1] = line(11, (1.0, 0.5e-8), (1.0, 1.0));
        let p = ValidProfile::new(profile(vec![l])).unwrap();
        let outer = &p.lumps()[0].outer;
        // The vertex is the next segment's start; closure keeps every gap
        // within resolution.
        assert_eq!(outer.vertices[1], Point2::new(1.0, 0.5e-8));
        assert_eq!(outer.tolerances, [LINEAR_RESOLUTION; 4]);
    }

    #[test]
    fn a_whole_circle_and_a_half_disk() {
        let c = circle((1.0, 1.0), 2.0);
        let p = ValidProfile::new(profile(vec![Loop {
            key: LoopKey(1),
            segments: vec![arc(1, c.clone(), TAU, 0.0)],
        }]))
        .unwrap();
        let outer = &p.lumps()[0].outer;
        assert!((outer.area - 4.0 * PI).abs() <= 1e-13);
        assert_eq!((outer.segments[0].t0, outer.segments[0].t1), (0.0, TAU));
        let half = Loop {
            key: LoopKey(2),
            segments: vec![arc(2, c, 0.0, PI), line(3, (-1.0, 1.0), (3.0, 1.0))],
        };
        assert!(ValidProfile::new(profile(vec![half])).is_ok());
    }

    #[test]
    fn structural_faults() {
        assert_eq!(fault(vec![]), ProfileError::Empty);
        let empty = Loop {
            key: LoopKey(7),
            segments: vec![],
        };
        assert_eq!(
            fault(vec![square(1, 10, (0.0, 0.0), 1.0), empty]),
            ProfileError::EmptyLoop { loop_: LoopKey(7) }
        );
        let a = square(1, 10, (0.0, 0.0), 1.0);
        let b = square(1, 20, (5.0, 0.0), 1.0);
        assert_eq!(fault(vec![a.clone(), b]), ProfileError::DuplicateKey);
        let c = square(2, 13, (5.0, 0.0), 1.0);
        assert_eq!(fault(vec![a, c]), ProfileError::DuplicateKey);
    }

    #[test]
    fn bad_ranges() {
        let key = SegmentKey(10);
        let bad = |s: Segment| {
            let mut l = square(1, 10, (0.0, 0.0), 1.0);
            l.segments[0] = s;
            fault(vec![l])
        };
        let mut s = line(10, (0.0, 0.0), (1.0, 0.0));
        s.t1 = f64::NAN;
        assert_eq!(bad(s.clone()), ProfileError::BadRange { segment: key });
        s.t1 = 0.0;
        assert_eq!(bad(s), ProfileError::BadRange { segment: key });
        // Longer than a period.
        let c = arc(10, circle((0.0, 0.0), 1.0), 0.0, TAU + 1e-9);
        assert_eq!(bad(c), ProfileError::BadRange { segment: key });
        // Outside a clamped NURBS domain.
        let k = KnotVector::clamped(1, &[0.0, 1.0], &[2, 2]).unwrap();
        let n = NurbsCurve2::new(k, vec![Point2::ORIGIN, Point2::new(1.0, 0.0)], None).unwrap();
        let s = arc(10, Curve2::Nurbs2(Arc::new(n)), 0.0, 1.5);
        assert_eq!(bad(s), ProfileError::BadRange { segment: key });
    }

    #[test]
    fn short_segments_by_the_explicit_rule() {
        let mut l = square(1, 10, (0.0, 0.0), 1.0);
        let up = Line2::new(Point2::new(1.0, 0.0), Vector2::new(0.0, 1.0)).unwrap();
        l.segments
            .insert(1, arc(99, Curve2::Line2(up), 0.0, 0.9e-8));
        assert_eq!(
            fault(vec![l]),
            ProfileError::ShortSegment {
                segment: SegmentKey(99)
            }
        );
        // A tiny closed triangle is short even though it is closed.
        let k = KnotVector::periodic(1, &[0.0, 1.0, 2.0, 3.0], &[1, 1, 1, 1]).unwrap();
        let pts = vec![
            Point2::ORIGIN,
            Point2::new(0.4e-8, 0.0),
            Point2::new(0.0, 0.4e-8),
        ];
        let n = Curve2::Nurbs2(Arc::new(NurbsCurve2::new(k, pts, None).unwrap()));
        let tiny = Loop {
            key: LoopKey(1),
            segments: vec![arc(5, n, 0.0, 3.0)],
        };
        assert_eq!(
            fault(vec![tiny]),
            ProfileError::ShortSegment {
                segment: SegmentKey(5)
            }
        );
    }

    #[test]
    fn closed_segments_stand_alone() {
        let c = circle((0.0, 0.0), 1.0);
        let l = Loop {
            key: LoopKey(1),
            segments: vec![arc(1, c.clone(), 0.0, TAU), line(2, (1.0, 0.0), (2.0, 0.0))],
        };
        assert_eq!(
            fault(vec![l]),
            ProfileError::ClosedSegmentNotAlone {
                segment: SegmentKey(1)
            }
        );
        let f = Frame2::new(Point2::ORIGIN, Vector2::new(1.0, 0.0), false).unwrap();
        let e = Curve2::Ellipse2(Ellipse2::new(f, 2.0, 1.0).unwrap());
        let seam = Loop {
            key: LoopKey(1),
            segments: vec![arc(3, e, 1.0, 1.0 + 6.0)],
        };
        assert_eq!(
            fault(vec![seam]),
            ProfileError::SeamLoop {
                segment: SegmentKey(3)
            }
        );
    }

    #[test]
    fn open_loops() {
        let mut l = square(4, 10, (0.0, 0.0), 1.0);
        l.segments[2] = line(12, (1.0, 1.0), (2e-8, 1.0));
        assert_eq!(
            fault(vec![l]),
            ProfileError::NotClosed {
                loop_: LoopKey(4),
                after: SegmentKey(12)
            }
        );
    }

    #[test]
    fn the_size_box() {
        assert_eq!(
            fault(vec![square(1, 10, (499.0, 0.0), 2.0)]),
            ProfileError::OutsideSizeBox
        );
        // The frame carries the profile out.
        let frame = Frame::new(
            crate::geom::Point3::new(0.0, 0.0, 499.5),
            crate::geom::Vector3::new(1.0, 0.0, 0.0),
            crate::geom::Vector3::new(0.0, 0.0, 1.0),
        )
        .unwrap();
        let p = Profile {
            frame,
            loops: vec![square(1, 10, (0.0, 0.0), 1.0)],
        };
        assert_eq!(
            ValidProfile::new(p),
            Err(Error::InvalidProfile(ProfileError::OutsideSizeBox))
        );
    }

    #[test]
    fn degenerate_loops() {
        let flat = polygon(3, 10, &[(0.0, 0.0), (1.0, 0.0), (0.5, 1.5e-12)]);
        assert_eq!(
            fault(vec![flat]),
            ProfileError::Degenerate { loop_: LoopKey(3) }
        );
    }
}
