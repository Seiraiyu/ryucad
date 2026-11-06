//! Bent sections (design 346): a sketch's one open chain of lines and arcs
//! made into the closed profile of a sheet `t` thick. The chain is one side
//! of the sheet, walked from its end first in natural order; the material
//! lies on its left (`reverse`: its right). Each sharp corner between two
//! lines takes a bend of inner radius `r`, its other side `r + t`; each arc
//! keeps its centre, with a parallel arc `t` away. Lines and circles only,
//! so the profile is exact.
//!
//! Segment keys are SipHash of `ryucad.sheet\0<part>\0<id>\0<role>`: an
//! entity's `inner` (its own side) and `outer` (the side `t` away), a
//! corner's `corner_inner` and `corner_outer` (its arcs of radius `r` and
//! `r + t`) over the point between its lines, an end's cap (`end`) over
//! its point, and the loop over the first end's point (`loop`). So a new
//! thickness, radius or length renames nothing.

use crate::doc::id::Id;
use crate::doc::sketch::{ConstraintType, Geometry};
use crate::error::{Code, Error, Result};
use crate::profile::{Keys, natural};
use crate::sketch::Solved;
use core::f64::consts::{PI, TAU};
use ryukei::geom::{Circle2, Curve2, Frame, Frame2, Line2, Point2, Vector2};
use ryukei::math;
use ryukei::name::OpKey;
use ryukei::sweep::{Loop, LoopKey, Profile, Segment, SegmentKey, ValidProfile};
use std::collections::BTreeMap;

/// Directions within this many radians meet tangentially: the sketch
/// solver holds tangency within `ANGLE_TOL` (1e-12).
pub const TANGENT: f64 = 1e-9;

/// What no piece of a section may be shorter than, nor any radius.
const SHORTEST: f64 = 10.0 * ryukei::tol::LINEAR_RESOLUTION;

/// A piece of the chain as walked.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Piece {
    Line { from: Point2, to: Point2 },
    /// About `centre`, counterclockwise from `from` to `to` when `ccw`.
    Arc { centre: Point2, radius: f64, from: Point2, to: Point2, ccw: bool },
}

impl Piece {
    fn ends(&self) -> (Point2, Point2) {
        match *self {
            Piece::Line { from, to } | Piece::Arc { from, to, .. } => (from, to),
        }
    }

    /// The unit direction of travel at `p`, one of its ends.
    fn direction_at(&self, p: Point2) -> Vector2 {
        match *self {
            Piece::Line { from, to } => unit(to - from),
            Piece::Arc { centre, ccw, .. } => {
                let x = p - centre;
                let h = Vector2::new(-x.y, x.x);
                unit(if ccw { h } else { -h })
            }
        }
    }

    fn reversed(&self) -> Piece {
        match *self {
            Piece::Line { from, to } => Piece::Line { from: to, to: from },
            Piece::Arc { centre, radius, from, to, ccw } => {
                Piece::Arc { centre, radius, from: to, to: from, ccw: !ccw }
            }
        }
    }

    /// The trimmed curve it is.
    fn segment(&self, key: u128) -> Segment {
        match *self {
            Piece::Line { from, to } => Segment {
                key: SegmentKey(key),
                curve: Curve2::Line2(Line2::through(from, to).expect("checked longer than resolution")),
                t0: 0.0,
                t1: from.distance(to),
            },
            Piece::Arc { centre, radius, from, to, ccw } => {
                let x = unit(from - centre);
                let frame = Frame2::new(centre, x, ccw).expect("a direction");
                let circle = Circle2::new(frame, radius).expect("checked longer than resolution");
                let u = unit(to - centre);
                let (cross, dot) = (x.x * u.y - x.y * u.x, x.x * u.x + x.y * u.y);
                let mut a = math::atan2(if ccw { cross } else { -cross }, dot);
                if a <= 0.0 {
                    a += TAU;
                }
                Segment { key: SegmentKey(key), curve: Curve2::Circle2(circle), t0: 0.0, t1: a }
            }
        }
    }
}

fn unit(v: Vector2) -> Vector2 {
    let l = math::hypot(v.x, v.y);
    Vector2::new(v.x / l, v.y / l)
}

fn left(h: Vector2) -> Vector2 {
    Vector2::new(-h.y, h.x)
}

/// The chain walked from its first end: each curve and its piece, and
/// the points between them (`points[k]` is where piece `k` starts, the
/// last where the chain ends).
#[derive(Clone, Debug, PartialEq)]
pub struct Walked {
    pub pieces: Vec<(Id, Piece)>,
    pub points: Vec<Id>,
}

/// One segment of a section's loop: the chain entity or point it is
/// keyed over (`at`), its role, and the segment.
#[derive(Clone, Debug, PartialEq)]
pub struct Offset {
    pub at: Id,
    pub role: &'static str,
    pub segment: Segment,
}

/// The key of a section's segment or loop.
pub fn key(part: &Id, at: &Id, role: &str) -> u128 {
    OpKey::from_bytes(format!("ryucad.sheet\0{part}\0{at}\0{role}").as_bytes()).0
}

fn refused(msg: String, entities: &[&Id]) -> Error {
    Error::new(Code::ProfileInvalid, msg).with("entities", entities)
}

/// The closed loop of a sheet `t` thick along `w`, its inner bend radius
/// `r`, the material on the chain's left when `on_left`: segments in
/// order, keys still to be given (each [`Offset::segment`]'s key is 0).
pub fn offset(w: &Walked, t: f64, r: f64, on_left: bool) -> Result<Vec<Offset>> {
    let s = if on_left { 1.0 } else { -1.0 };
    let n = w.pieces.len();
    // The corner at each joint, if it is one: its turn, the drawn and
    // offset sides' radii, and how much each line loses.
    struct Corner {
        phi: f64,
        drawn: f64,
        other: f64,
        trim: f64,
    }
    let mut corners: Vec<Option<Corner>> = Vec::with_capacity(n.saturating_sub(1));
    for k in 0..n.saturating_sub(1) {
        let ((a, pa), (b, pb)) = (&w.pieces[k], &w.pieces[k + 1]);
        let at = pa.ends().1;
        let (ha, hb) = (pa.direction_at(at), pb.direction_at(pb.ends().0));
        let phi = math::atan2(ha.x * hb.y - ha.y * hb.x, ha.x * hb.x + ha.y * hb.y);
        let lines = matches!(pa, Piece::Line { .. }) && matches!(pb, Piece::Line { .. });
        if phi.abs() <= TANGENT {
            corners.push(None);
        } else if !lines {
            return Err(refused(
                format!("{a} meets {b} at an angle at {}: an arc meets what it joins tangentially, and only two lines take a bend at a sharp corner", w.points[k + 1]),
                &[a, b],
            ));
        } else if phi.abs() >= PI - TANGENT {
            return Err(refused(format!("{b} folds back along {a} at {}", w.points[k + 1]), &[a, b]));
        } else {
            let towards = (phi > 0.0) == (s > 0.0);
            let (drawn, other) = if towards { (r + t, r) } else { (r, r + t) };
            corners.push(Some(Corner { phi, drawn, other, trim: drawn * math::tan(phi.abs() / 2.0) }));
        }
    }
    // Each piece trimmed on its drawn side, and its offset.
    let mut drawn = Vec::new();
    let mut moved = Vec::new();
    for (k, (e, p)) in w.pieces.iter().enumerate() {
        let before = k.checked_sub(1).and_then(|j| corners[j].as_ref()).map_or(0.0, |c| c.trim);
        let after = corners.get(k).and_then(Option::as_ref).map_or(0.0, |c| c.trim);
        match *p {
            Piece::Line { from, to } => {
                let h = unit(to - from);
                if from.distance(to) - before - after < SHORTEST {
                    return Err(refused(
                        format!("the bends at {} and {} leave nothing of {e}: make it longer, or the radius smaller", w.points[k], w.points[k + 1]),
                        &[e],
                    ));
                }
                let (a, b) = (from + h * before, to - h * after);
                let m = left(h) * (s * t);
                drawn.push(Piece::Line { from: a, to: b });
                moved.push(Piece::Line { from: a + m, to: b + m });
            }
            Piece::Arc { centre, radius, from, to, ccw } => {
                let other = if ccw { radius - s * t } else { radius + s * t };
                if other < SHORTEST {
                    return Err(refused(
                        format!("{e} is tighter than the thickness on its inside: its radius must be more than {t} m"),
                        &[e],
                    ));
                }
                let at = |q: Point2| centre + unit(q - centre) * other;
                drawn.push(*p);
                moved.push(Piece::Arc { centre, radius: other, from: at(from), to: at(to), ccw });
            }
        }
    }
    // The drawn side forwards, its corners' arcs between its pieces.
    let mut out = Vec::new();
    let offset = |at: &Id, role: &'static str, p: &Piece| Offset {
        at: at.clone(),
        role,
        segment: p.segment(0),
    };
    let corner_role = |radius: f64| if radius == r { "corner_inner" } else { "corner_outer" };
    let mut arcs = Vec::new();
    for k in 0..n {
        out.push(offset(&w.pieces[k].0, "inner", &drawn[k]));
        if let Some(c) = corners.get(k).and_then(Option::as_ref) {
            let (pa, pb) = (&drawn[k], &drawn[k + 1]);
            let (ea, eb) = (pa.ends().1, pb.ends().0);
            let ha = pa.direction_at(ea);
            let turn = if c.phi > 0.0 { left(ha) } else { -left(ha) };
            let centre = ea + turn * c.drawn;
            let ccw = c.phi > 0.0;
            let d = Piece::Arc { centre, radius: c.drawn, from: ea, to: eb, ccw };
            let (ma, mb) = (moved[k].ends().1, moved[k + 1].ends().0);
            let m = Piece::Arc { centre, radius: c.other, from: ma, to: mb, ccw };
            out.push(offset(&w.points[k + 1], corner_role(c.drawn), &d));
            arcs.push((k, m, corner_role(c.other)));
        }
    }
    // The cap at the chain's end, the offset side backwards, the cap at
    // its start.
    let cap = |a: Point2, b: Point2| Piece::Line { from: a, to: b };
    out.push(offset(&w.points[n], "end", &cap(drawn[n - 1].ends().1, moved[n - 1].ends().1)));
    for k in (0..n).rev() {
        if let Some((_, m, role)) = arcs.iter().find(|(j, _, _)| *j == k) {
            out.push(offset(&w.points[k + 1], role, &m.reversed()));
        }
        out.push(offset(&w.pieces[k].0, "outer", &moved[k].reversed()));
    }
    out.push(offset(&w.points[0], "end", &cap(moved[0].ends().0, drawn[0].ends().0)));
    Ok(out)
}

/// The sketch's one open chain, walked from its end first in natural
/// order. Curves join where they share a point or their points are
/// coincident. A chain that branches, a sketch with two chains, a closed
/// loop or a circle are `profile_invalid`.
pub fn walk(sketch: &Id, solved: &Solved) -> Result<Walked> {
    let sk = &solved.sketch;
    if let Some(l) = solved.report.loops.first() {
        let ids: Vec<&Id> = l.iter().map(|(e, _)| e).collect();
        return Err(refused(
            format!("sketch {sketch} closes a loop: a bent section is one open chain of lines and arcs (a plate is a base flange with no depth)"),
            &ids,
        ));
    }
    let chains = &solved.report.open;
    let [(entities, _)] = chains.as_slice() else {
        let all: Vec<&Id> = chains.iter().flat_map(|(e, _)| e).collect();
        return Err(refused(
            format!("sketch {sketch} has {} open chains: a bent section is one", chains.len()),
            &all,
        ));
    };
    // Points joined by coincidence are one; each group named by its
    // first point in natural order.
    let mut group: BTreeMap<Id, Id> = BTreeMap::new();
    let find = |g: &BTreeMap<Id, Id>, mut p: Id| {
        while let Some(q) = g.get(&p).filter(|q| **q != p) {
            p = q.clone();
        }
        p
    };
    for c in sk.constraints.values() {
        if c.ty == ConstraintType::Coincident {
            let (a, b) = (find(&group, c.entities[0].clone()), find(&group, c.entities[1].clone()));
            if a != b {
                let (lo, hi) = if natural(&a) <= natural(&b) { (a, b) } else { (b, a) };
                group.insert(hi, lo);
            }
        }
    }
    let ends_of = |e: &Id| match &sk.entities[e].geometry {
        Geometry::Line { start, end } | Geometry::Arc { start, end, .. } => {
            Some((find(&group, start.clone()), find(&group, end.clone())))
        }
        _ => None,
    };
    let mut at: BTreeMap<Id, Vec<Id>> = BTreeMap::new();
    for e in entities {
        let (a, b) = ends_of(e).ok_or_else(|| refused(format!("{e} is a circle: a bent section is lines and arcs"), &[e]))?;
        at.entry(a).or_default().push(e.clone());
        at.entry(b).or_default().push(e.clone());
    }
    let mut ends: Vec<&Id> = at.iter().filter(|(_, c)| c.len() == 1).map(|(p, _)| p).collect();
    if ends.len() != 2 || at.values().any(|c| c.len() > 2) {
        let all: Vec<&Id> = entities.iter().collect();
        return Err(refused(format!("sketch {sketch}'s chain branches: a bent section is one chain with two ends"), &all));
    }
    ends.sort_by(|a, b| natural(a).cmp(&natural(b)));
    let point = |p: &Id| match sk.entities[p].geometry {
        Geometry::Point { m } => Point2::new(m[0], m[1]),
        _ => unreachable!("validated: curves join points"),
    };
    let mut here = ends[0].clone();
    let mut prev: Option<Id> = None;
    let mut out = Walked { pieces: Vec::new(), points: vec![here.clone()] };
    while let Some(e) = at[&here].iter().find(|c| prev.as_ref() != Some(*c)).cloned() {
        let (a, b) = ends_of(&e).expect("a line or an arc");
        let forwards = a == here;
        let piece = match &sk.entities[&e].geometry {
            Geometry::Line { start, end } => Piece::Line { from: point(start), to: point(end) },
            Geometry::Arc { center, start, end } => {
                let c = point(center);
                Piece::Arc { centre: c, radius: point(start).distance(c), from: point(start), to: point(end), ccw: true }
            }
            _ => unreachable!("lines and arcs"),
        };
        out.pieces.push((e.clone(), if forwards { piece } else { piece.reversed() }));
        here = if forwards { b } else { a };
        out.points.push(here.clone());
        prev = Some(e);
        if at[&here].len() == 1 {
            break;
        }
    }
    Ok(out)
}

/// A bent section's profile: [`offset`]'s loop in the sketch's frame,
/// keyed, and which entity or point each key came from (`keys`), with its
/// role (`roles`).
pub struct Section {
    pub valid: ValidProfile,
    pub keys: Keys,
    pub roles: BTreeMap<u128, (Id, &'static str)>,
}

pub fn profile(part: &Id, sketch: &Id, solved: &Solved, w: &Walked, t: f64, r: f64, on_left: bool) -> Result<Section> {
    let loop_ = offset(w, t, r, on_left)?;
    let mut keys = Keys::default();
    let mut roles = BTreeMap::new();
    let segments = loop_
        .into_iter()
        .map(|o| {
            let k = key(part, &o.at, o.role);
            keys.segments.insert(k, o.at.clone());
            roles.insert(k, (o.at, o.role));
            Segment { key: SegmentKey(k), ..o.segment }
        })
        .collect();
    let lk = key(part, &w.points[0], "loop");
    keys.loops.insert(lk, w.points[0].clone());
    let frame = Frame::from_rigid(&solved.frame).map_err(|e| {
        Error::new(Code::Internal, "a sketch frame that isn't one").with("kernel", format!("{e:?}"))
    })?;
    let valid = ValidProfile::new(Profile { frame, loops: vec![Loop { key: LoopKey(lk), segments }] })
        .map_err(|e| crate::profile::sketch_invalid(sketch, &keys, e))?;
    Ok(Section { valid, keys, roles })
}

/// Where each entity of a bent section's extrusion came from, by the hex
/// of its name, as [`crate::profile::origins`] gives an extrude's: a side
/// from its chain entity and side (`{entity, side: inner | outer}`), its
/// corner (`{corner, side}`) or its end's cap (`{end}`); a cap from its
/// end of the depth; an edge or vertex from the segments either side.
pub fn origins(key: ryukei::name::OpKey, feature: &Id, sketch: &Id, s: &Section) -> BTreeMap<String, serde_json::Value> {
    use ryukei::name::{Name, Role, SweepRole as R};
    use serde_json::json;
    let derive = |role: R, parents: &[Name]| Name::derive(key, Role::Sweep(role), parents);
    let of = |k: u128| -> serde_json::Value {
        let (at, role) = &s.roles[&k];
        match *role {
            "inner" | "outer" => json!({"entity": at, "side": role}),
            "corner_inner" => json!({"corner": at, "side": "inner"}),
            "corner_outer" => json!({"corner": at, "side": "outer"}),
            _ => json!({"end": at}),
        }
    };
    let with = |mut v: serde_json::Value, extra: serde_json::Value| {
        v["feature"] = json!(feature);
        v["sketch"] = json!(sketch);
        for (k, x) in extra.as_object().into_iter().flatten() {
            v[k] = x.clone();
        }
        v
    };
    let mut out = BTreeMap::new();
    for lump in s.valid.lumps() {
        let outer = &lump.outer;
        let stand = derive(R::Loop, &[Name::from_u128(outer.key.0)]);
        let caps = [("start", derive(R::StartCap, &[stand])), ("end", derive(R::EndCap, &[stand]))];
        for (end, cap) in caps {
            out.insert(cap.to_string(), with(json!({}), json!({"cap": end})));
        }
        let n = outer.segments.len();
        let stands: Vec<Name> = outer.segments.iter().map(|g| derive(R::Segment, &[Name::from_u128(g.key.0)])).collect();
        for (k, g) in outer.segments.iter().enumerate() {
            let side = derive(R::Side, &[stands[k]]);
            out.insert(side.to_string(), with(of(g.key.0), json!({})));
            for (end, cap) in caps {
                out.insert(derive(R::CapEdge, &[cap, side]).to_string(), with(of(g.key.0), json!({"cap": end})));
            }
            let prev = outer.segments[(k + n - 1) % n].key.0;
            let pair = [stands[(k + n - 1) % n], stands[k]];
            let between = json!({"between": [of(prev), of(g.key.0)]});
            for (role, extra) in [
                (R::Lateral, json!({})),
                (R::StartVertex, json!({"cap": "start"})),
                (R::EndVertex, json!({"cap": "end"})),
            ] {
                let mut v = with(between.clone(), json!({}));
                for (k, x) in extra.as_object().into_iter().flatten() {
                    v[k] = x.clone();
                }
                out.insert(derive(role, &pair).to_string(), v);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(s: &str) -> Id {
        Id::parse(s, "entity").unwrap()
    }

    fn line(e: &str, a: [f64; 2], b: [f64; 2]) -> (Id, Piece) {
        (id(e), Piece::Line { from: Point2::new(a[0], a[1]), to: Point2::new(b[0], b[1]) })
    }

    fn walked(pieces: Vec<(Id, Piece)>) -> Walked {
        let points = (0..=pieces.len()).map(|i| id(&format!("p{}", i + 1))).collect();
        Walked { pieces, points }
    }

    /// The signed area of a loop of segments, by sampling each densely.
    fn area(segments: &[Segment]) -> f64 {
        let mut a = 0.0;
        for g in segments {
            let n = 20_000;
            for i in 0..n {
                let t = |k: usize| g.t0 + (g.t1 - g.t0) * k as f64 / n as f64;
                let (p, q) = (g.curve.point(t(i)), g.curve.point(t(i + 1)));
                a += (p.x * q.y - q.x * p.y) / 2.0;
            }
        }
        a
    }

    /// The loop closes: each segment ends where the next starts.
    fn closes(segments: &[Segment]) {
        for (k, g) in segments.iter().enumerate() {
            let next = &segments[(k + 1) % segments.len()];
            let (end, start) = (g.curve.point(g.t1), next.curve.point(next.t0));
            assert!(end.distance(start) < 1e-12, "segment {k} ends {end:?}, the next starts {start:?}");
        }
    }

    /// A sheet's section area: `t` times its straight lengths plus, at
    /// each corner, `t |φ| (r + t / 2)`.
    fn want(t: f64, straight: f64, corners: &[(f64, f64)]) -> f64 {
        t * straight + corners.iter().map(|(phi, r)| t * phi.abs() * (r + t / 2.0)).sum::<f64>()
    }

    #[test]
    fn an_l_turning_away_from_its_material_bends_on_its_drawn_side() {
        // From (40, 0) to the origin, then up: a right turn, the material
        // on the left (below and to the left): the drawn corner is the
        // inner one, radius r.
        let (t, r) = (1.5e-3, 1e-3);
        let w = walked(vec![line("l1", [0.04, 0.0], [0.0, 0.0]), line("l2", [0.0, 0.0], [0.0, 0.03])]);
        let o = offset(&w, t, r, true).unwrap();
        assert_eq!(o.len(), 8);
        let segments: Vec<Segment> = o.iter().map(|x| x.segment.clone()).collect();
        closes(&segments);
        let straight = (0.04 - r) + (0.03 - r);
        let got = area(&segments).abs();
        let want = want(t, straight, &[(core::f64::consts::FRAC_PI_2, r)]);
        assert!((got - want).abs() < 1e-9 * want, "{got} against {want}");
        // The drawn corner is the inner arc, the offset one the outer.
        let roles: Vec<(&str, &str)> = o.iter().map(|x| (x.at.as_str(), x.role)).collect();
        assert_eq!(
            roles,
            [("l1", "inner"), ("p2", "corner_inner"), ("l2", "inner"), ("p3", "end"),
             ("l2", "outer"), ("p2", "corner_outer"), ("l1", "outer"), ("p1", "end")]
        );
    }

    #[test]
    fn a_u_turning_towards_its_material_bends_outside_on_its_drawn_side() {
        // Down, across and up: two left turns, the material inside the U.
        let (t, r) = (2e-3, 1e-3);
        let w = walked(vec![
            line("l1", [0.0, 0.03], [0.0, 0.0]),
            line("l2", [0.0, 0.0], [0.04, 0.0]),
            line("l3", [0.04, 0.0], [0.04, 0.03]),
        ]);
        let o = offset(&w, t, r, true).unwrap();
        let segments: Vec<Segment> = o.iter().map(|x| x.segment.clone()).collect();
        closes(&segments);
        // Each line loses r + t at each corner it meets.
        let straight = (0.03 - (r + t)) + (0.04 - 2.0 * (r + t)) + (0.03 - (r + t));
        let q = core::f64::consts::FRAC_PI_2;
        let want = want(t, straight, &[(q, r), (q, r)]);
        let got = area(&segments).abs();
        assert!((got - want).abs() < 1e-9 * want, "{got} against {want}");
        assert_eq!(o[1].role, "corner_outer");
    }

    #[test]
    fn a_z_bends_both_ways_and_reverse_moves_its_material() {
        let (t, r) = (1e-3, 2e-3);
        let pieces = || {
            vec![
                line("l1", [0.0, 0.0], [0.02, 0.0]),
                line("l2", [0.02, 0.0], [0.02, 0.025]),
                line("l3", [0.02, 0.025], [0.045, 0.025]),
            ]
        };
        let q = core::f64::consts::FRAC_PI_2;
        for left in [true, false] {
            let o = offset(&walked(pieces()), t, r, left).unwrap();
            let segments: Vec<Segment> = o.iter().map(|x| x.segment.clone()).collect();
            closes(&segments);
            // One corner turns towards the material and one away, whichever
            // side it lies on: each line loses r at one and r + t at the other.
            let straight = (0.02 - (r + if left { t } else { 0.0 }))
                + (0.025 - r - (r + t))
                + (0.025 - (r + if left { 0.0 } else { t }));
            let want = want(t, straight, &[(q, r), (-q, r)]);
            let got = area(&segments).abs();
            assert!((got - want).abs() < 1e-9 * want, "{left}: {got} against {want}");
        }
    }

    #[test]
    fn an_arc_keeps_its_centre() {
        // A J: along x, a tangent quarter arc of radius 10 mm turning left,
        // then up.
        let (t, r) = (1e-3, 1e-3);
        let c = Point2::new(0.02, 0.01);
        let arc = (
            id("a1"),
            Piece::Arc { centre: c, radius: 0.01, from: Point2::new(0.02, 0.0),
                         to: Point2::new(0.03, 0.01), ccw: true },
        );
        let w = walked(vec![line("l1", [0.0, 0.0], [0.02, 0.0]), arc,
                            line("l2", [0.03, 0.01], [0.03, 0.04])]);
        let o = offset(&w, t, r, true).unwrap();
        let segments: Vec<Segment> = o.iter().map(|x| x.segment.clone()).collect();
        closes(&segments);
        let q = core::f64::consts::FRAC_PI_2;
        // The arc's offset is inside it (R - t): its area is θ t (R - t / 2).
        let want = t * (0.02 + 0.03) + q * t * (0.01 - t / 2.0);
        let got = area(&segments).abs();
        assert!((got - want).abs() < 1e-9 * want, "{got} against {want}");
        assert_eq!(o.len(), 8);
    }

    #[test]
    fn sections_that_cant_be_made_are_refused_naming_their_entities() {
        let (t, r) = (1e-3, 1e-3);
        let refuse = |w: Walked, entities: &[&str]| {
            let e = offset(&w, t, r, true).unwrap_err();
            assert_eq!(e.code, Code::ProfileInvalid, "{e}");
            assert_eq!(e.details["entities"], serde_json::json!(entities), "{e}");
        };
        // An arc meeting a line at an angle.
        let arc = (id("a1"), Piece::Arc { centre: Point2::new(0.02, 0.01), radius: 0.01,
                   from: Point2::new(0.02, 0.0), to: Point2::new(0.03, 0.01), ccw: true });
        refuse(walked(vec![line("l1", [0.0, -0.01], [0.02, 0.0]), arc]), &["l1", "a1"]);
        // Corners that eat a line: 1.5 mm of line between two bends of 2 mm.
        refuse(
            walked(vec![
                line("l1", [0.0, 0.01], [0.0, 0.0]),
                line("l2", [0.0, 0.0], [0.0015, 0.0]),
                line("l3", [0.0015, 0.0], [0.0015, 0.01]),
            ]),
            &["l2"],
        );
        // An arc tighter than the thickness on its inside.
        let tight = (id("a1"), Piece::Arc { centre: Point2::new(0.0, 0.0005), radius: 0.0005,
                     from: Point2::new(0.0, 0.0), to: Point2::new(0.0005, 0.0005), ccw: true });
        refuse(walked(vec![tight]), &["a1"]);
        // A line folding back on the one before.
        refuse(
            walked(vec![line("l1", [0.0, 0.0], [0.01, 0.0]), line("l2", [0.01, 0.0], [0.0, 0.0])]),
            &["l1", "l2"],
        );
    }

    #[test]
    fn keys_come_from_ids_alone() {
        let part = Id::parse("p", "part").unwrap();
        let k = |e: &str, role: &str| key(&part, &id(e), role);
        assert_ne!(k("l1", "inner"), k("l1", "outer"));
        assert_ne!(k("p2", "corner_inner"), k("l2", "inner"));
        assert_eq!(k("l1", "inner"), k("l1", "inner"));
    }
}
