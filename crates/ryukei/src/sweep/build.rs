//! Swept bodies built through Euler operators, from geometry already
//! chosen by `extrude` and `revolve`.
//!
//! A capped sweep (an extrude, or a revolve by less than a full turn)
//! starts from a lamina on its near cap: a face whose path of edges, closed
//! by MEF, splits it into the near cap and a twin face that is then swept.
//! Each vertex of the twin grows a lateral edge (MEV) towards the far cap,
//! and an MEF across the top of every segment cuts off its side face,
//! leaving the twin as the far cap. Holes are made the same way inside the
//! near cap and moved onto the far cap by KFMRH. A full revolve has no
//! caps: its sides are chained by closed circular edges, and a ring of
//! them closes into a handle by KFMRH.
//!
//! "Near" and "far" are the caps the construction starts and ends on; the
//! caller decides which of the start and end caps each one is. `u` is the
//! direction the profile moves from the near cap to the far one, and paths
//! run counterclockwise about it (see [`LoopPlan`]).

use crate::Error;
use crate::euler::{Corner, kfmrh, kvr, mef, mev, mvfs, mvr, set_surface};
use crate::geom::Point3;
use crate::name::{Name, OpKey, Role, SweepRole};
use crate::topo::{Body, CurveId, EdgeId, FaceId, FinId, Sense, SurfaceId, VertexId};

/// An edge's curve and interval, the parameter increasing the way the
/// edge runs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Span {
    pub curve: CurveId,
    pub interval: [f64; 2],
}

/// A surface and the sense a face takes on it.
pub(crate) type Sided = (SurfaceId, Sense);

/// One segment of a capped loop, in path order.
#[derive(Clone, Debug)]
pub(crate) struct SegmentPlan {
    /// The segment's edge on the near cap and on the far cap, both running
    /// along the path. A segment on a revolve's axis has one edge, shared
    /// by the caps, and no side.
    pub near: Span,
    pub far: Span,
    pub side: Option<Sided>,
}

/// One profile vertex of a capped loop, in path order.
#[derive(Clone, Debug)]
pub(crate) struct VertexPlan {
    pub near: Point3,
    pub far: Point3,
    /// The lateral edge from the near point to the far one; `None` for a
    /// vertex on a revolve's axis, where the two points are one, and for
    /// the vertex of a closed segment, whose side has no seam.
    pub lateral: Option<Span>,
}

/// A loop of a capped sweep. Segment `i` runs from vertex `i` to vertex
/// `i + 1` (cyclically).
///
/// The path of an outer loop or a hole of several segments runs
/// counterclockwise about `u`. A loop of one closed segment instead says
/// which way its curve runs: `ccw` when counterclockwise about `u`.
#[derive(Clone, Debug)]
pub(crate) struct LoopPlan {
    pub segments: Vec<SegmentPlan>,
    pub vertices: Vec<VertexPlan>,
    pub closed: bool,
    pub ccw: bool,
}

/// One lump of a capped sweep. An outer loop of one closed segment must
/// run counterclockwise about `u`; the caller picks the near cap so that
/// it does.
#[derive(Clone, Debug)]
pub(crate) struct LumpPlan {
    pub outer: LoopPlan,
    pub holes: Vec<LoopPlan>,
    pub near_cap: Sided,
    pub far_cap: Sided,
}

/// What a capped loop became, by segment and by vertex in path order.
#[derive(Clone, Debug, Default)]
pub(crate) struct LoopBuilt {
    pub sides: Vec<Option<FaceId>>,
    pub near: Vec<EdgeId>,
    pub far: Vec<EdgeId>,
    pub near_vertices: Vec<VertexId>,
    pub far_vertices: Vec<VertexId>,
    pub laterals: Vec<Option<EdgeId>>,
}

/// What a capped lump became.
#[derive(Clone, Debug)]
pub(crate) struct LumpBuilt {
    pub near_cap: FaceId,
    pub far_cap: FaceId,
    pub outer: LoopBuilt,
    pub holes: Vec<LoopBuilt>,
}

/// Fresh keys for the Euler operators of one sweep. The operators' names
/// are only scaffolding, replaced when the sweep names its entities, but
/// they must stay unique on the way: two MVRs in one face, or the MVFS of
/// two lumps, would otherwise derive the same name.
pub(crate) struct Keys {
    base: OpKey,
    count: u128,
}

impl Keys {
    pub(crate) fn new(base: OpKey) -> Self {
        Self { base, count: 0 }
    }

    fn next(&mut self) -> OpKey {
        self.count += 1;
        let scaffold = Name::from_u128(self.count);
        let role = Role::Sweep(SweepRole::Vertex);
        OpKey(Name::derive(self.base, role, &[scaffold]).to_u128())
    }
}

/// Builds one capped lump into `b`, as a new solid region.
pub(crate) fn capped(b: &mut Body, keys: &mut Keys, plan: &LumpPlan) -> Result<LumpBuilt, Error> {
    let outer = &plan.outer;
    let start = outer.vertices[0].near;
    let m = mvfs(b, keys.next(), start, plan.near_cap.0)?.0;
    let mut built = LoopBuilt::default();
    // The lamina: the near cap, and its twin to sweep.
    let (near_cap, twin, twin_fins) = if outer.closed {
        debug_assert!(outer.ccw, "the caller picks the near cap");
        let lone = Corner::Lone(m.loop_);
        let s = &outer.segments[0];
        let made = mef(b, keys.next(), lone, lone, s.near.curve, s.near.interval)?.0;
        built.near.push(made.edge);
        built.near_vertices.push(m.vertex);
        // The old face keeps the fin against the edge: clockwise about u.
        (made.faces[1], made.faces[0], Vec::new())
    } else {
        let (path, closing) = path(b, keys, Corner::Lone(m.loop_), m.vertex, outer, &mut built)?;
        let mut fins = path;
        fins.push(closing.fins[0]);
        // Along the closing edge is the path's way round, counterclockwise
        // about u: the twin. Against it is the near cap.
        (closing.faces[1], closing.faces[0], fins)
    };
    let far_cap = if outer.closed {
        sweep_closed(b, keys, twin, outer, &mut built)?
    } else {
        sweep_path(b, keys, twin, &twin_fins, outer, true, &mut built)?
    };
    let mut holes = Vec::new();
    for hole in &plan.holes {
        holes.push(hole_through(b, keys, near_cap, far_cap, hole)?);
    }
    set_surface(b, near_cap, plan.near_cap.0, plan.near_cap.1)?;
    set_surface(b, far_cap, plan.far_cap.0, plan.far_cap.1)?;
    for (lp, made) in core::iter::once((outer, &built)).chain(plan.holes.iter().zip(&holes)) {
        for (s, side) in lp.segments.iter().zip(&made.sides) {
            if let (Some((surface, sense)), Some(face)) = (s.side, *side) {
                set_surface(b, face, surface, sense)?;
            }
        }
    }
    Ok(LumpBuilt {
        near_cap,
        far_cap,
        outer: built,
        holes,
    })
}

/// The near path of a loop of several segments from `from`, the corner at
/// its first vertex, closed by MEF. Returns the outgoing fins of the path's
/// MEVs, which start at vertices `0..n - 1`, and the closing MEF.
fn path(
    b: &mut Body,
    keys: &mut Keys,
    from: Corner,
    first: VertexId,
    lp: &LoopPlan,
    built: &mut LoopBuilt,
) -> Result<(Vec<FinId>, crate::euler::Mef), Error> {
    let n = lp.segments.len();
    built.near_vertices.push(first);
    let mut at = from;
    let mut fins = Vec::with_capacity(n);
    for i in 0..n - 1 {
        let s = &lp.segments[i].near;
        let p = lp.vertices[i + 1].near;
        let made = mev(b, keys.next(), at, p, s.curve, s.interval)?.0;
        built.near.push(made.edge);
        built.near_vertices.push(made.vertex);
        fins.push(made.fins[0]);
        at = Corner::Before(made.fins[1]);
    }
    let s = &lp.segments[n - 1].near;
    let to = match fins.first() {
        Some(&f) => Corner::Before(f),
        None => from,
    };
    let closing = mef(b, keys.next(), at, to, s.curve, s.interval)?.0;
    built.near.push(closing.edge);
    Ok((fins, closing))
}

/// Sweeps `face`, which holds a loop of several segments, from the near
/// cap to the far one. `fins[i]` is the face's fin starting at vertex `i`.
/// When `along`, the face's loop runs the path's way (the outer loop's
/// twin); otherwise it runs backwards (a hole's disc). Each top edge runs
/// along the path, from vertex `i` to `i + 1`. What is left of `face` is
/// the far cap, or a hole's far disc, which is returned.
fn sweep_path(
    b: &mut Body,
    keys: &mut Keys,
    face: FaceId,
    fins: &[FinId],
    lp: &LoopPlan,
    along: bool,
    built: &mut LoopBuilt,
) -> Result<FaceId, Error> {
    let n = lp.segments.len();
    // The fin of the face's loop that starts at each far vertex.
    let mut top = fins.to_vec();
    for (i, v) in lp.vertices.iter().enumerate() {
        match &v.lateral {
            Some(s) => {
                let made = mev(
                    b,
                    keys.next(),
                    Corner::Before(fins[i]),
                    v.far,
                    s.curve,
                    s.interval,
                )?
                .0;
                built.far_vertices.push(made.vertex);
                built.laterals.push(Some(made.edge));
                top[i] = made.fins[1];
            }
            None => {
                built.far_vertices.push(built.near_vertices[i]);
                built.laterals.push(None);
            }
        }
    }
    built.far = built.near.clone();
    built.sides = vec![None; n];
    let mut rest = face;
    for i in 0..n {
        let s = &lp.segments[i];
        if s.side.is_none() {
            continue;
        }
        let j = (i + 1) % n;
        let (from, to) = (Corner::Before(top[i]), Corner::Before(top[j]));
        let made = mef(b, keys.next(), from, to, s.far.curve, s.far.interval)?.0;
        // The side is the short way round between the two corners: against
        // the new edge when the face runs the path's way, along it when the
        // face runs backwards. The far vertex the side took leaves the rest
        // of the face holding the new edge's other fin.
        if along {
            built.sides[i] = Some(made.faces[1]);
            rest = made.faces[0];
            top[i] = made.fins[0];
        } else {
            built.sides[i] = Some(made.faces[0]);
            rest = made.faces[1];
            top[j] = made.fins[1];
        }
        built.far[i] = made.edge;
    }
    Ok(rest)
}

/// Sweeps `face`, whose loop is the near edge of a closed segment, to the
/// far cap: a lone far vertex closed into the far edge by MEF. The face
/// keeps the far edge's fin against it and becomes the side; the new face
/// along the edge is returned (the far cap, or a hole's far disc).
fn sweep_closed(
    b: &mut Body,
    keys: &mut Keys,
    face: FaceId,
    lp: &LoopPlan,
    built: &mut LoopBuilt,
) -> Result<FaceId, Error> {
    let s = &lp.segments[0];
    let r = mvr(b, keys.next(), face, lp.vertices[0].far)?.0;
    let lone = Corner::Lone(r.loop_);
    let made = mef(b, keys.next(), lone, lone, s.far.curve, s.far.interval)?.0;
    built.far.push(made.edge);
    built.far_vertices.push(r.vertex);
    built.laterals.push(None);
    built.sides.push(Some(made.faces[1]));
    Ok(made.faces[0])
}

/// A hole through a capped lump: its near loop inside the near cap, its
/// sides swept from there, and the disc left at the far end moved onto
/// the far cap by KFMRH. A closed segment whose curve runs
/// counterclockwise about `u` is made from the far cap instead, so that
/// both caps' loops run the right way without reversing the curve.
fn hole_through(
    b: &mut Body,
    keys: &mut Keys,
    near_cap: FaceId,
    far_cap: FaceId,
    lp: &LoopPlan,
) -> Result<LoopBuilt, Error> {
    let mut built = LoopBuilt::default();
    if lp.closed {
        let s = &lp.segments[0];
        let v = &lp.vertices[0];
        // The edge on the cap the hole starts from, and on the other.
        let (start_cap, end_cap, first, second, p, q) = if lp.ccw {
            (far_cap, near_cap, s.far, s.near, v.far, v.near)
        } else {
            (near_cap, far_cap, s.near, s.far, v.near, v.far)
        };
        let r = mvr(b, keys.next(), start_cap, p)?.0;
        let lone = Corner::Lone(r.loop_);
        let made = mef(b, keys.next(), lone, lone, first.curve, first.interval)?.0;
        // The cap keeps the fin against the edge; the disc inside is new.
        let disc = made.faces[0];
        let r2 = mvr(b, keys.next(), disc, q)?.0;
        let lone = Corner::Lone(r2.loop_);
        let made2 = mef(b, keys.next(), lone, lone, second.curve, second.interval)?.0;
        kfmrh(b, keys.next(), end_cap, made2.faces[0])?;
        let (near, far, nv, fv) = if lp.ccw {
            (made2.edge, made.edge, r2.vertex, r.vertex)
        } else {
            (made.edge, made2.edge, r.vertex, r2.vertex)
        };
        built.near.push(near);
        built.far.push(far);
        built.near_vertices.push(nv);
        built.far_vertices.push(fv);
        built.laterals.push(None);
        built.sides.push(Some(made2.faces[1]));
        return Ok(built);
    }
    let r = mvr(b, keys.next(), near_cap, lp.vertices[0].near)?.0;
    let (path_fins, closing) = path(b, keys, Corner::Lone(r.loop_), r.vertex, lp, &mut built)?;
    // The cap keeps the path's way round (its first fin is the path's);
    // the disc against the closing edge runs backwards: its fin at vertex
    // i is the return fin of the MEV that reached i, or at vertex 0 the
    // closing edge's own.
    let disc = closing.faces[1];
    let n = lp.segments.len();
    let mut fins = Vec::with_capacity(n);
    fins.push(closing.fins[1]);
    for i in 1..n {
        let back = b
            .fin(path_fins[i - 1])
            .map(|f| f.around)
            .ok_or_else(crate::euler::dead)?;
        fins.push(back);
    }
    let far_disc = sweep_path(b, keys, disc, &fins, lp, false, &mut built)?;
    kfmrh(b, keys.next(), far_cap, far_disc)?;
    Ok(built)
}

/// One segment of a full revolve, in loop order from an axis contact (or
/// from anywhere round a loop that never touches the axis).
#[derive(Clone, Debug)]
pub(crate) struct TurnSegment {
    pub side: Sided,
}

/// A profile vertex of a full revolve, off the axis: it sweeps a closed
/// circular edge with one vertex at `point`.
#[derive(Clone, Debug)]
pub(crate) struct TurnVertex {
    pub point: Point3,
    pub circle: Span,
}

/// A lump of a full revolve. Either a ring (`ends` is `None`): segment
/// `i` runs from vertex `i` to vertex `i + 1` cyclically, and there may be
/// one segment with no vertex (a closed curve turned whole); or a chain:
/// segment 0 starts on the axis, segment `i` runs from vertex `i - 1` to
/// vertex `i`, and the last segment ends on the axis. `ends` gives the
/// chain's two axis points and whether each is an apex, a lone vertex of
/// its side.
#[derive(Clone, Debug)]
pub(crate) struct TurnPlan {
    pub segments: Vec<TurnSegment>,
    pub vertices: Vec<TurnVertex>,
    pub ends: Option<[(Point3, bool); 2]>,
}

/// What a full revolve's lump became: a face per segment, a circle and its
/// vertex per profile vertex, and the apexes at the chain's ends.
#[derive(Clone, Debug, Default)]
pub(crate) struct TurnBuilt {
    pub sides: Vec<FaceId>,
    pub circles: Vec<EdgeId>,
    pub vertices: Vec<VertexId>,
    pub apexes: [Option<VertexId>; 2],
}

/// Builds one lump of a full revolve into `b`, as a new solid region.
///
/// The faces are made in a chain, one closed circle at a time: MVR then
/// MEF across a face leaves the old face with the circle's fin against it
/// and the new one along it. A ring's last face and first face are the
/// same side, joined by KFMRH. Each side face ends up with the fin along
/// the circle at its first vertex and against the circle at its last.
pub(crate) fn turned(b: &mut Body, keys: &mut Keys, plan: &TurnPlan) -> Result<TurnBuilt, Error> {
    let mut out = TurnBuilt::default();
    let n = plan.vertices.len();
    let first_side = plan.segments[0].side.0;
    if n == 0 {
        // One face on a closed surface: a whole closed curve turned, or a
        // chain of one segment from the axis to the axis. MVFS starts it
        // at an apex if there is one; otherwise KVR takes its vertex away.
        let [(p, a), (q, z)] = plan.ends.unwrap_or([(Point3::ORIGIN, false); 2]);
        let start = if !a && z { q } else { p };
        let m = mvfs(b, keys.next(), start, first_side)?.0;
        out.sides.push(m.face);
        match (a, z) {
            (false, false) => {
                kvr(b, m.loop_)?;
            }
            (true, false) => out.apexes[0] = Some(m.vertex),
            (false, true) => out.apexes[1] = Some(m.vertex),
            (true, true) => {
                out.apexes[0] = Some(m.vertex);
                out.apexes[1] = Some(mvr(b, keys.next(), m.face, q)?.0.vertex);
            }
        }
        set_sides(b, plan, &out)?;
        return Ok(out);
    }
    let m = mvfs(b, keys.next(), plan.vertices[0].point, first_side)?.0;
    let lone = Corner::Lone(m.loop_);
    let c = &plan.vertices[0].circle;
    let made = mef(b, keys.next(), lone, lone, c.curve, c.interval)?.0;
    out.circles.push(made.edge);
    out.vertices.push(m.vertex);
    // The face against the first circle is the side before it; the one
    // along it continues the chain.
    let before = made.faces[1];
    let mut cur = made.faces[0];
    let mut sides = Vec::with_capacity(plan.segments.len());
    for v in &plan.vertices[1..] {
        let r = mvr(b, keys.next(), cur, v.point)?.0;
        let lone = Corner::Lone(r.loop_);
        let made = mef(
            b,
            keys.next(),
            lone,
            lone,
            v.circle.curve,
            v.circle.interval,
        )?
        .0;
        out.circles.push(made.edge);
        out.vertices.push(r.vertex);
        sides.push(made.faces[1]);
        cur = made.faces[0];
    }
    match plan.ends {
        None => {
            // The ring closes: the last face and the first are one side.
            kfmrh(b, keys.next(), cur, before)?;
            sides.push(cur);
        }
        Some([(p, a), (q, z)]) => {
            sides.insert(0, before);
            sides.push(cur);
            if a {
                out.apexes[0] = Some(mvr(b, keys.next(), before, p)?.0.vertex);
            }
            if z {
                out.apexes[1] = Some(mvr(b, keys.next(), cur, q)?.0.vertex);
            }
        }
    }
    out.sides = sides;
    set_sides(b, plan, &out)?;
    Ok(out)
}

fn set_sides(b: &mut Body, plan: &TurnPlan, out: &TurnBuilt) -> Result<(), Error> {
    for (s, &f) in plan.segments.iter().zip(&out.sides) {
        set_surface(b, f, s.side.0, s.side.1)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::check::check;
    use crate::geom::{
        Circle, Cone, Curve, Cylinder, Frame, Line, Plane, Sphere, Surface, Vector3,
    };
    use core::f64::consts::{PI, TAU};

    fn span(b: &mut Body, p: Point3, q: Point3) -> Span {
        let c = b.add_curve(Curve::Line(Line::through(p, q).unwrap()));
        Span {
            curve: c,
            interval: [0.0, p.distance(q)],
        }
    }

    fn plane(b: &mut Body, at: Point3, z: Vector3) -> Sided {
        let f = Frame::new(at, z, Vector3::new(z.z, z.x, z.y)).unwrap();
        (b.add_surface(Surface::Plane(Plane::new(f))), Sense::Forward)
    }

    /// A prism loop along +z from height 0 to `h`: `pts` in path order,
    /// counterclockwise about +z; the solid is on the left of the path for
    /// an outer loop and on its right for a hole.
    fn prism_loop(b: &mut Body, pts: &[(f64, f64)], h: f64, hole: bool) -> LoopPlan {
        let n = pts.len();
        let at = |i: usize, z: f64| Point3::new(pts[i % n].0, pts[i % n].1, z);
        let mut segments = Vec::new();
        let mut vertices = Vec::new();
        for i in 0..n {
            let (p, q) = (at(i, 0.0), at(i + 1, 0.0));
            let d = q - p;
            let right = Vector3::new(d.y, -d.x, 0.0);
            let out = if hole { -right } else { right };
            segments.push(SegmentPlan {
                near: span(b, p, q),
                far: span(b, at(i, h), at(i + 1, h)),
                side: Some(plane(b, p, out)),
            });
            vertices.push(VertexPlan {
                near: p,
                far: at(i, h),
                lateral: Some(span(b, p, at(i, h))),
            });
        }
        LoopPlan {
            segments,
            vertices,
            closed: false,
            ccw: true,
        }
    }

    fn square(x: f64, y: f64, s: f64) -> Vec<(f64, f64)> {
        vec![(x, y), (x + s, y), (x + s, y + s), (x, y + s)]
    }

    fn prism(
        b: &mut Body,
        keys: &mut Keys,
        outer: &[(f64, f64)],
        holes: &[Vec<(f64, f64)>],
    ) -> LumpBuilt {
        let plan = LumpPlan {
            outer: prism_loop(b, outer, 1.0, false),
            holes: holes.iter().map(|h| prism_loop(b, h, 1.0, true)).collect(),
            near_cap: plane(b, Point3::ORIGIN, Vector3::new(0.0, 0.0, -1.0)),
            far_cap: plane(b, Point3::new(0.0, 0.0, 1.0), Vector3::new(0.0, 0.0, 1.0)),
        };
        capped(b, keys, &plan).unwrap()
    }

    #[test]
    fn a_square_prism() {
        let mut b = Body::new();
        let made = prism(
            &mut b,
            &mut Keys::new(OpKey(5)),
            &square(0.0, 0.0, 2.0),
            &[],
        );
        assert_eq!(check(&b), []);
        let c = b.counts();
        assert_eq!((c.vertices, c.edges, c.faces), (8, 12, 6));
        assert_eq!(made.outer.sides.iter().flatten().count(), 4);
        assert_ne!(made.near_cap, made.far_cap);
    }

    #[test]
    fn a_prism_with_holes() {
        let mut b = Body::new();
        let holes = [square(0.5, 0.5, 1.0), square(2.5, 0.5, 1.0)];
        let made = prism(
            &mut b,
            &mut Keys::new(OpKey(5)),
            &square(0.0, 0.0, 4.0),
            &holes,
        );
        assert_eq!(check(&b), []);
        let c = b.counts();
        assert_eq!((c.vertices, c.edges, c.faces, c.loops), (24, 36, 14, 18));
        assert_eq!(made.holes.len(), 2);
        assert_eq!(b.face_loops(made.near_cap).count(), 3);
        assert_eq!(b.face_loops(made.far_cap).count(), 3);
    }

    #[test]
    fn two_lumps_in_one_body() {
        let mut b = Body::new();
        let mut keys = Keys::new(OpKey(5));
        prism(&mut b, &mut keys, &square(0.0, 0.0, 1.0), &[]);
        prism(&mut b, &mut keys, &square(3.0, 0.0, 1.0), &[]);
        assert_eq!(check(&b), []);
        assert_eq!(b.regions().count(), 3);
    }

    /// The volume a body's mesh encloses, after checking the body and the
    /// mesh.
    fn volume(b: &Body) -> f64 {
        use crate::mesh::{MeshParams, check_mesh, tessellate};
        assert_eq!(check(b), []);
        let m = tessellate(b, &MeshParams::new(1e-3, 0.5)).unwrap();
        assert!(m.faults.is_empty(), "{:?}", m.faults);
        check_mesh(b, &m).unwrap();
        m.welded_triangles()
            .map(|t| {
                let [a, b, c] = t.map(|i| m.positions[i as usize].to_vector());
                a.dot(b.cross(c)) / 6.0
            })
            .sum()
    }

    fn circle_span(b: &mut Body, centre: Point3, r: f64, ccw: bool) -> Span {
        let z = Vector3::new(0.0, 0.0, if ccw { 1.0 } else { -1.0 });
        let f = Frame::new(centre, z, Vector3::new(1.0, 0.0, 0.0)).unwrap();
        let c = b.add_curve(Curve::Circle(Circle::new(f, r).unwrap()));
        Span {
            curve: c,
            interval: [0.0, TAU],
        }
    }

    /// A circle of radius `r` about the z axis at (x, 0), extruded from 0
    /// to 1, its curve running counterclockwise about +z or not.
    fn disc_loop(b: &mut Body, x: f64, r: f64, ccw: bool, hole: bool) -> LoopPlan {
        let at = |z: f64| Point3::new(x, 0.0, z);
        let cyl = Cylinder::new(
            Frame::new(
                at(0.0),
                Vector3::new(0.0, 0.0, 1.0),
                Vector3::new(1.0, 0.0, 0.0),
            )
            .unwrap(),
            r,
        )
        .unwrap();
        let sense = if hole {
            Sense::Reversed
        } else {
            Sense::Forward
        };
        let start = Point3::new(x + r, 0.0, 0.0);
        LoopPlan {
            segments: vec![SegmentPlan {
                near: circle_span(b, at(0.0), r, ccw),
                far: circle_span(b, at(1.0), r, ccw),
                side: Some((b.add_surface(Surface::Cylinder(cyl)), sense)),
            }],
            vertices: vec![VertexPlan {
                near: start,
                far: start + Vector3::new(0.0, 0.0, 1.0),
                lateral: None,
            }],
            closed: true,
            ccw,
        }
    }

    fn caps(b: &mut Body) -> (Sided, Sided) {
        (
            plane(b, Point3::ORIGIN, Vector3::new(0.0, 0.0, -1.0)),
            plane(b, Point3::new(0.0, 0.0, 1.0), Vector3::new(0.0, 0.0, 1.0)),
        )
    }

    #[test]
    fn a_cylinder_with_holes_either_way_round() {
        let mut b = Body::new();
        let (near_cap, far_cap) = caps(&mut b);
        let outer = disc_loop(&mut b, 0.0, 3.0, true, false);
        let mut holes = vec![
            disc_loop(&mut b, -1.5, 0.5, true, true),
            disc_loop(&mut b, 1.5, 0.5, false, true),
        ];
        holes.push(prism_loop(&mut b, &square(-0.5, 1.0, 1.0), 1.0, true));
        let plan = LumpPlan {
            outer,
            holes,
            near_cap,
            far_cap,
        };
        capped(&mut b, &mut Keys::new(OpKey(5)), &plan).unwrap();
        let exact = PI * 9.0 - 2.0 * PI * 0.25 - 1.0;
        assert!((volume(&b) - exact).abs() < 0.05, "{}", volume(&b));
    }

    /// A full turn about the z axis of the profile with corners `pts` in
    /// the xz half-plane (as (x, z), in path order), the sides given.
    fn turn_plan(b: &mut Body, pts: &[(f64, f64)], sides: Vec<Surface>, ends: bool) -> TurnPlan {
        let mut vertices = Vec::new();
        let inner = if ends { &pts[1..pts.len() - 1] } else { pts };
        for &(x, z) in inner {
            let point = Point3::new(x, 0.0, z);
            vertices.push(TurnVertex {
                point,
                circle: circle_span(b, Point3::new(0.0, 0.0, z), x, true),
            });
        }
        let ends = ends.then(|| {
            let (a, z) = (pts[0], pts[pts.len() - 1]);
            [
                (Point3::new(a.0, 0.0, a.1), false),
                (Point3::new(z.0, 0.0, z.1), false),
            ]
        });
        TurnPlan {
            segments: sides
                .into_iter()
                .map(|s| TurnSegment {
                    side: (b.add_surface(s), Sense::Forward),
                })
                .collect(),
            vertices,
            ends,
        }
    }

    fn flat(z: f64, up: bool) -> Surface {
        let n = Vector3::new(0.0, 0.0, if up { 1.0 } else { -1.0 });
        Surface::Plane(Plane::new(
            Frame::new(Point3::new(0.0, 0.0, z), n, Vector3::new(1.0, 0.0, 0.0)).unwrap(),
        ))
    }

    fn cylinder(r: f64) -> Surface {
        Surface::Cylinder(Cylinder::new(Frame::WORLD, r).unwrap())
    }

    #[test]
    fn a_square_ring_turned_whole() {
        // Counterclockwise about -y (the direction a turn about +z moves
        // the xz half-plane is +y): out along the bottom, up, back, down.
        let pts = [(1.0, 0.0), (2.0, 0.0), (2.0, 1.0), (1.0, 1.0)];
        let mut b = Body::new();
        let sides = vec![
            flat(0.0, false),
            cylinder(2.0),
            flat(1.0, true),
            cylinder(1.0),
        ];
        let mut plan = turn_plan(&mut b, &pts, sides, false);
        plan.segments[3].side.1 = Sense::Reversed;
        turned(&mut b, &mut Keys::new(OpKey(5)), &plan).unwrap();
        assert!((volume(&b) - 3.0 * PI).abs() < 0.02);
    }

    #[test]
    fn chains_from_the_axis() {
        // A solid cylinder: the bottom disc, the side, the top disc.
        let pts = [(0.0, 0.0), (1.0, 0.0), (1.0, 2.0), (0.0, 2.0)];
        let mut b = Body::new();
        let sides = vec![flat(0.0, false), cylinder(1.0), flat(2.0, true)];
        let plan = turn_plan(&mut b, &pts, sides, true);
        let made = turned(&mut b, &mut Keys::new(OpKey(5)), &plan).unwrap();
        assert_eq!(made.sides.len(), 3);
        assert!((volume(&b) - 2.0 * PI).abs() < 0.02);
        // A sphere: one face, no vertices.
        let mut b = Body::new();
        let sphere = Surface::Sphere(Sphere::new(Frame::WORLD, 1.0).unwrap());
        let plan = turn_plan(&mut b, &[(0.0, -1.0), (0.0, 1.0)], vec![sphere], true);
        turned(&mut b, &mut Keys::new(OpKey(5)), &plan).unwrap();
        assert_eq!(b.counts().vertices, 0);
        assert!((volume(&b) - 4.0 / 3.0 * PI).abs() < 0.02);
        // A cone with its apex up: the apex is a lone vertex of the side.
        let mut b = Body::new();
        let down = Frame::new(
            Point3::ORIGIN,
            Vector3::new(0.0, 0.0, -1.0),
            Vector3::new(1.0, 0.0, 0.0),
        );
        let cone = Cone::new(down.unwrap(), 1.0, crate::math::atan2(1.0, 2.0)).unwrap();
        let sides = vec![flat(0.0, false), Surface::Cone(cone)];
        let mut plan = turn_plan(&mut b, &[(0.0, 0.0), (1.0, 0.0), (0.0, 2.0)], sides, true);
        plan.ends.as_mut().unwrap()[1].1 = true;
        let made = turned(&mut b, &mut Keys::new(OpKey(5)), &plan).unwrap();
        assert!(made.apexes[1].is_some());
        assert_eq!(b.counts().vertices, 2);
        assert!((volume(&b) - 2.0 * PI / 3.0).abs() < 0.02);
    }
}
