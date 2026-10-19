//! MEF and KEF: an edge that splits a face in two.

use super::{Corner, check_interval, finish, live, next_id};
use crate::name::{Event, Journal, OpKey, Role};
use crate::tol::Tolerance;
use crate::topo::{
    Body, CurveId, Edge, EdgeId, EntityId, Face, FaceId, Fin, FinId, Id, Loop, LoopId, Sense,
};
use crate::{Error, TopologyError};

/// What [`mef`] made. `fins[0]` runs along the edge, `fins[1]` against it;
/// `faces[i]` is the face holding `fins[i]` (one of them is the old face).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mef {
    pub edge: EdgeId,
    pub face: FaceId,
    pub loop_: LoopId,
    pub fins: [FinId; 2],
    pub faces: [FaceId; 2],
}

/// Make edge, face: joins the vertices of two corners of one loop with a
/// new edge from `from` to `to`, splitting the loop and its face.
///
/// The old loop keeps the side that holds its first fin; with no such fin
/// (a lone-vertex loop, or both corners the same) it keeps the side
/// against the edge. The other side becomes the one loop of a new face on
/// the same surface, with the same sense. Other loops stay on the old face.
/// Two lone corners of the same loop make a closed edge.
///
/// Names: the old face's name retires; the face along the edge is
/// `(key, Part(0), [face])`, the face against it `(key, Part(1), [face])`,
/// and the edge `(key, NewEdge, [face, from, to])`.
pub fn mef(
    body: &mut Body,
    key: OpKey,
    from: Corner,
    to: Corner,
    curve: CurveId,
    interval: [f64; 2],
) -> Result<(Mef, Journal), Error> {
    let (lp, va) = body.corner(from)?;
    let (lp2, vb) = body.corner(to)?;
    if lp != lp2 {
        return Err(TopologyError::WrongLoop.into());
    }
    live(&body.curves, curve)?;
    let interval = check_interval(interval)?;
    // Against the edge: a .. b.prev; along it: b .. a.prev.
    let (against, along) = match (from, to) {
        (Corner::Before(a), Corner::Before(b)) if a == b => (body.fins_between(a, a), Vec::new()),
        (Corner::Before(a), Corner::Before(b)) => {
            (body.fins_between(a, b), body.fins_between(b, a))
        }
        _ => (Vec::new(), Vec::new()),
    };
    let face = body.loops[lp].face;
    let (edge, new_face, new_loop) = (
        next_id(&body.edges),
        next_id(&body.faces),
        next_id(&body.loops),
    );
    let fwd = next_id(&body.fins);
    let rev = <FinId as Id>::from_index(fwd.index() + 1);
    let parent = body.faces[face].name;
    let parents = [parent, body.vertices[va].name, body.vertices[vb].name];
    let edge_name = body.derive_name("mef", key, Role::NewEdge, &parents);
    let part0 = body.derive_name("mef", key, Role::Part(0), &[parent]);
    let part1 = body.derive_name("mef", key, Role::Part(1), &[parent]);
    let old_keeps_along = matches!(body.loops[lp].first_fin, Some(f) if along.contains(&f));
    let (along_face, against_face) = if old_keeps_along {
        (face, new_face)
    } else {
        (new_face, face)
    };
    let f = &body.faces[face];
    let (shell, surface, sense) = (f.shell, f.surface, f.sense);
    body.edges.push(Edge {
        fin: fwd,
        start: va,
        end: vb,
        curve,
        interval,
        tolerance: Tolerance::RESOLUTION,
        name: edge_name,
    });
    for (fin, sense, around) in [(fwd, Sense::Forward, rev), (rev, Sense::Reversed, fwd)] {
        body.fins.push(Fin {
            loop_: lp,
            edge,
            sense,
            next: fin,
            prev: fin,
            around,
            pcurve: None,
        });
    }
    body.faces.push(Face {
        shell,
        next: new_face,
        prev: new_face,
        first_loop: None,
        surface,
        sense,
        name: if old_keeps_along { part1 } else { part0 },
    });
    body.loops.push(Loop {
        face: new_face,
        next: new_loop,
        prev: new_loop,
        first_fin: None,
        vertex: None,
    });
    body.link_face(shell, new_face);
    body.link_loop(new_face, new_loop);
    let along_ring: Vec<FinId> = along.into_iter().chain([fwd]).collect();
    let against_ring: Vec<FinId> = against.into_iter().chain([rev]).collect();
    let (old_ring, new_ring, new_first) = if old_keeps_along {
        (along_ring, against_ring, rev)
    } else {
        (against_ring, along_ring, fwd)
    };
    body.ring_fins(lp, &old_ring);
    body.ring_fins(new_loop, &new_ring);
    body.loops[new_loop].first_fin = Some(new_first);
    if body.loops[lp].first_fin.is_none() {
        // A lone vertex: it is now on the closed edge.
        body.loops[lp].first_fin = Some(old_ring[0]);
        body.loops[lp].vertex = None;
        body.vertices[va].lone = None;
        body.vertices[va].edge = Some(edge);
    }
    body.rename(
        EntityId::Face(face),
        if old_keeps_along { part0 } else { part1 },
    );
    body.index_name(EntityId::Face(new_face));
    body.index_name(EntityId::Edge(edge));
    let mut journal = Journal::new();
    journal.push(Event::Split {
        parent,
        children: [part0, part1],
    });
    journal.push(Event::Created(edge_name));
    finish(body);
    Ok((
        Mef {
            edge,
            face: new_face,
            loop_: new_loop,
            fins: [fwd, rev],
            faces: [along_face, against_face],
        },
        journal,
    ))
}

/// What [`kef`] kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Kef {
    pub face: FaceId,
}

/// Kill edge, face: the inverse of [`mef`]. The edge's two fins must be on
/// different faces. The face with the higher ID dies: its loop merges into
/// the other face's loop around the edge, and its other loops move to the
/// kept face. Fins that move to a face on another surface lose their
/// pcurves.
///
/// Names: both faces' names retire; the kept face is
/// `(key, Merged, [a, b])` with the two names sorted.
pub fn kef(body: &mut Body, key: OpKey, edge: EdgeId) -> Result<(Kef, Journal), Error> {
    let [f1, f2] = body.two_fins(edge)?;
    let face_of = |f: FinId| body.loops[body.fins[f].loop_].face;
    if face_of(f1) == face_of(f2) {
        return Err(TopologyError::WrongFace.into());
    }
    let (kx, ky) = if face_of(f1) < face_of(f2) {
        (f1, f2)
    } else {
        (f2, f1)
    };
    let (keep, gone) = (face_of(kx), face_of(ky));
    let (lk, ld) = (body.fins[kx].loop_, body.fins[ky].loop_);
    let names = [body.faces[keep].name, body.faces[gone].name];
    let merged = body.derive_name("kef", key, Role::Merged, &sorted(names));
    let mut journal = Journal::new();
    journal.push(Event::merged(names[0], names[1], merged));
    journal.push(Event::Deleted(body.edges[edge].name));
    // ... kx.prev, ky.next .. ky.prev, kx.next ...
    let mut ring = body.fins_between(ky, ky);
    ring.remove(0);
    let mut kept = body.fins_between(kx, kx);
    kept.remove(0);
    ring.extend(kept);
    let other_loops: Vec<LoopId> = body.face_loops(gone).filter(|&l| l != ld).collect();
    if body.faces[keep].surface != body.faces[gone].surface {
        for l in other_loops.iter().copied().chain([ld]) {
            body.clear_pcurves(l);
        }
    }
    let (start, end) = (body.edges[edge].start, body.edges[edge].end);
    body.unlink_loop(ld);
    body.loops.kill(ld);
    for l in other_loops {
        body.unlink_loop(l);
        body.link_loop(keep, l);
    }
    body.kill_edge(edge, [kx, ky]);
    if ring.is_empty() {
        // Only a closed edge bounded the faces: its vertex is left alone.
        body.loops[lk].first_fin = None;
        body.loops[lk].vertex = Some(start);
        body.vertices[start].edge = None;
        body.vertices[start].lone = Some(lk);
    } else {
        body.ring_fins(lk, &ring);
        if !matches!(body.loops[lk].first_fin, Some(f) if ring.contains(&f)) {
            body.loops[lk].first_fin = Some(ring[0]);
        }
        for v in [start, end] {
            if body.vertices[v].edge == Some(edge) {
                let at = ring.iter().find(|&&f| body.start_of(f) == v);
                body.vertices[v].edge = at.map(|&f| body.fins[f].edge);
            }
        }
    }
    body.kill_face(gone);
    body.rename(EntityId::Face(keep), merged);
    finish(body);
    Ok((Kef { face: keep }, journal))
}

fn sorted<T: Ord>(mut pair: [T; 2]) -> [T; 2] {
    pair.sort();
    pair
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::euler::tests::{K, counts, line, plane_body, shape};
    use crate::euler::{Mev, mev, mvfs};
    use crate::geom::{Circle, Curve, Frame, Point3};
    use crate::name::Name;

    fn p(x: f64, y: f64) -> Point3 {
        Point3::new(x, y, 0.0)
    }

    /// The path (0,0) -> (1,0) -> (1,1) -> (0,1) as three MEVs.
    fn path() -> (Body, [Mev; 3]) {
        let (mut b, s) = plane_body();
        let (m, _) = mvfs(&mut b, K, p(0.0, 0.0), s).unwrap();
        let pts = [p(0.0, 0.0), p(1.0, 0.0), p(1.0, 1.0), p(0.0, 1.0)];
        let mut at = Corner::Lone(m.loop_);
        let mut made = Vec::new();
        for i in 0..3 {
            let (c, t) = line(&mut b, pts[i], pts[i + 1]);
            let (e, _) = mev(&mut b, OpKey(i as u128 + 1), at, pts[i + 1], c, t).unwrap();
            at = Corner::Before(e.fins[1]);
            made.push(e);
        }
        (b, [made[0], made[1], made[2]])
    }

    /// The path closed into a square: two faces, one on each side.
    fn square() -> (Body, [Mev; 3], Mef) {
        let (mut b, e) = path();
        let (c, t) = line(&mut b, p(0.0, 1.0), p(0.0, 0.0));
        let from = Corner::Before(e[2].fins[1]);
        let to = Corner::Before(e[0].fins[0]);
        let (m, _) = mef(&mut b, OpKey(9), from, to, c, t).unwrap();
        (b, e, m)
    }

    fn circle(b: &mut Body) -> (CurveId, [f64; 2]) {
        let c = Circle::new(Frame::WORLD, 1.0).unwrap();
        (b.add_curve(Curve::Circle(c)), [0.0, core::f64::consts::TAU])
    }

    #[test]
    fn mef_closes_a_square() {
        let (b, e, m) = square();
        assert_eq!(b.counts(), counts(4, 4, 2, 2));
        // The old loop held e[0]'s outgoing fin, which is along the edge.
        let along = [e[0].fins[0], e[1].fins[0], e[2].fins[0], m.fins[0]];
        let old = b.fin(e[0].fins[0]).unwrap().loop_;
        assert_eq!(b.loop_fins(old).collect::<Vec<_>>(), along);
        assert_eq!(
            b.loop_fins(m.loop_).collect::<Vec<_>>(),
            [m.fins[1], e[2].fins[1], e[1].fins[1], e[0].fins[1]]
        );
        assert_eq!(m.faces, [b.loop_(old).unwrap().face, m.face]);
        for l in b.loops() {
            let fins: Vec<_> = b.loop_fins(l).collect();
            for (k, &f) in fins.iter().enumerate() {
                assert_eq!(b.fin_end(f), b.fin_start(fins[(k + 1) % 4]));
            }
        }
        for v in b.vertices() {
            assert_eq!(b.vertex_edges(v).len(), 2);
        }
    }

    #[test]
    fn mef_names_the_parts_by_side() {
        let (mut b, e) = path();
        let (c, t) = line(&mut b, p(0.0, 1.0), p(0.0, 0.0));
        let face = b.loop_(b.fin(e[0].fins[0]).unwrap().loop_).unwrap().face;
        let parent = b.name_of(EntityId::Face(face)).unwrap();
        let from = Corner::Before(e[2].fins[1]);
        let to = Corner::Before(e[0].fins[0]);
        let (m, j) = mef(&mut b, OpKey(9), from, to, c, t).unwrap();
        let part = |i| Name::derive(OpKey(9), Role::Part(i), &[parent]);
        assert_eq!(
            j.events()[0],
            Event::Split {
                parent,
                children: [part(0), part(1)]
            }
        );
        assert_eq!(b.name_of(EntityId::Face(m.faces[0])), Some(part(0)));
        assert_eq!(b.name_of(EntityId::Face(m.faces[1])), Some(part(1)));
        assert_eq!(b.by_name(parent), None);
    }

    #[test]
    fn closed_edges_from_a_lone_vertex_and_from_a_corner() {
        let (mut b, s) = plane_body();
        let (v, _) = mvfs(&mut b, K, p(1.0, 0.0), s).unwrap();
        let (c, t) = circle(&mut b);
        let lone = Corner::Lone(v.loop_);
        let (m, _) = mef(&mut b, OpKey(2), lone, lone, c, t).unwrap();
        assert_eq!(b.counts(), counts(1, 1, 2, 2));
        let e = b.edge(m.edge).unwrap();
        assert_eq!((e.start, e.end), (v.vertex, v.vertex));
        // The old loop kept the side against the edge.
        assert_eq!(b.loop_fins(v.loop_).collect::<Vec<_>>(), [m.fins[1]]);
        assert_eq!(b.loop_fins(m.loop_).collect::<Vec<_>>(), [m.fins[0]]);
        assert_eq!(b.vertex(v.vertex).unwrap().edge, Some(m.edge));
        // From a corner of the square back to itself: the new face is
        // bounded by the new edge alone.
        let (mut b, e, _) = square();
        let (c, t) = circle(&mut b);
        let corner = e[1].fins[0];
        let at = Corner::Before(corner);
        let (m, _) = mef(&mut b, OpKey(3), at, at, c, t).unwrap();
        assert_eq!(b.loop_fins(m.loop_).collect::<Vec<_>>(), [m.fins[0]]);
        assert_eq!(b.loop_fins(b.fin(corner).unwrap().loop_).count(), 5);
    }

    #[test]
    fn kef_undoes_mef() {
        let (b, e) = path();
        let mut sq = b.clone();
        let (c, t) = line(&mut sq, p(0.0, 1.0), p(0.0, 0.0));
        let (m, _) = mef(
            &mut sq,
            OpKey(9),
            Corner::Before(e[2].fins[1]),
            Corner::Before(e[0].fins[0]),
            c,
            t,
        )
        .unwrap();
        let (k, j) = kef(&mut sq, OpKey(10), m.edge).unwrap();
        assert_eq!(k.face, m.faces[0].min(m.faces[1]));
        assert_eq!(j.events().len(), 2);
        assert_eq!(shape(&sq), shape(&b));
        // The lone-vertex closed edge and the single-corner closed edge.
        let (mut b, s) = plane_body();
        let (v, _) = mvfs(&mut b, K, p(1.0, 0.0), s).unwrap();
        let before = shape(&b);
        let (c, t) = circle(&mut b);
        let lone = Corner::Lone(v.loop_);
        let (m, _) = mef(&mut b, OpKey(2), lone, lone, c, t).unwrap();
        kef(&mut b, OpKey(3), m.edge).unwrap();
        assert_eq!(shape(&b), before);
        assert_eq!(b.vertex(v.vertex).unwrap().lone, Some(v.loop_));
        let (mut b, e, _) = square();
        let before = shape(&b);
        let (c, t) = circle(&mut b);
        let at = Corner::Before(e[1].fins[0]);
        let (m, _) = mef(&mut b, OpKey(3), at, at, c, t).unwrap();
        kef(&mut b, OpKey(4), m.edge).unwrap();
        assert_eq!(shape(&b), before);
    }

    #[test]
    fn kef_merges_names() {
        let (mut b, _, m) = square();
        let [a, c] = m.faces.map(|f| b.name_of(EntityId::Face(f)).unwrap());
        let (k, j) = kef(&mut b, OpKey(5), m.edge).unwrap();
        let merged = Name::derive(OpKey(5), Role::Merged, &sorted([a, c]));
        assert_eq!(j.events()[0], Event::merged(a, c, merged));
        assert_eq!(b.name_of(EntityId::Face(k.face)), Some(merged));
        assert_eq!(b.counts(), counts(4, 3, 1, 1));
    }

    #[test]
    fn preconditions_leave_the_body_unchanged() {
        // An edge with both fins in one face.
        let (mut b, e) = path();
        let before = b.dump();
        assert_eq!(
            kef(&mut b, K, e[0].edge),
            Err(TopologyError::WrongFace.into())
        );
        assert_eq!(b.dump(), before);
        // Corners in the two different loops of the square.
        let (mut b, _, m) = square();
        let (c, t) = line(&mut b, p(0.0, 1.0), p(0.0, 0.0));
        let before = b.dump();
        let (from, to) = (Corner::Before(m.fins[0]), Corner::Before(m.fins[1]));
        assert_eq!(
            mef(&mut b, K, from, to, c, t),
            Err(TopologyError::WrongLoop.into())
        );
        assert_eq!(b.dump(), before);
    }

    /// The tetrahedron with the same placeholder pcurve on every fin.
    fn with_pcurves_on_a_tetrahedron() -> (crate::topo::Body, crate::topo::Pcurve) {
        use crate::geom::{Curve2, Line2, Point2, Vector2};
        let mut b = crate::topo::fixture::tetrahedron();
        let c = b.add_curve2(Curve2::Line2(
            Line2::new(Point2::new(0.0, 0.0), Vector2::new(1.0, 0.0)).unwrap(),
        ));
        let p = crate::topo::Pcurve {
            curve: c,
            map: [0.0, 1.0],
        };
        for f in b.fins().collect::<Vec<_>>() {
            crate::euler::set_pcurve(&mut b, f, Some(p)).unwrap();
        }
        (b, p)
    }

    #[test]
    fn kef_drops_the_pcurves_of_fins_that_change_surface() {
        let (mut b, p) = with_pcurves_on_a_tetrahedron();
        // Edge 0 joins faces 0 and 1, on different planes.
        let e = b.edges().next().unwrap();
        let (k, _) = kef(&mut b, OpKey(1), e).unwrap();
        let fins: Vec<_> = b
            .face_loops(k.face)
            .flat_map(|l| b.loop_fins(l).collect::<Vec<_>>())
            .collect();
        let kept = fins
            .iter()
            .filter(|&&f| b.fin(f).unwrap().pcurve == Some(p))
            .count();
        let lost = fins
            .iter()
            .filter(|&&f| b.fin(f).unwrap().pcurve.is_none())
            .count();
        assert_eq!((kept, lost), (2, 2));
        // The other faces are untouched.
        let others = b.fins().filter(|f| !fins.contains(f));
        assert!(
            others
                .into_iter()
                .all(|f| b.fin(f).unwrap().pcurve == Some(p))
        );
    }
}
