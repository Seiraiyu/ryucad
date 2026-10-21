//! Naming a blend's entities, and the journal.
//!
//! Names come only from the blend's key and the names of the entities
//! the blend replaces or lies between, never from the order things were
//! built in (design 43):
//!
//! - a blend face from the blended edges it replaces, sorted, so resizing
//!   a blend renames nothing;
//! - a rail from its blend face and the face it rolls on, an end edge
//!   from its blend face, the end face and the vertex the blend took away
//!   there, a section from the two faces either side, sorted: two blend
//!   faces, or a blend face and a corner face;
//! - a corner face from the vertex it took away and the faces it touches,
//!   sorted;
//! - every vertex on a blend face from the edges meeting there, sorted;
//! - faces the blend trims and edges it shortens keep their names.
//!
//! The journal gives each blended edge, and each vertex the blend took
//! away, as [`Event::Replaced`] by the blend faces now where it was; the
//! new edges and vertices as `Created`; the faces and edges the blend
//! changed as `Modified`.

use super::build::Made;
use crate::name::{BlendRole, Event, Journal, Name, OpKey, Role};
use crate::topo::{Body, EntityId, FaceId, FinId, VertexId};
use std::collections::{BTreeMap, BTreeSet};

/// Names what the rebuild made and returns the journal.
pub(crate) fn name(body: &mut Body, key: OpKey, made: &Made) -> Journal {
    let role = |r: BlendRole| Role::Blend(r);
    let face_of = |b: &Body, f: FinId| b.loops[b.fins[f].loop_].face;
    let mut created: BTreeSet<Name> = BTreeSet::new();
    let mut modified: BTreeSet<EntityId> = made.modified.clone();
    // Edges operators renamed that keep their identity.
    for &(entity, old) in &made.keep {
        if body.name_of(entity).is_some() {
            body.rename(entity, old);
        }
    }
    // Blend faces.
    let mut faces: Vec<Name> = Vec::with_capacity(made.segments.len());
    for s in &made.segments {
        let mut parents = s.edges.clone();
        parents.sort();
        let n = body.derive_name("blend", key, role(BlendRole::Face), &parents);
        body.rename(EntityId::Face(s.face), n);
        faces.push(n);
    }
    // Corner faces, and the sections between them and the blend faces.
    let mut corner_faces: Vec<Name> = Vec::with_capacity(made.corners.len());
    for c in &made.corners {
        let mut parents = vec![c.vertex];
        parents.extend(&c.faces);
        let n = body.derive_name("blend", key, role(BlendRole::Corner), &parents);
        body.rename(EntityId::Face(c.face), n);
        corner_faces.push(n);
    }
    let segment_of = |edge: Name| -> usize {
        made.segments
            .iter()
            .position(|s| s.edges.contains(&edge))
            .expect("a blended edge's segment")
    };
    for (c, &cf) in made.corners.iter().zip(&corner_faces) {
        for &(e, edge) in &c.sections {
            let mut parents = [faces[segment_of(edge)], cf];
            parents.sort();
            let n = body.derive_name("blend", key, role(BlendRole::Section), &parents);
            body.rename(EntityId::Edge(e), n);
            created.insert(n);
        }
    }
    let blend_faces: BTreeSet<FaceId> = made
        .segments
        .iter()
        .map(|s| s.face)
        .chain(made.corners.iter().map(|c| c.face))
        .collect();
    // Rails, and the faces they roll on keep their names.
    for (k, s) in made.segments.iter().enumerate() {
        for side in 0..2 {
            let many = s.rails[side].len() > 1;
            for (i, &e) in s.rails[side].iter().enumerate() {
                let mut parents = vec![faces[k], s.faces[side]];
                if many {
                    parents.push(Name::from_u128(i as u128));
                }
                let n = body.derive_name("blend", key, role(BlendRole::Rail), &parents);
                body.rename(EntityId::Edge(e), n);
                created.insert(n);
                let fins: Vec<FinId> = body.edge_fins(e).collect();
                for f in fins {
                    let face = face_of(body, f);
                    if !blend_faces.contains(&face) {
                        if side == 0 && body.faces[face].name != s.faces[0] {
                            body.rename(EntityId::Face(face), s.faces[0]);
                        }
                        modified.insert(EntityId::Face(face));
                    }
                }
            }
        }
    }
    for &(e, k, end, vertex) in &made.ends {
        // A blend may end twice on one face: the vertex it took away tells
        // the ends apart.
        let n = body.derive_name(
            "blend",
            key,
            role(BlendRole::End),
            &[faces[k], body.faces[end].name, vertex],
        );
        body.rename(EntityId::Edge(e), n);
        created.insert(n);
    }
    for &(e, [a, b]) in &made.sections {
        let mut parents = [faces[a], faces[b]];
        parents.sort();
        let n = body.derive_name("blend", key, role(BlendRole::Section), &parents);
        body.rename(EntityId::Edge(e), n);
        created.insert(n);
    }
    // Every vertex on a blend face, from the edges meeting there.
    let mut vertices: BTreeSet<VertexId> = BTreeSet::new();
    for &f in &blend_faces {
        for l in body.face_loops(f) {
            if let Some(v) = body.loops[l].vertex {
                vertices.insert(v);
            }
            for fin in body.loop_fins(l) {
                vertices.insert(body.start_of(fin));
            }
        }
    }
    for v in vertices {
        let mut parents: Vec<Name> = body
            .vertex_edges(v)
            .iter()
            .map(|&e| body.edges[e].name)
            .collect();
        parents.sort();
        let n = body.derive_name("blend", key, role(BlendRole::Vertex), &parents);
        body.rename(EntityId::Vertex(v), n);
        created.insert(n);
    }
    // The journal.
    let mut journal = Journal::new();
    let mut replaced: BTreeMap<Name, Vec<Name>> = BTreeMap::new();
    for (k, s) in made.segments.iter().enumerate() {
        for &e in &s.edges {
            replaced.entry(e).or_default().push(faces[k]);
        }
    }
    for (old, segs) in &made.replaced {
        let entry = replaced.entry(*old).or_default();
        entry.extend(segs.iter().map(|&k| faces[k]));
    }
    for (c, &cf) in made.corners.iter().zip(&corner_faces) {
        let entry = replaced.entry(c.vertex).or_default();
        entry.push(cf);
        entry.extend(c.sections.iter().map(|&(_, e)| faces[segment_of(e)]));
    }
    for (old, mut new) in replaced {
        new.sort();
        new.dedup();
        journal.push(Event::Replaced { old, new });
    }
    for n in created {
        journal.push(Event::Created(n));
    }
    let mut changed: BTreeSet<Name> = BTreeSet::new();
    for entity in modified {
        if let Some(n) = body.name_of(entity)
            && !journal
                .events()
                .iter()
                .any(|e| matches!(e, Event::Created(c) if *c == n))
        {
            changed.insert(n);
        }
    }
    for n in changed {
        journal.push(Event::Modified(n));
    }
    journal
}
