//! MEV and KEV: an edge with a new vertex at its far end. SEMV and JEKV:
//! a vertex that splits an edge in two.

use super::{Corner, check_interval, check_point, finish, live, next_id};
use crate::geom::Point3;
use crate::name::{Event, Journal, OpKey, Role};
use crate::tol::Tolerance;
use crate::topo::{Body, CurveId, Edge, EdgeId, EntityId, Fin, FinId, Id, Sense, Vertex, VertexId};
use crate::{Error, TopologyError};

/// What [`mev`] made. `fins[0]` runs out to the new vertex, `fins[1]` back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mev {
    pub edge: EdgeId,
    pub vertex: VertexId,
    pub fins: [FinId; 2],
}

/// Make edge, vertex: a new edge from the corner's vertex to a new vertex
/// at `point`, along `curve` over `interval`, inside the corner's loop.
///
/// Names: the edge is `(key, NewEdge, [face, vertex])` and the new vertex
/// `(key, NewVertex, [face, vertex])`, from the loop's face and the
/// corner's vertex.
pub fn mev(
    body: &mut Body,
    key: OpKey,
    at: Corner,
    point: Point3,
    curve: CurveId,
    interval: [f64; 2],
) -> Result<(Mev, Journal), Error> {
    let (lp, v) = body.corner(at)?;
    live(&body.curves, curve)?;
    let point = check_point(point)?;
    let interval = check_interval(interval)?;
    let parents = [body.faces[body.loops[lp].face].name, body.vertices[v].name];
    let (edge, vertex) = (next_id(&body.edges), next_id(&body.vertices));
    let out = next_id(&body.fins);
    let back = <FinId as Id>::from_index(out.index() + 1);
    let edge_name = body.derive_name("mev", key, Role::NewEdge, &parents);
    let vertex_name = body.derive_name("mev", key, Role::NewVertex, &parents);
    body.vertices.push(Vertex {
        point,
        tolerance: Tolerance::RESOLUTION,
        edge: Some(edge),
        lone: None,
        name: vertex_name,
    });
    body.edges.push(Edge {
        fin: out,
        start: v,
        end: vertex,
        curve,
        interval,
        tolerance: Tolerance::RESOLUTION,
        name: edge_name,
    });
    // Splice `out, back` in just before the corner's fin, or make them the
    // whole loop.
    let (prev, next) = match at {
        Corner::Before(f) => (body.fins[f].prev, f),
        Corner::Lone(_) => (back, out),
    };
    body.fins.push(Fin {
        loop_: lp,
        edge,
        sense: Sense::Forward,
        next: back,
        prev,
        around: back,
        pcurve: None,
    });
    body.fins.push(Fin {
        loop_: lp,
        edge,
        sense: Sense::Reversed,
        next,
        prev: out,
        around: out,
        pcurve: None,
    });
    match at {
        Corner::Before(f) => {
            body.fins[prev].next = out;
            body.fins[f].prev = back;
        }
        Corner::Lone(l) => {
            body.loops[l].first_fin = Some(out);
            body.loops[l].vertex = None;
            body.vertices[v].lone = None;
            body.vertices[v].edge = Some(edge);
        }
    }
    body.index_name(EntityId::Edge(edge));
    body.index_name(EntityId::Vertex(vertex));
    let mut journal = Journal::new();
    journal.push(Event::Created(edge_name));
    journal.push(Event::Created(vertex_name));
    finish(body);
    Ok((
        Mev {
            edge,
            vertex,
            fins: [out, back],
        },
        journal,
    ))
}

/// Kill edge, vertex: the inverse of [`mev`]. `vertex` must be an end of
/// `edge` with no other edge, so the edge's two fins meet there.
pub fn kev(body: &mut Body, edge: EdgeId, vertex: VertexId) -> Result<Journal, Error> {
    let [f1, f2] = body.two_fins(edge)?;
    live(&body.vertices, vertex)?;
    let e = &body.edges[edge];
    if e.start == e.end || (e.start != vertex && e.end != vertex) {
        return Err(TopologyError::NotRemovable.into());
    }
    // x runs into the vertex and y straight back out.
    let (x, y) = if body.end_of(f1) == vertex {
        (f1, f2)
    } else {
        (f2, f1)
    };
    if body.fins[x].next != y {
        return Err(TopologyError::NotRemovable.into());
    }
    let u = body.start_of(x);
    let lp = body.fins[x].loop_;
    let mut journal = Journal::new();
    journal.push(Event::Deleted(body.edges[edge].name));
    journal.push(Event::Deleted(body.vertices[vertex].name));
    if body.fins[y].next == x {
        // The loop was only this edge: it shrinks to u.
        body.loops[lp].first_fin = None;
        body.loops[lp].vertex = Some(u);
        body.vertices[u].edge = None;
        body.vertices[u].lone = Some(lp);
    } else {
        let (p, n) = (body.fins[x].prev, body.fins[y].next);
        body.fins[p].next = n;
        body.fins[n].prev = p;
        if matches!(body.loops[lp].first_fin, Some(f) if f == x || f == y) {
            body.loops[lp].first_fin = Some(n);
        }
        // n starts at u, so its edge is still there.
        if body.vertices[u].edge == Some(edge) {
            body.vertices[u].edge = Some(body.fins[n].edge);
        }
    }
    body.kill_edge(edge, [x, y]);
    body.kill_vertex(vertex);
    finish(body);
    Ok(journal)
}

/// What [`semv`] made: the edge on the end side, and the vertex.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Semv {
    pub edge: EdgeId,
    pub vertex: VertexId,
}

/// Split edge, make vertex: splits `edge` at parameter `t`, strictly inside
/// its interval, with a new vertex at `point`. The edge keeps the start
/// side; a new edge on the same curve takes the end side. Every fin of the
/// edge gets a partner on the new edge beside it in its loop, with the
/// fin's pcurve (the parameter is the same on both halves).
///
/// Names: the edge's name retires; the start side is
/// `(key, Part(0), [edge])`, the end side `(key, Part(1), [edge])` and the
/// vertex `(key, NewVertex, [edge])`.
pub fn semv(
    body: &mut Body,
    key: OpKey,
    edge: EdgeId,
    t: f64,
    point: Point3,
) -> Result<(Semv, Journal), Error> {
    let e = live(&body.edges, edge)?;
    if !t.is_finite() {
        return Err(Error::NonFinite);
    }
    let [t0, t1] = e.interval;
    if !(t0 < t && t < t1) {
        return Err(Error::Degenerate);
    }
    let point = check_point(point)?;
    let (start, end, curve, tolerance) = (e.start, e.end, e.curve, e.tolerance);
    let fins: Vec<FinId> = body.edge_fins(edge).collect();
    let (rest, vertex) = (next_id(&body.edges), next_id(&body.vertices));
    let first = next_id(&body.fins);
    let partners: Vec<FinId> = (0..fins.len())
        .map(|i| <FinId as Id>::from_index(first.index() + i))
        .collect();
    let parent = body.edges[edge].name;
    let part0 = body.derive_name("semv", key, Role::Part(0), &[parent]);
    let part1 = body.derive_name("semv", key, Role::Part(1), &[parent]);
    let vertex_name = body.derive_name("semv", key, Role::NewVertex, &[parent]);
    body.vertices.push(Vertex {
        point,
        tolerance: Tolerance::RESOLUTION,
        edge: Some(edge),
        lone: None,
        name: vertex_name,
    });
    body.edges.push(Edge {
        fin: first,
        start: vertex,
        end,
        curve,
        interval: [t, t1],
        tolerance,
        name: part1,
    });
    for (i, &f) in fins.iter().enumerate() {
        let g = partners[i];
        let fin = &body.fins[f];
        let (loop_, sense, pcurve) = (fin.loop_, fin.sense, fin.pcurve);
        // Along the edge the partner follows the fin; against it, it comes
        // first.
        let (prev, next) = match sense {
            Sense::Forward => (f, fin.next),
            Sense::Reversed => (fin.prev, f),
        };
        body.fins.push(Fin {
            loop_,
            edge: rest,
            sense,
            next,
            prev,
            around: partners[(i + 1) % partners.len()],
            pcurve,
        });
        body.fins[prev].next = g;
        body.fins[next].prev = g;
    }
    body.edges[edge].end = vertex;
    body.edges[edge].interval[1] = t;
    if end != start && body.vertices[end].edge == Some(edge) {
        body.vertices[end].edge = Some(rest);
    }
    body.rename(EntityId::Edge(edge), part0);
    body.index_name(EntityId::Edge(rest));
    body.index_name(EntityId::Vertex(vertex));
    let mut journal = Journal::new();
    journal.push(Event::Split {
        parent,
        children: [part0, part1],
    });
    journal.push(Event::Created(vertex_name));
    finish(body);
    Ok((Semv { edge: rest, vertex }, journal))
}

/// What [`jekv`] kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Jekv {
    pub edge: EdgeId,
}

/// Join edges, kill vertex: the inverse of [`semv`]. `vertex` must have
/// exactly two edges, one ending there and one starting there, on the same
/// curve with touching intervals, each fin of the second beside a fin of
/// the first with the same pcurve. The edge ending at the vertex absorbs
/// the other.
///
/// Names: both edges' names retire; the kept edge is `(key, Merged, [a, b])`
/// with the two names sorted.
pub fn jekv(body: &mut Body, key: OpKey, vertex: VertexId) -> Result<(Jekv, Journal), Error> {
    live(&body.vertices, vertex)?;
    let not_removable = || Error::Topology(TopologyError::NotRemovable);
    let edges = body.vertex_edges(vertex);
    let [a, b] = edges[..] else {
        return Err(not_removable());
    };
    let (e1, e2) = if body.edges[a].end == vertex {
        (a, b)
    } else {
        (b, a)
    };
    let (x, y) = (&body.edges[e1], &body.edges[e2]);
    if x.end != vertex
        || y.start != vertex
        || x.start == vertex
        || y.end == vertex
        || x.curve != y.curve
        || x.interval[1] != y.interval[0]
    {
        return Err(not_removable());
    }
    body.two_fins(e1)?;
    let partners = body.two_fins(e2)?;
    for g in partners {
        let fin = &body.fins[g];
        let beside = match fin.sense {
            Sense::Forward => fin.prev,
            Sense::Reversed => fin.next,
        };
        if body.fins[beside].edge != e1
            || body.fins[beside].sense != fin.sense
            || body.fins[beside].pcurve != fin.pcurve
        {
            return Err(not_removable());
        }
    }
    let names = [body.edges[e1].name, body.edges[e2].name];
    let mut pair = names;
    pair.sort();
    let merged = body.derive_name("jekv", key, Role::Merged, &pair);
    let mut journal = Journal::new();
    journal.push(Event::merged(names[0], names[1], merged));
    journal.push(Event::Deleted(body.vertices[vertex].name));
    for g in partners {
        let (lp, p, n) = (body.fins[g].loop_, body.fins[g].prev, body.fins[g].next);
        body.fins[p].next = n;
        body.fins[n].prev = p;
        if body.loops[lp].first_fin == Some(g) {
            body.loops[lp].first_fin = Some(n);
        }
    }
    let (end, t1) = (body.edges[e2].end, body.edges[e2].interval[1]);
    body.edges[e1].end = end;
    body.edges[e1].interval[1] = t1;
    if body.vertices[end].edge == Some(e2) {
        body.vertices[end].edge = Some(e1);
    }
    body.kill_edge(e2, partners);
    body.kill_vertex(vertex);
    body.rename(EntityId::Edge(e1), merged);
    finish(body);
    Ok((Jekv { edge: e1 }, journal))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::euler::tests::{K, counts, line, plane_body, shape};
    use crate::euler::{Mvfs, mvfs};
    use crate::name::Name;

    fn p(x: f64, y: f64) -> Point3 {
        Point3::new(x, y, 0.0)
    }

    /// A skeleton at the origin and a path origin -> (1, 0) -> (1, 1).
    fn path() -> (Body, Mvfs, Mev, Mev) {
        let (mut b, s) = plane_body();
        let (m, _) = mvfs(&mut b, K, p(0.0, 0.0), s).unwrap();
        let (c, i) = line(&mut b, p(0.0, 0.0), p(1.0, 0.0));
        let (a, _) = mev(&mut b, OpKey(1), Corner::Lone(m.loop_), p(1.0, 0.0), c, i).unwrap();
        let (c, i) = line(&mut b, p(1.0, 0.0), p(1.0, 1.0));
        let at = Corner::Before(a.fins[1]);
        let (z, _) = mev(&mut b, OpKey(2), at, p(1.0, 1.0), c, i).unwrap();
        (b, m, a, z)
    }

    #[test]
    fn mev_from_a_lone_vertex() {
        let (mut b, s) = plane_body();
        let (m, _) = mvfs(&mut b, K, p(0.0, 0.0), s).unwrap();
        let (c, i) = line(&mut b, p(0.0, 0.0), p(1.0, 0.0));
        let (out, j) = mev(&mut b, OpKey(1), Corner::Lone(m.loop_), p(1.0, 0.0), c, i).unwrap();
        assert_eq!(b.counts(), counts(2, 1, 1, 1));
        assert_eq!(b.loop_(m.loop_).unwrap().vertex, None);
        assert_eq!(b.loop_fins(m.loop_).collect::<Vec<_>>(), out.fins);
        assert_eq!(b.fin_start(out.fins[0]), Some(m.vertex));
        assert_eq!(b.fin_end(out.fins[0]), Some(out.vertex));
        assert_eq!(b.vertex(m.vertex).unwrap().edge, Some(out.edge));
        let face = Name::derive(K, Role::NewFace, &[]);
        let corner = Name::derive(K, Role::NewVertex, &[]);
        let e = Name::derive(OpKey(1), Role::NewEdge, &[face, corner]);
        let v = Name::derive(OpKey(1), Role::NewVertex, &[face, corner]);
        assert_eq!(j.events(), &[Event::Created(e), Event::Created(v)]);
        assert_eq!(
            b.recipe(e),
            Some("mev#1(mvfs#10/new-face,mvfs#10/new-vertex)/new-edge")
        );
    }

    #[test]
    fn mev_before_a_fin_splices_into_the_loop() {
        let (b, m, a, z) = path();
        assert_eq!(b.counts(), counts(3, 2, 1, 1));
        let fins: Vec<_> = b.loop_fins(m.loop_).collect();
        assert_eq!(fins, [a.fins[0], z.fins[0], z.fins[1], a.fins[1]]);
        for (k, &f) in fins.iter().enumerate() {
            assert_eq!(b.fin_end(f), b.fin_start(fins[(k + 1) % 4]));
        }
        assert_eq!(b.vertex_edges(a.vertex).len(), 2);
    }

    #[test]
    fn kev_undoes_mev() {
        let (mut b, m, a, z) = path();
        let before = shape(&b);
        let (c, i) = line(&mut b, p(1.0, 1.0), p(2.0, 1.0));
        let (w, _) = mev(
            &mut b,
            OpKey(3),
            Corner::Before(z.fins[1]),
            p(2.0, 1.0),
            c,
            i,
        )
        .unwrap();
        let j = kev(&mut b, w.edge, w.vertex).unwrap();
        assert_eq!(j.events().len(), 2);
        assert_eq!(shape(&b), before);
        // Back down to the skeleton, through the lone-vertex case.
        kev(&mut b, z.edge, z.vertex).unwrap();
        kev(&mut b, a.edge, a.vertex).unwrap();
        assert_eq!(b.loop_(m.loop_).unwrap().vertex, Some(m.vertex));
        assert_eq!(b.vertex(m.vertex).unwrap().lone, Some(m.loop_));
        assert_eq!(b.counts(), counts(1, 0, 1, 1));
    }

    #[test]
    fn preconditions_leave_the_body_unchanged() {
        let (mut b, m, a, z) = path();
        let (c, i) = line(&mut b, p(0.0, 0.0), p(1.0, 0.0));
        let before = b.dump();
        // a's far vertex also has z's edge; z's start is not its free end.
        assert_eq!(
            kev(&mut b, a.edge, a.vertex),
            Err(TopologyError::NotRemovable.into())
        );
        assert_eq!(
            kev(&mut b, z.edge, a.vertex),
            Err(TopologyError::NotRemovable.into())
        );
        assert_eq!(
            kev(&mut b, a.edge, z.vertex),
            Err(TopologyError::NotRemovable.into())
        );
        assert_eq!(
            mev(&mut b, K, Corner::Lone(m.loop_), p(2.0, 0.0), c, i),
            Err(TopologyError::NotLone.into())
        );
        let at = Corner::Before(a.fins[0]);
        assert_eq!(
            mev(&mut b, K, at, p(2.0, 0.0), c, [1.0, 1.0]),
            Err(Error::Degenerate)
        );
        assert_eq!(
            mev(&mut b, K, at, p(2.0, 0.0), c, [0.0, f64::NAN]),
            Err(Error::NonFinite)
        );
        assert_eq!(b.dump(), before);
    }

    /// The path closed into a unit square.
    fn square() -> (Body, crate::euler::Mef) {
        let (mut b, _, a, z) = path();
        let (c, i) = line(&mut b, p(1.0, 1.0), p(0.0, 1.0));
        let (w, _) = mev(
            &mut b,
            OpKey(3),
            Corner::Before(z.fins[1]),
            p(0.0, 1.0),
            c,
            i,
        )
        .unwrap();
        let (c, i) = line(&mut b, p(0.0, 1.0), p(0.0, 0.0));
        let from = Corner::Before(w.fins[1]);
        let to = Corner::Before(a.fins[0]);
        let (sq, _) = crate::euler::mef(&mut b, OpKey(4), from, to, c, i).unwrap();
        (b, sq)
    }

    #[test]
    fn semv_splits_an_edge_between_two_faces() {
        let (mut b, sq) = square();
        let parent = b.name_of(EntityId::Edge(sq.edge)).unwrap();
        let (s, j) = semv(&mut b, OpKey(5), sq.edge, 0.25, p(0.0, 0.75)).unwrap();
        assert_eq!(b.counts(), counts(5, 5, 2, 2));
        let part = |i| Name::derive(OpKey(5), Role::Part(i), &[parent]);
        assert_eq!(
            j.events()[0],
            Event::Split {
                parent,
                children: [part(0), part(1)]
            }
        );
        let e = b.edge(sq.edge).unwrap();
        assert_eq!((e.end, e.interval), (s.vertex, [0.0, 0.25]));
        let r = b.edge(s.edge).unwrap();
        assert_eq!((r.start, r.interval), (s.vertex, [0.25, 1.0]));
        for l in b.loops() {
            let fins: Vec<_> = b.loop_fins(l).collect();
            assert_eq!(fins.len(), 5);
            for (k, &f) in fins.iter().enumerate() {
                assert_eq!(b.fin_end(f), b.fin_start(fins[(k + 1) % 5]));
            }
        }
    }

    #[test]
    fn jekv_undoes_semv() {
        // Between two faces, on a strut, and on a closed circle.
        let (mut b, sq) = square();
        let before = shape(&b);
        let (s, _) = semv(&mut b, OpKey(5), sq.edge, 0.25, p(0.0, 0.75)).unwrap();
        let (k, j) = jekv(&mut b, OpKey(6), s.vertex).unwrap();
        assert_eq!(k.edge, sq.edge);
        assert_eq!(j.events().len(), 2);
        assert_eq!(shape(&b), before);
        let (mut b, _, _, z) = path();
        let before = shape(&b);
        let (s, _) = semv(&mut b, OpKey(5), z.edge, 0.5, p(1.0, 0.5)).unwrap();
        assert_eq!(b.loop_fins(b.fin(z.fins[0]).unwrap().loop_).count(), 6);
        jekv(&mut b, OpKey(6), s.vertex).unwrap();
        assert_eq!(shape(&b), before);
        let (mut b, s0) = plane_body();
        let (m, _) = mvfs(&mut b, K, p(1.0, 0.0), s0).unwrap();
        let circle = crate::geom::Circle::new(crate::geom::Frame::WORLD, 1.0).unwrap();
        let c = b.add_curve(crate::geom::Curve::Circle(circle));
        let lone = Corner::Lone(m.loop_);
        let tau = core::f64::consts::TAU;
        crate::euler::mef(&mut b, OpKey(2), lone, lone, c, [0.0, tau]).unwrap();
        let before = shape(&b);
        let e = b.edges().next().unwrap();
        let (s, _) = semv(&mut b, OpKey(5), e, 3.0, p(-1.0, 0.0)).unwrap();
        assert_eq!(b.vertex_edges(s.vertex).len(), 2);
        assert_eq!(b.vertex_edges(m.vertex).len(), 2);
        jekv(&mut b, OpKey(6), s.vertex).unwrap();
        assert_eq!(shape(&b), before);
    }

    #[test]
    fn split_and_join_preconditions() {
        let (mut b, sq) = square();
        let before = b.dump();
        assert_eq!(
            semv(&mut b, K, sq.edge, 0.0, p(0.0, 1.0)),
            Err(Error::Degenerate)
        );
        assert_eq!(
            semv(&mut b, K, sq.edge, 1.0, p(0.0, 0.0)),
            Err(Error::Degenerate)
        );
        assert_eq!(
            semv(&mut b, K, sq.edge, f64::NAN, p(0.0, 0.0)),
            Err(Error::NonFinite)
        );
        // A corner of the square joins two edges on different lines.
        let corner = b.edge(sq.edge).unwrap().end;
        assert_eq!(
            jekv(&mut b, K, corner),
            Err(TopologyError::NotRemovable.into())
        );
        assert_eq!(b.dump(), before);
    }

    #[test]
    fn semv_shares_pcurves_and_jekv_needs_them_equal() {
        use crate::euler::set_pcurve;
        use crate::geom::{Curve2, Line2, Point2, Vector2};
        use crate::topo::Pcurve;
        let (mut b, sq) = square();
        let l2 = b.add_curve2(Curve2::Line2(
            Line2::new(Point2::new(0.0, 1.0), Vector2::new(0.0, -1.0)).unwrap(),
        ));
        let p0 = Pcurve {
            curve: l2,
            map: [0.0, 1.0],
        };
        set_pcurve(&mut b, sq.fins[0], Some(p0)).unwrap();
        let before = shape(&b);
        let (s, _) = semv(&mut b, OpKey(5), sq.edge, 0.25, p(0.0, 0.75)).unwrap();
        let halves: Vec<Option<Pcurve>> = b
            .edge_fins(s.edge)
            .map(|f| b.fin(f).unwrap().pcurve)
            .collect();
        assert!(halves.contains(&Some(p0)) && halves.contains(&None));
        // Halves whose pcurves differ can't be joined.
        let half = b
            .edge_fins(s.edge)
            .find(|&f| b.fin(f).unwrap().pcurve.is_some())
            .unwrap();
        let p1 = Pcurve {
            map: [0.0, 2.0],
            ..p0
        };
        set_pcurve(&mut b, half, Some(p1)).unwrap();
        let held = b.dump();
        assert_eq!(
            jekv(&mut b, OpKey(6), s.vertex),
            Err(TopologyError::NotRemovable.into())
        );
        assert_eq!(b.dump(), held);
        set_pcurve(&mut b, half, Some(p0)).unwrap();
        jekv(&mut b, OpKey(6), s.vertex).unwrap();
        assert_eq!(shape(&b), before);
    }
}
