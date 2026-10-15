//! Geometry edits that leave topology alone.

use super::{check_interval, check_point, live};
use crate::Error;
use crate::geom::Point3;
use crate::name::{Event, Journal};
use crate::topo::{Body, CurveId, EdgeId, FaceId, Sense, SurfaceId, VertexId};

/// Moves a vertex to `point`.
pub fn set_point(body: &mut Body, vertex: VertexId, point: Point3) -> Result<Journal, Error> {
    live(&body.vertices, vertex)?;
    let point = check_point(point)?;
    body.vertices[vertex].point = point;
    let mut journal = Journal::new();
    journal.push(Event::Modified(body.vertices[vertex].name));
    Ok(journal)
}

/// Puts an edge on `curve` over `interval`.
pub fn set_curve(
    body: &mut Body,
    edge: EdgeId,
    curve: CurveId,
    interval: [f64; 2],
) -> Result<Journal, Error> {
    live(&body.edges, edge)?;
    live(&body.curves, curve)?;
    let interval = check_interval(interval)?;
    let e = &mut body.edges[edge];
    (e.curve, e.interval) = (curve, interval);
    let mut journal = Journal::new();
    journal.push(Event::Modified(e.name));
    Ok(journal)
}

/// Puts a face on `surface`, using its natural normal or the reverse.
pub fn set_surface(
    body: &mut Body,
    face: FaceId,
    surface: SurfaceId,
    sense: Sense,
) -> Result<Journal, Error> {
    live(&body.faces, face)?;
    live(&body.surfaces, surface)?;
    let f = &mut body.faces[face];
    (f.surface, f.sense) = (surface, sense);
    let mut journal = Journal::new();
    journal.push(Event::Modified(f.name));
    Ok(journal)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::euler::tests::{K, line, plane_body};
    use crate::euler::{Corner, mev, mvfs};
    use crate::geom::{Frame, Plane, Surface};
    use crate::name::OpKey;
    use crate::topo::{EntityId, Id};
    use crate::{Error, TopologyError};

    #[test]
    fn setters_change_geometry_and_report_it() {
        let (mut b, s) = plane_body();
        let (m, _) = mvfs(&mut b, K, Point3::ORIGIN, s).unwrap();
        let q = Point3::new(1.0, 0.0, 0.0);
        let (c, t) = line(&mut b, Point3::ORIGIN, q);
        let (e, _) = mev(&mut b, OpKey(2), Corner::Lone(m.loop_), q, c, t).unwrap();
        let name = |b: &Body, x| b.name_of(x).unwrap();

        let j = set_point(&mut b, e.vertex, Point3::new(2.0, 0.0, 0.0)).unwrap();
        assert_eq!(
            j.events(),
            &[Event::Modified(name(&b, EntityId::Vertex(e.vertex)))]
        );
        assert_eq!(
            b.vertex(e.vertex).unwrap().point,
            Point3::new(2.0, 0.0, 0.0)
        );

        let (c2, _) = line(&mut b, Point3::ORIGIN, Point3::new(2.0, 0.0, 0.0));
        let j = set_curve(&mut b, e.edge, c2, [0.0, 2.0]).unwrap();
        assert_eq!(
            j.events(),
            &[Event::Modified(name(&b, EntityId::Edge(e.edge)))]
        );
        assert_eq!(b.edge(e.edge).unwrap().interval, [0.0, 2.0]);

        let s2 = b.add_surface(Surface::Plane(Plane::new(Frame::WORLD.flipped())));
        let j = set_surface(&mut b, m.face, s2, Sense::Reversed).unwrap();
        assert_eq!(
            j.events(),
            &[Event::Modified(name(&b, EntityId::Face(m.face)))]
        );
        let f = b.face(m.face).unwrap();
        assert_eq!((f.surface, f.sense), (s2, Sense::Reversed));
    }

    #[test]
    fn setters_check_their_input() {
        let (mut b, s) = plane_body();
        let (m, _) = mvfs(&mut b, K, Point3::ORIGIN, s).unwrap();
        let before = b.dump();
        let far = Point3::new(0.0, 0.0, 501.0);
        assert_eq!(set_point(&mut b, m.vertex, far), Err(Error::OutsideSizeBox));
        let dead = <SurfaceId as Id>::from_index(9);
        assert_eq!(
            set_surface(&mut b, m.face, dead, Sense::Forward),
            Err(TopologyError::DeadEntity.into())
        );
        let edge = <EdgeId as Id>::from_index(0);
        assert_eq!(
            set_curve(&mut b, edge, <CurveId as Id>::from_index(0), [0.0, 1.0]),
            Err(TopologyError::DeadEntity.into())
        );
        assert_eq!(b.dump(), before);
    }
}
