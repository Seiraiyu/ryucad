//! The geometry of each chain of a blend, made before anything is built.
//!
//! A chain is cut at its smooth joins into segments, one blend face each:
//! the edges of a segment share both faces, so one spine and one surface
//! serve them all. Each segment has a rail on the chain's rail face and
//! one on its other face, running along the chain, and the stretch of
//! each it covers: from an end's crossings, from a smooth join's joint,
//! or a whole period round a closed chain. At a plain join the other
//! face's rail is cut where the ball passes the vertex. At a smooth join
//! the two segments' spines meet where the ball touches the smooth edge,
//! and the section of the ball there separates their faces.

use super::chain::{Chain, Class};
use super::end::{Crossing, EndCurve, EndFail, crossing, end_curve, on_edge};
use super::fillet::{SurfaceFail, fillet_surface};
use super::rail::{Rail, rail};
use super::spec::Spec;
use super::spine::{Spine, ball_on, nearest, spine};
use super::{BlendKind, chamfer};
use crate::geom::{Circle, Curve, Frame, NurbsCurve, Point3, Surface};
use crate::intersect::Failure;
use crate::name::Name;
use crate::topo::{Body, EdgeId, FaceId, Sense, VertexId};
use crate::{BlendError, Error};

/// One blend face of a chain.
#[derive(Clone, Debug)]
pub(crate) struct Segment {
    /// Positions in the chain's edges, in order: edges joined at plain
    /// joins.
    pub(crate) edges: Vec<usize>,
    pub(crate) surface: Surface,
    pub(crate) sense: Sense,
    /// The faces the segment lies between: the chain's rail face and its
    /// other face.
    pub(crate) faces: [FaceId; 2],
    /// The rails on those faces, both running along the chain.
    pub(crate) rails: [Rail; 2],
    /// The stretch of each rail's own parameter the segment covers.
    pub(crate) intervals: [[f64; 2]; 2],
    /// The other face's rail parameter at each plain join inside the
    /// segment, in order.
    pub(crate) joins: Vec<f64>,
    /// How far the blend reaches from its edges: the ball's diameter, or
    /// twice a chamfer's larger setback.
    pub(crate) reach: f64,
    /// The rails' parameters are the parameter along a pipe's spine (a
    /// fillet's), so a rail's pcurve on a pipe needs no projection.
    pub(crate) spine: bool,
}

/// The arc of the ball where two segments meet at a smooth join, from the
/// corner on the rail face to the point on the smooth edge.
#[derive(Clone, Debug)]
pub(crate) struct Section {
    pub(crate) curve: Curve,
    pub(crate) interval: [f64; 2],
}

/// What a chain's vertex becomes.
#[derive(Clone, Debug)]
pub(crate) enum Joint {
    Closed,
    /// `crossings` and the end curve's direction are in the order rail
    /// face, other face.
    End {
        face: FaceId,
        crossings: [Crossing; 2],
        curve: Box<EndCurve>,
    },
    Join,
    /// The vertex moves to `point`, the smooth edge's parameter `at`; the
    /// section starts where the rails on the rail face meet.
    SmoothJoin {
        smooth: EdgeId,
        point: Point3,
        at: f64,
        section: Section,
    },
}

/// A chain's geometry.
#[derive(Clone, Debug)]
pub(crate) struct Plan {
    pub(crate) segments: Vec<Segment>,
    /// One per chain vertex.
    pub(crate) joints: Vec<Joint>,
}

/// The chain vertices where segments start (positions of their first
/// edges), and whether the chain is one closed segment.
fn boundaries(chain: &Chain) -> (Vec<usize>, bool) {
    let smooth: Vec<usize> = (0..chain.vertices.len())
        .filter(|&i| matches!(chain.classes[i], Class::SmoothJoin { .. }))
        .collect();
    if chain.closed {
        if smooth.is_empty() {
            (vec![0], true)
        } else {
            (smooth, false)
        }
    } else {
        let mut b = vec![0];
        b.extend(smooth);
        (b, false)
    }
}

/// A segment's carrier and rails, before its ends are known.
pub(crate) struct Partial {
    pub(crate) edges: Vec<usize>,
    /// The curve the segment's sections follow, running along the chain:
    /// a fillet's spine, a chamfer's edge.
    pub(crate) carrier: Curve,
    /// A fillet's spine.
    pub(crate) spine: Option<Spine>,
    /// The rail face and the other face.
    pub(crate) faces: [FaceId; 2],
    /// A fillet's spine's indices of those faces.
    pub(crate) sides: [usize; 2],
    pub(crate) rails: [Rail; 2],
    /// A chamfer's rails fitted on shared knots, where its surface is
    /// ruled.
    pub(crate) ruled: Option<[NurbsCurve; 2]>,
    /// The carrier's parameters the rails cover.
    pub(crate) span: [f64; 2],
    /// The carrier's parameters where each rail starts and ends.
    pub(crate) start: [Option<f64>; 2],
    pub(crate) end: [Option<f64>; 2],
    pub(crate) closed: bool,
    pub(crate) reach: f64,
}

/// What an end is found to be before the end curve is made.
struct EndAt {
    vertex: usize,
    face: FaceId,
    crossings: [Crossing; 2],
}

/// The joint found at a smooth join.
pub(crate) struct SmoothAt {
    pub(crate) vertex: usize,
    pub(crate) smooth: EdgeId,
    pub(crate) point: Point3,
    pub(crate) at: f64,
    pub(crate) section: Section,
}

/// The plan of a chain, its edges all fillets of one radius or all
/// chamfers alike.
pub(crate) fn plan(body: &Body, specs: &[Spec], chain: &Chain) -> Result<Plan, Error> {
    let kind = specs[chain.edges[0].0].kind;
    let (starts, closed) = boundaries(chain);
    let n = chain.edges.len();
    let mut parts: Vec<Partial> = Vec::new();
    for (i, &s) in starts.iter().enumerate() {
        let stop = if closed {
            s + n
        } else if chain.closed {
            let next = starts[(i + 1) % starts.len()];
            if next > s { next } else { next + n }
        } else {
            starts.get(i + 1).copied().unwrap_or(n)
        };
        let edges: Vec<usize> = (s..stop).map(|p| p % n).collect();
        parts.push(match kind {
            BlendKind::Fillet { radius } => {
                fillet_partial(body, specs, chain, edges, radius, closed)?
            }
            BlendKind::Chamfer(c) => chamfer::partial(body, specs, chain, edges, &c, closed)?,
        });
    }
    let mut ends: Vec<EndAt> = Vec::new();
    let mut smooths: Vec<SmoothAt> = Vec::new();
    for (vi, class) in chain.classes.iter().enumerate() {
        match *class {
            Class::End { face, cuts } => {
                let first = vi == 0;
                let p = if first { 0 } else { parts.len() - 1 };
                let spec = &specs[chain.edges[if first { 0 } else { n - 1 }].0];
                let part = &mut parts[p];
                let surface = &body.surfaces[body.faces[face].surface];
                let mut x = Vec::with_capacity(2);
                for side in 0..2 {
                    let f = part.faces[side];
                    let cut = cuts[usize::from(spec.faces[1] == f)];
                    let rl = &part.rails[side];
                    let range = [rl.param(part.span[0]), rl.param(part.span[1])];
                    let near = body.vertices[chain.vertices[vi]].point;
                    let c = crossing(body, rl, range, surface, cut, near).map_err(|e| {
                        end_error(e, spec.name, body.faces[f].name, body.faces[face].name)
                    })?;
                    let t = (c.tau - rl.map[0]) / rl.map[1];
                    if first {
                        part.start[side] = Some(t);
                    } else {
                        part.end[side] = Some(t);
                    }
                    x.push(c);
                }
                ends.push(EndAt {
                    vertex: vi,
                    face,
                    crossings: [x[0], x[1]],
                });
            }
            Class::SmoothJoin { smooth, .. } => {
                let after = parts
                    .iter()
                    .position(|p| p.edges[0] == vi)
                    .expect("a segment starts at a smooth join");
                let before = (after + parts.len() - 1) % parts.len();
                let (a, b) = (&parts[before], &parts[after]);
                let j = match kind {
                    BlendKind::Fillet { .. } => fillet_joint(body, chain, a, b, vi, smooth)?,
                    BlendKind::Chamfer(_) => chamfer::joint(body, chain, a, b, vi, smooth)?,
                };
                parts[before].end = [Some(j.ta); 2];
                parts[after].start = [Some(j.tb); 2];
                smooths.push(j.at);
            }
            Class::Closed | Class::Join => {}
        }
    }
    let mut segments = Vec::with_capacity(parts.len());
    for part in &parts {
        segments.push(segment(body, specs, chain, part)?);
    }
    let mut joints: Vec<Joint> = chain
        .classes
        .iter()
        .map(|c| match c {
            Class::Closed => Joint::Closed,
            _ => Joint::Join,
        })
        .collect();
    for e in ends {
        let seg = if e.vertex == 0 {
            &segments[0]
        } else {
            &segments[segments.len() - 1]
        };
        let vertex = chain.vertices[e.vertex];
        let end = &body.surfaces[body.faces[e.face].surface];
        let curve = end_curve(
            &seg.surface,
            end,
            e.crossings[0].point,
            e.crossings[1].point,
            body.vertices[vertex].point,
            seg.reach,
        )
        .map_err(|f| match f {
            EndFail::Unsupported => BlendError::Unsupported {
                at: body.faces[e.face].name,
            },
            _ => BlendError::Uncertain {
                at: body.vertices[vertex].name,
            },
        })?;
        joints[e.vertex] = Joint::End {
            face: e.face,
            crossings: e.crossings,
            curve: Box::new(curve),
        };
    }
    for s in smooths {
        joints[s.vertex] = Joint::SmoothJoin {
            smooth: s.smooth,
            point: s.point,
            at: s.at,
            section: s.section,
        };
    }
    Ok(Plan { segments, joints })
}

/// The spine and rails of the segment of `edges` (chain positions), for
/// fillets of `radius`.
fn fillet_partial(
    body: &Body,
    specs: &[Spec],
    chain: &Chain,
    edges: Vec<usize>,
    radius: f64,
    closed: bool,
) -> Result<Partial, Error> {
    let (k, forward) = chain.edges[edges[0]];
    let spec = &specs[k];
    let mut s = spine(body, spec, radius)?;
    if !forward {
        s = s.reversed();
    }
    for &p in &edges[1..] {
        let other = &specs[chain.edges[p].0];
        if !ball_on(body, other, &s) {
            return Err(BlendError::Uncertain { at: other.name }.into());
        }
    }
    let rail_side = chain.rail_side(specs, k);
    let sides = [rail_side, 1 - rail_side];
    let faces = sides.map(|i| spec.faces[i]);
    let surfaces = faces.map(|f| body.surfaces[body.faces[f].surface].clone());
    let span = match s.curve.period() {
        Some(p) if !closed => [s.mid - p / 2.0, s.mid + p / 2.0],
        _ => s.range,
    };
    let mut rails = Vec::with_capacity(2);
    for side in 0..2 {
        let r = rail(&s, &surfaces[side], sides[side], span).map_err(|f| {
            let at = spec.name;
            match f {
                Failure::Unsupported => BlendError::Unsupported { at },
                Failure::Uncertain => BlendError::Uncertain { at },
            }
        })?;
        rails.push(r);
    }
    let (start, end) = if closed {
        ([Some(span[0]); 2], [Some(span[1]); 2])
    } else {
        ([None; 2], [None; 2])
    };
    Ok(Partial {
        edges,
        carrier: s.curve.clone(),
        spine: Some(s),
        faces,
        sides,
        rails: [rails[0].clone(), rails[1].clone()],
        ruled: None,
        span,
        start,
        end,
        closed,
        reach: 2.0 * radius,
    })
}

/// A finished segment: its surface over the stretch its rails cover, and
/// the other face's rail parameters at its plain joins.
fn segment(body: &Body, specs: &[Spec], chain: &Chain, part: &Partial) -> Result<Segment, Error> {
    let name = specs[chain.edges[part.edges[0]].0].name;
    let uncertain = || -> Error { BlendError::Uncertain { at: name }.into() };
    let mut ts = [[0.0; 2]; 2];
    for (side, t) in ts.iter_mut().enumerate() {
        let (Some(a), Some(b)) = (part.start[side], part.end[side]) else {
            return Err(uncertain());
        };
        if b <= a {
            return Err(BlendError::Overflow {
                edge: name,
                face: body.faces[part.faces[side]].name,
            }
            .into());
        }
        *t = [a, b];
    }
    let lo = ts[0][0].min(ts[1][0]);
    let hi = ts[0][1].max(ts[1][1]);
    let range = if part.closed {
        part.span
    } else {
        let margin = (hi - lo) / 4.0;
        [lo - margin, hi + margin]
    };
    let (surface, sense) = match &part.spine {
        Some(spine) => fillet_face(body, part, spine, range, name)?,
        None => chamfer::face(body, specs, chain, part, name)?,
    };
    let mut joins = Vec::new();
    for &p in &part.edges[1..] {
        let v = body.vertices[chain.vertices[p]].point;
        let (t, _) = nearest(&part.carrier, ts[1], v);
        joins.push(part.rails[1].param(t));
    }
    let intervals = [0, 1].map(|side| ts[side].map(|t| part.rails[side].param(t)));
    Ok(Segment {
        edges: part.edges.clone(),
        surface,
        sense,
        faces: part.faces,
        rails: part.rails.clone(),
        intervals,
        joins,
        reach: part.reach,
        spine: part.spine.is_some(),
    })
}

/// A fillet's surface over the spine's parameters `range`.
fn fillet_face(
    body: &Body,
    part: &Partial,
    spine: &Spine,
    range: [f64; 2],
    name: Name,
) -> Result<(Surface, Sense), Error> {
    let surfaces = part
        .faces
        .map(|f| body.surfaces[body.faces[f].surface].clone());
    let feet = [0, 1].map(|i| {
        let mut both = [&surfaces[0], &surfaces[1]];
        if part.sides[0] == 1 {
            both.swap(0, 1);
        }
        spine.foot(both, i, spine.mid)
    });
    let anchor = if surfaces[0].frame().is_some() {
        &surfaces[0]
    } else {
        &surfaces[1]
    };
    fillet_surface(spine, anchor, range, feet).map_err(|f| {
        match f {
            SurfaceFail::TooLarge => BlendError::RadiusTooLarge { edge: name },
            SurfaceFail::Unsupported => BlendError::Unsupported { at: name },
        }
        .into()
    })
}

/// Where two segments' carriers meet at a smooth join.
pub(crate) struct Joint2 {
    pub(crate) ta: f64,
    pub(crate) tb: f64,
    pub(crate) at: SmoothAt,
}

/// The joint of fillets at chain vertex `vi`, a smooth join on `smooth`,
/// between the segment `a` ending there and `b` starting there: the ball
/// on `a`'s spine whose foot on `a`'s other face lies on the smooth edge
/// (by the secant method on the foot's signed distance from the edge
/// across the face), on `b`'s spine too.
fn fillet_joint(
    body: &Body,
    chain: &Chain,
    a: &Partial,
    b: &Partial,
    vi: usize,
    smooth: EdgeId,
) -> Result<Joint2, Error> {
    let vertex: VertexId = chain.vertices[vi];
    let at = body.vertices[vertex].name;
    let uncertain = || -> Error { BlendError::Uncertain { at }.into() };
    let le = &body.edges[smooth];
    let lc = &body.curves[le.curve];
    let surfaces = a
        .faces
        .map(|f| body.surfaces[body.faces[f].surface].clone());
    let mut both = [&surfaces[0], &surfaces[1]];
    if a.sides[0] == 1 {
        both.swap(0, 1);
    }
    let (sa, sb) = (
        a.spine.as_ref().ok_or_else(uncertain)?,
        b.spine.as_ref().ok_or_else(uncertain)?,
    );
    let foot = |t: f64, side: usize| sa.foot(both, a.sides[side], t);
    let other = &surfaces[1];
    // The foot's signed distance from the smooth edge, across the face.
    let across = |t: f64| -> Option<f64> {
        let p = foot(t, 1);
        let s = lc.project(p).param;
        let e = lc.eval(s);
        let (u, v) = other.project(e.point).param;
        let n = other.normal(u, v)?.get();
        let w = n.cross(e.d1).normalize()?.get();
        Some((p - e.point).dot(w))
    };
    let v = body.vertices[vertex].point;
    let (t0, _) = nearest(&sa.curve, a.span, v);
    let h = (a.span[1] - a.span[0]) * 1e-3;
    let (mut x0, mut x1) = (t0 - h, t0);
    let (mut f0, mut f1) = (
        across(x0).ok_or_else(uncertain)?,
        across(x1).ok_or_else(uncertain)?,
    );
    for _ in 0..60 {
        if f1 == f0 || f1.abs() <= 1e-15 {
            break;
        }
        let x2 = x1 - f1 * (x1 - x0) / (f1 - f0);
        (x0, f0) = (x1, f1);
        x1 = x2;
        f1 = across(x1).ok_or_else(uncertain)?;
    }
    if f1.abs() > crate::tol::LINEAR_RESOLUTION {
        return Err(uncertain());
    }
    let ta = x1;
    let c = sa.curve.point(ta);
    let (tb, d) = nearest(&sb.curve, b.span, c);
    if d > 10.0 * sa.tolerance.max(sb.tolerance) {
        return Err(uncertain());
    }
    let corner = foot(ta, 0);
    let point = foot(ta, 1);
    let reach = le.tolerance.get() + 10.0 * crate::tol::LINEAR_RESOLUTION;
    let Some(param) = on_edge(lc, le.interval, point, reach) else {
        return Err(BlendError::Overflow {
            edge: body.edges[smooth].name,
            face: body.faces[a.faces[1]].name,
        }
        .into());
    };
    // The section: the ball's circle in the plane normal to the spine,
    // from the corner to the point on the smooth edge.
    let (x, y) = (corner - c, point - c);
    let z = x.cross(y);
    let frame = Frame::new(c, z, x).map_err(|_| uncertain())?;
    let circle = Circle::new(frame, sa.radius).map_err(|_| uncertain())?;
    let phi = crate::math::atan2(z.length(), x.dot(y));
    Ok(Joint2 {
        ta,
        tb,
        at: SmoothAt {
            vertex: vi,
            smooth,
            point,
            at: param,
            section: Section {
                curve: Curve::Circle(circle),
                interval: [0.0, phi],
            },
        },
    })
}

/// The error for a failed end.
pub(crate) fn end_error(e: EndFail, edge: Name, face: Name, end: Name) -> Error {
    match e {
        EndFail::Overflow => BlendError::Overflow { edge, face },
        EndFail::Unsupported => BlendError::Unsupported { at: end },
        EndFail::Uncertain => BlendError::Uncertain { at: edge },
    }
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blend::chain::chains;
    use crate::blend::spec::resolve;
    use crate::blend::tests::{block, edge_of, slot};
    use crate::blend::{BlendKind, EdgeBlend};
    use crate::geom::Rigid;
    use crate::name::{FaceRole, OpKey};

    fn plan_of(b: &Body, edges: &[Name], r: f64) -> Result<Vec<Plan>, Error> {
        let blends: Vec<EdgeBlend> = edges
            .iter()
            .map(|&edge| EdgeBlend {
                edge,
                kind: BlendKind::Fillet { radius: r },
            })
            .collect();
        let specs = resolve(b, &blends)?;
        chains(b, &specs)?
            .iter()
            .map(|c| plan(b, &specs, c))
            .collect()
    }

    /// Every rail's ends where the plan says, on the segment's surface.
    fn rails_on_surfaces(p: &Plan) {
        for s in &p.segments {
            for side in 0..2 {
                for t in s.intervals[side] {
                    let q = s.rails[side].curve.point(t);
                    assert!(s.surface.distance(q) < 1e-9, "{}", s.surface.distance(q));
                }
            }
        }
    }

    #[test]
    fn a_box_edge_is_one_segment_between_two_ends() {
        let b = block();
        let p = plan_of(&b, &[edge_of(&b, FaceRole::PlusX, FaceRole::PlusZ)], 0.2).unwrap();
        assert_eq!(p.len(), 1);
        let p = &p[0];
        assert_eq!(p.segments.len(), 1);
        assert!(matches!(p.segments[0].surface, Surface::Cylinder(_)));
        assert!(p.joints.iter().all(|j| matches!(j, Joint::End { .. })));
        for side in 0..2 {
            let [a, z] = p.segments[0].intervals[side];
            // A rail runs the edge's whole length: the end faces are
            // square to it.
            assert!((z - a - 1.0).abs() < 1e-12, "{a} {z}");
        }
        rails_on_surfaces(p);
        for j in &p.joints {
            let Joint::End {
                crossings, curve, ..
            } = j
            else {
                panic!()
            };
            assert!(
                curve
                    .curve
                    .point(curve.interval[0])
                    .distance(crossings[0].point)
                    < 1e-9
            );
            assert!(
                curve
                    .curve
                    .point(curve.interval[1])
                    .distance(crossings[1].point)
                    < 1e-9
            );
        }
    }

    #[test]
    fn a_slots_cap_is_four_segments_meeting_in_sections() {
        let b = slot();
        let mut caps: Vec<Name> = b
            .edges()
            .filter(|&e| {
                let ed = b.edge(e).unwrap();
                b.vertex(ed.start).unwrap().point.z > 0.5 && b.vertex(ed.end).unwrap().point.z > 0.5
            })
            .map(|e| b.edge(e).unwrap().name)
            .collect();
        caps.sort();
        let p = plan_of(&b, &caps, 0.1).unwrap().remove(0);
        assert_eq!(p.segments.len(), 4);
        let (cyl, tor) = p.segments.iter().fold((0, 0), |(c, t), s| match s.surface {
            Surface::Cylinder(_) => (c + 1, t),
            Surface::Torus(_) => (c, t + 1),
            _ => panic!(),
        });
        assert_eq!((cyl, tor), (2, 2));
        rails_on_surfaces(&p);
        for j in &p.joints {
            let Joint::SmoothJoin { point, section, .. } = j else {
                panic!("{j:?}")
            };
            let corner = &section.curve.point(section.interval[0]);
            // A radius in from the cap's edge, and a radius down the side.
            assert!((corner.z - 1.0).abs() < 1e-12);
            assert!((point.z - 0.9).abs() < 1e-12, "{point:?}");
            let [a, z] = section.interval;
            assert!((z - a - core::f64::consts::FRAC_PI_2).abs() < 1e-9);

            assert!(section.curve.point(z).distance(*point) < 1e-12);
        }
        // Each segment's rails end where the next one's start.
        for k in 0..4 {
            let (s, t) = (&p.segments[k], &p.segments[(k + 1) % 4]);
            for side in 0..2 {
                let end = s.rails[side].curve.point(s.intervals[side][1]);
                let start = t.rails[side].curve.point(t.intervals[side][0]);
                assert!(end.distance(start) < 1e-9);
            }
        }
    }

    #[test]
    fn a_rim_is_one_closed_segment_and_a_split_edge_one_with_a_join() {
        let (cyl, _) = crate::primitive::cylinder(OpKey(1), 1.0, 1.0, &Rigid::IDENTITY).unwrap();
        let rim = cyl.edges().next().unwrap();
        let p = plan_of(&cyl, &[cyl.edge(rim).unwrap().name], 0.2)
            .unwrap()
            .remove(0);
        assert_eq!(p.segments.len(), 1);
        assert!(matches!(p.joints[..], [Joint::Closed]));
        rails_on_surfaces(&p);
        let mut b = block();
        let n = edge_of(&b, FaceRole::PlusX, FaceRole::PlusZ);
        let e = crate::blend::tests::edge_id(&b, n);
        let [t0, t1] = b.edge(e).unwrap().interval;
        let q = b
            .curve(b.edge(e).unwrap().curve)
            .unwrap()
            .point((t0 + t1) / 2.0);
        let (s, _) = crate::euler::semv(&mut b, OpKey(5), e, (t0 + t1) / 2.0, q).unwrap();
        crate::pcurve::fill(&mut b).unwrap();
        let halves = [b.edge(e).unwrap().name, b.edge(s.edge).unwrap().name];
        let p = plan_of(&b, &halves, 0.2).unwrap().remove(0);
        assert_eq!(p.segments.len(), 1);
        assert_eq!(p.segments[0].joins.len(), 1);
        assert!(matches!(p.joints[1], Joint::Join));
    }
}
