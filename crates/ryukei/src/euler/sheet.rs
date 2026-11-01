//! The sheet operators (design 295): MVSR moves a shell between the void
//! around everything and a solid region of its own; KFL and MFL kill and
//! make a face on a sheet's boundary, leaving or taking laminar edges.

use super::{finish, live, next_id};
use crate::name::{Event, Journal, Name, OpKey, Role};
use crate::topo::{
    Body, EdgeId, EntityId, Face, FaceId, Fin, FinId, Id, Loop, LoopId, Region, RegionId,
    RegionKind, Sense, ShellId, SurfaceId, VertexId,
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

/// Kill face, laminar: kills a face of a sheet, leaving each edge it
/// shared with one fin fewer. An inner edge becomes laminar; a laminar
/// edge, or one the face used twice, goes; a vertex left on no edge goes.
/// The shell must be a sheet's (`SolidShell`: open a solid by making it a
/// closed sheet with [`mvsr`] first), keep another face (`NotRemovable`),
/// stay joined (`WouldDisconnect`), and leave every vertex one fan
/// (`NonManifoldVertex`). [`mfl`] makes a face back on a boundary.
///
/// Names: the face, and each edge and vertex that goes, retire
/// (`Deleted`); each edge left laminar is `Modified`.
pub fn kfl(body: &mut Body, face: FaceId) -> Result<(Kfl, Journal), Error> {
    let shell = live(&body.faces, face)?.shell;
    if body.shells[shell].region != body.outer_region() {
        return Err(TopologyError::SolidShell.into());
    }
    if body.faces[face].next == face {
        return Err(TopologyError::NotRemovable.into());
    }
    let fins: BTreeSet<FinId> = body
        .face_loops(face)
        .flat_map(|l| body.loop_fins(l))
        .collect();
    // The other faces stay joined across the edges they keep.
    let rest: Vec<FaceId> = body.shell_faces(shell).filter(|&f| f != face).collect();
    let mut reached: BTreeSet<FaceId> = BTreeSet::new();
    let mut stack = vec![rest[0]];
    while let Some(f) = stack.pop() {
        if !reached.insert(f) {
            continue;
        }
        for l in body.face_loops(f) {
            for x in body.loop_fins(l) {
                for y in body.edge_fins(body.fins[x].edge) {
                    let g = body.loops[body.fins[y].loop_].face;
                    if g != face && !reached.contains(&g) {
                        stack.push(g);
                    }
                }
            }
        }
    }
    if reached.len() != rest.len() {
        return Err(TopologyError::WouldDisconnect.into());
    }
    // Each vertex the face touches keeps its other corners in one run.
    for &x in &fins {
        let (fan, closed) = body.fan_from(x);
        let kept: Vec<bool> = fan.iter().map(|c| !fins.contains(c)).collect();
        if runs(&kept, closed) > 1 {
            return Err(TopologyError::NonManifoldVertex.into());
        }
    }
    let face_name = body.faces[face].name;
    let mut edge_names: BTreeMap<EdgeId, Name> = BTreeMap::new();
    let mut vertex_names: BTreeMap<VertexId, Name> = BTreeMap::new();
    for &x in &fins {
        let e = body.fins[x].edge;
        edge_names.insert(e, body.edges[e].name);
        let v = body.start_of(x);
        vertex_names.insert(v, body.vertices[v].name);
    }
    for l in body.face_loops(face) {
        if let Some(v) = body.loops[l].vertex {
            vertex_names.insert(v, body.vertices[v].name);
        }
    }
    let done = remove_face(body, face);
    let mut journal = Journal::new();
    journal.push(Event::Deleted(face_name));
    for e in &done.edges {
        journal.push(Event::Deleted(edge_names[e]));
    }
    for v in &done.vertices {
        journal.push(Event::Deleted(vertex_names[v]));
    }
    for e in &done.laminar {
        journal.push(Event::Modified(edge_names[e]));
    }
    finish(body);
    Ok((done, journal))
}

/// How many runs of kept corners a fan has; round a closed fan the first
/// and last runs are one.
fn runs(kept: &[bool], closed: bool) -> usize {
    let starts = (0..kept.len())
        .filter(|&i| kept[i] && (i == 0 || !kept[i - 1]))
        .count();
    if closed && starts > 1 && kept[0] && kept[kept.len() - 1] {
        starts - 1
    } else {
        starts
    }
}

/// What [`mfl`] made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mfl {
    pub face: FaceId,
    pub loop_: LoopId,
}

/// Make face, laminar: the inverse of [`kfl`] for a face of one loop. A
/// new face on `surface` with `sense`, bounded by the boundary through
/// the laminar `fin`, its one loop running that boundary's fins the other
/// way (the new face on its left), so every edge of the boundary becomes
/// an inner edge. New fins have no pcurve. `NotLaminar` unless `fin`'s
/// edge has one fin; `SolidShell` unless its shell is a sheet's.
///
/// Names: the face is `(key, NewFace)`.
pub fn mfl(
    body: &mut Body,
    key: OpKey,
    fin: FinId,
    surface: SurfaceId,
    sense: Sense,
) -> Result<(Mfl, Journal), Error> {
    live(&body.fins, fin)?;
    live(&body.surfaces, surface)?;
    if body.fins[fin].around != fin {
        return Err(TopologyError::NotLaminar.into());
    }
    let shell = body.faces[body.loops[body.fins[fin].loop_].face].shell;
    if body.shells[shell].region != body.outer_region() {
        return Err(TopologyError::SolidShell.into());
    }
    let mut ring = vec![fin];
    loop {
        let n = body.boundary_next(ring[ring.len() - 1]);
        if n == fin {
            break;
        }
        if ring.len() > body.fins.slot_count() {
            return Err(TopologyError::Inconsistent.into());
        }
        ring.push(n);
    }
    let (face, loop_, first) = (
        next_id(&body.faces),
        next_id(&body.loops),
        next_id(&body.fins),
    );
    let n = ring.len();
    let new: Vec<FinId> = (0..n)
        .map(|i| <FinId as Id>::from_index(first.index() + i))
        .collect();
    let name = body.derive_name("mfl", key, Role::NewFace, &[]);
    body.faces.push(Face {
        shell,
        next: face,
        prev: face,
        first_loop: None,
        surface,
        sense,
        name,
    });
    body.loops.push(Loop {
        face,
        next: loop_,
        prev: loop_,
        first_fin: Some(new[0]),
        vertex: None,
    });
    // Ring fin i runs from s_i to s_(i+1); its twin runs back, so the fin
    // after it starts at s_i: twin i - 1.
    for (i, &b) in ring.iter().enumerate() {
        let (edge, s) = (body.fins[b].edge, body.fins[b].sense.flipped());
        body.fins.push(Fin {
            loop_,
            edge,
            sense: s,
            next: new[(i + n - 1) % n],
            prev: new[(i + 1) % n],
            around: b,
            pcurve: None,
        });
        body.fins[b].around = new[i];
    }
    body.link_face(shell, face);
    body.link_loop(face, loop_);
    body.index_name(EntityId::Face(face));
    let mut journal = Journal::new();
    journal.push(Event::Created(name));
    finish(body);
    Ok((Mfl { face, loop_ }, journal))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::check::{check, check_topology};
    use crate::euler::tests::K;
    use crate::euler::tests::shape;
    use crate::geom::Rigid;
    use crate::name::{Event, FaceRole, Name, Role};
    use crate::primitive::block;
    use crate::topo::{BodyKind, EntityId, FaceId, Id, Sense};

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

    /// The block `1 × 0.6 × 0.4` as a closed sheet.
    pub(crate) fn closed_box() -> Body {
        let (mut b, _) = block(K, 1.0, 0.6, 0.4, &Rigid::IDENTITY).unwrap();
        let s = b.shells().next().unwrap();
        mvsr(&mut b, s).unwrap();
        b
    }

    pub(crate) fn face_of(b: &Body, r: FaceRole) -> FaceId {
        match b.by_name(Name::derive(K, Role::Face(r), &[])) {
            Some(EntityId::Face(f)) => f,
            other => panic!("{r:?}: {other:?}"),
        }
    }

    #[test]
    fn a_box_opens_at_its_top() {
        let mut b = closed_box();
        let top = face_of(&b, FaceRole::PlusZ);
        let name = b.face(top).unwrap().name;
        let (k, j) = kfl(&mut b, top).unwrap();
        assert_eq!(
            (k.laminar.len(), k.edges.len(), k.vertices.len()),
            (4, 0, 0)
        );
        assert_eq!(j.events()[0], Event::Deleted(name));
        let modified = j
            .events()
            .iter()
            .filter(|e| matches!(e, Event::Modified(_)))
            .count();
        assert_eq!(modified, 4);
        assert_eq!(b.faces().count(), 5);
        assert!(k.laminar.iter().all(|&e| b.is_laminar(e)));
        assert_eq!(check(&b), []);
    }

    #[test]
    fn kfl_refuses_what_would_not_be_a_sheet() {
        let (mut solid, _) = block(K, 1.0, 0.6, 0.4, &Rigid::IDENTITY).unwrap();
        let top = face_of(&solid, FaceRole::PlusZ);
        assert_eq!(
            kfl(&mut solid, top).unwrap_err(),
            TopologyError::SolidShell.into()
        );
        // A band of four sides, then three: taking a side between the two
        // left would part them.
        let mut b = closed_box();
        for r in [FaceRole::PlusZ, FaceRole::MinusZ, FaceRole::PlusX] {
            let f = face_of(&b, r);
            kfl(&mut b, f).unwrap();
        }
        let before = shape(&b);
        let side = face_of(&b, FaceRole::MinusX);
        assert_eq!(
            kfl(&mut b, side).unwrap_err(),
            TopologyError::WouldDisconnect.into()
        );
        assert_eq!(shape(&b), before);
        // An octahedron opened at one face of its top, then the opposite
        // one: what is left would meet at the top alone.
        let mut o = crate::topo::fixture::octahedron();
        let s = o.shells().next().unwrap();
        mvsr(&mut o, s).unwrap();
        kfl(&mut o, FaceId::from_index(0)).unwrap();
        let before = shape(&o);
        assert_eq!(
            kfl(&mut o, FaceId::from_index(2)).unwrap_err(),
            TopologyError::NonManifoldVertex.into()
        );
        assert_eq!(shape(&o), before);
    }

    #[test]
    fn a_sheets_last_face_stays() {
        // Off the top, then the sides: each side left still stands on the
        // bottom, so the rest stays joined.
        let mut last = closed_box();
        let keep = face_of(&last, FaceRole::MinusZ);
        let order = [
            FaceRole::PlusZ,
            FaceRole::PlusX,
            FaceRole::PlusY,
            FaceRole::MinusX,
            FaceRole::MinusY,
        ];
        for r in order {
            let f = face_of(&last, r);
            kfl(&mut last, f).unwrap();
        }
        assert_eq!(last.faces().collect::<Vec<_>>(), [keep]);
        assert_eq!(check(&last), []);
        assert_eq!(
            kfl(&mut last, keep).unwrap_err(),
            TopologyError::NotRemovable.into()
        );
    }

    #[test]
    fn mfl_puts_the_top_back() {
        let mut b = closed_box();
        let top = face_of(&b, FaceRole::PlusZ);
        let (surface, sense) = (b.faces[top].surface, b.faces[top].sense);
        let (k, _) = kfl(&mut b, top).unwrap();
        let fin = b.edges[k.laminar[0]].fin;
        let (m, j) = mfl(&mut b, crate::name::OpKey(0x20), fin, surface, sense).unwrap();
        assert_eq!(j.events().len(), 1);
        assert_eq!(b.face_loops(m.face).count(), 1);
        assert_eq!(b.loop_fins(m.loop_).count(), 4);
        assert!(b.edges().all(|e| !b.is_laminar(e)));
        assert_eq!(check_topology(&b), []);
        // Closed again, it encloses the block's volume once more.
        crate::pcurve::fill(&mut b).unwrap();
        let s = b.shells().next().unwrap();
        mvsr(&mut b, s).unwrap();
        assert_eq!(check(&b), []);
    }

    #[test]
    fn mfl_needs_a_laminar_fin_of_a_sheet() {
        let mut b = closed_box();
        let s = b.faces[face_of(&b, FaceRole::PlusZ)].surface;
        let inner = b.fins().next().unwrap();
        let before = shape(&b);
        assert_eq!(
            mfl(&mut b, crate::name::OpKey(0x21), inner, s, Sense::Forward).unwrap_err(),
            TopologyError::NotLaminar.into()
        );
        assert_eq!(shape(&b), before);
    }

    #[test]
    fn a_laminar_edge_splits_and_joins_again() {
        let mut b = closed_box();
        let top = face_of(&b, FaceRole::PlusZ);
        let (k, _) = kfl(&mut b, top).unwrap();
        let before = shape(&b);
        let e = k.laminar[0];
        let [t0, t1] = b.edges[e].interval;
        let t = (t0 + t1) / 2.0;
        let p = b.curves[b.edges[e].curve].point(t);
        let (s, _) = crate::euler::semv(&mut b, crate::name::OpKey(0x22), e, t, p).unwrap();
        assert!(b.is_laminar(e) && b.is_laminar(s.edge));
        assert_eq!(check(&b), []);
        crate::euler::jekv(&mut b, crate::name::OpKey(0x23), s.vertex).unwrap();
        assert_eq!(shape(&b), before);
    }

    #[test]
    fn a_sheet_face_splits_across_its_laminar_edges() {
        let mut b = closed_box();
        let top = face_of(&b, FaceRole::PlusZ);
        kfl(&mut b, top).unwrap();
        // A side face: two of its corners on the open rim, two below.
        let side = face_of(&b, FaceRole::PlusX);
        let lp = b.face_loops(side).next().unwrap();
        let fins: Vec<FinId> = b.loop_fins(lp).collect();
        let (p, q) = (
            b.vertices[b.start_of(fins[0])].point,
            b.vertices[b.start_of(fins[2])].point,
        );
        let (c, i) = crate::euler::tests::line(&mut b, p, q);
        let at = crate::euler::Corner::Before;
        crate::euler::mef(
            &mut b,
            crate::name::OpKey(0x24),
            at(fins[0]),
            at(fins[2]),
            c,
            i,
        )
        .unwrap();
        assert_eq!(check_topology(&b), []);
        assert_eq!(b.faces().count(), 6);
    }
}
