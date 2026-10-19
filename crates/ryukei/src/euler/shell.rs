//! MVFS and KVFS: start and end a body's skeleton. MVS: another shell in a
//! solid region, which KVFS ends too. MVR and KVR: a lone vertex in a
//! face.

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

/// What [`mvs`] made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mvs {
    pub shell: ShellId,
    pub face: FaceId,
    pub loop_: LoopId,
    pub vertex: VertexId,
}

/// Make vertex, face, shell in a region: like [`mvfs`], but the new shell
/// joins the solid `region` instead of starting one. This is how a solid
/// gets a cavity: the shell grows into a closed void shell inside the
/// region's outer shell, its faces pointing into the cavity. [`kvfs`] is
/// its inverse.
///
/// Names: the face is `(key, NewFace)`, the vertex `(key, NewVertex)`.
pub fn mvs(
    body: &mut Body,
    key: OpKey,
    region: RegionId,
    point: Point3,
    surface: SurfaceId,
) -> Result<(Mvs, Journal), Error> {
    if live(&body.regions, region)?.kind != RegionKind::Solid {
        return Err(TopologyError::WrongRegion.into());
    }
    live(&body.surfaces, surface)?;
    let point = check_point(point)?;
    let (shell, face, loop_, vertex) = (
        next_id(&body.shells),
        next_id(&body.faces),
        next_id(&body.loops),
        next_id(&body.vertices),
    );
    let face_name = body.derive_name("mvs", key, Role::NewFace, &[]);
    let vertex_name = body.derive_name("mvs", key, Role::NewVertex, &[]);
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
        Mvs {
            shell,
            face,
            loop_,
            vertex,
        },
        journal,
    ))
}

/// Kill vertex, face, shell: the inverse of [`mvfs`] and [`mvs`]. The
/// shell must hold exactly one face with exactly one loop, a lone vertex.
/// Its region goes too when it has no other shell.
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

/// What [`mvr`] made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mvr {
    pub loop_: LoopId,
    pub vertex: VertexId,
}

/// Make vertex, ring: a new lone-vertex loop at `point` on `face`.
///
/// Names: the vertex is `(key, NewVertex, [face])`.
pub fn mvr(
    body: &mut Body,
    key: OpKey,
    face: FaceId,
    point: Point3,
) -> Result<(Mvr, Journal), Error> {
    live(&body.faces, face)?;
    let point = check_point(point)?;
    let (loop_, vertex) = (next_id(&body.loops), next_id(&body.vertices));
    let parent = body.faces[face].name;
    let name = body.derive_name("mvr", key, Role::NewVertex, &[parent]);
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
        name,
    });
    body.link_loop(face, loop_);
    body.index_name(EntityId::Vertex(vertex));
    let mut journal = Journal::new();
    journal.push(Event::Created(name));
    finish(body);
    Ok((Mvr { loop_, vertex }, journal))
}

/// Kill vertex, ring: the inverse of [`mvr`]. Removes a lone-vertex loop
/// and its vertex. It may be its face's last loop only when the face's
/// surface closes up (a sphere or torus), which leaves a face with no
/// loops.
pub fn kvr(body: &mut Body, loop_: LoopId) -> Result<Journal, Error> {
    let l = live(&body.loops, loop_)?;
    let vertex = l.vertex.ok_or(TopologyError::NotLone)?;
    let face = &body.faces[l.face];
    if l.next == loop_ && body.surfaces[face.surface].closed_genus().is_none() {
        return Err(TopologyError::NotRemovable.into());
    }
    let mut journal = Journal::new();
    journal.push(Event::Deleted(body.vertices[vertex].name));
    body.kill_vertex(vertex);
    body.unlink_loop(loop_);
    body.loops.kill(loop_);
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

    #[test]
    fn mvs_adds_a_shell_to_a_region_and_kvfs_takes_it_away() {
        let (mut b, s) = plane_body();
        let (m, _) = mvfs(&mut b, K, Point3::ORIGIN, s).unwrap();
        let before = shape(&b);
        let (v, j) = mvs(&mut b, OpKey(2), m.region, Point3::new(1.0, 0.0, 0.0), s).unwrap();
        assert_eq!(j.events().len(), 2);
        assert_eq!(b.shell(v.shell).unwrap().region, m.region);
        assert_eq!(
            b.region_shells(m.region).collect::<Vec<_>>(),
            [m.shell, v.shell]
        );
        assert_eq!(b.counts().regions, 2);
        assert_eq!(b.counts().shells, 2);
        kvfs(&mut b, v.shell).unwrap();
        assert_eq!(shape(&b), before);
        assert_eq!(b.region(m.region).unwrap().first_shell, Some(m.shell));
        // Never in the void around the body, nor in a dead region.
        let held = b.dump();
        let void = b.outer_region();
        assert_eq!(
            mvs(&mut b, K, void, Point3::ORIGIN, s),
            Err(TopologyError::WrongRegion.into())
        );
        let dead = <RegionId as crate::topo::Id>::from_index(9);
        assert_eq!(
            mvs(&mut b, K, dead, Point3::ORIGIN, s),
            Err(TopologyError::DeadEntity.into())
        );
        assert_eq!(b.dump(), held);
    }

    #[test]
    fn mvr_and_kvr() {
        let (mut b, s) = plane_body();
        let (m, _) = mvfs(&mut b, K, Point3::ORIGIN, s).unwrap();
        let before = shape(&b);
        let (r, j) = mvr(&mut b, OpKey(2), m.face, Point3::new(1.0, 0.0, 0.0)).unwrap();
        let face = Name::derive(K, Role::NewFace, &[]);
        let v = Name::derive(OpKey(2), Role::NewVertex, &[face]);
        assert_eq!(j.events(), &[Event::Created(v)]);
        assert_eq!(b.face_loops(m.face).collect::<Vec<_>>(), [m.loop_, r.loop_]);
        assert_eq!(b.vertex(r.vertex).unwrap().lone, Some(r.loop_));
        // Two lone vertices: no longer a skeleton.
        let snapshot = b.dump();
        assert_eq!(
            kvfs(&mut b, m.shell),
            Err(TopologyError::NotRemovable.into())
        );
        assert_eq!(b.dump(), snapshot);
        kvr(&mut b, r.loop_).unwrap();
        assert_eq!(shape(&b), before);
        // The last loop of a plane face stays.
        assert_eq!(
            kvr(&mut b, m.loop_),
            Err(TopologyError::NotRemovable.into())
        );
    }

    #[test]
    fn kvr_leaves_a_sphere_face_without_loops() {
        let mut b = Body::new();
        let sphere = crate::geom::Sphere::new(crate::geom::Frame::WORLD, 1.0).unwrap();
        let s = b.add_surface(crate::geom::Surface::Sphere(sphere));
        let (m, _) = mvfs(&mut b, K, Point3::new(1.0, 0.0, 0.0), s).unwrap();
        kvr(&mut b, m.loop_).unwrap();
        assert_eq!(b.face(m.face).unwrap().first_loop, None);
        assert_eq!(b.counts().vertices, 0);
        assert_eq!(crate::check::check_topology(&b), []);
    }
}
