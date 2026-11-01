//! The sheet operators (design 295): MVSR moves a shell between the void
//! around everything and a solid region of its own; KFL and MFL kill and
//! make a face on a sheet's boundary, leaving or taking laminar edges.

use super::{finish, live, next_id};
use crate::name::Journal;
use crate::topo::{
    Body, EdgeId, FaceId, FinId, LoopId, Region, RegionId, RegionKind, ShellId, VertexId,
};
use crate::{Error, TopologyError};
use std::collections::{BTreeMap, BTreeSet};

/// What [`mvsr`] did: the solid region the shell went into, or `None`
/// when it went out into the void.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mvsr {
    pub region: Option<RegionId>,
}

/// Move shell region: a solid region's only shell goes out into the void
/// region and the solid region goes (a solid becomes a closed sheet); a
/// closed shell in the void, one with no laminar edges, goes into a new
/// solid region (a closed sheet becomes a solid, its faces' normals its
/// outside). Its own inverse. A solid with cavities is `NotRemovable`; a
/// shell with laminar edges can't enclose anything (`OpenShell`).
///
/// Names: nothing is renamed, so the journal is empty.
pub fn mvsr(body: &mut Body, shell: ShellId) -> Result<(Mvsr, Journal), Error> {
    let region = live(&body.shells, shell)?.region;
    let outer = body.outer_region();
    if region == outer {
        let open = body.shell_faces(shell).any(|f| {
            body.face_loops(f)
                .any(|l| body.loop_fins(l).any(|x| body.fins[x].around == x))
        });
        if open {
            return Err(TopologyError::OpenShell.into());
        }
        let r = next_id(&body.regions);
        body.unlink_shell(shell);
        body.regions.push(Region {
            kind: RegionKind::Solid,
            first_shell: None,
        });
        body.link_shell(r, shell);
        finish(body);
        Ok((Mvsr { region: Some(r) }, Journal::new()))
    } else {
        if body.shells[shell].next != shell {
            return Err(TopologyError::NotRemovable.into());
        }
        body.unlink_shell(shell);
        body.regions.kill(region);
        body.link_shell(outer, shell);
        finish(body);
        Ok((Mvsr { region: None }, Journal::new()))
    }
}

/// What [`kfl`] did to the face's edges and vertices, each list in ID
/// order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Kfl {
    /// Edges the face shared, left with one fin.
    pub laminar: Vec<EdgeId>,
    /// Edges only the face used, gone with it.
    pub edges: Vec<EdgeId>,
    /// Vertices left on no edge, gone too (and the face's lone vertices).
    pub vertices: Vec<VertexId>,
}

/// Kills `face`, its loops and its fins without checking anything: each
/// edge loses the face's fins, an edge left with none goes, and a vertex
/// left on no edge goes. [`kfl`] checks first; the checker's tests use
/// this to make bodies that aren't sound.
pub(crate) fn remove_face(body: &mut Body, face: FaceId) -> Kfl {
    let loops: Vec<LoopId> = body.face_loops(face).collect();
    let fins: BTreeSet<FinId> = loops.iter().flat_map(|&l| body.loop_fins(l)).collect();
    let edges: BTreeSet<EdgeId> = fins.iter().map(|&f| body.fins[f].edge).collect();
    let mut touched: BTreeSet<VertexId> = BTreeSet::new();
    for &e in &edges {
        touched.insert(body.edges[e].start);
        touched.insert(body.edges[e].end);
    }
    // Every edge at each vertex the face touches, read while the body is
    // whole.
    let around: BTreeMap<VertexId, Vec<EdgeId>> =
        touched.iter().map(|&v| (v, body.vertex_edges(v))).collect();
    let lone: Vec<VertexId> = loops.iter().filter_map(|&l| body.loops[l].vertex).collect();
    let mut out = Kfl::default();
    for &e in &edges {
        let keep: Vec<FinId> = body.edge_fins(e).filter(|f| !fins.contains(f)).collect();
        if keep.is_empty() {
            let name = body.edges[e].name;
            body.retire_name(name);
            body.edges.kill(e);
            out.edges.push(e);
        } else {
            for (i, &f) in keep.iter().enumerate() {
                body.fins[f].around = keep[(i + 1) % keep.len()];
            }
            body.edges[e].fin = keep[0];
            if keep.len() == 1 {
                out.laminar.push(e);
            }
        }
    }
    for &f in &fins {
        body.fins.kill(f);
    }
    for &l in &loops {
        body.loops.kill(l);
    }
    body.kill_face(face);
    for (&v, list) in &around {
        match list.iter().copied().find(|&e| body.edges.get(e).is_some()) {
            Some(e) => body.vertices[v].edge = Some(e),
            None => {
                body.kill_vertex(v);
                out.vertices.push(v);
            }
        }
    }
    for v in lone {
        body.kill_vertex(v);
        out.vertices.push(v);
    }
    out.vertices.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::check::{check, check_topology};
    use crate::euler::tests::K;
    use crate::geom::Rigid;
    use crate::primitive::block;
    use crate::topo::BodyKind;

    #[test]
    fn a_solid_goes_out_into_the_void_and_back() {
        let (mut b, _) = block(K, 1.0, 0.6, 0.4, &Rigid::IDENTITY).unwrap();
        let s = b.shells().next().unwrap();
        let (m, j) = mvsr(&mut b, s).unwrap();
        assert_eq!(m.region, None);
        assert!(j.is_empty());
        assert_eq!(b.kind(), BodyKind::Sheet);
        assert_eq!(b.regions().count(), 1);
        assert_eq!(check_topology(&b), []);
        let (m, _) = mvsr(&mut b, s).unwrap();
        assert!(m.region.is_some());
        assert_eq!(b.kind(), BodyKind::Solid);
        assert_eq!(check(&b), []);
    }

    #[test]
    fn a_solid_with_a_cavity_stays_one() {
        let mut b = crate::topo::fixture::hollow_cube();
        let s = b.shells().next().unwrap();
        assert_eq!(
            mvsr(&mut b, s).unwrap_err(),
            TopologyError::NotRemovable.into()
        );
        assert_eq!(b.kind(), BodyKind::Solid);
    }
}
