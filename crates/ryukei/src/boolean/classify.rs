//! Classification: where each piece lies against every other operand.
//!
//! Pieces of one operand joined across an edge that nothing of another
//! operand touches (a piece of the operand's own edge, with no stretch,
//! no other operand's edge and no other operand's face along it) can't be
//! separated by another operand's boundary, so they form a group with one
//! class against each other operand. Each group gets one point inside one
//! of its pieces, just off its longest boundary arc, classified by a
//! certified ray cast ([`point_in_body`]). A piece lying in a face of
//! another operand on the same surface (by the table's coincident pairs)
//! is on that operand, with the same or the opposite outward normal, and
//! needs no ray.

use super::table::{Boundary, GeId, Member, Table};
use super::{FaceRef, Fail};
use crate::geom::{Box3, Point2, Point3, Vector2};
use crate::intersect::{Inclusion, Place, face_box, point_in_body, point_in_face};
use crate::split::Piece;
use crate::topo::{Body, Sense};
use rayon::prelude::*;
use std::collections::BTreeMap;

/// Where a piece lies against another operand.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Class {
    In,
    Out,
    /// In a face of the operand, on one surface; `same` when their
    /// outward normals agree.
    On {
        same: bool,
    },
}

/// Each piece's class against every operand, `None` against its own.
pub(crate) fn classify(
    ops: &[Body],
    table: &Table,
    pieces: &[Piece],
) -> Result<Vec<Vec<Option<Class>>>, Fail> {
    let groups = groups(ops, table, pieces);
    let boxes: Vec<Option<Box3>> = ops
        .iter()
        .map(|b| b.faces().map(|f| face_box(b, f)).reduce(|x, y| x.union(&y)))
        .collect();
    // Points inside pieces, and pieces lying on other operands' faces.
    let on: Vec<BTreeMap<usize, Class>> = pieces
        .par_iter()
        .map(|p| coincident(ops, table, p))
        .collect::<Result<_, _>>()?;
    // One ray cast per group and other operand.
    let mut members: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (i, &g) in groups.iter().enumerate() {
        members.entry(g).or_default().push(i);
    }
    let lists: Vec<Vec<usize>> = members.into_values().collect();
    let found: Vec<Vec<(usize, Vec<Option<Class>>)>> = lists
        .par_iter()
        .map(|list| {
            let classes = group_classes(ops, table, pieces, &on, &boxes, list)?;
            Ok(list.iter().map(|&i| (i, classes.clone())).collect())
        })
        .collect::<Result<_, Fail>>()?;
    let mut out = vec![Vec::new(); pieces.len()];
    for (i, classes) in found.into_iter().flatten() {
        let mut c = classes;
        for (&j, &k) in &on[i] {
            c[j] = Some(k);
        }
        out[i] = c;
    }
    Ok(out)
}

/// Each piece's group, as the least piece index in it.
fn groups(ops: &[Body], table: &Table, pieces: &[Piece]) -> Vec<usize> {
    let mut parent: Vec<usize> = (0..pieces.len()).collect();
    fn find(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
    // Global edges some face of another operand uses.
    let mut used_by: BTreeMap<GeId, Vec<usize>> = BTreeMap::new();
    for (f, w) in &table.faces {
        let ges = w
            .loops
            .iter()
            .flat_map(|l| match l {
                Boundary::Arcs(a) => a.iter().map(|x| x.0).collect(),
                Boundary::Lone(_) => Vec::new(),
            })
            .chain(w.interior.iter().copied());
        for g in ges {
            let list = used_by.entry(g).or_default();
            if !list.contains(&f.op) {
                list.push(f.op);
            }
        }
    }
    let untouched = |g: GeId, op: usize| -> bool {
        let e = &table.edges[g];
        e.members
            .iter()
            .all(|m| matches!(m, Member::Edge(r) if r.op == op))
            && used_by.get(&g).is_none_or(|l| l.iter().all(|&o| o == op))
    };
    let mut by_edge: BTreeMap<GeId, Vec<usize>> = BTreeMap::new();
    for (i, p) in pieces.iter().enumerate() {
        for l in &p.loops {
            if let Boundary::Arcs(a) = l {
                for &(g, _) in a {
                    by_edge.entry(g).or_default().push(i);
                }
            }
        }
    }
    let _ = ops;
    for (g, list) in by_edge {
        for w in list.windows(2) {
            let (a, b) = (w[0], w[1]);
            let op = pieces[a].face.op;
            if pieces[b].face.op != op || !untouched(g, op) {
                continue;
            }
            let (x, y) = (find(&mut parent, a), find(&mut parent, b));
            if x != y {
                parent[x.max(y)] = x.min(y);
            }
        }
    }
    (0..pieces.len()).map(|i| find(&mut parent, i)).collect()
}

/// The classes of a piece lying in faces of other operands on the same
/// surface.
fn coincident(ops: &[Body], table: &Table, p: &Piece) -> Result<BTreeMap<usize, Class>, Fail> {
    let mut out = BTreeMap::new();
    let partners: Vec<(FaceRef, bool)> = table
        .coincident
        .iter()
        .filter_map(|c| {
            if c.a == p.face {
                Some((c.b, c.same))
            } else if c.b == p.face {
                Some((c.a, c.same))
            } else {
                None
            }
        })
        .collect();
    if partners.is_empty() {
        return Ok(out);
    }
    let (q, _) = inner_point(ops, table, p, 0).ok_or(Fail::Uncertain(p.face, p.face))?;
    for (g, same) in partners {
        let gb = &ops[g.op];
        let surface = &gb.surfaces[gb.faces[g.face].surface];
        let (u, v) = surface.project(q).param;
        let place =
            point_in_face(gb, g.face, Point2::new(u, v)).map_err(|_| Fail::Uncertain(p.face, g))?;
        match place {
            Place::Inside => {
                let fs = ops[p.face.op].faces[p.face.face].sense;
                let gs = gb.faces[g.face].sense;
                out.insert(
                    g.op,
                    Class::On {
                        same: same == (fs == gs),
                    },
                );
            }
            Place::Outside => {}
            // The middle of a piece can't be on the other face's boundary:
            // the boundary was imprinted.
            _ => return Err(Fail::Uncertain(p.face, g)),
        }
    }
    Ok(out)
}

/// The classes of a group of pieces against every other operand, from a
/// point inside one of them.
fn group_classes(
    ops: &[Body],
    table: &Table,
    pieces: &[Piece],
    on: &[BTreeMap<usize, Class>],
    boxes: &[Option<Box3>],
    list: &[usize],
) -> Result<Vec<Option<Class>>, Fail> {
    let op = pieces[list[0]].face.op;
    let mut out: Vec<Option<Class>> = vec![None; ops.len()];
    // Points inside the pieces, each found once however many operands it
    // is classified against.
    let mut points: BTreeMap<(usize, usize), Option<Point3>> = BTreeMap::new();
    let mut point = |i: usize, attempt: usize| {
        *points
            .entry((i, attempt))
            .or_insert_with(|| inner_point(ops, table, &pieces[i], attempt).map(|(p, _)| p))
    };
    for j in 0..ops.len() {
        if j == op {
            continue;
        }
        if list.iter().all(|&i| on[i].contains_key(&j)) {
            continue;
        }
        let mut class = None;
        'pieces: for &i in list.iter().filter(|&&i| !on[i].contains_key(&j)) {
            for attempt in 0..4 {
                let Some(p) = point(i, attempt) else {
                    continue;
                };
                if boxes[j].is_none_or(|b| !b.grow(1e-6).intersects(&Box3::from_point(p))) {
                    class = Some(Class::Out);
                    break 'pieces;
                }
                match point_in_body(&ops[j], p) {
                    Ok(Inclusion::Inside) => {
                        class = Some(Class::In);
                        break 'pieces;
                    }
                    Ok(Inclusion::Outside) => {
                        class = Some(Class::Out);
                        break 'pieces;
                    }
                    Ok(Inclusion::Boundary) | Err(_) => {}
                }
            }
        }
        let face = pieces[list[0]].face;
        let other = ops[j]
            .faces()
            .next()
            .map_or(face, |f| FaceRef { op: j, face: f });
        out[j] = Some(class.ok_or(Fail::Uncertain(face, other))?);
    }
    Ok(out)
}

/// A point inside a piece, a little way off one of its boundary arcs to
/// the piece's side: off the longest arc of its first loop, at a distance
/// that shrinks with `attempt` and stays under half the way to the rest
/// of the boundary, so it can't cross into another region however the
/// surface's parameters are scaled; or the lone vertex of a piece with
/// only lone vertices. Also returns its parameters.
pub(crate) fn inner_point(
    ops: &[Body],
    table: &Table,
    p: &Piece,
    attempt: usize,
) -> Option<(Point3, Point2)> {
    let b = &ops[p.face.op];
    let face = &b.faces[p.face.face];
    let surface = &b.surfaces[face.surface];
    let sigma = if face.sense == Sense::Forward {
        1.0
    } else {
        -1.0
    };
    // Arcs of every loop, longest first by their pcurves' chords, with
    // their places in the loops.
    let mut arcs: Vec<(f64, GeId, Sense, (usize, usize))> = Vec::new();
    for (li, l) in p.loops.iter().enumerate() {
        if let Boundary::Arcs(a) = l {
            for (ai, &(g, s)) in a.iter().enumerate() {
                let e = &table.edges[g];
                let (c, map) = &e.pcurves[&p.face];
                let [s0, s1] = e.interval.map(|t| map[0] + map[1] * t);
                let len = (0..8)
                    .map(|k| {
                        let at = |k: i32| c.point(s0 + (s1 - s0) * f64::from(k) / 8.0);
                        at(k).distance(at(k + 1))
                    })
                    .sum();
                arcs.push((len, g, s, (li, ai)));
            }
        }
    }
    if arcs.is_empty() {
        // A whole closed face: any point of it.
        let [[u0, u1], [v0, v1]] = surface.domain();
        let u = if u0.is_finite() && u1.is_finite() {
            0.37 * u0 + 0.63 * u1
        } else {
            0.0
        };
        let v = if v0.is_finite() && v1.is_finite() {
            0.41 * v0 + 0.59 * v1
        } else {
            0.0
        };
        return Some((surface.point(u, v), Point2::new(u, v)));
    }
    arcs.sort_by(|x, y| y.0.total_cmp(&x.0));
    let (_, g, s, place) = arcs[attempt % arcs.len()];
    let e = &table.edges[g];
    let (c, map) = &e.pcurves[&p.face];
    let [s0, s1] = e.interval.map(|t| map[0] + map[1] * t);
    let (a, z) = match s {
        Sense::Forward => (s0, s1),
        Sense::Reversed => (s1, s0),
    };
    let mid = c.point((a + z) / 2.0);
    let ev = c.eval((a + z) / 2.0);
    let dir = if z >= a { 1.0 } else { -1.0 };
    let t = ev.d1 * dir;
    let n = t.length();
    if n == 0.0 {
        return None;
    }
    // The piece is on the left of its loops (on the right on a reversed
    // face).
    let left = Vector2::new(-t.y, t.x) / n * sigma;
    // Distances along the surface, never in its parameters: a step
    // along the parameters is no step along the surface (a band round a
    // small cylinder is far shorter across, in metres, than a fraction of
    // its length round, in radians), and a pcurve may lie a period round
    // from its neighbour's. The distance from `x` to an arc's edge:
    let from = |h: GeId, x: Point3| {
        let e = &table.edges[h];
        let [a, b] = e.interval;
        let (lo, hi) = (a.min(b), a.max(b));
        let foot = e.curve.project(x);
        let t = match e.curve.period() {
            Some(p) => lo + (foot.param - lo).rem_euclid(p),
            None => foot.param,
        };
        if t >= lo && t <= hi {
            foot.distance
        } else {
            e.curve
                .point(lo)
                .distance(x)
                .min(e.curve.point(hi).distance(x))
        }
    };
    let boundary = || {
        p.loops.iter().enumerate().flat_map(|(li, l)| match l {
            Boundary::Arcs(a) => a
                .iter()
                .enumerate()
                .map(move |(ai, &(h, _))| ((li, ai), h))
                .collect(),
            Boundary::Lone(_) => Vec::new(),
        })
    };
    // The step reaches less than half the way, along the surface, to the
    // rest of the boundary, so it never crosses it: to the other arcs, and
    // to the arc's own stretches away from its middle (the far side of a
    // loop that is a single closed arc).
    let at = surface.eval(mid.x, mid.y);
    // Off the arc at right angles on the surface, not in the parameters,
    // which may stretch one way far more than the other: the parameter
    // direction whose image is the surface's normal crossed with the
    // arc's tangent, by the first fundamental form, on the piece's side.
    // Where the form degenerates (a pole) the parameters' own right angle
    // stands in.
    let tangent = at.du * t.x + at.dv * t.y;
    let across = at.du.cross(at.dv).cross(tangent);
    let (e, f, h) = (at.du.dot(at.du), at.du.dot(at.dv), at.dv.dot(at.dv));
    let det = e * h - f * f;
    let (bu, bv) = (at.du.dot(across), at.dv.dot(across));
    let mut off = if det > 1e-12 * e * h {
        Vector2::new((h * bu - f * bv) / det, (e * bv - f * bu) / det)
    } else {
        left
    };
    let side = |w: Vector2| t.x * w.y - t.y * w.x;
    if side(off) * side(left) < 0.0 {
        off = -off;
    }
    let speed = (at.du * off.x + at.dv * off.y).length();
    let own = &table.edges[g];
    let [i0, i1] = own.interval;
    let along = |k: i32| own.curve.point(i0 + (i1 - i0) * f64::from(k) / 16.0);
    let length: f64 = (0..16).map(|k| along(k).distance(along(k + 1))).sum();
    let ends = (0..=4)
        .chain(12..=16)
        .map(|k| along(k).distance(at.point))
        .fold(f64::INFINITY, f64::min);
    // The arc's own edge may bound the piece again from its other side (a
    // torus band cut along one circle): no step off one side reaches it.
    let room = boundary()
        .filter(|&(w, h)| w != place && h != g)
        .map(|(_, h)| from(h, at.point))
        .fold(ends, f64::min);
    let step = (length * [1e-2, 1e-3, 1e-4, 3e-3][attempt.min(3)]).min(room * 0.5);
    let delta = step / speed;
    if !delta.is_finite() || delta <= 0.0 {
        return None;
    }
    let q = mid + off * delta;
    let point = surface.point(q.x, q.y);
    // Nothing of the piece's boundary may come nearer than half the step.
    let moved = point.distance(at.point);
    if boundary().any(|(_, h)| from(h, point) < moved * 0.5) {
        return None;
    }
    Some((point, q))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::boolean::table::build;
    use crate::geom::{Rigid, Surface, Vector3};
    use crate::name::OpKey;
    use crate::primitive;
    use crate::split::split_all;

    fn at(x: f64, y: f64, z: f64) -> Rigid {
        Rigid::translation(Vector3::new(x, y, z))
    }

    /// Every piece of operand `op` on a surface passing `test`, with its
    /// class against the other operand of two.
    fn classes(ops: &[Body], op: usize, test: impl Fn(&Surface) -> bool) -> Vec<Class> {
        let t = build(ops).unwrap();
        let pieces = split_all(ops, &t).unwrap();
        let c = classify(ops, &t, &pieces).unwrap();
        let mut out: Vec<Class> = pieces
            .iter()
            .zip(&c)
            .filter(|(p, _)| {
                let b = &ops[p.face.op];
                p.face.op == op && test(&b.surfaces[b.faces[p.face.face].surface])
            })
            .map(|(_, c)| c[1 - op].unwrap())
            .collect();
        out.sort_by_key(|c| format!("{c:?}"));
        out
    }

    #[test]
    fn a_rod_through_a_plate() {
        let plate = primitive::block(OpKey(1), 4.0, 4.0, 1.0, &Rigid::IDENTITY)
            .unwrap()
            .0;
        let rod = primitive::cylinder(OpKey(2), 0.5, 3.0, &at(2.0, 2.0, -1.0))
            .unwrap()
            .0;
        let ops = [plate, rod];
        let planes = |s: &Surface| matches!(s, Surface::Plane(_));
        // The plate's top and bottom: a disc in the rod, the rest out; its
        // four sides out.
        let got = classes(&ops, 0, planes);
        assert_eq!(got.iter().filter(|&&c| c == Class::In).count(), 2);
        assert_eq!(got.iter().filter(|&&c| c == Class::Out).count(), 6);
        // The rod's side: the band in the plate, the two outside; its caps
        // out.
        let side = classes(&ops, 1, |s| matches!(s, Surface::Cylinder(_)));
        assert_eq!(side, [Class::In, Class::Out, Class::Out]);
        assert_eq!(classes(&ops, 1, planes), [Class::Out, Class::Out]);
    }

    #[test]
    fn flush_faces_are_on() {
        // B on A's top, half over its edge: B's bottom lies partly on A's
        // top, facing the other way; A's top partly under B.
        let a = primitive::block(OpKey(1), 2.0, 2.0, 1.0, &Rigid::IDENTITY)
            .unwrap()
            .0;
        let b = primitive::block(OpKey(2), 2.0, 2.0, 1.0, &at(1.0, 0.0, 1.0))
            .unwrap()
            .0;
        let ops = [a, b];
        let top = |s: &Surface| {
            matches!(s, Surface::Plane(_))
                && s.distance(crate::geom::Point3::new(0.0, 0.0, 1.0)) < 1e-12
                && s.frame().unwrap().z().get().z.abs() > 0.5
        };
        let mut got = classes(&ops, 0, top);
        got.sort_by_key(|c| format!("{c:?}"));
        assert_eq!(got, [Class::On { same: false }, Class::Out]);
        let got = classes(&ops, 1, top);
        assert_eq!(got, [Class::On { same: false }, Class::Out]);
        // Side by side, flush tops: on with the same normal.
        let c = primitive::block(OpKey(3), 2.0, 2.0, 1.0, &at(1.0, 1.0, 0.0))
            .unwrap()
            .0;
        let ops = [ops[0].clone(), c];
        let got = classes(&ops, 0, top);
        assert_eq!(got, [Class::On { same: true }, Class::Out]);
    }

    #[test]
    fn a_sphere_inside_a_block() {
        let block = primitive::block(OpKey(1), 4.0, 4.0, 4.0, &at(-2.0, -2.0, -2.0))
            .unwrap()
            .0;
        let sphere = primitive::sphere(OpKey(2), 1.0, &Rigid::IDENTITY)
            .unwrap()
            .0;
        let ops = [block, sphere];
        assert_eq!(classes(&ops, 1, |_| true), [Class::In]);
        let all = classes(&ops, 0, |_| true);
        assert_eq!(all.len(), 6);
        assert!(all.iter().all(|&c| c == Class::Out));
    }
}
