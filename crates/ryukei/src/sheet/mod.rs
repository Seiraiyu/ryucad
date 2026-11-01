//! Sheets from faces (design 295, plan 12.A). [`extract`] copies the
//! named faces of a solid or a sheet into a sheet; [`remove`] copies the
//! rest. Edges the copied faces share stay inner edges, the others become
//! laminar, and every face, edge and vertex keeps its name. The faces
//! must make a manifold sheet: two of them meeting at a vertex alone is
//! `NonManifoldVertex`.

use crate::TopologyError;
use crate::error::{Error, SheetError};
use crate::layout::{self, Boundary, BuildError, Layout, Use};
use crate::name::{Event, Journal, Name, OpKey, Role};
use crate::topo::{Body, EdgeId, EntityId, FaceId, VertexId};
use std::collections::{BTreeMap, BTreeSet};

/// The scaffold names' role: everything is renamed after building.
const ROLE: Role = Role::Merged;

/// The faces named in `faces` as a sheet, and the journal of what
/// happened to `body`'s names: faces, edges and vertices left behind are
/// `Deleted`, edges left with one face `Modified`.
pub fn extract(key: OpKey, body: &Body, faces: &[Name]) -> Result<(Body, Journal), Error> {
    let chosen = resolve(body, faces)?;
    build(key, body, &chosen)
}

/// Every face but those named in `faces`, as a sheet: [`extract`] of the
/// rest. Naming none copies the whole body as a closed sheet.
pub fn remove(key: OpKey, body: &Body, faces: &[Name]) -> Result<(Body, Journal), Error> {
    let named = resolve(body, faces)?;
    let rest: BTreeSet<FaceId> = body.faces().filter(|f| !named.contains(f)).collect();
    build(key, body, &rest)
}

fn resolve(body: &Body, faces: &[Name]) -> Result<BTreeSet<FaceId>, Error> {
    faces
        .iter()
        .map(|&name| match body.by_name(name) {
            Some(EntityId::Face(f)) => Ok(f),
            _ => Err(SheetError::NoSuchFace { name }.into()),
        })
        .collect()
}

/// The layout index of `id`, made by `make` the first time it is asked.
fn slot<I: Ord + Copy, T>(
    map: &mut BTreeMap<I, usize>,
    list: &mut Vec<T>,
    id: I,
    make: impl FnOnce() -> T,
) -> usize {
    *map.entry(id).or_insert_with(|| {
        list.push(make());
        list.len() - 1
    })
}

fn build(key: OpKey, body: &Body, chosen: &BTreeSet<FaceId>) -> Result<(Body, Journal), Error> {
    if chosen.is_empty() {
        return Err(SheetError::NoFaces.into());
    }
    // The chosen faces in body order, each piece of geometry and each edge
    // and vertex once.
    let mut out = Layout::default();
    let (mut surfaces, mut curves, mut curves2) =
        (BTreeMap::new(), BTreeMap::new(), BTreeMap::new());
    let mut edges: BTreeMap<EdgeId, usize> = BTreeMap::new();
    let mut vertices: BTreeMap<VertexId, usize> = BTreeMap::new();
    let vertex = |v: VertexId| layout::Vertex {
        point: body.vertices[v].point,
        tolerance: body.vertices[v].tolerance.get(),
    };
    for &f in chosen {
        let face = &body.faces[f];
        let surface = slot(&mut surfaces, &mut out.surfaces, face.surface, || {
            body.surfaces[face.surface].clone()
        });
        let mut loops = Vec::new();
        for l in body.face_loops(f) {
            if let Some(v) = body.loops[l].vertex {
                loops.push(Boundary::Lone(slot(
                    &mut vertices,
                    &mut out.vertices,
                    v,
                    || vertex(v),
                )));
                continue;
            }
            let mut uses = Vec::new();
            for x in body.loop_fins(l) {
                let fin = &body.fins[x];
                let e = &body.edges[fin.edge];
                let start = slot(&mut vertices, &mut out.vertices, e.start, || {
                    vertex(e.start)
                });
                let end = slot(&mut vertices, &mut out.vertices, e.end, || vertex(e.end));
                let curve = slot(&mut curves, &mut out.curves, e.curve, || {
                    body.curves[e.curve].clone()
                });
                let edge = slot(&mut edges, &mut out.edges, fin.edge, || layout::Edge {
                    start,
                    end,
                    curve,
                    interval: e.interval,
                    tolerance: e.tolerance.get(),
                });
                let pcurve = fin.pcurve.map(|p| {
                    let c = slot(&mut curves2, &mut out.curves2, p.curve, || {
                        body.curves2[p.curve].clone()
                    });
                    (c, p.map)
                });
                uses.push(Use {
                    edge,
                    sense: fin.sense,
                    pcurve,
                });
            }
            loops.push(Boundary::Arcs(uses));
        }
        out.faces.push(layout::Face {
            surface,
            sense: face.sense,
            loops,
        });
    }
    let built = layout::build_sheet(&out, key, ROLE).map_err(|e| match e {
        BuildError::NonManifold { at, .. } => {
            match vertices.keys().find(|&&v| body.vertices[v].point == at) {
                Some(&v) => SheetError::NonManifoldVertex {
                    vertex: body.vertices[v].name,
                }
                .into(),
                None => Error::Topology(TopologyError::Inconsistent),
            }
        }
        BuildError::Inconsistent => Error::Topology(TopologyError::Inconsistent),
    })?;
    let edge_of: BTreeMap<usize, EdgeId> = edges.iter().map(|(&e, &g)| (g, e)).collect();
    let vertex_of: BTreeMap<usize, VertexId> = vertices.iter().map(|(&v, &g)| (g, v)).collect();
    let mut sheet = built.body;
    sheet.set_recipes(true);
    for (&f, &made) in chosen.iter().zip(&built.faces) {
        name_as(&mut sheet, body, EntityId::Face(made), body.faces[f].name);
    }
    for (g, &made) in &built.edges {
        name_as(
            &mut sheet,
            body,
            EntityId::Edge(made),
            body.edges[edge_of[g]].name,
        );
    }
    for (g, &made) in &built.vertices {
        name_as(
            &mut sheet,
            body,
            EntityId::Vertex(made),
            body.vertices[vertex_of[g]].name,
        );
    }
    let mut journal = Journal::new();
    for f in body.faces().filter(|f| !chosen.contains(f)) {
        journal.push(Event::Deleted(body.faces[f].name));
    }
    for e in body.edges() {
        match edges.get(&e) {
            None => journal.push(Event::Deleted(body.edges[e].name)),
            Some(g) if sheet.is_laminar(built.edges[g]) && !body.is_laminar(e) => {
                journal.push(Event::Modified(body.edges[e].name));
            }
            Some(_) => {}
        }
    }
    for v in body.vertices().filter(|v| !vertices.contains_key(v)) {
        journal.push(Event::Deleted(body.vertices[v].name));
    }
    if cfg!(debug_assertions) {
        let faults = crate::check::check(&sheet);
        assert!(
            faults.is_empty(),
            "a sheet made from faces is invalid: {faults:?}"
        );
    }
    Ok((sheet, journal))
}

/// Names an entity of the sheet as its source was, with the source's
/// recipe.
fn name_as(sheet: &mut Body, from: &Body, entity: EntityId, name: Name) {
    sheet.rename(entity, name);
    if let (Some(recipes), Some(text)) = (&mut sheet.recipes, from.recipe(name)) {
        recipes.insert(name, text.to_owned());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::check::{check, check_intersections};
    use crate::geom::Rigid;
    use crate::name::FaceRole;
    use crate::primitive;
    use crate::topo::BodyKind;

    const K: OpKey = OpKey(0x5348);

    fn face(r: FaceRole) -> Name {
        Name::derive(K, Role::Face(r), &[])
    }

    fn block() -> Body {
        primitive::block(K, 1.0, 0.6, 0.4, &Rigid::IDENTITY)
            .unwrap()
            .0
    }

    #[test]
    fn a_face_alone_keeps_its_names() {
        let b = block();
        let (s, j) = extract(OpKey(1), &b, &[face(FaceRole::PlusZ)]).unwrap();
        assert_eq!(s.kind(), BodyKind::Sheet);
        assert_eq!(check(&s), []);
        assert_eq!(
            (s.faces().count(), s.edges().count(), s.vertices().count()),
            (1, 4, 4)
        );
        assert!(s.edges().all(|e| s.is_laminar(e)));
        assert!(matches!(
            s.by_name(face(FaceRole::PlusZ)),
            Some(EntityId::Face(_))
        ));
        for e in s.edges() {
            let name = s.edge(e).unwrap().name;
            assert!(matches!(b.by_name(name), Some(EntityId::Edge(_))));
        }
        let count = |f: fn(&Event) -> bool| j.events().iter().filter(|e| f(e)).count();
        assert_eq!(count(|e| matches!(e, Event::Deleted(_))), 5 + 8 + 4);
        assert_eq!(count(|e| matches!(e, Event::Modified(_))), 4);
    }

    #[test]
    fn removing_the_top_leaves_an_open_box() {
        let b = block();
        let (s, _) = remove(OpKey(2), &b, &[face(FaceRole::PlusZ)]).unwrap();
        assert_eq!(s.kind(), BodyKind::Sheet);
        assert_eq!(s.faces().count(), 5);
        assert_eq!(s.edges().filter(|&e| s.is_laminar(e)).count(), 4);
        assert_eq!(check(&s), []);
        assert_eq!(check_intersections(&s), []);
        // And from that sheet, two of its faces.
        let (t, _) = extract(
            OpKey(3),
            &s,
            &[face(FaceRole::MinusZ), face(FaceRole::PlusX)],
        )
        .unwrap();
        assert_eq!((t.faces().count(), t.edges().count()), (2, 7));
        assert_eq!(check(&t), []);
    }

    #[test]
    fn every_face_is_a_closed_sheet() {
        let b = block();
        let (mut s, j) = remove(OpKey(4), &b, &[]).unwrap();
        assert_eq!(s.kind(), BodyKind::Sheet);
        assert!(j.is_empty());
        assert!(s.edges().all(|e| !s.is_laminar(e)));
        assert_eq!(check(&s), []);
        let shell = s.shells().next().unwrap();
        crate::euler::mvsr(&mut s, shell).unwrap();
        assert_eq!(check(&s), []);
    }

    #[test]
    fn a_cylinders_side_alone() {
        let (c, _) = primitive::cylinder(K, 0.3, 0.5, &Rigid::IDENTITY).unwrap();
        let (s, _) = remove(OpKey(5), &c, &[face(FaceRole::Top), face(FaceRole::Bottom)]).unwrap();
        assert_eq!((s.faces().count(), s.edges().count()), (1, 2));
        assert!(s.edges().all(|e| s.is_laminar(e)));
        assert_eq!(check(&s), []);
    }

    #[test]
    fn refusals() {
        let b = block();
        assert_eq!(
            extract(OpKey(6), &b, &[]).unwrap_err(),
            SheetError::NoFaces.into()
        );
        let all: Vec<Name> = b.faces().map(|f| b.face(f).unwrap().name).collect();
        assert_eq!(
            remove(OpKey(6), &b, &all).unwrap_err(),
            SheetError::NoFaces.into()
        );
        let stranger = Name::derive(OpKey(99), Role::NewFace, &[]);
        assert_eq!(
            extract(OpKey(6), &b, &[stranger]).unwrap_err(),
            SheetError::NoSuchFace { name: stranger }.into()
        );
        // Two of an octahedron's top faces meeting at its apex alone.
        let o = crate::topo::fixture::octahedron();
        let named = |i: u128| Name::derive(OpKey(1000 + i), Role::NewFace, &[]);
        let apex = Name::derive(OpKey(4), Role::NewVertex, &[]);
        assert_eq!(
            extract(OpKey(7), &o, &[named(1), named(3)]).unwrap_err(),
            SheetError::NonManifoldVertex { vertex: apex }.into()
        );
    }
}
