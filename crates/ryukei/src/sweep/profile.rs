//! Profiles and their validation.
//!
//! Checks run in a fixed order and the first fault found is reported, in
//! input order: the structure (loops, segments, keys), each segment
//! (range, length, closed segments), the closure of each loop, the size
//! box, the separation of the segments, then orientation and nesting.

use super::nest::{self, Place};
use super::{area, separate};
use crate::geom::chain2::Chain2;
use crate::geom::{Box2, Curve2, Frame, Point2};
use crate::tol::{self, LINEAR_RESOLUTION};
use crate::{Error, ProfileError};
use rayon::prelude::*;
use std::collections::BTreeSet;

/// A caller's key for a segment (ryucad hashes sketch entity IDs into
/// them). Unique across the profile.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SegmentKey(pub u128);

/// A caller's key for a loop. Unique across the profile.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LoopKey(pub u128);

/// A caller's key for an open chain. Chain and loop keys share one space:
/// no chain has a loop's key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ChainKey(pub u128);

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

/// An open chain of segments, each ending where the next one starts; the
/// last doesn't end where the first starts. Swept, it makes a sheet (see
/// [`ValidProfile::surface`]).
#[derive(Clone, Debug, PartialEq)]
pub struct Chain {
    pub key: ChainKey,
    pub segments: Vec<Segment>,
}

/// A planar region: closed loops in the `xy` plane of `frame`, in any
/// orientation and any nesting. See [`ValidProfile::new`].
#[derive(Clone, Debug, PartialEq)]
pub struct Profile {
    pub frame: Frame,
    pub loops: Vec<Loop>,
}

/// A validated profile: loops oriented and nested into lumps, and open
/// chains. One made by [`ValidProfile::surface`] sweeps to a sheet.
#[derive(Clone, Debug, PartialEq)]
pub struct ValidProfile {
    frame: Frame,
    lumps: Vec<Lump>,
    chains: Vec<ValidChain>,
    surface: bool,
}

/// A chain of a validated profile, as given: its own way round.
#[derive(Clone, Debug, PartialEq)]
pub struct ValidChain {
    pub key: ChainKey,
    pub segments: Vec<Segment>,
    /// `vertices[i]` is where `segments[i]` starts; the last is where the
    /// last segment ends (one more than the segments).
    pub vertices: Vec<Point2>,
    /// The tolerance of `vertices[i]`: the larger of `LINEAR_RESOLUTION`
    /// and the gap between the previous segment's end and it (resolution
    /// at the chain's ends).
    pub tolerances: Vec<f64>,
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
    /// Validates a profile and nests its loops into lumps: a loop inside
    /// an even number of others is the outer loop of a lump, turned
    /// counterclockwise; one inside an odd number is a hole of the loop
    /// directly around it, turned clockwise.
    ///
    /// Fails with [`Error::InvalidProfile`], reporting the first fault in
    /// this order: no loops, an empty loop, a repeated key; then for each
    /// segment in input order a bad range, a segment shorter than
    /// resolution, a whole period sharing its loop, a lone segment that is
    /// not a whole period; then each loop's closure; the size box; two
    /// segments that touch (the pair whose first segment comes first, then
    /// whose second does); then a loop whose area is too small to orient,
    /// or whose nesting can't be certified.
    pub fn new(profile: Profile) -> Result<ValidProfile, Error> {
        Self::validate(profile, Vec::new(), false)
    }

    /// Validates a profile to be swept into a sheet (design 295): each
    /// loop sweeps its sides with no caps, and each open chain the sides
    /// of its segments. Loops are checked and nested as [`Self::new`]
    /// checks them; chains are checked in input order after the loops'
    /// segments: a chain with no segments (`EmptyChain`), each segment's
    /// range and length as a loop's, a whole period (`ChainCloses`), each
    /// segment ending within resolution of the next one's start
    /// (`ChainBroken`), and the last not ending there (`ChainCloses`).
    /// Every segment, of loops and chains alike, must keep apart from the
    /// others but its neighbours (`Touching`), and loop and chain keys are
    /// unique together (`DuplicateKey`). A profile of chains alone is
    /// valid; one of nothing is `Empty`.
    pub fn surface(profile: Profile, chains: Vec<Chain>) -> Result<ValidProfile, Error> {
        Self::validate(profile, chains, true)
    }

    fn validate(profile: Profile, chains: Vec<Chain>, surface: bool) -> Result<Self, Error> {
        check_structure(&profile, &chains)?;
        for l in &profile.loops {
            check_segments(l)?;
        }
        for c in &chains {
            check_chain_segments(c)?;
        }
        for l in &profile.loops {
            check_closed(l)?;
        }
        for c in &chains {
            check_chain_joined(c)?;
        }
        check_size_box(&profile, &chains)?;
        let loop_chains = check_separation(&profile, &chains)?;
        let mut areas = Vec::with_capacity(profile.loops.len());
        for l in &profile.loops {
            let a = area::signed_area(&l.segments);
            areas.push(a.ok_or(invalid(ProfileError::Degenerate { loop_: l.key }))?);
        }
        let points: Vec<_> = profile
            .loops
            .iter()
            .map(|l| l.segments[0].start().to_vector())
            .collect();
        let loop_chains: Vec<&[Chain2]> = loop_chains.iter().map(Vec::as_slice).collect();
        let places = nest::places(&loop_chains, &points).map_err(|i| {
            invalid(ProfileError::Degenerate {
                loop_: profile.loops[i].key,
            })
        })?;
        let lumps = lumps(profile.loops, &areas, &places);
        Ok(Self {
            frame: profile.frame,
            lumps,
            chains: chains.into_iter().map(ValidChain::new).collect(),
            surface,
        })
    }

    pub fn frame(&self) -> &Frame {
        &self.frame
    }

    /// The same profile with its frame moved by `r`.
    pub(super) fn moved(&self, r: &crate::geom::Rigid) -> Self {
        Self {
            frame: self.frame.transformed(r),
            ..self.clone()
        }
    }

    /// The lumps, in the order of their outer loops in the input.
    pub fn lumps(&self) -> &[Lump] {
        &self.lumps
    }

    /// The open chains, in input order.
    pub fn chains(&self) -> &[ValidChain] {
        &self.chains
    }

    /// Whether the profile sweeps to a sheet: made by [`Self::surface`].
    pub fn is_surface(&self) -> bool {
        self.surface
    }
}

impl ValidChain {
    fn new(c: Chain) -> Self {
        let n = c.segments.len();
        let mut vertices: Vec<Point2> = c.segments.iter().map(Segment::start).collect();
        vertices.push(c.segments[n - 1].end());
        let tolerances = (0..=n)
            .map(|i| match i {
                0 => LINEAR_RESOLUTION,
                i if i == n => LINEAR_RESOLUTION,
                i => c.segments[i - 1]
                    .end()
                    .distance(vertices[i])
                    .max(LINEAR_RESOLUTION),
            })
            .collect();
        Self {
            key: c.key,
            segments: c.segments,
            vertices,
            tolerances,
        }
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

/// Loops or chains present, none empty, every key used once (loop and
/// chain keys in one space).
fn check_structure(p: &Profile, chains: &[Chain]) -> Result<(), Error> {
    if p.loops.is_empty() && chains.is_empty() {
        return Err(invalid(ProfileError::Empty));
    }
    if let Some(l) = p.loops.iter().find(|l| l.segments.is_empty()) {
        return Err(invalid(ProfileError::EmptyLoop { loop_: l.key }));
    }
    if let Some(c) = chains.iter().find(|c| c.segments.is_empty()) {
        return Err(invalid(ProfileError::EmptyChain { chain: c.key }));
    }
    let mut keys = BTreeSet::new();
    let mut segments = BTreeSet::new();
    let groups = p
        .loops
        .iter()
        .map(|l| (l.key.0, &l.segments))
        .chain(chains.iter().map(|c| (c.key.0, &c.segments)));
    for (key, segs) in groups {
        let fresh = keys.insert(key);
        if !(fresh && segs.iter().all(|s| segments.insert(s.key))) {
            return Err(invalid(ProfileError::DuplicateKey));
        }
    }
    Ok(())
}

/// Each chain segment's range and length, as a loop's; a whole period
/// closes the chain.
fn check_chain_segments(c: &Chain) -> Result<(), Error> {
    for s in &c.segments {
        let segment = s.key;
        if !range_ok(s) {
            return Err(invalid(ProfileError::BadRange { segment }));
        }
        if is_short(s) {
            return Err(invalid(ProfileError::ShortSegment { segment }));
        }
        if s.is_closed() {
            return Err(invalid(ProfileError::ChainCloses { chain: c.key }));
        }
    }
    Ok(())
}

/// Each chain segment ends within resolution of the next one's start, and
/// the last doesn't end within resolution of the first's start.
fn check_chain_joined(c: &Chain) -> Result<(), Error> {
    for pair in c.segments.windows(2) {
        if pair[0].end().distance(pair[1].start()) > LINEAR_RESOLUTION {
            return Err(invalid(ProfileError::ChainBroken {
                chain: c.key,
                after: pair[0].key,
            }));
        }
    }
    let (first, last) = (&c.segments[0], &c.segments[c.segments.len() - 1]);
    if last.end().distance(first.start()) <= LINEAR_RESOLUTION {
        return Err(invalid(ProfileError::ChainCloses { chain: c.key }));
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
fn check_size_box(p: &Profile, chains: &[Chain]) -> Result<(), Error> {
    let segments = p
        .loops
        .iter()
        .flat_map(|l| &l.segments)
        .chain(chains.iter().flat_map(|c| &c.segments));
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

/// No two segments touch, except neighbours at their shared ends: round a
/// loop, along a chain. Returns each loop's segments as chains.
fn check_separation(p: &Profile, open: &[Chain]) -> Result<Vec<Vec<Chain2>>, Error> {
    let segments: Vec<&Segment> = p
        .loops
        .iter()
        .flat_map(|l| &l.segments)
        .chain(open.iter().flat_map(|c| &c.segments))
        .collect();
    let chains: Vec<Chain2> = segments
        .par_iter()
        .map(|s| Chain2::new(&s.curve, s.t0, s.t1))
        .collect();
    let mut groups = Vec::with_capacity(p.loops.len() + open.len());
    let mut first = 0;
    for l in &p.loops {
        groups.push((first, l.segments.len(), true));
        first += l.segments.len();
    }
    for c in open {
        groups.push((first, c.segments.len(), false));
        first += c.segments.len();
    }
    if let Some((i, j)) = separate::first_touching(&chains, &groups) {
        return Err(invalid(ProfileError::Touching {
            a: segments[i].key,
            b: segments[j].key,
        }));
    }
    let mut rest = chains.into_iter();
    Ok(p.loops
        .iter()
        .map(|l| rest.by_ref().take(l.segments.len()).collect())
        .collect())
}

/// The loops gathered into lumps by their places, each oriented: lumps
/// in the order of their outer loops, holes in input order.
fn lumps(loops: Vec<Loop>, areas: &[f64], places: &[Place]) -> Vec<Lump> {
    let mut outer_of = vec![None; loops.len()];
    let mut lumps: Vec<Lump> = Vec::new();
    let mut holes = Vec::new();
    for (i, (l, &area)) in loops.into_iter().zip(areas).enumerate() {
        // A loop at odd depth is a hole; `nest::places` checked that it has
        // a parent, one level up, so an outer loop.
        let place = places[i];
        match place.parent.filter(|_| !place.depth.is_multiple_of(2)) {
            Some(parent) => holes.push((parent, ValidLoop::oriented(l, area, false))),
            None => {
                outer_of[i] = Some(lumps.len());
                lumps.push(Lump {
                    outer: ValidLoop::oriented(l, area, true),
                    holes: Vec::new(),
                });
            }
        }
    }
    for (parent, hole) in holes {
        debug_assert!(outer_of[parent].is_some(), "a hole's parent is outer");
        if let Some(lump) = outer_of[parent] {
            lumps[lump].holes.push(hole);
        }
    }
    lumps
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

    /// An open polyline through `pts`, segment keys from `first`.
    pub(crate) fn chain(key: u128, first: u128, pts: &[(f64, f64)]) -> Chain {
        Chain {
            key: ChainKey(key),
            segments: (0..pts.len() - 1)
                .map(|i| line(first + i as u128, pts[i], pts[i + 1]))
                .collect(),
        }
    }

    pub(crate) fn surface_fault(loops: Vec<Loop>, chains: Vec<Chain>) -> ProfileError {
        match ValidProfile::surface(profile(loops), chains) {
            Err(Error::InvalidProfile(e)) => e,
            other => panic!("expected a profile error, got {other:?}"),
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

    #[test]
    fn chains_alone_and_beside_loops() {
        let zig = chain(5, 50, &[(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (2.0, 1.0)]);
        let p = ValidProfile::surface(profile(vec![]), vec![zig.clone()]).unwrap();
        assert!(p.is_surface());
        assert!(p.lumps().is_empty());
        let c = &p.chains()[0];
        assert_eq!(c.key, ChainKey(5));
        assert_eq!(c.vertices.len(), 4);
        assert_eq!(c.vertices[3], Point2::new(2.0, 1.0));
        assert_eq!(c.tolerances, [LINEAR_RESOLUTION; 4]);
        // Beside a loop, and the loop is a lump as ever.
        let sq = square(1, 10, (5.0, 0.0), 1.0);
        let p = ValidProfile::surface(profile(vec![sq.clone()]), vec![zig]).unwrap();
        assert_eq!((p.lumps().len(), p.chains().len()), (1, 1));
        // A loop alone made a surface is still a lump.
        let q = ValidProfile::surface(profile(vec![sq]), vec![]).unwrap();
        assert!(q.is_surface() && q.chains().is_empty());
        assert!(
            !ValidProfile::new(profile(vec![square(1, 10, (0.0, 0.0), 1.0)]))
                .unwrap()
                .is_surface()
        );
    }

    #[test]
    fn chain_faults() {
        assert_eq!(surface_fault(vec![], vec![]), ProfileError::Empty);
        let empty = Chain {
            key: ChainKey(7),
            segments: vec![],
        };
        assert_eq!(
            surface_fault(vec![], vec![empty]),
            ProfileError::EmptyChain { chain: ChainKey(7) }
        );
        // A chain sharing a loop's key.
        let sq = square(1, 10, (5.0, 0.0), 1.0);
        let same = chain(1, 20, &[(0.0, 0.0), (1.0, 0.0)]);
        assert_eq!(
            surface_fault(vec![sq], vec![same]),
            ProfileError::DuplicateKey
        );
        let mut broken = chain(2, 20, &[(0.0, 0.0), (1.0, 0.0), (1.0, 1.0)]);
        broken.segments[1] = line(21, (1.0, 1e-6), (1.0, 1.0));
        assert_eq!(
            surface_fault(vec![], vec![broken]),
            ProfileError::ChainBroken {
                chain: ChainKey(2),
                after: SegmentKey(20)
            }
        );
        let closing = chain(3, 20, &[(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 0.0)]);
        assert_eq!(
            surface_fault(vec![], vec![closing]),
            ProfileError::ChainCloses { chain: ChainKey(3) }
        );
        let whole = Chain {
            key: ChainKey(4),
            segments: vec![arc(20, circle((0.0, 0.0), 1.0), 0.0, TAU)],
        };
        assert_eq!(
            surface_fault(vec![], vec![whole]),
            ProfileError::ChainCloses { chain: ChainKey(4) }
        );
        // A chain whose last segment crosses its first, and one crossing a
        // loop.
        let hook = chain(
            5,
            20,
            &[(0.0, 0.0), (2.0, 0.0), (2.0, 1.0), (1.0, 1.0), (1.0, -1.0)],
        );
        assert_eq!(
            surface_fault(vec![], vec![hook]),
            ProfileError::Touching {
                a: SegmentKey(20),
                b: SegmentKey(23)
            }
        );
        let through = chain(6, 30, &[(4.0, 0.5), (5.5, 0.5)]);
        let sq = square(1, 10, (5.0, 0.0), 1.0);
        assert_eq!(
            surface_fault(vec![sq], vec![through]),
            ProfileError::Touching {
                a: SegmentKey(13),
                b: SegmentKey(30)
            }
        );
        // Two segments meeting at their shared end only, at a sharp angle.
        let vee = chain(7, 40, &[(0.0, 1.0), (0.0, 0.0), (0.1, 1.0)]);
        assert!(ValidProfile::surface(profile(vec![]), vec![vee]).is_ok());
    }
}
