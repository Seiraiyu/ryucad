//! The flat pattern: the flat's outer side as exact plane curves, and its
//! bend lines.
//!
//! The side drawn is the fixed flange's side away from its first bend's
//! centre ([`super::BendDirection`]), and everything coplanar with it that
//! unfolding laid flat: the sides of the flanges reached from the fixed
//! one through unfolded bends, and the unfolded bends' strips. Edges those
//! faces share (bend lines) are inside; the rest bound the pattern, taken
//! loop by loop as the faces' loops run (counterclockwise seen from the
//! viewer round the outline, clockwise round holes), each segment its
//! fin's pcurve carried rigidly into the pattern's frame.

use super::curve2;
use super::recognize::viewer;
use super::shape;
use super::unfold::{Plan, inner_normal};
use super::{Allowance, BendDirection, SheetModel};
use crate::SheetMetalError;
use crate::geom::{Box2, Curve2, Frame, Point2};
use crate::name::Name;
use crate::topo::{Body, EntityId, FaceId, FinId, Sense, VertexId};
use std::collections::{BTreeMap, BTreeSet};

/// A sheet's flat outline, its holes and its bend lines, in the plane of
/// `frame` (its `z` towards the viewer), coordinates its `x` and `y`.
#[derive(Clone, Debug, PartialEq)]
pub struct FlatPattern {
    pub frame: Frame,
    /// Outlines first, then holes, each group by its least edge name.
    pub loops: Vec<PatternLoop>,
    /// In the order unfolding met the bends.
    pub bend_lines: Vec<BendLine>,
    pub thickness: f64,
    pub bounds: Box2,
}

/// A closed loop of the pattern: an outline runs counterclockwise, a hole
/// clockwise.
#[derive(Clone, Debug, PartialEq)]
pub struct PatternLoop {
    pub outer: bool,
    pub segments: Vec<PatternSegment>,
}

impl PatternLoop {
    /// The signed area the loop encloses: positive round an outline,
    /// negative round a hole.
    pub fn area(&self) -> f64 {
        0.5 * area(&self.segments)
    }
}

/// A trimmed curve of the pattern from `curve(t0)` to `curve(t1)`
/// (backwards along the curve when `t1 < t0`), naming the edge it draws.
#[derive(Clone, Debug, PartialEq)]
pub struct PatternSegment {
    pub edge: Name,
    pub curve: Curve2,
    pub t0: f64,
    pub t1: f64,
}

/// An unfolded bend's centre line across the pattern.
#[derive(Clone, Debug, PartialEq)]
pub struct BendLine {
    pub bend: Name,
    pub start: Point2,
    pub end: Point2,
    /// Its bend-line edges on the drawn side, sorted.
    pub edges: Vec<Name>,
    pub direction: BendDirection,
    pub angle: f64,
    pub radius: f64,
    /// The developed length.
    pub length: f64,
    pub allowance: Allowance,
}

/// The pattern frame's coordinates of a point.
fn local(frame: &Frame, p: crate::geom::Point3) -> Point2 {
    let l = frame.to_local(p);
    Point2::new(l.x, l.y)
}

/// Twice the signed area a loop encloses, by Gauss–Legendre's eight
/// points on each segment.
fn area(segments: &[PatternSegment]) -> f64 {
    const NODES: [(f64, f64); 4] = [
        (0.183_434_642_495_649_8, 0.362_683_783_378_362),
        (0.525_532_409_916_329, 0.313_706_645_877_887_3),
        (0.796_666_477_413_626_7, 0.222_381_034_453_374_5),
        (0.960_289_856_497_536_3, 0.101_228_536_290_376_26),
    ];
    let mut sum = 0.0;
    for s in segments {
        let half = 0.5 * (s.t1 - s.t0);
        let mid = 0.5 * (s.t0 + s.t1);
        for (x, w) in NODES {
            for t in [mid - half * x, mid + half * x] {
                let e = s.curve.eval(t);
                sum += w * half * (e.point.x * e.d1.y - e.point.y * e.d1.x);
            }
        }
    }
    sum
}

pub(super) fn pattern(
    b: &Body,
    model: &SheetModel,
    p: &Plan,
) -> Result<FlatPattern, SheetMetalError> {
    let fixed = p.fixed;
    let fallback = SheetMetalError::Uncertain {
        at: model.flanges[fixed].name,
    };
    let view = viewer(model, fixed);
    let base = model.flanges[fixed]
        .sides
        .iter()
        .find(|s| s.plane.frame().z().dot(view) > 0.0)
        .ok_or(fallback.clone())?;
    let origin = base.plane.frame().origin();
    let n = view.get();
    let hint = if n.x.abs() < 0.9 {
        crate::geom::Vector3::new(1.0, 0.0, 0.0)
    } else {
        crate::geom::Vector3::new(0.0, 1.0, 0.0)
    };
    let frame = Frame::new(origin, n, hint).map_err(|_| fallback.clone())?;
    // The flanges laid flat with the fixed one.
    let mut flat_flanges = BTreeSet::from([fixed]);
    for &k in &p.walk.order {
        let o = &p.bends[k];
        if o.length.is_some() && flat_flanges.contains(&o.near) {
            flat_flanges.insert(o.far);
        }
    }
    let mut candidates: Vec<Name> = Vec::new();
    for &i in &flat_flanges {
        for s in &model.flanges[i].sides {
            candidates.extend(&s.faces);
        }
    }
    for (k, bend) in model.bends.iter().enumerate() {
        let o = &p.bends[k];
        if o.length.is_some() && flat_flanges.contains(&o.near) {
            candidates.extend(bend.inner.iter().chain(&bend.outer));
        }
    }
    let tol = model.tolerance;
    let mut drawn: BTreeSet<FaceId> = BTreeSet::new();
    for n in candidates {
        let Some(EntityId::Face(f)) = b.by_name(n) else {
            continue;
        };
        let Some(fl) = shape::flat(b, f) else {
            continue;
        };
        let eps = tol + shape::face_tolerance(b, f);
        let facing = fl.normal.get().dot(frame.z().get()) > 0.0;
        if facing && frame.to_local(fl.origin).z.abs() <= eps {
            drawn.insert(f);
        }
    }
    // Boundary fins, each from its start vertex.
    let mut from: BTreeMap<VertexId, Vec<FinId>> = BTreeMap::new();
    for &f in &drawn {
        for fin in shape::fins_of(b, f) {
            let e = b.fins[fin].edge;
            let inside = shape::across(b, e, f).is_some_and(|g| drawn.contains(&g));
            if !inside {
                from.entry(b.start_of(fin)).or_default().push(fin);
            }
        }
    }
    let mut used: BTreeSet<FinId> = BTreeSet::new();
    let mut all: Vec<FinId> = from.values().flatten().copied().collect();
    all.sort_by_key(|&fin| b.edges[b.fins[fin].edge].name);
    let mut loops = Vec::new();
    for start in all {
        if used.contains(&start) {
            continue;
        }
        let mut segments = Vec::new();
        let mut fin = start;
        loop {
            used.insert(fin);
            segments.push(segment(b, &frame, fin).ok_or(fallback.clone())?);
            let next = from
                .get(&b.end_of(fin))
                .and_then(|fs| fs.iter().copied().find(|f| !used.contains(f)));
            match next {
                Some(f) => fin = f,
                None => break,
            }
        }
        if b.end_of(fin) != b.start_of(start) {
            return Err(fallback);
        }
        let outer = area(&segments) > 0.0;
        loops.push(PatternLoop { outer, segments });
    }
    loops.sort_by_key(|l| {
        (
            !l.outer,
            l.segments.iter().map(|s| s.edge).min().expect("a segment"),
        )
    });
    let mut bounds: Option<Box2> = None;
    for l in &loops {
        for s in &l.segments {
            let bx = s.curve.bounds([s.t0.min(s.t1), s.t0.max(s.t1)]);
            bounds = Some(bounds.map_or(bx, |x| x.union(&bx)));
        }
    }
    let bend_lines = p
        .walk
        .order
        .iter()
        .filter_map(|&k| {
            let o = &p.bends[k];
            let length = o.length?;
            if !flat_flanges.contains(&o.near) {
                return None;
            }
            let bend = &model.bends[k];
            let strip = super::unfold::strip_frame(p, k);
            let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
            for n in bend.inner.iter().chain(&bend.outer) {
                let Some(EntityId::Face(f)) = b.by_name(*n) else {
                    continue;
                };
                for e in shape::edges_of(b, f) {
                    for q in shape::ends(b, e) {
                        let y = strip.to_local(q).y;
                        lo = lo.min(y);
                        hi = hi.max(y);
                    }
                }
            }
            let at = |y: f64| local(&frame, strip.to_world(0.5 * length, y, 0.0));
            let mut edges: Vec<Name> = bend
                .lines
                .iter()
                .flatten()
                .copied()
                .filter(|&n| match b.by_name(n) {
                    Some(EntityId::Edge(e)) => {
                        shape::faces_of(b, e).iter().all(|f| drawn.contains(f))
                    }
                    _ => false,
                })
                .collect();
            edges.sort_unstable();
            let up = inner_normal(model, p, k).dot(frame.z().get()) > 0.0;
            Some(BendLine {
                bend: o.name,
                start: at(lo),
                end: at(hi),
                edges,
                direction: if up {
                    BendDirection::Up
                } else {
                    BendDirection::Down
                },
                angle: o.angle,
                radius: o.radius,
                length,
                allowance: o.allowance,
            })
        })
        .collect();
    Ok(FlatPattern {
        frame,
        loops,
        bend_lines,
        thickness: model.thickness,
        bounds: bounds.unwrap_or_else(|| Box2::from_point(Point2::ORIGIN)),
    })
}

/// A boundary fin as a pattern segment: its pcurve on its face, carried
/// into the pattern's frame, run as the fin runs.
fn segment(b: &Body, frame: &Frame, fin: FinId) -> Option<PatternSegment> {
    let f = shape::face_of(b, fin);
    let crate::geom::Surface::Plane(plane) = &b.surfaces[b.faces[f].surface] else {
        return None;
    };
    let fi = &b.fins[fin];
    let pc = fi.pcurve?;
    let affine = curve2::between(plane.frame(), frame);
    let (curve, [r0, r1]) = curve2::mapped(&b.curves2[pc.curve], &affine).ok()?;
    let [t0, t1] = b.edges[fi.edge].interval.map(|t| r0 + r1 * pc.param(t));
    let (t0, t1) = match fi.sense {
        Sense::Forward => (t0, t1),
        Sense::Reversed => (t1, t0),
    };
    Some(PatternSegment {
        edge: b.edges[fi.edge].name,
        curve,
        t0,
        t1,
    })
}
