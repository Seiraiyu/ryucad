use super::Fault;
use crate::topo::{Body, EdgeId, EntityId, FaceId, Id, RegionKind, ShellId, VertexId};
use std::collections::{BTreeMap, BTreeSet};

/// The members of a ring starting at `first`. `links(id)` gives a live
/// member's `(next, prev, owned)`, or `None` for a dead one. `None` when
/// the ring is broken: a dead or foreign member, a `prev` that does not
/// point back, or no return to `first` within `bound` steps.
fn walk<I: Copy + Eq>(
    first: I,
    bound: usize,
    links: impl Fn(I) -> Option<(I, I, bool)>,
) -> Option<Vec<I>> {
    let mut members = Vec::new();
    let mut cur = first;
    loop {
        let (next, _, owned) = links(cur)?;
        let (_, back, _) = links(next)?;
        if !owned || back != cur || members.len() >= bound {
            return None;
        }
        members.push(cur);
        cur = next;
        if cur == first {
            return Some(members);
        }
    }
}

/// Marks a walked ring's members, or reports the owner. Returns whether
/// the ring is sound.
fn mark<I: Id>(
    ring: Option<Vec<I>>,
    seen: &mut [bool],
    owner: EntityId,
    out: &mut Vec<Fault>,
) -> bool {
    match ring {
        Some(members) => {
            for m in members {
                seen[m.index()] = true;
            }
            true
        }
        None => {
            out.push(Fault::BrokenRing { owner });
            false
        }
    }
}

/// A live member missing from its owner's ring: a dead owner is a dead
/// reference; a live owner whose ring was otherwise sound has a broken
/// ring. (A broken ring was already reported.)
fn orphan(
    from: EntityId,
    owner: EntityId,
    owner_live: bool,
    owner_ring_ok: bool,
    out: &mut Vec<Fault>,
) {
    if !owner_live {
        out.push(Fault::DeadReference { from, to: owner });
    } else if owner_ring_ok {
        out.push(Fault::BrokenRing { owner });
    }
}

pub(super) fn structure(b: &Body, out: &mut Vec<Fault>) {
    // Regions own shells.
    if b.regions.get(b.outer_region()).is_none() {
        out.push(Fault::BadRegion {
            region: b.outer_region(),
        });
    }
    let mut shell_seen = vec![false; b.shells.slot_count()];
    let mut region_ok = vec![false; b.regions.slot_count()];
    for (r, region) in b.regions.live() {
        let void = region.kind == RegionKind::Void;
        if void != (r == b.outer_region()) || (!void && region.first_shell.is_none()) {
            out.push(Fault::BadRegion { region: r });
        }
        region_ok[r.index()] = match region.first_shell {
            None => true,
            Some(s0) => mark(
                walk(s0, b.shells.slot_count(), |s| {
                    b.shells.get(s).map(|x| (x.next, x.prev, x.region == r))
                }),
                &mut shell_seen,
                EntityId::Region(r),
                out,
            ),
        };
    }
    for (s, shell) in b.shells.live() {
        if !shell_seen[s.index()] {
            let live = b.regions.get(shell.region).is_some();
            let ok = live && region_ok[shell.region.index()];
            orphan(
                EntityId::Shell(s),
                EntityId::Region(shell.region),
                live,
                ok,
                out,
            );
        }
    }

    // Shells own faces.
    let mut face_seen = vec![false; b.faces.slot_count()];
    let mut shell_ok = vec![false; b.shells.slot_count()];
    for (s, shell) in b.shells.live() {
        shell_ok[s.index()] = match shell.first_face {
            None => true,
            Some(f0) => mark(
                walk(f0, b.faces.slot_count(), |f| {
                    b.faces.get(f).map(|x| (x.next, x.prev, x.shell == s))
                }),
                &mut face_seen,
                EntityId::Shell(s),
                out,
            ),
        };
    }
    for (f, face) in b.faces.live() {
        if !face_seen[f.index()] {
            let live = b.shells.get(face.shell).is_some();
            let ok = live && shell_ok[face.shell.index()];
            orphan(
                EntityId::Face(f),
                EntityId::Shell(face.shell),
                live,
                ok,
                out,
            );
        }
    }

    // Faces own loops.
    let mut loop_seen = vec![false; b.loops.slot_count()];
    let mut face_ok = vec![false; b.faces.slot_count()];
    for (f, face) in b.faces.live() {
        match b.surfaces.get(face.surface) {
            None => out.push(Fault::DeadGeometry {
                from: EntityId::Face(f),
            }),
            Some(s) if face.first_loop.is_none() && s.closed_genus().is_none() => {
                out.push(Fault::Loopless { face: f });
            }
            Some(_) => {}
        }
        face_ok[f.index()] = match face.first_loop {
            None => true,
            Some(l0) => mark(
                walk(l0, b.loops.slot_count(), |l| {
                    b.loops.get(l).map(|x| (x.next, x.prev, x.face == f))
                }),
                &mut loop_seen,
                EntityId::Face(f),
                out,
            ),
        };
    }
    for (l, lp) in b.loops.live() {
        if !loop_seen[l.index()] {
            let live = b.faces.get(lp.face).is_some();
            let ok = live && face_ok[lp.face.index()];
            orphan(EntityId::Loop(l), EntityId::Face(lp.face), live, ok, out);
        }
    }

    // Loops own fins or a lone vertex.
    let mut fin_seen = vec![false; b.fins.slot_count()];
    let mut loop_ok = vec![false; b.loops.slot_count()];
    for (l, lp) in b.loops.live() {
        match (lp.first_fin, lp.vertex) {
            (Some(f0), None) => {
                let ring = walk(f0, b.fins.slot_count(), |f| {
                    b.fins.get(f).map(|x| (x.next, x.prev, x.loop_ == l))
                });
                loop_ok[l.index()] = mark(ring.clone(), &mut fin_seen, EntityId::Loop(l), out);
                for f in ring.unwrap_or_default() {
                    let next = b.fins[f].next;
                    let ends = [f, next].map(|x| b.edges.get(b.fins[x].edge).map(|_| x));
                    if let [Some(f), Some(next)] = ends
                        && b.end_of(f) != b.start_of(next)
                    {
                        out.push(Fault::Disjoint { fin: f });
                    }
                }
            }
            (None, Some(v)) => {
                loop_ok[l.index()] = true;
                match b.vertices.get(v) {
                    None => out.push(Fault::DeadReference {
                        from: EntityId::Loop(l),
                        to: EntityId::Vertex(v),
                    }),
                    Some(vx) if vx.lone != Some(l) || vx.edge.is_some() => {
                        out.push(Fault::BadLoop { loop_: l });
                    }
                    Some(_) => {}
                }
            }
            _ => out.push(Fault::BadLoop { loop_: l }),
        }
    }
    for (f, fin) in b.fins.live() {
        if !fin_seen[f.index()] {
            let live = b.loops.get(fin.loop_).is_some();
            let ok = live && loop_ok[fin.loop_.index()];
            orphan(EntityId::Fin(f), EntityId::Loop(fin.loop_), live, ok, out);
        }
    }

    edges(b, out);
    vertices(b, out);
}

/// Each edge's ring of fins, its vertices and its curve.
fn edges(b: &Body, out: &mut Vec<Fault>) {
    let mut around_seen = vec![false; b.fins.slot_count()];
    let mut edge_ok = vec![false; b.edges.slot_count()];
    for (e, edge) in b.edges.live() {
        let from = EntityId::Edge(e);
        if b.curves.get(edge.curve).is_none() {
            out.push(Fault::DeadGeometry { from });
        }
        let ends = if edge.start == edge.end {
            &[edge.start][..]
        } else {
            &[edge.start, edge.end][..]
        };
        for &v in ends {
            match b.vertices.get(v) {
                None => out.push(Fault::DeadReference {
                    from,
                    to: EntityId::Vertex(v),
                }),
                Some(vx) if vx.edge.is_none() => out.push(Fault::BadVertex { vertex: v }),
                Some(_) => {}
            }
        }
        // The ring of fins on an edge is singly linked through `around`.
        let mut fins = Vec::new();
        let mut cur = edge.fin;
        let ring_ok = loop {
            match b.fins.get(cur) {
                Some(f) if f.edge == e && fins.len() < b.fins.slot_count() => {
                    fins.push(cur);
                    cur = f.around;
                    if cur == edge.fin {
                        break true;
                    }
                }
                _ => break false,
            }
        };
        if !ring_ok {
            out.push(Fault::BrokenRing { owner: from });
            continue;
        }
        edge_ok[e.index()] = true;
        for &f in &fins {
            around_seen[f.index()] = true;
        }
        if fins.len() != 2 {
            out.push(Fault::NotManifold {
                edge: e,
                fins: fins.len(),
            });
        } else if b.fins[fins[0]].sense == b.fins[fins[1]].sense {
            out.push(Fault::SameSense { edge: e });
        }
    }
    for (f, fin) in b.fins.live() {
        if !around_seen[f.index()] {
            let live = b.edges.get(fin.edge).is_some();
            let ok = live && edge_ok[fin.edge.index()];
            orphan(EntityId::Fin(f), EntityId::Edge(fin.edge), live, ok, out);
        }
    }
}

/// Each vertex's back-reference.
fn vertices(b: &Body, out: &mut Vec<Fault>) {
    for (v, vx) in b.vertices.live() {
        let sound = match (vx.edge, vx.lone) {
            (Some(e), None) => b.edges.get(e).is_some_and(|e| e.start == v || e.end == v),
            (None, Some(l)) => b.loops.get(l).is_some_and(|l| l.vertex == Some(v)),
            _ => false,
        };
        if !sound {
            out.push(Fault::BadVertex { vertex: v });
        }
    }
}

/// `V - E + Σ_f (2 - 2 g_f - L_f)` for one shell, with `g_f` the genus of
/// a closed face surface (0 for open surfaces).
pub(super) fn euler_characteristic(b: &Body, s: ShellId) -> i64 {
    let mut vertices: BTreeSet<VertexId> = BTreeSet::new();
    let mut edges: BTreeSet<EdgeId> = BTreeSet::new();
    let mut sum = 0i64;
    for f in b.shell_faces(s) {
        let genus = b.surfaces[b.faces[f].surface].closed_genus().unwrap_or(0);
        let mut loops = 0i64;
        for l in b.face_loops(f) {
            loops += 1;
            if let Some(v) = b.loops[l].vertex {
                vertices.insert(v);
            }
            for fin in b.loop_fins(l) {
                vertices.insert(b.start_of(fin));
                edges.insert(b.fins[fin].edge);
            }
        }
        sum += 2 - 2 * i64::from(genus) - loops;
    }
    vertices.len() as i64 - edges.len() as i64 + sum
}

/// Connectivity and the Euler–Poincaré formula, for a structurally sound
/// body.
pub(super) fn shells(b: &Body, out: &mut Vec<Fault>) {
    for s in b.shells() {
        let faces: Vec<FaceId> = b.shell_faces(s).collect();
        let mut reached: BTreeSet<FaceId> = BTreeSet::new();
        let mut stack: Vec<FaceId> = faces.first().copied().into_iter().collect();
        let mut across: Vec<EdgeId> = Vec::new();
        while let Some(f) = stack.pop() {
            if !reached.insert(f) {
                continue;
            }
            for l in b.face_loops(f) {
                for fin in b.loop_fins(l) {
                    let e = b.fins[fin].edge;
                    for other in b.edge_fins(e) {
                        let g = b.loops[b.fins[other].loop_].face;
                        if b.faces[g].shell != s {
                            if !across.contains(&e) {
                                across.push(e);
                            }
                        } else if !reached.contains(&g) {
                            stack.push(g);
                        }
                    }
                }
            }
        }
        across.sort();
        for edge in across {
            out.push(Fault::EdgeAcrossShells { edge });
        }
        if reached.len() != faces.len() {
            out.push(Fault::Disconnected { shell: s });
        }
        let chi = euler_characteristic(b, s);
        if chi % 2 != 0 || chi > 2 {
            out.push(Fault::EulerPoincare { shell: s, chi });
        }
    }
}

/// Every live face, edge and vertex has a unique name, and the index holds
/// exactly those names.
pub(super) fn names(b: &Body, out: &mut Vec<Fault>) {
    let mut held: BTreeMap<crate::name::Name, Vec<EntityId>> = BTreeMap::new();
    for (f, face) in b.faces.live() {
        held.entry(face.name).or_default().push(EntityId::Face(f));
    }
    for (e, edge) in b.edges.live() {
        held.entry(edge.name).or_default().push(EntityId::Edge(e));
    }
    for (v, vx) in b.vertices.live() {
        held.entry(vx.name).or_default().push(EntityId::Vertex(v));
    }
    let names: BTreeSet<_> = held.keys().chain(b.names.keys()).copied().collect();
    for name in names {
        match (held.get(&name).map(Vec::as_slice), b.names.get(&name)) {
            (Some([one]), Some(indexed)) if one == indexed => {}
            (Some([_, _, ..]), _) => out.push(Fault::DuplicateName { name }),
            _ => out.push(Fault::NameIndex { name }),
        }
    }
}
