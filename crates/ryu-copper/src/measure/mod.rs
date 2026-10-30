//! Exact measurement of the true shapes: segments and centre-form arcs,
//! never their flattenings.
//!
//! A [`Body`] is what a shape's positive parts cover, as solids: regions
//! (outlines with their insides) and thick pieces (a segment or an arc
//! with a round pen's width). Every question compares a distance with a
//! threshold exactly ([`within`], [`closer`], [`touches`], [`contains`]);
//! [`distance`] finds the distance rounded to the nanometre from them.
//!
//! Square and symbol pens along an arc, and symbol pens with arcs in their
//! outline, sweep straight edges at irrational places, so they are
//! measured on chords within [`BodyOptions::bound`] and their answers
//! carry that bound: a question the bound can't settle is
//! [`Verdict::Uncertain`], never guessed.

pub(crate) mod body;
pub mod core;
mod distance;
mod estimate;
pub mod exact;
pub mod pair;
pub mod region;

use std::cmp::Ordering;

use ryu_board::geom::{Nm, Point, Rect};

use crate::tree::{Packed, grown};
pub use body::{BodyOptions, chords, shell as shell_of};
use core::{Core, SegP};
pub use distance::{Distance, distance};

/// The largest distance a question takes: no two points within the board's
/// ±2^40 nm are farther apart, so larger thresholds answer alike.
pub const MAX_REACH: Nm = 1 << 42;

/// A region: closed outlines (an outer one and its holes) and the inside
/// the nonzero rule gives them, with a packed tree of its pieces' boxes.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Shell {
    /// Every piece of every outline.
    pub cores: Vec<Core>,
    /// An integer point of the outlines, for containment.
    pub probe: Point,
    tree: Packed,
}

impl Shell {
    pub fn new(cores: Vec<Core>, probe: Point) -> Shell {
        let mut entries = Vec::new();
        let mut boxes = Vec::new();
        for (k, c) in cores.iter().enumerate() {
            boxes.clear();
            c.boxes(0, &mut boxes);
            entries.extend(boxes.iter().map(|r| (*r, k as u32)));
        }
        let tree = Packed::new(entries);
        Shell { cores, probe, tree }
    }

    /// The pieces whose boxes meet `q`, in order.
    pub fn near(&self, q: &Rect) -> impl Iterator<Item = &Core> {
        let mut found = Vec::new();
        self.tree.query(q, &mut found);
        found.sort_unstable();
        found.into_iter().map(|k| &self.cores[k as usize])
    }

    /// Whether an integer point lies inside or on the outlines.
    pub fn contains(&self, p: Point) -> bool {
        if !self.bounds().contains(p) {
            return false;
        }
        let at = Core::Seg(SegP::new(p, p));
        if self
            .near(&Rect::of_point(p))
            .any(|c| pair::core_cmp(c, &at, 0) == Ordering::Equal)
        {
            return true;
        }
        // Only pieces whose boxes meet the horizontal to the point's
        // right can wind round it.
        let ray = Rect {
            min: p,
            max: Point::new(MAX_REACH, p.y),
        };
        let crossing: Vec<Core> = self.near(&ray).copied().collect();
        region::winding(&crossing, p) != 0
    }

    fn bounds(&self) -> Rect {
        self.tree.bounds().unwrap_or(Rect::of_point(self.probe))
    }

    /// A lower bound on the distance from the region to a box, or more
    /// than `limit` when it lies farther. Zero where it holds a corner of
    /// the box (the box inside it), or the box its probe (it inside the
    /// box); else the least of its pieces' within `limit` of the box,
    /// found through its tree, and with none, more than `limit`.
    fn lower_to_box(&self, r: &Rect, limit: f64) -> f64 {
        let reach = if limit < MAX_REACH as f64 {
            limit.ceil() as Nm + 1
        } else {
            MAX_REACH
        };
        let mut near = Vec::new();
        self.tree.query(&grown(r, reach), &mut near);
        let d = near
            .iter()
            .map(|&k| pair::lower_to_box(&self.cores[k as usize], r))
            .fold(limit + 1.0, f64::min);
        if d > 0.0 && (r.contains(self.probe) || self.contains(r.min)) {
            return 0.0;
        }
        d
    }

    /// A lower bound on the distance from piece `k` to a box.
    fn lower_to(&self, k: u32, r: &Rect, _limit: f64) -> f64 {
        pair::lower_to_box(&self.cores[k as usize], r)
    }
}

/// What a body is made of.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Solid {
    Region(Shell),
    /// Every point within `w / 2` of the piece.
    Thick {
        core: Core,
        w: Nm,
    },
}

impl Solid {
    /// An integer point of the solid.
    pub fn probe(&self) -> Point {
        match self {
            Solid::Region(s) => s.probe,
            Solid::Thick { core, .. } => core.probe(),
        }
    }

    fn bounds(&self) -> Rect {
        match self {
            Solid::Region(s) => s.bounds(),
            Solid::Thick { core, w } => {
                let r = core.bounds();
                let h = (w + 1) / 2;
                Rect {
                    min: r.min.offset(-h, -h),
                    max: r.max.offset(h, h),
                }
            }
        }
    }
}

/// What a shape covers, ready to measure.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Body {
    pub solids: Vec<Solid>,
    /// Each solid's box.
    pub boxes: Vec<Rect>,
    /// How far the solids may lie from the true shape: 0 when exact.
    pub bound: Nm,
    /// Negative parts left out: measurement through them is the resolved
    /// copper's (plan 1.C).
    pub negatives: usize,
    tree: Packed,
}

impl Body {
    pub fn new(solids: Vec<Solid>, bound: Nm) -> Body {
        let boxes: Vec<Rect> = solids.iter().map(Solid::bounds).collect();
        let mut b = Body {
            solids,
            boxes,
            bound,
            negatives: 0,
            tree: Packed::default(),
        };
        b.tree = Packed::new(b.entries());
        b
    }

    /// Boxes that between them hold every solid, with the solid each
    /// holds: a region's box, a thick piece's boxes (see [`Core::boxes`])
    /// grown by half its width. The body's tree, and the index's, hold
    /// these.
    pub fn entries(&self) -> Vec<(Rect, u32)> {
        let mut out = Vec::new();
        let mut boxes = Vec::new();
        for (k, (s, r)) in self.solids.iter().zip(&self.boxes).enumerate() {
            match s {
                Solid::Region(_) => out.push((*r, k as u32)),
                Solid::Thick { core, w } => {
                    boxes.clear();
                    core.boxes((w + 1) / 2, &mut boxes);
                    out.extend(boxes.iter().map(|b| (*b, k as u32)));
                }
            }
        }
        out
    }

    /// A lower bound on the distance from solid `k` to a box (or more than
    /// `limit` when it lies farther): a thick piece's less half its width,
    /// a region's its own (see [`Shell::lower_to_box`]).
    fn lower_to(&self, k: u32, r: &Rect, limit: f64) -> f64 {
        match &self.solids[k as usize] {
            Solid::Thick { core, w } => pair::lower_to_box(core, r) - *w as f64 / 2.0,
            Solid::Region(s) => s.lower_to_box(r, limit),
        }
    }

    /// The solids whose boxes meet `q`, in order.
    pub fn near(&self, q: &Rect) -> Vec<usize> {
        let mut found = Vec::new();
        self.tree.query(q, &mut found);
        found.sort_unstable();
        found.into_iter().map(|k| k as usize).collect()
    }

    /// A point.
    pub fn point(p: Point) -> Body {
        Body::stroke(p, p, 0)
    }

    /// The segment from `a` to `b` drawn with a round pen of width `w`
    /// (a drilled hole, a slot, a disc).
    pub fn stroke(a: Point, b: Point, w: Nm) -> Body {
        Body::new(
            vec![Solid::Thick {
                core: Core::Seg(SegP::new(a, b)),
                w,
            }],
            0,
        )
    }

    /// The box holding every solid, or `None` for an empty body.
    pub fn bounds(&self) -> Option<Rect> {
        self.boxes.iter().copied().reduce(Rect::union)
    }

    pub fn is_empty(&self) -> bool {
        self.solids.is_empty()
    }
}

/// An answer the bounds may leave open.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Verdict {
    Yes,
    No,
    /// Within the bodies' bounds of the threshold, so either.
    Uncertain,
}

impl Verdict {
    pub fn is_yes(self) -> bool {
        self == Verdict::Yes
    }
}

fn zero(dd: i128) -> Ordering {
    if dd == 0 {
        Ordering::Equal
    } else {
        Ordering::Less
    }
}

/// A thick distance, `max(0, core distance - w / 2)`, against `dd / 2`,
/// from the core distance against any doubled threshold.
fn thick(dd: i128, w: Nm, core: impl Fn(i128) -> Ordering) -> Ordering {
    if dd == 0 {
        // Touching: the core within w / 2.
        match core(i128::from(w)) {
            Ordering::Greater => Ordering::Greater,
            _ => Ordering::Equal,
        }
    } else {
        core(dd + i128::from(w))
    }
}

/// The least distance from a shell's outlines to a piece against
/// `dd / 2`: only pieces whose boxes come that close are compared.
fn least(s: &Shell, other: &Core, dd: i128) -> Ordering {
    let one = Packed::new(vec![(other.bounds(), 0)]);
    let mut best = Ordering::Greater;
    s.tree.nearest_pairs(
        &one,
        reach_of(dd),
        |k, r, l| s.lower_to(k, r, l),
        |_, r, _| pair::lower_to_box(other, r),
        |k, _| {
            best = best.min(pair::core_cmp(&s.cores[k as usize], other, dd));
            (!settled(best, dd)).then_some(reach_of(dd))
        },
    );
    best
}

/// How far apart boxes may lie and still hold pieces within `dd / 2`, with
/// a nanometre to spare.
fn reach_of(dd: i128) -> f64 {
    dd as f64 / 2.0 + 1.0
}

/// Whether a walk may stop: nothing can be nearer than touching, so at a
/// zero threshold touching settles it, else anything nearer does.
fn settled(best: Ordering, dd: i128) -> bool {
    best == zero(dd)
}

/// Two solids' distance against `dd / 2`. Exact.
fn solid_cmp(a: &Solid, b: &Solid, dd: i128) -> Ordering {
    match (a, b) {
        (Solid::Thick { core: c, w }, Solid::Thick { core: d, w: v }) => {
            thick(dd, w + v, |dd| pair::core_cmp(c, d, dd))
        }
        (Solid::Region(s), Solid::Thick { core, w })
        | (Solid::Thick { core, w }, Solid::Region(s)) => {
            // Containment first (a box test, then a count along a ray),
            // then the outlines.
            if s.contains(core.probe()) {
                return zero(dd);
            }
            thick(dd, *w, |dd| least(s, core, dd))
        }
        (Solid::Region(s), Solid::Region(t)) => {
            if s.contains(t.probe) || t.contains(s.probe) {
                return zero(dd);
            }
            regions(s, t, dd)
        }
    }
}

/// Two shells' outlines against `dd / 2`.
fn regions(s: &Shell, t: &Shell, dd: i128) -> Ordering {
    // Pairs of pieces whose boxes come that close, nearest first.
    let mut best = Ordering::Greater;
    s.tree.nearest_pairs(
        &t.tree,
        reach_of(dd),
        |i, r, l| s.lower_to(i, r, l),
        |j, r, l| t.lower_to(j, r, l),
        |i, j| {
            best = best.min(pair::core_cmp(
                &s.cores[i as usize],
                &t.cores[j as usize],
                dd,
            ));
            (!settled(best, dd)).then_some(reach_of(dd))
        },
    );
    best
}

/// The least distance between two bodies' solids against `dd / 2`,
/// `dd >= 0`. Exact on the solids; empty bodies are infinitely far apart.
pub fn compare(a: &Body, b: &Body, dd: i128) -> Ordering {
    // Pairs of solids whose boxes come that close, nearest first.
    let mut best = Ordering::Greater;
    a.tree.nearest_pairs(
        &b.tree,
        reach_of(dd),
        |i, r, l| a.lower_to(i, r, l),
        |j, r, l| b.lower_to(j, r, l),
        |i, j| {
            let (i, j) = (i as usize, j as usize);
            if !pair::boxes_apart(&a.boxes[i], &b.boxes[j], dd) {
                best = best.min(solid_cmp(&a.solids[i], &b.solids[j], dd));
            }
            (!settled(best, dd)).then_some(reach_of(dd))
        },
    );
    best
}

fn reach(d: Nm) -> i128 {
    i128::from(d.min(MAX_REACH))
}

/// Whether the bodies are within `d` of each other: distance at most `d`.
pub fn within(a: &Body, b: &Body, d: Nm) -> Verdict {
    if d < 0 {
        return Verdict::No;
    }
    let bound = a.bound + b.bound;
    if bound == 0 {
        return if compare(a, b, 2 * reach(d)) == Ordering::Greater {
            Verdict::No
        } else {
            Verdict::Yes
        };
    }
    if d >= bound && compare(a, b, 2 * reach(d - bound)) != Ordering::Greater {
        Verdict::Yes
    } else if compare(a, b, 2 * reach(d + bound)) == Ordering::Greater {
        Verdict::No
    } else {
        Verdict::Uncertain
    }
}

/// Whether the bodies are closer than `d`: distance under `d`.
pub fn closer(a: &Body, b: &Body, d: Nm) -> Verdict {
    if d <= 0 {
        return Verdict::No;
    }
    let bound = a.bound + b.bound;
    if bound == 0 {
        return if compare(a, b, 2 * reach(d)) == Ordering::Less {
            Verdict::Yes
        } else {
            Verdict::No
        };
    }
    if d > bound && compare(a, b, 2 * reach(d - bound)) == Ordering::Less {
        Verdict::Yes
    } else if compare(a, b, 2 * reach(d + bound)) != Ordering::Less {
        Verdict::No
    } else {
        Verdict::Uncertain
    }
}

/// Whether the bodies touch: overlap, containment, or contact at an edge
/// or a point.
pub fn touches(a: &Body, b: &Body) -> Verdict {
    within(a, b, 0)
}

/// Whether a body covers a point, its outline included.
pub fn contains(a: &Body, p: Point) -> Verdict {
    touches(a, &Body::point(p))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::measure::core::edge_cores;
    use ryu_board::geom::{Contour, Edge};

    fn p(x: i64, y: i64) -> Point {
        Point::new(x, y)
    }

    /// The square [x0, x1] × [y0, y1] as a region body.
    fn square(x0: i64, y0: i64, x1: i64, y1: i64) -> Body {
        let q = [p(x0, y0), p(x1, y0), p(x1, y1), p(x0, y1)];
        let c = Contour::new((0..4).map(|k| Edge::line(q[k], q[(k + 1) % 4])).collect()).unwrap();
        let mut cores = Vec::new();
        for e in c.edges() {
            edge_cores(e, &mut cores);
        }
        Body::new(vec![Solid::Region(Shell::new(cores, q[0]))], 0)
    }

    #[test]
    fn strokes_and_points() {
        // A trace 200 wide and a point 150 above its middle: 50 apart.
        let t = Body::stroke(p(0, 0), p(1000, 0), 200);
        let x = Body::point(p(500, 150));
        assert_eq!(compare(&t, &x, 100), Ordering::Equal);
        assert_eq!(within(&t, &x, 50), Verdict::Yes);
        assert_eq!(within(&t, &x, 49), Verdict::No);
        assert_eq!(closer(&t, &x, 50), Verdict::No);
        assert_eq!(closer(&t, &x, 51), Verdict::Yes);
        assert_eq!(touches(&t, &x), Verdict::No);
        assert_eq!(contains(&t, p(500, 100)), Verdict::Yes);
        assert_eq!(contains(&t, p(500, 101)), Verdict::No);
        assert_eq!(within(&t, &x, -1), Verdict::No);
        // Thresholds past the reach answer as the reach does.
        assert_eq!(within(&t, &x, Nm::MAX), Verdict::Yes);
    }

    #[test]
    fn regions_touch_contain_and_keep_apart() {
        let a = square(0, 0, 100, 100);
        assert_eq!(compare(&a, &square(130, 40, 200, 60), 60), Ordering::Equal);
        assert_eq!(touches(&a, &square(100, 100, 200, 200)), Verdict::Yes);
        // One inside the other: no outline meets, yet they overlap.
        let inner = square(40, 40, 60, 60);
        assert_eq!(touches(&a, &inner), Verdict::Yes);
        assert_eq!(touches(&inner, &a), Verdict::Yes);
        assert_eq!(closer(&a, &inner, 1), Verdict::Yes);
        assert_eq!(contains(&a, p(50, 50)), Verdict::Yes);
        assert_eq!(contains(&a, p(100, 50)), Verdict::Yes);
        assert_eq!(contains(&a, p(101, 50)), Verdict::No);
        // A trace inside the square, its pen reaching no outline.
        assert_eq!(
            touches(&a, &Body::stroke(p(30, 50), p(70, 50), 10)),
            Verdict::Yes
        );
    }

    #[test]
    fn bounds_leave_questions_open() {
        let x = Body::point(p(0, 0));
        let mut y = Body::point(p(100, 0));
        y.bound = 5;
        assert_eq!(within(&x, &y, 94), Verdict::No);
        assert_eq!(within(&x, &y, 95), Verdict::Uncertain);
        assert_eq!(within(&x, &y, 105), Verdict::Yes);
        assert_eq!(closer(&x, &y, 105), Verdict::Uncertain);
        assert_eq!(closer(&x, &y, 106), Verdict::Yes);
        assert_eq!(closer(&x, &y, 95), Verdict::No);
        assert!(Body::new(vec![], 0).bounds().is_none());
        assert_eq!(
            compare(&Body::new(vec![], 0), &x, 1 << 40),
            Ordering::Greater
        );
    }
}
