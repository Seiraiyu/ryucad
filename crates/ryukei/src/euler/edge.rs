//! MEV and KEV: an edge with a new vertex at its far end.

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
    });
    body.fins.push(Fin {
        loop_: lp,
        edge,
        sense: Sense::Reversed,
        next,
        prev: out,
        around: out,
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
}
