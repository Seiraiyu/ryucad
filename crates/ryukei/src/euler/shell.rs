//! MVFS and KVFS: start and end a body's skeleton.

use super::{check_point, finish, live, next_id};
use crate::geom::Point3;
use crate::name::{Event, Journal, OpKey, Role};
use crate::tol::Tolerance;
use crate::topo::{
    Body, EntityId, Face, FaceId, Loop, LoopId, Region, RegionId, RegionKind, Sense, Shell,
    ShellId, SurfaceId, Vertex, VertexId,
};
use crate::{Error, TopologyError};

/// What [`mvfs`] made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mvfs {
    pub region: RegionId,
    pub shell: ShellId,
    pub face: FaceId,
    pub loop_: LoopId,
    pub vertex: VertexId,
}

/// Make vertex, face, shell: starts a new solid region with one shell, one
/// face on `surface` (with its natural normal) and one lone-vertex loop at
/// `point`.
///
/// Names: the face is `(key, NewFace)`, the vertex `(key, NewVertex)`.
pub fn mvfs(
    body: &mut Body,
    key: OpKey,
    point: Point3,
    surface: SurfaceId,
) -> Result<(Mvfs, Journal), Error> {
    live(&body.surfaces, surface)?;
    let point = check_point(point)?;
    let (region, shell, face, loop_, vertex) = (
        next_id(&body.regions),
        next_id(&body.shells),
        next_id(&body.faces),
        next_id(&body.loops),
        next_id(&body.vertices),
    );
    let face_name = body.derive_name("mvfs", key, Role::NewFace, &[]);
    let vertex_name = body.derive_name("mvfs", key, Role::NewVertex, &[]);
    body.regions.push(Region {
        kind: RegionKind::Solid,
        first_shell: None,
    });
    body.shells.push(Shell {
        region,
        next: shell,
        prev: shell,
        first_face: None,
    });
    body.faces.push(Face {
        shell,
        next: face,
        prev: face,
        first_loop: None,
        surface,
        sense: Sense::Forward,
        name: face_name,
    });
    body.loops.push(Loop {
        face,
        next: loop_,
        prev: loop_,
        first_fin: None,
        vertex: Some(vertex),
    });
    body.vertices.push(Vertex {
        point,
        tolerance: Tolerance::RESOLUTION,
        edge: None,
        lone: Some(loop_),
        name: vertex_name,
    });
    body.link_shell(region, shell);
    body.link_face(shell, face);
    body.link_loop(face, loop_);
    body.index_name(EntityId::Face(face));
    body.index_name(EntityId::Vertex(vertex));
    let mut journal = Journal::new();
    journal.push(Event::Created(face_name));
    journal.push(Event::Created(vertex_name));
    finish(body);
    Ok((
        Mvfs {
            region,
            shell,
            face,
            loop_,
            vertex,
        },
        journal,
    ))
}

/// Kill vertex, face, shell: the inverse of [`mvfs`]. The shell must hold
/// exactly one face with exactly one loop, a lone vertex. Its region goes
/// too when it has no other shell.
pub fn kvfs(body: &mut Body, shell: ShellId) -> Result<Journal, Error> {
    let s = live(&body.shells, shell)?;
    let face = s.first_face.ok_or(TopologyError::NotRemovable)?;
    let f = &body.faces[face];
    let lp = f.first_loop.ok_or(TopologyError::NotRemovable)?;
    let l = &body.loops[lp];
    if f.next != face || l.next != lp {
        return Err(TopologyError::NotRemovable.into());
    }
    let vertex = l.vertex.ok_or(TopologyError::NotLone)?;
    let region = s.region;
    let mut journal = Journal::new();
    journal.push(Event::Deleted(body.faces[face].name));
    journal.push(Event::Deleted(body.vertices[vertex].name));
    body.kill_vertex(vertex);
    body.unlink_loop(lp);
    body.loops.kill(lp);
    body.kill_face(face);
    body.unlink_shell(shell);
    body.shells.kill(shell);
    if body.regions[region].first_shell.is_none() {
        body.regions.kill(region);
    }
    finish(body);
    Ok(journal)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::euler::tests::{K, plane_body, shape};
    use crate::name::Name;
    use crate::topo::Counts;

    #[test]
    fn mvfs_makes_a_skeleton() {
        let (mut b, s) = plane_body();
        let (m, j) = mvfs(&mut b, K, Point3::new(1.0, 2.0, 0.0), s).unwrap();
        assert_eq!(
            b.counts(),
            Counts {
                regions: 2,
                shells: 1,
                faces: 1,
                loops: 1,
                fins: 0,
                edges: 0,
                vertices: 1,
            }
        );
        assert_eq!(b.loop_(m.loop_).unwrap().vertex, Some(m.vertex));
        assert_eq!(b.vertex(m.vertex).unwrap().lone, Some(m.loop_));
        assert_eq!(b.face(m.face).unwrap().surface, s);
        let face = Name::derive(K, Role::NewFace, &[]);
        let vertex = Name::derive(K, Role::NewVertex, &[]);
        assert_eq!(j.events(), &[Event::Created(face), Event::Created(vertex)]);
        assert_eq!(b.by_name(face), Some(EntityId::Face(m.face)));
        assert_eq!(b.recipe(vertex), Some("mvfs#10/new-vertex"));
    }

    #[test]
    fn kvfs_undoes_mvfs() {
        let (mut b, s) = plane_body();
        let before = shape(&b);
        let (m, _) = mvfs(&mut b, K, Point3::ORIGIN, s).unwrap();
        let j = kvfs(&mut b, m.shell).unwrap();
        assert_eq!(j.events().len(), 2);
        assert_eq!(shape(&b), before);
        assert!(b.names.is_empty());
        assert_eq!(b.recipes.as_ref().map(|r| r.len()), Some(0));
    }

    #[test]
    fn preconditions_leave_the_body_unchanged() {
        let (mut b, s) = plane_body();
        let dead = <SurfaceId as crate::topo::Id>::from_index(7);
        assert_eq!(
            mvfs(&mut b, K, Point3::ORIGIN, dead),
            Err(TopologyError::DeadEntity.into())
        );
        assert_eq!(
            mvfs(&mut b, K, Point3::new(0.0, 0.0, 600.0), s),
            Err(Error::OutsideSizeBox)
        );
        assert_eq!(
            mvfs(&mut b, K, Point3::new(f64::NAN, 0.0, 0.0), s),
            Err(Error::NonFinite)
        );
        assert_eq!(b.counts().faces, 0);
        let (m, _) = mvfs(&mut b, K, Point3::ORIGIN, s).unwrap();
        let before = b.dump();
        let dead = <ShellId as crate::topo::Id>::from_index(5);
        assert_eq!(kvfs(&mut b, dead), Err(TopologyError::DeadEntity.into()));
        assert_eq!(b.dump(), before);
        assert!(kvfs(&mut b, m.shell).is_ok());
    }
}
