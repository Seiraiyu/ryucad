//! Sketches to ryukei profiles: the closed loops a sweep takes, with keys
//! made from the sketch's own IDs, and the kernel's faults mapped back to
//! them.
//!
//! A segment's key is SipHash of `ryucad.sketch\0<part>\0<sketch>\0<entity>`
//! and a loop's the same over its anchor: the first of the points where its
//! curves join, in [`natural`] order (a lone circle's anchor is the circle).
//! Editing a dimension, where a loop starts, or adding a segment or a
//! fillet renames nothing: generated point IDs only grow, so a new point
//! never sorts first.

use crate::doc::id::Id;
use crate::doc::sketch::Geometry;
use crate::error::{Code, Error, Result};
use crate::sketch::Solved;
use core::f64::consts::TAU;
use ryukei::ProfileError;
use ryukei::geom::{Circle2, Curve2, Frame, Frame2, Line2, Point2, Vector2};
use ryukei::math;
use ryukei::name::OpKey;
use ryukei::sweep::{Loop, LoopKey, Profile, Segment, SegmentKey, ValidProfile};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// The 128-bit key for a sketch entity.
pub fn entity_key(part: &Id, sketch: &Id, entity: &Id) -> u128 {
    let mut bytes = b"ryucad.sketch\0".to_vec();
    for (i, s) in [part, sketch, entity].iter().enumerate() {
        if i > 0 {
            bytes.push(0);
        }
        bytes.extend_from_slice(s.as_str().as_bytes());
    }
    OpKey::from_bytes(&bytes).0
}

/// Which sketch entity each key of a profile came from.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Keys {
    pub segments: BTreeMap<u128, Id>,
    /// A loop's key, and its anchor, which names it.
    pub loops: BTreeMap<u128, Id>,
    /// An open chain's key, and its least curve in [`natural`] order,
    /// which names it.
    pub chains: BTreeMap<u128, Id>,
    /// Where each point at a loop's joins lies in the profile's plane: a
    /// sketch's points, a face's vertices by name. A loft starts a loop at
    /// one of them.
    pub points: BTreeMap<Id, Point2>,
}

impl Keys {
    fn segment(&self, k: SegmentKey) -> Id {
        self.segments[&k.0].clone()
    }

    fn loop_(&self, k: LoopKey) -> Id {
        self.loops[&k.0].clone()
    }

    /// The entities the kernel's profile fault names.
    pub fn of_fault(&self, e: &ProfileError) -> Vec<Id> {
        let mut out = match *e {
            ProfileError::EmptyLoop { loop_ } | ProfileError::Degenerate { loop_ } => {
                vec![self.loop_(loop_)]
            }
            ProfileError::BadRange { segment }
            | ProfileError::ShortSegment { segment }
            | ProfileError::ClosedSegmentNotAlone { segment }
            | ProfileError::SeamLoop { segment } => vec![self.segment(segment)],
            ProfileError::NotClosed { after, .. } => vec![self.segment(after)],
            ProfileError::Touching { a, b } => vec![self.segment(a), self.segment(b)],
            ProfileError::EmptyChain { chain } | ProfileError::ChainCloses { chain } => {
                self.chains.get(&chain.0).cloned().into_iter().collect()
            }
            ProfileError::ChainBroken { after, .. } => vec![self.segment(after)],
            _ => Vec::new(),
        };
        out.dedup();
        out
    }
}

/// What a profile fault means, for people.
fn describe(e: &ProfileError) -> &'static str {
    match e {
        ProfileError::Touching { .. } => "curves cross, overlap or touch",
        ProfileError::ShortSegment { .. } => "a curve is shorter than the resolution",
        ProfileError::NotClosed { .. } => "a loop doesn't close",
        ProfileError::Degenerate { .. } => "a loop encloses too little area to orient",
        ProfileError::OutsideSizeBox => "the profile leaves the ±500 m size box",
        ProfileError::ChainBroken { .. } => "an open chain's curves don't meet end to end",
        ProfileError::ChainCloses { .. } => "an open chain closes on itself: it is a loop",
        _ => "the loops don't make a profile",
    }
}

pub use crate::doc::sketch::natural;

/// A loop's anchor: the first of its curves' end points in [`natural`]
/// order, or for a loop with none (a circle), its first curve.
pub fn anchor(solved: &Solved, l: &[(Id, bool)]) -> Id {
    let ends = l
        .iter()
        .flat_map(|(e, _)| match &solved.sketch.entities[e].geometry {
            Geometry::Line { start, end } | Geometry::Arc { start, end, .. } => vec![start, end],
            _ => vec![],
        });
    ends.min_by(|a, b| natural(a).cmp(&natural(b)))
        .or_else(|| {
            l.iter()
                .map(|(e, _)| e)
                .min_by(|a, b| natural(a).cmp(&natural(b)))
        })
        .expect("loops aren't empty")
        .clone()
}

fn point(solved: &Solved, id: &Id) -> Point2 {
    match solved.sketch.entities[id].geometry {
        Geometry::Point { m } => Point2::new(m[0], m[1]),
        _ => unreachable!("validated: curves are built on points"),
    }
}

/// The trimmed curve a sketch curve is, walked backwards when `reversed`.
fn segment(solved: &Solved, id: &Id, reversed: bool, key: SegmentKey) -> Result<Segment> {
    let degenerate = |e: ryukei::Error| {
        Error::new(
            Code::ProfileInvalid,
            format!("{id} has no length or no radius"),
        )
        .with("entities", [id])
        .with("kernel", format!("{e:?}"))
    };
    let (curve, t0, t1) = match &solved.sketch.entities[id].geometry {
        Geometry::Line { start, end } => {
            let (a, b) = (point(solved, start), point(solved, end));
            let line = Line2::through(a, b).map_err(degenerate)?;
            (Curve2::Line2(line), 0.0, math::hypot(b.x - a.x, b.y - a.y))
        }
        Geometry::Arc { center, start, end } => {
            let (c, s, e) = (
                point(solved, center),
                point(solved, start),
                point(solved, end),
            );
            let x = Vector2::new(s.x - c.x, s.y - c.y);
            let r = math::hypot(x.x, x.y);
            let frame = Frame2::new(c, x, true).map_err(degenerate)?;
            let circle = Circle2::new(frame, r).map_err(degenerate)?;
            // The end's angle from the start, counterclockwise, in (0, 2π].
            let (dx, dy) = (e.x - c.x, e.y - c.y);
            let mut a = math::atan2(x.x * dy - x.y * dx, x.x * dx + x.y * dy);
            if a <= 0.0 {
                a += TAU;
            }
            (Curve2::Circle2(circle), 0.0, a)
        }
        Geometry::Circle { center, r_m } => {
            let frame = Frame2::new(point(solved, center), Vector2::new(1.0, 0.0), true)
                .map_err(degenerate)?;
            let circle = Circle2::new(frame, *r_m).map_err(degenerate)?;
            (Curve2::Circle2(circle), 0.0, TAU)
        }
        Geometry::Point { .. } => unreachable!("loops hold curves"),
    };
    let (t0, t1) = if reversed { (t1, t0) } else { (t0, t1) };
    Ok(Segment { key, curve, t0, t1 })
}

/// The profile of a solved sketch: every closed loop, or only those that
/// hold one of `chosen`'s entities. Faults are `profile_invalid`, naming
/// the sketch's entities.
pub fn profile(
    part: &Id,
    sketch: &Id,
    solved: &Solved,
    chosen: &[Id],
) -> Result<(ValidProfile, Keys)> {
    let all = &solved.report.loops;
    for c in chosen {
        if !all.iter().any(|l| l.iter().any(|(e, _)| e == c)) {
            return Err(Error::new(
                Code::ProfileInvalid,
                format!("{c} is not on a closed loop of sketch {sketch}"),
            )
            .with("entities", [c]));
        }
    }
    let loops: Vec<&Vec<(Id, bool)>> = all
        .iter()
        .filter(|l| chosen.is_empty() || l.iter().any(|(e, _)| chosen.contains(e)))
        .collect();
    if loops.is_empty() {
        return Err(Error::new(
            Code::ProfileInvalid,
            format!("sketch {sketch} has no closed loop to sweep"),
        )
        .with("entities", Vec::<Id>::new()));
    }
    let mut keys = Keys::default();
    let mut out = Vec::new();
    for l in loops {
        let anchor = anchor(solved, l);
        let lk = entity_key(part, sketch, &anchor);
        keys.loops.insert(lk, anchor);
        let mut segments = Vec::new();
        for (e, reversed) in l {
            if let Geometry::Line { start, end } | Geometry::Arc { start, end, .. } =
                &solved.sketch.entities[e].geometry
            {
                for q in [start, end] {
                    keys.points.insert(q.clone(), point(solved, q));
                }
            }
            let k = entity_key(part, sketch, e);
            keys.segments.insert(k, e.clone());
            segments.push(segment(solved, e, *reversed, SegmentKey(k))?);
        }
        out.push(Loop {
            key: LoopKey(lk),
            segments,
        });
    }
    let frame = Frame::from_rigid(&solved.frame).map_err(|e| {
        Error::new(Code::Internal, "a sketch frame that isn't one").with("kernel", format!("{e:?}"))
    })?;
    let valid = ValidProfile::new(Profile { frame, loops: out })
        .map_err(|e| sketch_invalid(sketch, &keys, e))?;
    Ok((valid, keys))
}

/// The surface profile of a solved sketch (ryucad 18): its closed loops,
/// swept without caps, and its open chains, every one or those holding
/// one of `chosen`'s entities. A chain runs end to end the way its least
/// curve (in [`natural`] order) is drawn, and is keyed over that curve:
/// a curve added at either end has a larger ID, so the key and the way it
/// runs stay, and only its old end's lateral and vertices rename. A chain
/// that branches is `profile_invalid` (`reason: branches`) naming the
/// points where it does.
pub fn surface(
    part: &Id,
    sketch: &Id,
    solved: &Solved,
    chosen: &[Id],
) -> Result<(ValidProfile, Keys)> {
    use ryukei::sweep::{Chain, ChainKey};
    let report = &solved.report;
    let on_loop = |c: &Id| report.loops.iter().any(|l| l.iter().any(|(e, _)| e == c));
    let on_chain = |c: &Id| report.open.iter().any(|(es, _)| es.contains(c));
    for c in chosen {
        if !on_loop(c) && !on_chain(c) {
            return Err(Error::new(
                Code::ProfileInvalid,
                format!("{c} is on no loop or open chain of sketch {sketch}"),
            )
            .with("entities", [c]));
        }
    }
    let picked = |es: &[&Id]| chosen.is_empty() || es.iter().any(|e| chosen.contains(e));
    let mut keys = Keys::default();
    let mut loops = Vec::new();
    for l in report
        .loops
        .iter()
        .filter(|l| picked(&l.iter().map(|(e, _)| e).collect::<Vec<_>>()))
    {
        let anchor = anchor(solved, l);
        let lk = entity_key(part, sketch, &anchor);
        keys.loops.insert(lk, anchor);
        let mut segments = Vec::new();
        for (e, reversed) in l {
            for q in ends_of(solved, e) {
                keys.points.insert(q.clone(), point(solved, q));
            }
            let k = entity_key(part, sketch, e);
            keys.segments.insert(k, e.clone());
            segments.push(segment(solved, e, *reversed, SegmentKey(k))?);
        }
        loops.push(Loop {
            key: LoopKey(lk),
            segments,
        });
    }
    let mut chains = Vec::new();
    for ((entities, _), walk) in report.open.iter().zip(&report.paths) {
        if !picked(&entities.iter().collect::<Vec<_>>()) {
            continue;
        }
        let walk = match walk {
            Ok(w) => w,
            Err(points) => {
                return Err(Error::new(
                    Code::ProfileInvalid,
                    format!(
                        "sketch {sketch}: the open chain of {} branches at {}: a surface sweeps chains that run end to end",
                        entities.iter().map(Id::as_str).collect::<Vec<_>>().join(", "),
                        points.iter().map(Id::as_str).collect::<Vec<_>>().join(", ")
                    ),
                )
                .with("reason", "branches")
                .with("entities", entities)
                .with("points", points));
            }
        };
        let least = walk
            .iter()
            .map(|(e, _)| e)
            .min_by(|a, b| natural(a).cmp(&natural(b)))
            .expect("a chain has curves")
            .clone();
        // Run the least curve's way.
        let backwards = walk.iter().any(|(e, r)| *e == least && *r);
        let walk: Vec<(Id, bool)> = if backwards {
            walk.iter().rev().map(|(e, r)| (e.clone(), !r)).collect()
        } else {
            walk.clone()
        };
        let ck = entity_key(part, sketch, &least);
        keys.chains.insert(ck, least);
        let mut segments = Vec::new();
        for (e, reversed) in &walk {
            for q in ends_of(solved, e) {
                keys.points.insert(q.clone(), point(solved, q));
            }
            let k = entity_key(part, sketch, e);
            keys.segments.insert(k, e.clone());
            segments.push(segment(solved, e, *reversed, SegmentKey(k))?);
        }
        chains.push(Chain {
            key: ChainKey(ck),
            segments,
        });
    }
    if loops.is_empty() && chains.is_empty() {
        return Err(Error::new(
            Code::ProfileInvalid,
            format!("sketch {sketch} has no loop or open chain to sweep"),
        )
        .with("entities", Vec::<Id>::new()));
    }
    let frame = Frame::from_rigid(&solved.frame).map_err(|e| {
        Error::new(Code::Internal, "a sketch frame that isn't one").with("kernel", format!("{e:?}"))
    })?;
    let valid = ValidProfile::surface(Profile { frame, loops }, chains)
        .map_err(|e| sketch_invalid(sketch, &keys, e))?;
    Ok((valid, keys))
}

/// Planar feature `id`'s body: its profile's closed loops as one plane
/// face per lump (`ryukei::sweep::planar`), named as an extrude's start
/// cap would be. A profile with open chains, or with no loops, is
/// `profile_invalid` (`reason: open_profile`) naming the chains' curves.
pub fn planar(
    part: &crate::doc::project::Part,
    id: &Id,
    x: &crate::doc::sweep::Planar,
    before: &dyn crate::rebuild::Results,
) -> Result<crate::rebuild::Built> {
    use crate::doc::geometry::GeomRef;
    use crate::doc::sweep::Source;
    let key = crate::rebuild::op_key(&part.id, id);
    let (valid, keys, sketch) = match &x.profile.source {
        Source::Sketch(sketch) => {
            let solved = crate::rebuild::sketch_input(part, id, sketch, before)?;
            let (v, k) = surface(&part.id, sketch, solved, &x.profile.loops)
                .map_err(|e| e.with("feature", id).with("sketch", sketch))?;
            (v, k, Some(sketch))
        }
        Source::Face(GeomRef::Entity(r)) => {
            let (v, k) = of_face(
                part,
                part.order(),
                id,
                "profile",
                r,
                &x.profile.loops,
                before,
                true,
            )?;
            (v, k, None)
        }
        Source::Face(_) => unreachable!("a face section is an entity, as read"),
    };
    if valid.lumps().is_empty() || !valid.chains().is_empty() {
        let open: Vec<Id> = valid
            .chains()
            .iter()
            .flat_map(|c| c.segments.iter().map(|g| keys.segments[&g.key.0].clone()))
            .collect();
        return Err(Error::new(
            Code::ProfileInvalid,
            format!(
                "planar {id}: a planar face is closed loops' region; its profile has open curves ({}) or no loop",
                open.iter().map(Id::as_str).collect::<Vec<_>>().join(", ")
            ),
        )
        .with("feature", id)
        .with("reason", "open_profile")
        .with("entities", open));
    }
    let made = ryukei::sweep::planar(key, &valid).map_err(|e| {
        Error::new(Code::RebuildFailed, format!("planar {id}: {e}"))
            .with("feature", id)
            .with("kernel", format!("{e:?}"))
    })?;
    // Named as an extrude's start cap: the origins an extrude's profile
    // gives, from the sketch or the face.
    let placeholder = Id::parse("face", "feature").expect("an ID");
    let mut origins = origins(key, sketch.unwrap_or(&placeholder), &valid, &keys);
    for o in origins.values_mut() {
        if sketch.is_none()
            && let Some(m) = o.as_object_mut()
        {
            m.remove("sketch");
            m.insert("face".into(), x.profile.to_json()["face"].clone());
            for (from, to) in [("entity", "edge"), ("entities", "edges")] {
                if let Some(v) = m.remove(from) {
                    m.insert(to.into(), v);
                }
            }
        }
        o["feature"] = json!(id);
    }
    crate::rebuild::finish(id, "planar", made, origins)
}

/// A line's or arc's end points; none for a circle.
fn ends_of<'a>(solved: &'a Solved, e: &Id) -> Vec<&'a Id> {
    match &solved.sketch.entities[e].geometry {
        Geometry::Line { start, end } | Geometry::Arc { start, end, .. } => vec![start, end],
        _ => Vec::new(),
    }
}

/// The key of a face section's edge: SipHash of
/// `ryucad.section\0<part>\0edge\0<name>`, so a face's profile renames
/// nothing when its solid is resized.
pub fn section_edge_key(part: &Id, name: ryukei::name::Name) -> u128 {
    OpKey::from_bytes(format!("ryucad.section\0{part}\0edge\0{name}").as_bytes()).0
}

/// The key of a face section's loop, over its anchor: the least name of
/// its vertices, as a sketch's loop is keyed over its first point.
pub fn section_vertex_key(part: &Id, name: ryukei::name::Name) -> u128 {
    OpKey::from_bytes(format!("ryucad.section\0{part}\0vertex\0{name}").as_bytes()).0
}

/// A face section's names as the IDs [`Keys`] holds: a stable name's 32
/// hex digits are an ID.
fn name_id(n: ryukei::name::Name) -> Id {
    Id::parse(&n.to_string(), "entity").expect("hex is an ID")
}

/// The profile of planar face `face` (in `owner`'s `role`, as `owner`
/// sees it): every loop of the face, or those holding an edge or vertex
/// `chosen` names, in the frame a sketch on the face takes (its outward
/// normal, the part origin projected onto it, the part's `x`). Each
/// segment is its edge's exact curve in that plane, keyed by
/// [`section_edge_key`]; each loop is keyed over its least vertex. A face
/// that isn't planar is `reference_wrong_kind`; loops that make no
/// profile are `profile_invalid`, naming edges. With `surface`, the loops
/// are a surface profile's, swept without caps (ryucad 18).
#[allow(clippy::too_many_arguments)]
pub fn of_face(
    part: &crate::doc::project::Part,
    order: &[Id],
    owner: &Id,
    role: &str,
    face: &crate::doc::reference::Reference,
    chosen: &[Id],
    results: &dyn crate::rebuild::Results,
    sheet: bool,
) -> Result<(ValidProfile, Keys)> {
    use ryukei::geom::{Plane, Surface};
    use ryukei::topo::{EntityId, Sense};
    let text = face.text(&part.id);
    let found = crate::reference::resolve(
        part,
        order,
        owner,
        role,
        crate::reference::Follow::One,
        face,
        results,
    )?;
    crate::reference::plane(part, owner, role, face, found)?;
    let (built, entity) = found;
    let EntityId::Face(f) = entity else {
        unreachable!("plane takes faces only")
    };
    let on = crate::doc::sketch::OnFace {
        face: face.clone(),
        origin: None,
        x: None,
    };
    let rigid = crate::sketch::face_frame(part, order, owner, &on, results)?;
    let frame = Frame::from_rigid(&rigid).map_err(|e| {
        Error::new(Code::Internal, "a face frame that isn't one").with("kernel", format!("{e:?}"))
    })?;
    let surface = Surface::Plane(Plane::new(frame));
    let body = &built.body;
    let invalid = |msg: String, names: &[Id]| {
        Error::new(
            Code::ProfileInvalid,
            format!("{owner}: its {role} {text}: {msg}"),
        )
        .with("feature", owner)
        .with("role", role)
        .with("face", &text)
        .with("entities", names)
    };
    // Each loop's fins, its edges' and vertices' names, and where each
    // vertex lies in the plane.
    let mut loops = Vec::new();
    let mut points = BTreeMap::new();
    for l in body.face_loops(f) {
        let fins: Vec<_> = body.loop_fins(l).collect();
        if fins.is_empty() {
            continue;
        }
        let mut names = Vec::new();
        let mut vertices = Vec::new();
        for &fin in &fins {
            let e = body
                .edge(body.fin(fin).expect("a live fin").edge)
                .expect("a live edge");
            names.push(e.name);
            for v in [e.start, e.end] {
                let v = body.vertex(v).expect("a live vertex");
                vertices.push(v.name);
                let q = frame.to_local(v.point);
                points.insert(name_id(v.name), Point2::new(q.x, q.y));
            }
        }
        loops.push((fins, names, vertices));
    }
    for c in chosen {
        let on_loop = loops.iter().any(|(_, names, vertices)| {
            names
                .iter()
                .chain(vertices.iter())
                .any(|n| n.to_string() == c.as_str())
        });
        if !on_loop {
            return Err(invalid(
                format!("{c} is no edge or vertex of its loops"),
                std::slice::from_ref(c),
            ));
        }
    }
    let mut keys = Keys::default();
    let mut out = Vec::new();
    for (fins, names, vertices) in &loops {
        let picked = chosen.is_empty()
            || names
                .iter()
                .chain(vertices.iter())
                .any(|n| chosen.iter().any(|c| n.to_string() == c.as_str()));
        if !picked {
            continue;
        }
        let anchor = *vertices
            .iter()
            .min_by_key(|n| n.to_string())
            .expect("a loop of fins has vertices");
        for n in vertices {
            let id = name_id(*n);
            if let Some(q) = points.get(&id) {
                keys.points.insert(id, *q);
            }
        }
        let lk = section_vertex_key(&part.id, anchor);
        keys.loops.insert(lk, name_id(anchor));
        let mut segments = Vec::new();
        for &fin in fins {
            let fin = body.fin(fin).expect("a live fin");
            let e = body.edge(fin.edge).expect("a live edge");
            let curve = body.curve(e.curve).expect("a live curve");
            let tol = e.tolerance.get().max(ryukei::tol::LINEAR_RESOLUTION);
            let Some((c2, map)) = ryukei::pcurve::exact(&surface, curve, e.interval, tol) else {
                return Err(invalid(
                    format!("its edge {} doesn't lie in its plane", e.name),
                    &[name_id(e.name)],
                ));
            };
            let at = |t: f64| map[0] + map[1] * t;
            let (a, b) = (at(e.interval[0]), at(e.interval[1]));
            let (t0, t1) = match fin.sense {
                Sense::Forward => (a, b),
                Sense::Reversed => (b, a),
            };
            let k = section_edge_key(&part.id, e.name);
            keys.segments.insert(k, name_id(e.name));
            segments.push(Segment {
                key: SegmentKey(k),
                curve: c2,
                t0,
                t1,
            });
        }
        out.push(Loop {
            key: LoopKey(lk),
            segments,
        });
    }
    let profile = Profile { frame, loops: out };
    let valid = if sheet {
        ValidProfile::surface(profile, Vec::new())
    } else {
        ValidProfile::new(profile)
    };
    let valid = valid.map_err(|e| match e {
        ryukei::Error::InvalidProfile(p) => {
            let entities = keys.of_fault(&p);
            invalid(describe(&p).to_owned(), &entities).with("kernel", format!("{p:?}"))
        }
        e => invalid(e.to_string(), &[]).with("kernel", format!("{e:?}")),
    })?;
    Ok((valid, keys))
}

/// Where each entity of a sweep came from, by the hex of its stable name:
/// a side from its sketch entity, a cap from its loop and end, an edge or
/// vertex from the entities either side of it. These are the names the
/// sweep derives from the profile's keys under `key`; entities a sweep
/// doesn't make simply have no origin.
pub fn origins(
    key: OpKey,
    sketch: &Id,
    valid: &ValidProfile,
    keys: &Keys,
) -> BTreeMap<String, Value> {
    use ryukei::name::{Name, Role, SweepRole as R};
    let derive = |role: R, parents: &[Name]| Name::derive(key, Role::Sweep(role), parents);
    let mut out = BTreeMap::new();
    let mut put = |n: Name, v: Value| {
        out.insert(n.to_string(), v);
    };
    for lump in valid.lumps() {
        let outer = &lump.outer;
        let lid = keys.loops[&outer.key.0].clone();
        let stand = derive(R::Loop, &[Name::from_u128(outer.key.0)]);
        let caps = [
            ("start", derive(R::StartCap, &[stand])),
            ("end", derive(R::EndCap, &[stand])),
        ];
        for (end, cap) in caps {
            put(cap, json!({"sketch": sketch, "cap": end, "loop": lid}));
        }
        for l in std::iter::once(outer).chain(&lump.holes) {
            let n = l.segments.len();
            let ids: Vec<Id> = l
                .segments
                .iter()
                .map(|g| keys.segments[&g.key.0].clone())
                .collect();
            let stands: Vec<Name> = l
                .segments
                .iter()
                .map(|g| derive(R::Segment, &[Name::from_u128(g.key.0)]))
                .collect();
            for (k, e) in ids.iter().enumerate() {
                let side = derive(R::Side, &[stands[k]]);
                put(side, json!({"sketch": sketch, "entity": e}));
                put(
                    derive(R::AxisEdge, &[stands[k]]),
                    json!({"sketch": sketch, "entity": e, "axis": true}),
                );
                for (end, cap) in caps {
                    put(
                        derive(R::CapEdge, &[cap, side]),
                        json!({"sketch": sketch, "entity": e, "cap": end}),
                    );
                }
                // The profile vertex between this curve and the one before.
                let pair = [stands[(k + n - 1) % n], stands[k]];
                let between = [&ids[(k + n - 1) % n], e];
                for (role, extra) in [
                    (R::Lateral, None),
                    (R::Vertex, None),
                    (R::StartVertex, Some(("cap", json!("start")))),
                    (R::EndVertex, Some(("cap", json!("end")))),
                    (R::Apex, Some(("apex", json!(true)))),
                ] {
                    let mut v = json!({"sketch": sketch, "entities": between});
                    if let Some((k, x)) = extra {
                        v[k] = x;
                    }
                    put(derive(role, &pair), v);
                }
            }
        }
    }
    // Open chains (ryucad 18), named as `ryukei::sweep::sheets` names a
    // swept chain: sides from their segments' stand-ins, laterals and
    // vertices from the pair either side (the chain's ends by `ChainEnd`
    // stand-ins), rails as cap edges of the chain's own caps.
    for c in valid.chains() {
        let ch = derive(R::Loop, &[Name::from_u128(c.key.0)]);
        let least = keys.chains[&c.key.0].clone();
        let ends = [0u128, 1].map(|e| derive(R::ChainEnd, &[ch, Name::from_u128(e)]));
        let caps = [
            ("start", derive(R::StartCap, &[ch])),
            ("end", derive(R::EndCap, &[ch])),
        ];
        let ids: Vec<Id> = c
            .segments
            .iter()
            .map(|g| keys.segments[&g.key.0].clone())
            .collect();
        let stands: Vec<Name> = c
            .segments
            .iter()
            .map(|g| derive(R::Segment, &[Name::from_u128(g.key.0)]))
            .collect();
        let n = ids.len();
        for (k, e) in ids.iter().enumerate() {
            let side = derive(R::Side, &[stands[k]]);
            put(side, json!({"sketch": sketch, "entity": e}));
            for (end, cap) in caps {
                put(
                    derive(R::CapEdge, &[cap, side]),
                    json!({"sketch": sketch, "entity": e, "cap": end, "chain": least}),
                );
            }
        }
        // Chain vertex `i`, between curves i - 1 and i, or an end.
        for i in 0..=n {
            let before = if i == 0 { ends[0] } else { stands[i - 1] };
            let after = if i == n { ends[1] } else { stands[i] };
            let v = match i {
                0 => json!({"sketch": sketch, "entity": ids[0], "chain_end": 0}),
                i if i == n => json!({"sketch": sketch, "entity": ids[n - 1], "chain_end": 1}),
                i => json!({"sketch": sketch, "entities": [ids[i - 1], ids[i]]}),
            };
            for (role, extra) in [
                (R::Lateral, None),
                (R::Vertex, None),
                (R::StartVertex, Some(("cap", json!("start")))),
                (R::EndVertex, Some(("cap", json!("end")))),
                (R::Apex, Some(("apex", json!(true)))),
            ] {
                let mut x = v.clone();
                if let Some((k, y)) = extra {
                    x[k] = y;
                }
                put(derive(role, &[before, after]), x);
            }
        }
    }
    out
}

/// The kernel's refusal of a profile, in the sketch's terms:
/// `profile_invalid` naming the entities `keys` gives for its fault.
pub(crate) fn sketch_invalid(sketch: &Id, keys: &Keys, e: ryukei::Error) -> Error {
    match e {
        ryukei::Error::InvalidProfile(p) => {
            let entities = keys.of_fault(&p);
            Error::new(
                Code::ProfileInvalid,
                format!(
                    "sketch {sketch}: {} ({})",
                    describe(&p),
                    entities
                        .iter()
                        .map(Id::as_str)
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            )
            .with("entities", &entities)
            .with("kernel", format!("{p:?}"))
        }
        e => Error::new(Code::ProfileInvalid, format!("sketch {sketch}: {e}"))
            .with("kernel", format!("{e:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::sketch::Sketch;
    use crate::units::Unit;
    use serde_json::json;

    fn id(s: &str) -> Id {
        Id::parse(s, "entity").unwrap()
    }

    fn solved(v: serde_json::Value) -> Solved {
        let mut f = crate::doc::fixtures::rectangle();
        for (k, x) in v.as_object().unwrap() {
            f[k] = x.clone();
        }
        let p = [(
            "width".to_owned(),
            crate::expr::Expr::parse("20 mm").unwrap(),
        )]
        .into();
        let env = crate::expr::Env::new(&p);
        let s: Sketch = match crate::doc::files::Feature::from_json(&f, Unit::Mm, &env).unwrap() {
            crate::doc::files::Feature::Sketch(s) => s,
            _ => unreachable!(),
        };
        crate::sketch::solve(&id("s"), &s).unwrap()
    }

    use crate::sketch::tests::solve_ops;

    fn u_ops(extra: bool) -> serde_json::Value {
        // An open U: l1 up, l2 across drawn backwards, l3 down; and with
        // `extra`, l4 on from d.
        let mut ops = vec![
            json!({"op": "add", "id": "a", "type": "point", "at": [0, 0]}),
            json!({"op": "add", "id": "b", "type": "point", "at": [0, 10]}),
            json!({"op": "add", "id": "c", "type": "point", "at": [10, 10]}),
            json!({"op": "add", "id": "d", "type": "point", "at": [10, 0]}),
            json!({"op": "add", "id": "l1", "type": "line", "start": "a", "end": "b"}),
            json!({"op": "add", "id": "l2", "type": "line", "start": "c", "end": "b"}),
            json!({"op": "add", "id": "l3", "type": "line", "start": "c", "end": "d"}),
        ];
        if extra {
            ops.push(json!({"op": "add", "id": "e", "type": "point", "at": [20, 0]}));
            ops.push(json!({"op": "add", "id": "l4", "type": "line", "start": "d", "end": "e"}));
        }
        json!(ops)
    }

    #[test]
    fn chains_are_keyed_by_their_least_curve_and_run_its_way() {
        let p = Id::parse("p", "part").unwrap();
        let s = solve_ops(u_ops(false));
        let (v, keys) = surface(&p, &id("s"), &s, &[]).unwrap();
        assert!(v.lumps().is_empty());
        assert!(v.is_surface());
        let chain = &v.chains()[0];
        assert_eq!(chain.key.0, entity_key(&p, &id("s"), &id("l1")));
        assert_eq!(keys.chains[&chain.key.0], id("l1"));
        let order: Vec<&Id> = chain
            .segments
            .iter()
            .map(|g| &keys.segments[&g.key.0])
            .collect();
        assert_eq!(order, [&id("l1"), &id("l2"), &id("l3")]);
        // It runs l1's way: from a up to b.
        assert!(chain.vertices[0].x.abs() < 1e-12 && chain.vertices[0].y.abs() < 1e-12);
        assert!((chain.vertices[1].y - 0.01).abs() < 1e-12);
        // Extending it at d with l4 keeps its key.
        let (v2, _) = surface(&p, &id("s"), &solve_ops(u_ops(true)), &[]).unwrap();
        assert_eq!(v2.chains()[0].key, chain.key);
        assert_eq!(v2.chains()[0].segments.len(), 4);
        // A rectangle is a loop, swept without caps.
        let (r, _) = surface(&p, &id("s"), &solved(json!({})), &[]).unwrap();
        assert_eq!((r.lumps().len(), r.chains().len()), (1, 0));
        assert!(r.is_surface());
    }

    #[test]
    fn a_chains_sides_laterals_and_rails_have_origins() {
        use ryukei::name::{Name, Role, SweepRole as R};
        let p = Id::parse("p", "part").unwrap();
        let s = solve_ops(json!([
            {"op": "add", "id": "a", "type": "point", "at": [0, 0]},
            {"op": "add", "id": "b", "type": "point", "at": [10, 0]},
            {"op": "add", "id": "c", "type": "point", "at": [10, 10]},
            {"op": "add", "id": "l1", "type": "line", "start": "a", "end": "b"},
            {"op": "add", "id": "l2", "type": "line", "start": "b", "end": "c"}
        ]));
        let (v, keys) = surface(&p, &id("s"), &s, &[]).unwrap();
        let key = OpKey::from_bytes(b"ryucad.feature\0p\0w");
        let (body, _) = ryukei::sweep::extrude(key, &v, v.frame().z().get(), 0.0, 0.02).unwrap();
        assert_eq!(body.kind(), ryukei::topo::BodyKind::Sheet);
        let o = origins(key, &id("s"), &v, &keys);
        // Every face, edge and vertex the sweep made has an origin.
        let names = body
            .faces()
            .map(|f| body.face(f).unwrap().name)
            .chain(body.edges().map(|e| body.edge(e).unwrap().name))
            .chain(body.vertices().map(|x| body.vertex(x).unwrap().name));
        for n in names {
            assert!(o.contains_key(&n.to_string()), "{n} has none");
        }
        let derive = |r: R, ps: &[Name]| Name::derive(key, Role::Sweep(r), ps);
        let seg = |e: &str| {
            derive(
                R::Segment,
                &[Name::from_u128(entity_key(&p, &id("s"), &id(e)))],
            )
        };
        assert_eq!(
            o[&derive(R::Side, &[seg("l1")]).to_string()],
            json!({"sketch": "s", "entity": "l1"})
        );
        let ch = derive(R::Loop, &[Name::from_u128(v.chains()[0].key.0)]);
        let end0 = derive(R::ChainEnd, &[ch, Name::from_u128(0)]);
        assert_eq!(
            o[&derive(R::Lateral, &[end0, seg("l1")]).to_string()],
            json!({"sketch": "s", "entity": "l1", "chain_end": 0})
        );
    }

    #[test]
    fn a_branching_chain_is_refused_naming_the_point() {
        let p = Id::parse("p", "part").unwrap();
        let s = solve_ops(json!([
            {"op": "add", "id": "a", "type": "point", "at": [0, 0]},
            {"op": "add", "id": "b", "type": "point", "at": [10, 0]},
            {"op": "add", "id": "c", "type": "point", "at": [20, 0]},
            {"op": "add", "id": "d", "type": "point", "at": [10, 10]},
            {"op": "add", "id": "l1", "type": "line", "start": "a", "end": "b"},
            {"op": "add", "id": "l2", "type": "line", "start": "b", "end": "c"},
            {"op": "add", "id": "l3", "type": "line", "start": "b", "end": "d"}
        ]));
        let e = surface(&p, &id("s"), &s, &[]).unwrap_err();
        assert_eq!(e.code, Code::ProfileInvalid);
        assert_eq!(e.details["reason"], "branches");
        assert_eq!(e.details["points"], json!(["b"]));
    }

    #[test]
    fn keys_come_from_ids() {
        let p = Id::parse("p", "part").unwrap();
        let k = entity_key(&p, &id("s"), &id("l1"));
        assert_eq!(k, OpKey::from_bytes(b"ryucad.sketch\0p\0s\0l1").0);
        assert_ne!(k, entity_key(&p, &id("s"), &id("l2")));
    }

    #[test]
    fn a_rectangle_is_one_lump() {
        let s = solved(serde_json::json!({}));
        let p = Id::parse("p", "part").unwrap();
        let (v, keys) = profile(&p, &id("s"), &s, &[]).unwrap();
        assert_eq!(v.lumps().len(), 1);
        assert_eq!(v.lumps()[0].outer.segments.len(), 4);
        assert_eq!(keys.segments.len(), 4);
        assert_eq!(keys.loops.values().collect::<Vec<_>>(), [&id("p1")]);
        // Chosen by any of its entities; the construction diagonal isn't on one.
        assert!(profile(&p, &id("s"), &s, &[id("l3")]).is_ok());
        let e = profile(&p, &id("s"), &s, &[id("l5")]).unwrap_err();
        assert_eq!(
            (e.code, &e.details["entities"]),
            (Code::ProfileInvalid, &json!(["l5"]))
        );
    }

    #[test]
    fn anchors_sort_naturally() {
        let mut ids: Vec<Id> = ["p10", "p2", "a1", "p02", "corner", "p1"].map(id).into();
        ids.sort_by(|a, b| natural(a).cmp(&natural(b)));
        let ids: Vec<&str> = ids.iter().map(Id::as_str).collect();
        assert_eq!(ids, ["a1", "corner", "p1", "p02", "p2", "p10"]);
    }

    #[test]
    fn arcs_and_circles() {
        // A slot: two lines and two half circles, and a hole.
        let s = solved(serde_json::json!({
            "constraints": {},
            "entities": {
                "c1": {"m": [0, 0], "type": "point"},
                "c2": {"m": [0.02, 0], "type": "point"},
                "p1": {"m": [0, -0.005], "type": "point"},
                "p2": {"m": [0.02, -0.005], "type": "point"},
                "p3": {"m": [0.02, 0.005], "type": "point"},
                "p4": {"m": [0, 0.005], "type": "point"},
                "l1": {"end": "p2", "start": "p1", "type": "line"},
                "a1": {"center": "c2", "end": "p3", "start": "p2", "type": "arc"},
                "l2": {"end": "p4", "start": "p3", "type": "line"},
                "a2": {"center": "c1", "end": "p1", "start": "p4", "type": "arc"},
                "h": {"center": "c1", "r_m": 0.002, "type": "circle"}
            }
        }));
        let p = Id::parse("p", "part").unwrap();
        let (v, keys) = profile(&p, &id("s"), &s, &[]).unwrap();
        assert_eq!(v.lumps().len(), 1);
        assert_eq!(v.lumps()[0].holes.len(), 1);
        assert_eq!(keys.segments.len(), 5);
        // The slot is anchored on its first point, the hole on itself.
        let anchors: Vec<&Id> = keys.loops.values().collect();
        assert_eq!(anchors.len(), 2);
        assert!(anchors.contains(&&id("p1")) && anchors.contains(&&id("h")));
        let arc = v.lumps()[0]
            .outer
            .segments
            .iter()
            .find(|seg| keys.segments[&seg.key.0] == id("a1"))
            .unwrap();
        assert!(((arc.t1 - arc.t0).abs() - core::f64::consts::PI).abs() < 1e-12);
    }

    /// Planar face `n` of part `p`'s box `b` as a profile, seen by a
    /// feature `w` listed after it, narrowed to `chosen`.
    fn box_face(n: [f64; 3], chosen: &[&str]) -> (serde_json::Value, Result<(ValidProfile, Keys)>) {
        use crate::sketch::tests::{face_along, with_box};
        let (_dir, mut s, body) = with_box();
        let pid = Id::parse("p", "part").unwrap();
        let part = s.project().part(&pid).unwrap().clone();
        let results = s.rebuilt(&pid).unwrap();
        let face = crate::doc::reference::Reference {
            feature: id("b"),
            name: crate::doc::reference::parse_name(&face_along(&body, n)).unwrap(),
        };
        let order = [id("b"), id("w")];
        let chosen: Vec<Id> = chosen.iter().map(|c| id(c)).collect();
        let r = of_face(
            &part,
            &order,
            &id("w"),
            "profile",
            &face,
            &chosen,
            results,
            false,
        );
        (body, r)
    }

    #[test]
    fn a_planar_face_is_a_profile_keyed_by_its_names() {
        let (body, r) = box_face([0.0, 0.0, 1.0], &[]);
        let (v, keys) = r.unwrap();
        assert_eq!(v.lumps().len(), 1);
        assert_eq!(v.lumps()[0].outer.segments.len(), 4);
        // Each segment is keyed by its edge's name, the loop by its least
        // vertex's.
        let p = Id::parse("p", "part").unwrap();
        for (k, e) in &keys.segments {
            let n = crate::doc::reference::parse_name(e.as_str()).unwrap();
            assert_eq!(*k, section_edge_key(&p, n));
            assert!(
                body["edges"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|x| x["name"] == e.as_str())
            );
        }
        let top: Vec<String> = body["vertices"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|v| v["point"][2].as_f64().unwrap() > 0.004)
            .map(|v| v["name"].as_str().unwrap().to_owned())
            .collect();
        let least = top.iter().min().unwrap();
        assert_eq!(keys.loops.values().collect::<Vec<_>>(), [&id(least)]);
        // Its frame is a sketch's on the face: the outward normal, the
        // part origin projected.
        let f = v.frame();
        assert_eq!(f.z().get().z, 1.0);
        assert_eq!(f.origin().z, 0.005);
        // Narrowed by a vertex of its loop; a name not on it is named.
        let (_, r) = box_face([0.0, 0.0, 1.0], &[least.as_str()]);
        assert_eq!(r.unwrap().0.lumps().len(), 1);
        let (_, r) = box_face([0.0, 0.0, 1.0], &["0123456789abcdef0123456789abcdef"]);
        let e = r.unwrap_err();
        assert_eq!(e.code, Code::ProfileInvalid, "{e}");
        assert_eq!(
            e.details["entities"],
            json!(["0123456789abcdef0123456789abcdef"])
        );
    }

    #[test]
    fn a_face_that_isnt_planar_is_the_wrong_kind() {
        use crate::testing::call;
        let (_dir, mut s) = crate::testing::fresh();
        call(&mut s, "part.create", json!({"part": "p"})).unwrap();
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "cylinder", "id": "b", "params": {"radius": 5, "height": 5}}),
        )
        .unwrap();
        let body = call(&mut s, "inspect", json!({"part": "p", "feature": "b"})).unwrap()["bodies"]
            [0]
        .clone();
        let side = body["faces"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["surface"]["type"] == "cylinder")
            .unwrap()["name"]
            .as_str()
            .unwrap()
            .to_owned();
        let pid = Id::parse("p", "part").unwrap();
        let part = s.project().part(&pid).unwrap().clone();
        let results = s.rebuilt(&pid).unwrap();
        let face = crate::doc::reference::Reference {
            feature: id("b"),
            name: crate::doc::reference::parse_name(&side).unwrap(),
        };
        let order = [id("b"), id("w")];
        let e = of_face(
            &part,
            &order,
            &id("w"),
            "profile",
            &face,
            &[],
            results,
            false,
        )
        .unwrap_err();
        assert_eq!(e.code, Code::ReferenceWrongKind, "{e}");
    }

    #[test]
    fn crossing_curves_are_named() {
        // A bow tie: l2 and l4 cross.
        let s = solved(serde_json::json!({
            "constraints": {},
            "entities": {
                "p1": {"m": [0, 0], "type": "point"},
                "p2": {"m": [0.02, 0], "type": "point"},
                "p3": {"m": [0, 0.01], "type": "point"},
                "p4": {"m": [0.02, 0.01], "type": "point"},
                "l1": {"end": "p2", "start": "p1", "type": "line"},
                "l2": {"end": "p3", "start": "p2", "type": "line"},
                "l3": {"end": "p4", "start": "p3", "type": "line"},
                "l4": {"end": "p1", "start": "p4", "type": "line"}
            }
        }));
        let p = Id::parse("p", "part").unwrap();
        let e = profile(&p, &id("s"), &s, &[]).unwrap_err();
        assert_eq!(e.code, Code::ProfileInvalid);
        assert_eq!(e.details["entities"], json!(["l2", "l4"]));
        assert!(e.message.contains("cross"), "{}", e.message);
    }
}
