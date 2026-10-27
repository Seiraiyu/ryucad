//! What extrude and a partial revolve share: a profile lump planned as a
//! capped sweep, built, and named.

use super::build::{
    self, Keys, LoopBuilt, LoopPlan, LumpBuilt, LumpPlan, Ring, SegmentPlan, Span, VertexPlan,
};
use super::names::Namer;
use super::place;
use super::{Lump, Segment, ValidLoop};
use crate::Error;
use crate::geom::{Curve, Frame, Plane, Point2, Point3, Rigid, Surface, Vector2, Vector3};
use crate::name::{Name, SweepRole};
use crate::tol::Tolerance;
use crate::topo::{Body, EntityId, Sense};

/// How a capped sweep moves its profile from the start cap to the end
/// cap.
pub(crate) trait Motion {
    /// The rigid motion that puts the profile on the start cap, or on the
    /// end cap.
    fn at(&self, end: bool) -> Rigid;
    /// `+1` when the profile moves from the start cap towards the end cap
    /// along its frame's normal, `-1` against it.
    fn sign(&self) -> f64;
    /// Whether a profile point stays put (on a revolve's axis).
    fn fixed(&self, p: Point2) -> bool;
    /// The lateral edge from a profile point on the start cap (or the end
    /// cap, `from_end`) to its place on the other.
    fn lateral(&self, frame: &Frame, p: Point2, from_end: bool)
    -> Result<(Curve, [f64; 2]), Error>;
    /// The surface a segment sweeps, in the profile's own position, or
    /// `None` for a segment on a revolve's axis.
    fn side(&self, frame: &Frame, s: &Segment) -> Result<Option<Surface>, Error>;
}

/// How a lump's loops were laid out for the builder.
pub(crate) struct Layout {
    /// The builder's near cap is the start cap.
    near_is_start: bool,
    /// Each loop's path runs the loop's own way round.
    forward: Vec<bool>,
}

impl Layout {
    /// The loop index of path segment `k` of a loop of `n` segments.
    fn segment(&self, l: usize, k: usize, n: usize) -> usize {
        if self.forward[l] { k } else { n - 1 - k }
    }

    /// The loop index of path vertex `k`.
    fn vertex(&self, l: usize, k: usize, n: usize) -> usize {
        if self.forward[l] { k } else { (n - k) % n }
    }
}

/// The loops of a lump: the outer loop, then the holes.
pub(super) fn loops(lump: &Lump) -> impl Iterator<Item = &ValidLoop> {
    core::iter::once(&lump.outer).chain(&lump.holes)
}

/// A loop of one closed segment.
pub(super) fn closed(lp: &ValidLoop) -> bool {
    lp.segments.len() == 1
}

/// Whether a closed loop's curve, by increasing parameter, runs
/// counterclockwise about the profile's normal. The loop itself runs
/// counterclockwise when it is an outer loop.
pub(super) fn curve_ccw(lp: &ValidLoop, outer: bool) -> bool {
    let s = &lp.segments[0];
    (s.t1 > s.t0) == outer
}

/// The profile plane's outward direction from a segment: its right, as
/// the loop runs (the region is on the left of every loop).
pub(super) fn outward(frame: &Frame, s: &Segment) -> (Point3, Vector3) {
    let mut t = 0.5;
    loop {
        let at = s.t0 + (s.t1 - s.t0) * t;
        let e = s.curve.eval(at);
        let d = if s.t1 > s.t0 { e.d1 } else { -e.d1 };
        if d.length() > 0.0 || t < 0.1 {
            let right = Vector2::new(d.y, -d.x);
            return (place::point(frame, e.point), place::vector(frame, right));
        }
        t *= 0.7;
    }
}

/// The sense a side takes on its surface: the one whose normal points
/// out of the solid where the profile crosses it.
pub(crate) fn sense_of(surface: &Surface, frame: &Frame, s: &Segment) -> Sense {
    let (p, out) = outward(frame, s);
    let (u, v) = surface.project(p).param;
    match surface.normal(u, v) {
        Some(n) if n.get().dot(out) < 0.0 => Sense::Reversed,
        _ => Sense::Forward,
    }
}

/// A cap's plane: the profile's plane moved by `at`, facing `outward`
/// times the profile's normal.
pub(super) fn cap(frame: &Frame, at: &Rigid, outward: f64) -> Result<Surface, Error> {
    let moved = frame.transformed(at);
    let z = moved.z().get() * outward;
    let f = Frame::new(moved.origin(), z, moved.x().get())?;
    Ok(Surface::Plane(Plane::new(f)))
}

fn span(b: &mut Body, (curve, interval): (Curve, [f64; 2])) -> Span {
    Span {
        curve: b.add_curve(curve),
        interval,
    }
}

/// Plans one lump for the builder.
pub(crate) fn plan(
    b: &mut Body,
    frame: &Frame,
    lump: &Lump,
    motion: &dyn Motion,
) -> Result<(LumpPlan, Layout), Error> {
    // Start from the start cap unless the outer loop is one closed curve
    // running the wrong way round for that.
    let ccw_from_start =
        |lp: &ValidLoop, outer: bool| curve_ccw(lp, outer) == (motion.sign() > 0.0);
    let near_is_start = !closed(&lump.outer) || ccw_from_start(&lump.outer, true);
    // The direction from the near cap to the far one, against the normal.
    let sign = if near_is_start {
        motion.sign()
    } else {
        -motion.sign()
    };
    let (near_at, far_at) = (motion.at(!near_is_start), motion.at(near_is_start));
    let mut forward = Vec::new();
    let mut plans = Vec::new();
    for (l, lp) in loops(lump).enumerate() {
        let outer = l == 0;
        // Paths run counterclockwise about the motion: an outer loop
        // (counterclockwise about the normal) its own way when the motion
        // is along the normal, a hole (clockwise) the other way.
        let fwd = outer == (sign > 0.0);
        forward.push(fwd);
        let n = lp.segments.len();
        let (mut near_spans, mut points) = (Vec::with_capacity(n), Vec::with_capacity(n));
        let mut segments = Vec::with_capacity(n);
        let mut vertices = Vec::with_capacity(n);
        for k in 0..n {
            let j = if fwd { k } else { n - 1 - k };
            let s = &lp.segments[j];
            let range = if closed(lp) {
                [s.t0.min(s.t1), s.t0.max(s.t1)]
            } else if fwd {
                [s.t0, s.t1]
            } else {
                [s.t1, s.t0]
            };
            let side = match motion.side(frame, s)? {
                Some(surface) => {
                    let sense = sense_of(&surface, frame, s);
                    Some((b.add_surface(surface), sense))
                }
                None => None,
            };
            let near = span(b, place::span(frame, &s.curve, range, &near_at)?);
            let far = if side.is_some() {
                span(b, place::span(frame, &s.curve, range, &far_at)?)
            } else {
                near
            };
            near_spans.push(near);
            segments.push(SegmentPlan { far, side });
            let i = if fwd { k } else { (n - k) % n };
            let p = lp.vertices[i];
            let at = place::point(frame, p);
            let lateral = if closed(lp) || motion.fixed(p) {
                None
            } else {
                Some(span(b, motion.lateral(frame, p, !near_is_start)?))
            };
            points.push(near_at.apply_point(at));
            vertices.push(VertexPlan {
                far: far_at.apply_point(at),
                lateral,
            });
        }
        let ccw = closed(lp) && curve_ccw(lp, outer) == (sign > 0.0);
        plans.push(LoopPlan {
            near: near_spans,
            points,
            rings: vec![Ring { segments, vertices }],
            closed: closed(lp),
            ccw,
        });
    }
    // The near cap faces against the motion, the far cap along it.
    let near_cap = b.add_surface(cap(frame, &near_at, -sign)?);
    let far_cap = b.add_surface(cap(frame, &far_at, sign)?);
    let mut plans = plans.into_iter();
    let outer = plans.next().ok_or(Error::Degenerate)?;
    let plan = LumpPlan {
        outer,
        holes: plans.collect(),
        near_cap: (near_cap, Sense::Forward),
        far_cap: (far_cap, Sense::Forward),
    };
    Ok((
        plan,
        Layout {
            near_is_start,
            forward,
        },
    ))
}

/// Builds one lump: plans it, builds it, sets the profile's vertex
/// tolerances, and names it.
pub(crate) fn sweep(
    b: &mut Body,
    keys: &mut Keys,
    namer: &Namer,
    frame: &Frame,
    lump: &Lump,
    motion: &dyn Motion,
) -> Result<(), Error> {
    let (plan, layout) = plan(b, frame, lump, motion)?;
    let built = build::capped(b, keys, &plan)?;
    tolerances(b, lump, &layout, &built)?;
    name(b, namer, lump, &layout, &built);
    Ok(())
}

/// Each profile vertex's tolerance on its body vertices and its lateral
/// edge, which may stray from the side before it by the profile's gap.
fn tolerances(b: &mut Body, lump: &Lump, layout: &Layout, built: &LumpBuilt) -> Result<(), Error> {
    let made = core::iter::once(&built.outer).chain(&built.holes);
    for (l, (lp, m)) in loops(lump).zip(made).enumerate() {
        let n = lp.segments.len();
        for k in 0..m.near_vertices.len() {
            let tol = Tolerance::new(lp.tolerances[layout.vertex(l, k, n)])?;
            let vertices = core::iter::once(m.near_vertices[k])
                .chain(m.rings.iter().map(|r| r.far_vertices[k]));
            for v in vertices {
                if b.vertices[v].tolerance.get() < tol.get() {
                    b.vertices[v].tolerance = tol;
                }
            }
            for r in &m.rings {
                if let Some(e) = r.laterals[k] {
                    b.edges[e].tolerance = tol;
                }
            }
        }
    }
    Ok(())
}

/// Names a built lump from the profile's keys. Every role's parents are
/// caller keys (or names made from them), never positions, so editing a
/// dimension renames nothing.
fn name(b: &mut Body, namer: &Namer, lump: &Lump, layout: &Layout, built: &LumpBuilt) {
    let outer = namer.loop_(b, lump.outer.key.0);
    let (start_role, end_role) = (SweepRole::StartCap, SweepRole::EndCap);
    let (near_role, far_role) = if layout.near_is_start {
        (start_role, end_role)
    } else {
        (end_role, start_role)
    };
    let near_cap = namer.name(b, EntityId::Face(built.near_cap), near_role, &[outer]);
    let far_cap = namer.name(b, EntityId::Face(built.far_cap), far_role, &[outer]);
    let (near_vertex, far_vertex) = if layout.near_is_start {
        (SweepRole::StartVertex, SweepRole::EndVertex)
    } else {
        (SweepRole::EndVertex, SweepRole::StartVertex)
    };
    let made = core::iter::once(&built.outer).chain(&built.holes);
    for (l, (lp, m)) in loops(lump).zip(made).enumerate() {
        let n = lp.segments.len();
        let segs: Vec<Name> = lp
            .segments
            .iter()
            .map(|s| namer.segment(b, s.key.0))
            .collect();
        name_loop(
            b,
            namer,
            layout,
            l,
            n,
            &segs,
            m,
            [near_cap, far_cap],
            [near_vertex, far_vertex],
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn name_loop(
    b: &mut Body,
    namer: &Namer,
    layout: &Layout,
    l: usize,
    n: usize,
    segs: &[Name],
    m: &LoopBuilt,
    caps: [Name; 2],
    vertex_roles: [SweepRole; 2],
) {
    let (ring, (far, far_vertices)) = (&m.rings[0], m.far());
    for k in 0..n {
        let seg = segs[layout.segment(l, k, n)];
        match ring.sides[k] {
            Some(face) => {
                let side = namer.name(b, EntityId::Face(face), SweepRole::Side, &[seg]);
                namer.name(
                    b,
                    EntityId::Edge(m.near[k]),
                    SweepRole::CapEdge,
                    &[caps[0], side],
                );
                namer.name(
                    b,
                    EntityId::Edge(far[k]),
                    SweepRole::CapEdge,
                    &[caps[1], side],
                );
            }
            None => {
                namer.name(b, EntityId::Edge(m.near[k]), SweepRole::AxisEdge, &[seg]);
            }
        }
    }
    for (k, (&near, &far)) in m.near_vertices.iter().zip(far_vertices).enumerate() {
        // The profile vertex between the loop's segments i - 1 and i.
        let i = layout.vertex(l, k, n);
        let pair = [segs[(i + n - 1) % n], segs[i]];
        if let Some(e) = ring.laterals[k] {
            namer.name(b, EntityId::Edge(e), SweepRole::Lateral, &pair);
        }
        if near == far {
            namer.name(b, EntityId::Vertex(near), SweepRole::Vertex, &pair);
        } else {
            namer.name(b, EntityId::Vertex(near), vertex_roles[0], &pair);
            namer.name(b, EntityId::Vertex(far), vertex_roles[1], &pair);
        }
    }
}
