//! Geometry edits that leave topology alone.

use super::{check_interval, check_point, live};
use crate::Error;
use crate::geom::Point3;
use crate::name::{Event, Journal};
use crate::topo::{Body, CurveId, EdgeId, FaceId, FinId, Pcurve, Sense, SurfaceId, VertexId};

/// Moves a vertex to `point`.
pub fn set_point(body: &mut Body, vertex: VertexId, point: Point3) -> Result<Journal, Error> {
    live(&body.vertices, vertex)?;
    let point = check_point(point)?;
    body.vertices[vertex].point = point;
    let mut journal = Journal::new();
    journal.push(Event::Modified(body.vertices[vertex].name));
    Ok(journal)
}

/// Puts an edge on `curve` over `interval`. On another curve its fins lose
/// their pcurves; a new interval on the same curve keeps them.
pub fn set_curve(
    body: &mut Body,
    edge: EdgeId,
    curve: CurveId,
    interval: [f64; 2],
) -> Result<Journal, Error> {
    live(&body.edges, edge)?;
    live(&body.curves, curve)?;
    let interval = check_interval(interval)?;
    if body.edges[edge].curve != curve {
        let fins: Vec<FinId> = body.edge_fins(edge).collect();
        for f in fins {
            body.fins[f].pcurve = None;
        }
    }
    let e = &mut body.edges[edge];
    (e.curve, e.interval) = (curve, interval);
    let mut journal = Journal::new();
    journal.push(Event::Modified(e.name));
    Ok(journal)
}

/// Puts a face on `surface`, using its natural normal or the reverse. On
/// another surface its fins lose their pcurves.
pub fn set_surface(
    body: &mut Body,
    face: FaceId,
    surface: SurfaceId,
    sense: Sense,
) -> Result<Journal, Error> {
    live(&body.faces, face)?;
    live(&body.surfaces, surface)?;
    if body.faces[face].surface != surface {
        let loops: Vec<_> = body.face_loops(face).collect();
        for l in loops {
            body.clear_pcurves(l);
        }
    }
    let f = &mut body.faces[face];
    (f.surface, f.sense) = (surface, sense);
    let mut journal = Journal::new();
    journal.push(Event::Modified(f.name));
    Ok(journal)
}

/// Gives a fin a pcurve in its face's parameter plane, or takes it away.
/// The curve must be live and the map finite with a slope that is not
/// zero. Nothing checks that the pcurve fits; [`crate::check::check`]
/// does.
pub fn set_pcurve(body: &mut Body, fin: FinId, pcurve: Option<Pcurve>) -> Result<Journal, Error> {
    live(&body.fins, fin)?;
    if let Some(p) = pcurve {
        live(&body.curves2, p.curve)?;
        if !(p.map[0].is_finite() && p.map[1].is_finite()) {
            return Err(Error::NonFinite);
        }
        if p.map[1] == 0.0 {
            return Err(Error::Degenerate);
        }
    }
    body.fins[fin].pcurve = pcurve;
    let mut journal = Journal::new();
    journal.push(Event::Modified(body.edges[body.fins[fin].edge].name));
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

    #[test]
    fn pcurves_are_set_checked_and_dropped_when_they_go_stale() {
        use crate::geom::{Curve2, Line2, Point2, Vector2};
        let (mut b, s) = plane_body();
        let (m, _) = mvfs(&mut b, K, Point3::ORIGIN, s).unwrap();
        let q = Point3::new(1.0, 0.0, 0.0);
        let (c, t) = line(&mut b, Point3::ORIGIN, q);
        let (e, _) = mev(&mut b, OpKey(2), Corner::Lone(m.loop_), q, c, t).unwrap();
        let l2 = b.add_curve2(Curve2::Line2(
            Line2::new(Point2::new(0.0, 0.0), Vector2::new(1.0, 0.0)).unwrap(),
        ));
        let p = Pcurve {
            curve: l2,
            map: [0.0, 1.0],
        };
        let before = b.dump();
        let dead = Pcurve {
            curve: <crate::topo::Curve2Id as Id>::from_index(5),
            ..p
        };
        assert_eq!(
            set_pcurve(&mut b, e.fins[0], Some(dead)),
            Err(TopologyError::DeadEntity.into())
        );
        let flat = Pcurve {
            map: [1.0, 0.0],
            ..p
        };
        assert_eq!(
            set_pcurve(&mut b, e.fins[0], Some(flat)),
            Err(Error::Degenerate)
        );
        let nan = Pcurve {
            map: [f64::NAN, 1.0],
            ..p
        };
        assert_eq!(
            set_pcurve(&mut b, e.fins[0], Some(nan)),
            Err(Error::NonFinite)
        );
        assert_eq!(b.dump(), before);

        let j = set_pcurve(&mut b, e.fins[0], Some(p)).unwrap();
        assert_eq!(
            j.events(),
            &[Event::Modified(b.name_of(EntityId::Edge(e.edge)).unwrap())]
        );
        set_pcurve(&mut b, e.fins[1], Some(p)).unwrap();
        assert_eq!(b.fin_uv(e.fins[0], 0.5), Some(Point2::new(0.5, 0.0)));
        // A new interval on the same curve, and a new sense on the same
        // surface, keep the pcurves.
        set_curve(&mut b, e.edge, c, [0.0, 0.5]).unwrap();
        set_surface(&mut b, m.face, s, Sense::Reversed).unwrap();
        assert_eq!(b.fin(e.fins[0]).unwrap().pcurve, Some(p));
        // Another curve drops them.
        let (c2, _) = line(&mut b, Point3::ORIGIN, q);
        set_curve(&mut b, e.edge, c2, t).unwrap();
        assert_eq!(b.fin(e.fins[0]).unwrap().pcurve, None);
        assert_eq!(b.fin(e.fins[1]).unwrap().pcurve, None);
        // And so does another surface.
        set_pcurve(&mut b, e.fins[0], Some(p)).unwrap();
        let s2 = b.add_surface(Surface::Plane(Plane::new(Frame::WORLD)));
        set_surface(&mut b, m.face, s2, Sense::Forward).unwrap();
        assert_eq!(b.fin(e.fins[0]).unwrap().pcurve, None);
        set_pcurve(&mut b, e.fins[0], Some(p)).unwrap();
        set_pcurve(&mut b, e.fins[0], None).unwrap();
        assert_eq!(b.fin(e.fins[0]).unwrap().pcurve, None);
    }
}
