//! KEMR and MEKR: an edge that joins two loops of one face. KFMRH and
//! MFKRH: a face that becomes a loop of another (a handle).

use super::{Corner, check_interval, finish, live, next_id};
use crate::name::{Event, Journal, OpKey, Role};
use crate::tol::Tolerance;
use crate::topo::{
    Body, CurveId, Edge, EdgeId, EntityId, Face, FaceId, Fin, FinId, Id, Loop, LoopId, Sense,
};
use crate::{Error, TopologyError};
use std::collections::BTreeSet;

/// What [`kemr`] made: the new loop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Kemr {
    pub loop_: LoopId,
}

/// Kill edge, make ring: removes an edge whose two fins are in one loop,
/// splitting the loop in two. Each side is the fins between the two fins
/// one way round; an empty side becomes a lone-vertex loop.
///
/// The old loop keeps the side holding its first fin, or the side after
/// the edge's second fin when its first fin was one of the edge's. The
/// other side becomes a new loop of the same face.
pub fn kemr(body: &mut Body, edge: EdgeId) -> Result<(Kemr, Journal), Error> {
    let [x, y] = body.two_fins(edge)?;
    let lp = body.fins[x].loop_;
    if body.fins[y].loop_ != lp {
        return Err(TopologyError::WrongLoop.into());
    }
    let side = |from: FinId, to: FinId| -> Vec<FinId> {
        if body.fins[from].next == to {
            Vec::new()
        } else {
            body.fins_between(body.fins[from].next, to)
        }
    };
    // After x up to y, and after y up to x; an empty side leaves the vertex
    // where x (or y) ends.
    let (side_x, side_y) = (side(x, y), side(y, x));
    let (vx, vy) = (body.end_of(x), body.end_of(y));
    let first = body.loops[lp].first_fin;
    let keep_x = matches!(first, Some(f) if f == y || side_x.contains(&f));
    let ((old, v_old), (new, v_new)) = if keep_x {
        ((side_x, vx), (side_y, vy))
    } else {
        ((side_y, vy), (side_x, vx))
    };
    let face = body.loops[lp].face;
    let new_loop = next_id(&body.loops);
    let mut journal = Journal::new();
    journal.push(Event::Deleted(body.edges[edge].name));
    body.loops.push(Loop {
        face,
        next: new_loop,
        prev: new_loop,
        first_fin: None,
        vertex: None,
    });
    body.link_loop(face, new_loop);
    for (l, fins, v) in [(lp, &old, v_old), (new_loop, &new, v_new)] {
        if fins.is_empty() {
            body.loops[l].first_fin = None;
            body.loops[l].vertex = Some(v);
            body.vertices[v].edge = None;
            body.vertices[v].lone = Some(l);
        } else {
            body.ring_fins(l, fins);
            if !matches!(body.loops[l].first_fin, Some(f) if fins.contains(&f)) {
                body.loops[l].first_fin = Some(fins[0]);
            }
        }
    }
    let (start, end) = (body.edges[edge].start, body.edges[edge].end);
    for v in [start, end] {
        if body.vertices[v].edge == Some(edge) {
            let at = old.iter().chain(&new).find(|&&f| body.start_of(f) == v);
            body.vertices[v].edge = at.map(|&f| body.fins[f].edge);
        }
    }
    body.kill_edge(edge, [x, y]);
    finish(body);
    Ok((Kemr { loop_: new_loop }, journal))
}

/// What [`mekr`] made. `fins[0]` runs along the edge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mekr {
    pub edge: EdgeId,
    pub fins: [FinId; 2],
}

/// Make edge, kill ring: the inverse of [`kemr`]. Joins corners of two
/// different loops of one face with a new edge from `from` to `to`,
/// merging the loops. The loop with the higher ID dies.
///
/// Names: the edge is `(key, NewEdge, [face, from, to])`.
pub fn mekr(
    body: &mut Body,
    key: OpKey,
    from: Corner,
    to: Corner,
    curve: CurveId,
    interval: [f64; 2],
) -> Result<(Mekr, Journal), Error> {
    let (la, va) = body.corner(from)?;
    let (lb, vb) = body.corner(to)?;
    if la == lb {
        return Err(TopologyError::WrongLoop.into());
    }
    let face = body.loops[la].face;
    if body.loops[lb].face != face {
        return Err(TopologyError::WrongFace.into());
    }
    live(&body.curves, curve)?;
    let interval = check_interval(interval)?;
    let whole = |c: Corner| match c {
        Corner::Before(f) => body.fins_between(f, f),
        Corner::Lone(_) => Vec::new(),
    };
    let (p, q) = (whole(from), whole(to));
    let (keep, gone) = (la.min(lb), la.max(lb));
    let edge = next_id(&body.edges);
    let fwd = next_id(&body.fins);
    let rev = <FinId as Id>::from_index(fwd.index() + 1);
    let parents = [
        body.faces[face].name,
        body.vertices[va].name,
        body.vertices[vb].name,
    ];
    let name = body.derive_name("mekr", key, Role::NewEdge, &parents);
    body.edges.push(Edge {
        fin: fwd,
        start: va,
        end: vb,
        curve,
        interval,
        tolerance: Tolerance::RESOLUTION,
        name,
    });
    for (fin, sense, around) in [(fwd, Sense::Forward, rev), (rev, Sense::Reversed, fwd)] {
        body.fins.push(Fin {
            loop_: keep,
            edge,
            sense,
            next: fin,
            prev: fin,
            around,
        });
    }
    // fwd, then round the `to` loop, rev, then round the `from` loop.
    let ring: Vec<FinId> = [fwd].into_iter().chain(q).chain([rev]).chain(p).collect();
    body.ring_fins(keep, &ring);
    body.loops[keep].first_fin.get_or_insert(fwd);
    body.loops[keep].vertex = None;
    for v in [va, vb] {
        if body.vertices[v].lone.take().is_some() {
            body.vertices[v].edge = Some(edge);
        }
    }
    body.unlink_loop(gone);
    body.loops.kill(gone);
    body.index_name(EntityId::Edge(edge));
    let mut journal = Journal::new();
    journal.push(Event::Created(name));
    finish(body);
    Ok((
        Mekr {
            edge,
            fins: [fwd, rev],
        },
        journal,
    ))
}

/// What [`kfmrh`] moved: the killed face's loop, now on the kept face.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Kfmrh {
    pub loop_: LoopId,
}

/// Kill face, make ring (hole): `kill`, which must have exactly one loop
/// and share a shell with `keep`, dies and its loop becomes an extra loop
/// of `keep`. The shell's genus goes up by one.
///
/// Names: both faces' names retire; `keep` is `(key, Merged, [a, b])` with
/// the two names sorted.
pub fn kfmrh(
    body: &mut Body,
    key: OpKey,
    keep: FaceId,
    kill: FaceId,
) -> Result<(Kfmrh, Journal), Error> {
    let shell = live(&body.faces, keep)?.shell;
    let gone = live(&body.faces, kill)?;
    if keep == kill {
        return Err(TopologyError::WrongFace.into());
    }
    if gone.shell != shell {
        return Err(TopologyError::WrongShell.into());
    }
    let lp = gone.first_loop.ok_or(TopologyError::NotRemovable)?;
    if body.loops[lp].next != lp {
        return Err(TopologyError::NotRemovable.into());
    }
    let names = [body.faces[keep].name, body.faces[kill].name];
    let mut pair = names;
    pair.sort();
    let merged = body.derive_name("kfmrh", key, Role::Merged, &pair);
    let mut journal = Journal::new();
    journal.push(Event::merged(names[0], names[1], merged));
    body.unlink_loop(lp);
    body.link_loop(keep, lp);
    body.kill_face(kill);
    body.rename(EntityId::Face(keep), merged);
    finish(body);
    Ok((Kfmrh { loop_: lp }, journal))
}

/// What [`mfkrh`] made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mfkrh {
    pub face: FaceId,
}

/// Make face, kill ring (hole): the inverse of [`kfmrh`]. Moves `loop_`,
/// one of at least two loops of its face, to a new face on the same
/// surface with the same sense. The shell's genus goes down by one.
///
/// Which way the new face should point depends on the geometry: a loop
/// merged in by [`kfmrh`] keeps its old sense, while a true hole becomes a
/// face pointing the other way. The caller decides, with
/// [`set_surface`](super::set_surface).
///
/// Fails with `WouldDisconnect` when the new face would not be joined to
/// the rest of the shell through edges: splitting a shell is not a phase 1
/// operation.
///
/// Names: the face's name retires; it becomes `(key, Part(0), [face])`
/// and the new face `(key, Part(1), [face])`.
pub fn mfkrh(body: &mut Body, key: OpKey, loop_: LoopId) -> Result<(Mfkrh, Journal), Error> {
    let face = live(&body.loops, loop_)?.face;
    if body.loops[loop_].next == loop_ {
        return Err(TopologyError::NotRemovable.into());
    }
    if !connected_after_move(body, face, loop_) {
        return Err(TopologyError::WouldDisconnect.into());
    }
    let parent = body.faces[face].name;
    let part0 = body.derive_name("mfkrh", key, Role::Part(0), &[parent]);
    let part1 = body.derive_name("mfkrh", key, Role::Part(1), &[parent]);
    let f = &body.faces[face];
    let (shell, surface, sense) = (f.shell, f.surface, f.sense);
    let new_face = next_id(&body.faces);
    body.faces.push(Face {
        shell,
        next: new_face,
        prev: new_face,
        first_loop: None,
        surface,
        sense,
        name: part1,
    });
    body.link_face(shell, new_face);
    body.unlink_loop(loop_);
    body.link_loop(new_face, loop_);
    body.rename(EntityId::Face(face), part0);
    body.index_name(EntityId::Face(new_face));
    let mut journal = Journal::new();
    journal.push(Event::Split {
        parent,
        children: [part0, part1],
    });
    finish(body);
    Ok((Mfkrh { face: new_face }, journal))
}

/// Whether the shell stays connected when `lp` leaves `face` for a face of
/// its own. Nodes are the shell's faces plus `None` for the moved loop.
fn connected_after_move(b: &Body, face: FaceId, lp: LoopId) -> bool {
    let shell = b.faces[face].shell;
    let node = |l: LoopId| (l != lp).then(|| b.loops[l].face);
    let loops_of = |n: Option<FaceId>| -> Vec<LoopId> {
        match n {
            None => vec![lp],
            Some(f) => b.face_loops(f).filter(|&l| l != lp).collect(),
        }
    };
    let mut seen: BTreeSet<Option<FaceId>> = BTreeSet::new();
    let mut stack = vec![None];
    while let Some(n) = stack.pop() {
        if !seen.insert(n) {
            continue;
        }
        for l in loops_of(n) {
            for fin in b.loop_fins(l) {
                for other in b.edge_fins(b.fins[fin].edge) {
                    stack.push(node(b.fins[other].loop_));
                }
            }
        }
    }
    seen.len() == b.shell_faces(shell).count() + 1
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::euler::tests::{K, counts, line, plane_body, shape};
    use crate::euler::{Mef, Mev, mef, mev, mvfs};
    use crate::geom::Point3;

    fn p(x: f64, y: f64) -> Point3 {
        Point3::new(x, y, 0.0)
    }

    /// A 4 m square with a strut from (0,0) to (1,1) inside the face along
    /// its edges.
    fn strut() -> (Body, Mef, Mev) {
        let (mut b, s) = plane_body();
        let (m, _) = mvfs(&mut b, K, p(0.0, 0.0), s).unwrap();
        let pts = [p(0.0, 0.0), p(4.0, 0.0), p(4.0, 4.0), p(0.0, 4.0)];
        let mut at = Corner::Lone(m.loop_);
        let mut fins = Vec::new();
        for i in 0..3 {
            let (c, t) = line(&mut b, pts[i], pts[i + 1]);
            let (e, _) = mev(&mut b, OpKey(i as u128 + 1), at, pts[i + 1], c, t).unwrap();
            at = Corner::Before(e.fins[1]);
            fins.push(e.fins);
        }
        let (c, t) = line(&mut b, pts[3], pts[0]);
        let from = Corner::Before(fins[2][1]);
        let (sq, _) = mef(&mut b, OpKey(9), from, Corner::Before(fins[0][0]), c, t).unwrap();
        // The face along the closing edge holds the outgoing fins.
        let (c, t) = line(&mut b, p(0.0, 0.0), p(1.0, 1.0));
        let (st, _) = mev(
            &mut b,
            OpKey(10),
            Corner::Before(fins[0][0]),
            p(1.0, 1.0),
            c,
            t,
        )
        .unwrap();
        (b, sq, st)
    }

    #[test]
    fn kemr_on_a_strut_leaves_a_lone_vertex() {
        let (mut b, sq, st) = strut();
        assert_eq!(b.counts(), counts(5, 5, 2, 2));
        let outer = b.fin(st.fins[0]).unwrap().loop_;
        let (k, j) = kemr(&mut b, st.edge).unwrap();
        assert_eq!(j.events().len(), 1);
        assert_eq!(b.counts(), counts(5, 4, 2, 3));
        assert_eq!(b.loop_(k.loop_).unwrap().vertex, Some(st.vertex));
        assert_eq!(b.vertex(st.vertex).unwrap().lone, Some(k.loop_));
        assert_eq!(b.loop_fins(outer).count(), 4);
        assert_eq!(b.face_loops(sq.faces[0]).count(), 2);
    }

    #[test]
    fn mekr_undoes_kemr_on_a_strut() {
        let (mut b, _, st) = strut();
        let before = shape(&b);
        let outer = b.fin(st.fins[0]).unwrap().loop_;
        let corner = b.fin(st.fins[1]).unwrap().next;
        let curve = b.edge(st.edge).unwrap().curve;
        let interval = b.edge(st.edge).unwrap().interval;
        let (k, _) = kemr(&mut b, st.edge).unwrap();
        let (m, j) = mekr(
            &mut b,
            OpKey(11),
            Corner::Before(corner),
            Corner::Lone(k.loop_),
            curve,
            interval,
        )
        .unwrap();
        assert_eq!(j.events().len(), 1);
        assert_eq!(b.fin(m.fins[0]).unwrap().loop_, outer);
        assert_eq!(shape(&b), before);
    }

    #[test]
    fn kemr_cuts_out_a_hole() {
        // Close a triangle at the end of the strut, then cut the strut: the
        // triangle's outline becomes a hole in the square's face.
        let (mut b, sq, st) = strut();
        let pts = [p(1.0, 1.0), p(2.0, 1.0), p(1.0, 2.0)];
        let mut at = Corner::Before(st.fins[1]);
        let mut fins = Vec::new();
        for i in 0..2 {
            let (c, t) = line(&mut b, pts[i], pts[i + 1]);
            let (e, _) = mev(&mut b, OpKey(20 + i as u128), at, pts[i + 1], c, t).unwrap();
            at = Corner::Before(e.fins[1]);
            fins.push(e.fins);
        }
        let (c, t) = line(&mut b, pts[2], pts[0]);
        let (tri, _) = mef(&mut b, OpKey(30), at, Corner::Before(fins[0][0]), c, t).unwrap();
        assert_eq!(b.counts(), counts(7, 8, 3, 3));
        let face = sq.faces[0];
        let (k, _) = kemr(&mut b, st.edge).unwrap();
        assert_eq!(b.counts(), counts(7, 7, 3, 4));
        let loops: Vec<_> = b.face_loops(face).collect();
        assert_eq!(loops.len(), 2);
        assert_eq!(b.loop_fins(k.loop_).count(), 3);
        assert_eq!(b.loop_(k.loop_).unwrap().face, face);
        assert_ne!(tri.face, face);
        for v in b.vertices() {
            assert_eq!(b.vertex_edges(v).len(), 2);
        }
    }

    #[test]
    fn preconditions_leave_the_body_unchanged() {
        let (mut b, sq, st) = strut();
        let (c, t) = line(&mut b, p(0.0, 0.0), p(1.0, 1.0));
        let before = b.dump();
        // The square's closing edge has a fin in each face.
        assert_eq!(kemr(&mut b, sq.edge), Err(TopologyError::WrongLoop.into()));
        let a = Corner::Before(st.fins[0]);
        let same = Corner::Before(st.fins[1]);
        assert_eq!(
            mekr(&mut b, K, a, same, c, t),
            Err(TopologyError::WrongLoop.into())
        );
        let other = Corner::Before(sq.fins[1]);
        assert_eq!(
            mekr(&mut b, K, a, other, c, t),
            Err(TopologyError::WrongFace.into())
        );
        assert_eq!(b.dump(), before);
    }

    #[test]
    fn kfmrh_and_mfkrh_on_a_tetrahedron() {
        let mut b = crate::topo::fixture::tetrahedron();
        let [f0, f1] = [0, 1].map(<FaceId as Id>::from_index);
        let before = b.counts();
        let (k, j) = kfmrh(&mut b, OpKey(1), f0, f1).unwrap();
        assert_eq!(j.events().len(), 1);
        assert_eq!(b.counts().faces, 3);
        assert_eq!(b.face_loops(f0).collect::<Vec<_>>().len(), 2);
        assert_eq!(b.loop_(k.loop_).unwrap().face, f0);
        let genus1 = shape(&b);
        let (m, j) = mfkrh(&mut b, OpKey(2), k.loop_).unwrap();
        assert!(matches!(j.events()[0], Event::Split { .. }));
        assert_eq!(b.counts(), before);
        assert_eq!(b.face(m.face).unwrap().sense, Sense::Forward);
        assert_eq!(b.face_loops(m.face).collect::<Vec<_>>(), [k.loop_]);
        // MFKRH then KFMRH restores the genus-1 body exactly.
        kfmrh(&mut b, OpKey(3), f0, m.face).unwrap();
        assert_eq!(shape(&b), genus1);
    }

    #[test]
    fn mfkrh_refuses_to_split_a_shell() {
        // The hole cut around the triangle in `kemr_cuts_out_a_hole`: as a
        // face of its own it would join only the triangle.
        let (mut b, _, st) = strut();
        let pts = [p(1.0, 1.0), p(2.0, 1.0), p(1.0, 2.0)];
        let mut at = Corner::Before(st.fins[1]);
        let mut fins = Vec::new();
        for i in 0..2 {
            let (c, t) = line(&mut b, pts[i], pts[i + 1]);
            let (e, _) = mev(&mut b, OpKey(20 + i as u128), at, pts[i + 1], c, t).unwrap();
            at = Corner::Before(e.fins[1]);
            fins.push(e.fins);
        }
        let (c, t) = line(&mut b, pts[2], pts[0]);
        mef(&mut b, OpKey(30), at, Corner::Before(fins[0][0]), c, t).unwrap();
        let (hole, _) = kemr(&mut b, st.edge).unwrap();
        let before = b.dump();
        assert_eq!(
            mfkrh(&mut b, K, hole.loop_),
            Err(TopologyError::WouldDisconnect.into())
        );
        assert_eq!(b.dump(), before);
    }

    #[test]
    fn handle_preconditions() {
        let (mut b, s) = plane_body();
        let (x, _) = mvfs(&mut b, OpKey(1), p(0.0, 0.0), s).unwrap();
        let (y, _) = mvfs(&mut b, OpKey(2), p(1.0, 0.0), s).unwrap();
        let before = b.dump();
        assert_eq!(
            kfmrh(&mut b, K, x.face, y.face),
            Err(TopologyError::WrongShell.into())
        );
        assert_eq!(
            kfmrh(&mut b, K, x.face, x.face),
            Err(TopologyError::WrongFace.into())
        );
        assert_eq!(
            mfkrh(&mut b, K, x.loop_),
            Err(TopologyError::NotRemovable.into())
        );
        assert_eq!(b.dump(), before);
        // A face that dies must have exactly one loop; once the strut is
        // cut the square's face has two.
        let (mut b, sq, st) = strut();
        kemr(&mut b, st.edge).unwrap();
        let before = b.dump();
        assert_eq!(
            kfmrh(&mut b, K, sq.faces[1], sq.faces[0]),
            Err(TopologyError::NotRemovable.into())
        );
        assert_eq!(b.dump(), before);
    }
}
