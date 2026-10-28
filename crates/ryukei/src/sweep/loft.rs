//! Lofts: a skin through planar sections.
//!
//! Each loop of the first section is matched by its key in every other,
//! and its segments by their order from the loop's start, so the caller
//! connects sections by where it starts their loops. Matched segments are
//! made exact NURBS and compatible (`geom::nurbs::skin::compatible`), and
//! the skin through them (`skin::skin`) is one face per segment. Their
//! sides meet along the vertex columns' interpolants, which every face
//! along them shares exactly, since the columns' weights are 1 there. An
//! open loft is built as a capped sweep of one stretch, a closed one as a
//! full revolve's ring per loop.

use super::build::{self, Keys, LoopPlan, LumpPlan, Ring, SegmentPlan, Span, VertexPlan};
use super::build::{TurnPlan, TurnSegment, TurnVertex};
use super::capped;
use super::names::Namer;
use super::{Lump, ValidLoop, ValidProfile, place};
use crate::geom::nurbs::skin::{self, SkinFault, compatible};
use crate::geom::{
    Curve, Frame, KnotVector, NurbsCurve, NurbsSurface, Point3, Rigid, Surface, UnitVector3,
    Vector3,
};
use crate::name::{Journal, Name, OpKey, SweepRole};
use crate::tol::{ANGULAR_RESOLUTION, LINEAR_RESOLUTION, Tolerance};
use crate::topo::{Body, EntityId, FaceId, Sense};
use crate::{Error, LoftError, check, math};
use std::sync::Arc;

/// How a loft runs: round to its first section again when `closed`, and
/// at a given derivative from its first and into its last section.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LoftOptions {
    pub closed: bool,
    pub start: Option<Tangent>,
    pub end: Option<Tangent>,
}

/// A derivative at an end section: `weight × direction`, by the loft's
/// parameter across the sections, which runs from 0 at the first to 1 at
/// the last. The same for every point of the section.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tangent {
    pub direction: UnitVector3,
    pub weight: f64,
}

/// Lofts `sections` (two or more, three or more when closed): one solid
/// per lump of the first.
///
/// Every section's lumps and loops must match the first's by key, with
/// as many segments in each matched loop; its plane must differ from the
/// section before's (and round a closed loft, the first's from the
/// last's) and face that section's way. Refusals, in order: `TooFewSections`;
/// `BadTangent`; for each section in order `BadSection`, `LoopMismatch`,
/// `SegmentCountMismatch`; `BadWeights` naming the first section's segment
/// whose skin's weights fall to zero; then, built, `SelfIntersecting` or
/// `Uncertain` naming the first pair of faces in face order.
///
/// The skin's parameter across the sections is centripetal: each step
/// the square root of the mean distance the loops' vertices move, the
/// same for every face. Its degree is three, or less with fewer sections
/// and derivatives. Names come from `key` and the caller's keys: a side
/// from its loop and its segment in every section, in order; a lateral
/// and its vertices from the sides either side; caps, cap edges as an
/// extrude's. Resizing or moving a section renames nothing.
pub fn loft(
    key: OpKey,
    sections: &[ValidProfile],
    opts: &LoftOptions,
) -> Result<(Body, Journal), Error> {
    let m = sections.len();
    if m < 2 || (opts.closed && m < 3) {
        return Err(LoftError::TooFewSections.into());
    }
    check_tangents(sections, opts)?;
    for k in 1..m {
        check_section(sections, k)?;
    }
    if opts.closed {
        check_plane(&sections[m - 1], &sections[0], 0)?;
    }
    let matched = match_loops(sections)?;
    let sign = direction(sections)?;
    let v = parameters(sections, &matched, opts.closed);
    let mut b = Body::new();
    let mut keys = Keys::new(key);
    let namer = Namer::new("loft", key);
    let lofted = Lofted {
        sections,
        opts,
        v: &v,
        sign,
    };
    for (lump, matched) in sections[0].lumps().iter().zip(&matched) {
        lofted.lump(&mut b, &mut keys, &namer, lump, matched)?;
    }
    let journal = namer.finish(&mut b)?;
    self_check(&b)?;
    Ok((b, journal))
}

fn check_tangents(sections: &[ValidProfile], opts: &LoftOptions) -> Result<(), Error> {
    let ends = [
        (opts.start, &sections[0]),
        (opts.end, &sections[sections.len() - 1]),
    ];
    for (t, s) in ends {
        if let Some(t) = t {
            let along = t.direction.dot(s.frame().z()).abs();
            if opts.closed
                || !(t.weight.is_finite() && t.weight > 0.0)
                || along <= ANGULAR_RESOLUTION
            {
                return Err(LoftError::BadTangent.into());
            }
        }
    }
    Ok(())
}

/// Refuses section `k` (named `name`) in the plane of `before`.
fn check_plane(before: &ValidProfile, s: &ValidProfile, name: usize) -> Result<(), Error> {
    let (a, c) = (before.frame(), s.frame());
    let parallel = a.z().get().cross(c.z().get()).length() <= ANGULAR_RESOLUTION;
    let offset = a.to_local(c.origin()).z.abs();
    if parallel && offset <= LINEAR_RESOLUTION {
        return Err(LoftError::BadSection { section: name }.into());
    }
    Ok(())
}

fn check_section(sections: &[ValidProfile], k: usize) -> Result<(), Error> {
    check_plane(&sections[k - 1], &sections[k], k)?;
    if sections[k].frame().z().dot(sections[k - 1].frame().z()) <= 0.0 {
        return Err(LoftError::BadSection { section: k }.into());
    }
    Ok(())
}

/// For each lump of the first section, each loop's matching loop in
/// every section: `matched[lump][loop][section]`, the outer loop first.
type Matched<'a> = Vec<Vec<Vec<&'a ValidLoop>>>;

fn match_loops(sections: &[ValidProfile]) -> Result<Matched<'_>, Error> {
    let first = &sections[0];
    let mut out: Matched<'_> = Vec::new();
    for lump in first.lumps() {
        let loops: Vec<&ValidLoop> = capped::loops(lump).collect();
        out.push(loops.iter().map(|l| vec![*l]).collect());
    }
    for (k, s) in sections.iter().enumerate().skip(1) {
        let total: usize = s.lumps().iter().map(|l| 1 + l.holes.len()).sum();
        let want: usize = out.iter().map(Vec::len).sum();
        for (j, lump) in first.lumps().iter().enumerate() {
            let key = lump.outer.key;
            let found = s.lumps().iter().find(|l| l.outer.key == key);
            let Some(theirs) = found else {
                return Err(LoftError::LoopMismatch {
                    section: k,
                    loop_: key,
                }
                .into());
            };
            for (i, mine) in capped::loops(lump).enumerate() {
                let lp = if i == 0 {
                    Some(&theirs.outer)
                } else {
                    theirs.holes.iter().find(|h| h.key == mine.key)
                };
                let lp = lp.ok_or(LoftError::LoopMismatch {
                    section: k,
                    loop_: mine.key,
                })?;
                if lp.segments.len() != mine.segments.len() {
                    return Err(LoftError::SegmentCountMismatch {
                        section: k,
                        loop_: mine.key,
                    }
                    .into());
                }
                out[j][i].push(lp);
            }
        }
        if total != want {
            // A loop the first section doesn't have.
            let extra = s
                .lumps()
                .iter()
                .flat_map(capped::loops)
                .find(|l| !out.iter().flatten().any(|m| m[k].key == l.key))
                .map_or(first.lumps()[0].outer.key, |l| l.key);
            return Err(LoftError::LoopMismatch {
                section: k,
                loop_: extra,
            }
            .into());
        }
    }
    Ok(out)
}

/// The mean of a loop's vertices in model space.
fn centroid(frame: &Frame, lp: &ValidLoop) -> Point3 {
    let n = lp.vertices.len() as f64;
    let sum = lp.vertices.iter().fold(Vector3::ZERO, |a, &p| {
        a + place::point(frame, p).to_vector()
    });
    Point3::ORIGIN + sum / n
}

/// `1` when the loft runs along the first section's normal, `-1` against
/// it: the side of the first section's plane the second's outer loop
/// lies on.
fn direction(sections: &[ValidProfile]) -> Result<f64, Error> {
    let (a, c) = (&sections[0], &sections[1]);
    let p = centroid(c.frame(), &c.lumps()[0].outer);
    let h = a.frame().to_local(p).z;
    if h.abs() <= LINEAR_RESOLUTION {
        return Err(LoftError::BadSection { section: 1 }.into());
    }
    Ok(h.signum())
}

/// The skin's parameter at each section: steps the square root of the
/// mean distance the matched loops' vertices move, scaled to end at 1 (at
/// the last section, or round at the first again when closed).
fn parameters(sections: &[ValidProfile], matched: &Matched, closed: bool) -> Vec<f64> {
    let m = sections.len();
    let at = |k: usize, lp: &ValidLoop, i: usize| place::point(sections[k].frame(), lp.vertices[i]);
    let step = |a: usize, c: usize| {
        let (mut sum, mut count) = (0.0, 0usize);
        for lump in matched {
            for lp in lump {
                for i in 0..lp[0].vertices.len() {
                    sum += at(a, lp[a], i).distance(at(c, lp[c], i));
                    count += 1;
                }
            }
        }
        math::sqrt(sum / count as f64)
    };
    let mut v = vec![0.0];
    for k in 1..m {
        let next = v[k - 1] + step(k - 1, k);
        v.push(next);
    }
    let total = if closed {
        v[m - 1] + step(m - 1, 0)
    } else {
        v[m - 1]
    };
    let mut v: Vec<f64> = v.iter().map(|x| x / total).collect();
    if !closed {
        v[m - 1] = 1.0;
    }
    v
}

/// A segment of a section placed and as a NURBS: over its range the way
/// the loop runs (`fwd`) or back, a whole closed curve as its clamped
/// form over one period.
fn placed(frame: &Frame, s: &super::Segment, fwd: bool) -> Result<NurbsCurve, Error> {
    let range = if fwd { [s.t0, s.t1] } else { [s.t1, s.t0] };
    let (curve, interval) = place::span(frame, &s.curve, range, &Rigid::IDENTITY)?;
    let (n, reparam) = curve.to_nurbs(interval)?;
    let whole = curve.period() == Some(interval[1] - interval[0]);
    Ok(if whole {
        n.to_clamped()
    } else {
        match (&curve, reparam) {
            (Curve::Line(_), _) => n,
            (_, r) => n.part(interval.map(|t| r.to_nurbs(t)))?,
        }
    })
}

/// A surface clamped and closed along `u` (its first and last columns
/// one) in Bézier form along `u`, as the periodic surface of the same
/// pieces: the last column dropped, each clamped column `j` the periodic
/// one `degree - 1` later.
fn periodic_in_u(s: &NurbsSurface) -> Result<NurbsSurface, Error> {
    let [ku, kv] = s.knots();
    let p = ku.degree();
    let periodic = KnotVector::periodic(p, ku.knots(), &vec![p; ku.knots().len()])?;
    let [nu, nv] = s.counts();
    let n = nu - 1;
    let mut points = vec![Point3::ORIGIN; n * nv];
    let mut weights = vec![1.0; n * nv];
    for i in 0..n {
        let to = (i + p - 1) % n;
        for j in 0..nv {
            points[to * nv + j] = s.control_point(i, j);
            weights[to * nv + j] = s.weights()[i * nv + j];
        }
    }
    NurbsSurface::new([periodic, kv.clone()], points, Some(weights))
}

/// A clamped closed curve in Bézier form as the periodic one, as
/// [`periodic_in_u`] does for a surface.
fn periodic_curve(c: &NurbsCurve) -> Result<NurbsCurve, Error> {
    c.bezier_periodic().ok_or(Error::Degenerate)
}

/// The column of a surface's control net at `u` index `i`, as a curve in
/// `v`.
fn column(s: &NurbsSurface, i: usize) -> Result<NurbsCurve, Error> {
    let [_, kv] = s.knots();
    let nv = s.counts()[1];
    let pts = (0..nv).map(|j| s.control_point(i, j)).collect();
    let w = (0..nv).map(|j| s.weights()[i * nv + j]).collect();
    NurbsCurve::new(kv.clone(), pts, Some(w))
}

/// A curve over `[0, 1]` (or its domain) as a span of the body.
fn span(b: &mut Body, c: NurbsCurve) -> Span {
    let interval = c.domain();
    Span {
        curve: b.add_curve(Curve::Nurbs(Arc::new(c))),
        interval,
    }
}

/// Refuses a body whose faces meet away from their shared edges.
fn self_check(b: &Body) -> Result<(), Error> {
    let names = |a: FaceId, c: FaceId| {
        let (x, y) = (b.faces[a].name, b.faces[c].name);
        if x <= y { [x, y] } else { [y, x] }
    };
    match check::check_intersections(b).first() {
        None => Ok(()),
        Some(check::Fault::FacesCross { a, b: c } | check::Fault::FacesOverlap { a, b: c }) => {
            Err(LoftError::SelfIntersecting {
                faces: names(*a, *c),
            }
            .into())
        }
        Some(check::Fault::CannotCheck { a, b: c }) => Err(LoftError::Uncertain {
            faces: names(*a, *c),
        }
        .into()),
        Some(_) => Err(Error::Degenerate),
    }
}

/// What every lump is lofted with.
struct Lofted<'a> {
    sections: &'a [ValidProfile],
    opts: &'a LoftOptions,
    v: &'a [f64],
    sign: f64,
}

/// One matched loop's skins, in path order.
struct Skinned {
    /// The loop's index of each path segment and of each path vertex.
    segments: Vec<usize>,
    vertices: Vec<usize>,
    surfaces: Vec<NurbsSurface>,
    /// Each segment's compatible curve at the first and last section.
    ends: Vec<[NurbsCurve; 2]>,
}

impl Lofted<'_> {
    /// The skins of one matched loop: its segments in the order a capped
    /// sweep's paths run (`fwd`), each through every section.
    fn skins(&self, loops: &[&ValidLoop], fwd: bool) -> Result<Skinned, Error> {
        let n = loops[0].segments.len();
        let derivative = |t: Option<Tangent>| t.map(|t| t.direction.get() * t.weight * self.sign);
        let ends = (derivative(self.opts.start), derivative(self.opts.end));
        let mut out = Skinned {
            segments: Vec::with_capacity(n),
            vertices: Vec::with_capacity(n),
            surfaces: Vec::with_capacity(n),
            ends: Vec::with_capacity(n),
        };
        for k in 0..n {
            let j = if fwd { k } else { n - 1 - k };
            let curves = loops
                .iter()
                .zip(self.sections)
                .map(|(lp, s)| placed(s.frame(), &lp.segments[j], fwd))
                .collect::<Result<Vec<_>, _>>()?;
            let (knots, nets) = compatible(&curves)?;
            let surface = match skin::skin(&knots, &nets, self.v, self.opts.closed, ends)? {
                Ok(s) => s,
                Err(SkinFault::Weights { .. } | SkinFault::Singular) => {
                    return Err(LoftError::BadWeights {
                        loop_: loops[0].key,
                        segment: loops[0].segments[j].key,
                    }
                    .into());
                }
            };
            let whole = n == 1;
            let rebuilt = |h: &[crate::geom::Hom<Vector3>]| {
                NurbsCurve::new(
                    knots.clone(),
                    h.iter().map(|x| Point3::ORIGIN + x.p).collect(),
                    Some(h.iter().map(|x| x.w).collect()),
                )
            };
            let mut first = rebuilt(&nets[0])?;
            let mut last = rebuilt(&nets[nets.len() - 1])?;
            let surface = if whole {
                first = periodic_curve(&first)?;
                last = periodic_curve(&last)?;
                periodic_in_u(&surface)?
            } else {
                surface
            };
            out.segments.push(j);
            out.vertices.push(if fwd { k } else { (n - k) % n });
            out.surfaces.push(surface);
            out.ends.push([first, last]);
        }
        Ok(out)
    }

    /// The sense a side takes on its skin: the one whose normal points out
    /// of the solid where its segment's middle crosses the first section.
    fn sense(&self, surface: &Surface, lp: &ValidLoop, j: usize) -> Sense {
        let frame = self.sections[0].frame();
        let (p, out) = capped::outward(frame, &lp.segments[j]);
        let (u, v) = surface.project(p).param;
        match surface.normal(u, v) {
            Some(n) if n.get().dot(out) < 0.0 => Sense::Reversed,
            _ => Sense::Forward,
        }
    }

    /// Lofts, tolerances and names one lump.
    fn lump(
        &self,
        b: &mut Body,
        keys: &mut Keys,
        namer: &Namer,
        lump: &Lump,
        matched: &[Vec<&ValidLoop>],
    ) -> Result<(), Error> {
        if self.opts.closed {
            return self.closed(b, keys, namer, matched);
        }
        let mut plans = Vec::new();
        let mut made = Vec::new();
        for (l, loops) in matched.iter().enumerate() {
            let lp = loops[0];
            let outer = l == 0;
            let closed = capped::closed(lp);
            // Paths run counterclockwise about the loft's direction, as a
            // capped sweep's do. A loop of one closed curve runs the loop's
            // way, but an outer one runs counterclockwise about the loft:
            // the loop's way only when the loft runs along the normal.
            let along = self.sign > 0.0;
            let (fwd, ccw) = match (closed, outer) {
                (true, true) => (along, true),
                (true, false) => (true, !along),
                (false, _) => (outer == along, true),
            };
            let skinned = self.skins(loops, fwd)?;
            let mut near = Vec::new();
            let mut points = Vec::new();
            let mut segments = Vec::new();
            let mut vertices = Vec::new();
            for (k, surface) in skinned.surfaces.iter().enumerate() {
                let [first, last] = &skinned.ends[k];
                points.push(first.point(first.domain()[0]));
                near.push(span(b, first.clone()));
                let side = Surface::Nurbs(Arc::new(surface.clone()));
                let sense = self.sense(&side, lp, skinned.segments[k]);
                segments.push(SegmentPlan {
                    far: span(b, last.clone()),
                    side: Some((b.add_surface(side), sense)),
                });
                let lateral = if closed {
                    None
                } else {
                    Some(span(b, column(surface, 0)?))
                };
                vertices.push(VertexPlan {
                    far: last.point(last.domain()[0]),
                    lateral,
                });
            }
            plans.push(LoopPlan {
                near,
                points,
                rings: vec![Ring { segments, vertices }],
                closed,
                ccw,
            });
            made.push(skinned);
        }
        let first = self.sections[0].frame();
        let last = self.sections[self.sections.len() - 1].frame();
        let near_cap = b.add_surface(capped::cap(first, &Rigid::IDENTITY, -self.sign)?);
        let far_cap = b.add_surface(capped::cap(last, &Rigid::IDENTITY, self.sign)?);
        let mut plans = plans.into_iter();
        let outer = plans.next().ok_or(Error::Degenerate)?;
        let plan = LumpPlan {
            outer,
            holes: plans.collect(),
            near_cap: (near_cap, Sense::Forward),
            far_cap: (far_cap, Sense::Forward),
        };
        let built = build::capped(b, keys, &plan)?;
        // Tolerances: each vertex's gap in any section.
        let all = core::iter::once(&built.outer).chain(&built.holes);
        for ((loops, skinned), m) in matched.iter().zip(&made).zip(all) {
            for (k, &i) in skinned
                .vertices
                .iter()
                .enumerate()
                .take(m.near_vertices.len())
            {
                let gap = loops
                    .iter()
                    .map(|lp| lp.tolerances[i])
                    .fold(LINEAR_RESOLUTION, f64::max);
                let tol = Tolerance::new(gap)?;
                for v in [m.near_vertices[k], m.rings[0].far_vertices[k]] {
                    if b.vertices[v].tolerance.get() < tol.get() {
                        b.vertices[v].tolerance = tol;
                    }
                }
                if let Some(e) = m.rings[0].laterals[k] {
                    b.edges[e].tolerance = tol;
                }
            }
        }
        // Names.
        let outer_key = namer.loop_(b, lump.outer.key.0);
        let start = namer.name(
            b,
            EntityId::Face(built.near_cap),
            SweepRole::StartCap,
            &[outer_key],
        );
        let end = namer.name(
            b,
            EntityId::Face(built.far_cap),
            SweepRole::EndCap,
            &[outer_key],
        );
        let all = core::iter::once(&built.outer).chain(&built.holes);
        for ((loops, skinned), m) in matched.iter().zip(&made).zip(all) {
            let sides = self.side_names(b, namer, loops, skinned, &m.rings[0].sides);
            let n = sides.len();
            for (k, &side) in sides.iter().enumerate() {
                namer.name(
                    b,
                    EntityId::Edge(m.near[k]),
                    SweepRole::CapEdge,
                    &[start, side],
                );
                namer.name(
                    b,
                    EntityId::Edge(m.rings[0].far[k]),
                    SweepRole::CapEdge,
                    &[end, side],
                );
            }
            for k in 0..m.near_vertices.len() {
                // The vertex where the path's segment k starts, between the
                // sides of path segments k - 1 and k.
                let pair = [sides[(k + n - 1) % n], sides[k]];
                if let Some(e) = m.rings[0].laterals[k] {
                    namer.name(b, EntityId::Edge(e), SweepRole::Lateral, &pair);
                }
                namer.name(
                    b,
                    EntityId::Vertex(m.near_vertices[k]),
                    SweepRole::StartVertex,
                    &pair,
                );
                namer.name(
                    b,
                    EntityId::Vertex(m.rings[0].far_vertices[k]),
                    SweepRole::EndVertex,
                    &pair,
                );
            }
        }
        Ok(())
    }

    /// Names each side from its loop and its segment in every section, in
    /// path order.
    fn side_names(
        &self,
        b: &mut Body,
        namer: &Namer,
        loops: &[&ValidLoop],
        skinned: &Skinned,
        faces: &[Option<FaceId>],
    ) -> Vec<Name> {
        let lp = namer.loop_(b, loops[0].key.0);
        skinned
            .segments
            .iter()
            .zip(faces)
            .map(|(&j, face)| {
                let mut parents = vec![lp];
                for l in loops {
                    parents.push(namer.segment(b, l.segments[j].key.0));
                }
                let face = face.expect("every segment lofts a side");
                namer.name(b, EntityId::Face(face), SweepRole::Side, &parents)
            })
            .collect()
    }

    /// A closed loft of one lump: each loop a ring of bands between the
    /// closed curves its vertices' columns interpolate (a loop of one
    /// closed curve, one face with no edges on a spline closed both
    /// ways), the outer loop's a new solid region's outer shell and each
    /// hole's a cavity.
    fn closed(
        &self,
        b: &mut Body,
        keys: &mut Keys,
        namer: &Namer,
        matched: &[Vec<&ValidLoop>],
    ) -> Result<(), Error> {
        let mut region = None;
        for loops in matched {
            let lp = loops[0];
            // In the order a full revolve takes its ring: the loop's own
            // when the loft runs against the first section's normal.
            let forward = self.sign < 0.0;
            let skinned = self.skins(loops, forward)?;
            let mut plan = TurnPlan {
                segments: Vec::new(),
                vertices: Vec::new(),
                ends: None,
                region,
            };
            for (k, surface) in skinned.surfaces.iter().enumerate() {
                let side = Surface::Nurbs(Arc::new(surface.clone()));
                let sense = self.sense(&side, lp, skinned.segments[k]);
                plan.segments.push(TurnSegment {
                    side: (b.add_surface(side), sense),
                });
                // A loop of one closed curve skins a surface closed both
                // ways: one face with no edges.
                if capped::closed(lp) {
                    continue;
                }
                let c = column(surface, 0)?;
                let first = skinned.ends[k][0].point(0.0);
                plan.vertices.push(TurnVertex {
                    point: first,
                    circle: span(b, c),
                });
            }
            let made = build::turned(b, keys, &plan)?;
            region.get_or_insert(made.region);
            let faces: Vec<Option<FaceId>> = made.sides.iter().copied().map(Some).collect();
            let sides = self.side_names(b, namer, loops, &skinned, &faces);
            let n = sides.len();
            for (k, &i) in skinned
                .vertices
                .iter()
                .enumerate()
                .take(made.vertices.len())
            {
                let gap = loops
                    .iter()
                    .map(|l| l.tolerances[i])
                    .fold(LINEAR_RESOLUTION, f64::max);
                let tol = Tolerance::new(gap)?;
                b.vertices[made.vertices[k]].tolerance = tol;
                b.edges[made.circles[k]].tolerance = tol;
                let pair = [sides[(k + n - 1) % n], sides[k]];
                namer.name(
                    b,
                    EntityId::Edge(made.circles[k]),
                    SweepRole::Lateral,
                    &pair,
                );
                namer.name(
                    b,
                    EntityId::Vertex(made.vertices[k]),
                    SweepRole::Vertex,
                    &pair,
                );
            }
        }
        Ok(())
    }
}
