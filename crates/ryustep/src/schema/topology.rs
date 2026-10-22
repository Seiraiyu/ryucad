//! Topology (ISO 10303-42): vertices to shells, and the B-rep items that
//! hold shells.

use super::Entity;
use super::args::{Args, Id};
use crate::ImportError;
use crate::part21::Param;

/// `EDGE_CURVE`: from `start` to `end` vertex along `curve`, running the
/// way the curve does if `same_sense`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Edge {
    pub start: Id,
    pub end: Id,
    pub curve: Id,
    pub same_sense: bool,
}

/// A loop.
#[derive(Clone, Debug, PartialEq)]
pub enum Loop {
    /// `EDGE_LOOP`: oriented edges end to end.
    Edges(Vec<Id>),
    /// `VERTEX_LOOP`: a single vertex (at a pole, say).
    Vertex(Id),
    /// `POLY_LOOP`: straight edges through points, closed.
    Poly(Vec<Id>),
}

/// A face: `ADVANCED_FACE` or `FACE_SURFACE` with its surface (its normal
/// the face's if `same_sense`), or a plain `FACE` without one.
#[derive(Clone, Debug, PartialEq)]
pub struct Face {
    pub bounds: Vec<Id>,
    pub surface: Option<Id>,
    pub same_sense: bool,
}

/// A B-rep representation item.
#[derive(Clone, Debug, PartialEq)]
pub enum Brep {
    /// `MANIFOLD_SOLID_BREP`, `BREP_WITH_VOIDS` (with `voids`, oriented
    /// closed shells) or `FACETED_BREP` (`faceted`).
    Solid {
        outer: Id,
        voids: Vec<Id>,
        faceted: bool,
    },
    /// `SHELL_BASED_SURFACE_MODEL`: open or closed shells.
    Shells(Vec<Id>),
}

/// A simple record of a topological entity, or `None`.
pub(super) fn simple(id: Id, name: &str, params: &[Param]) -> Option<Result<Entity, ImportError>> {
    Some(match name {
        "VERTEX_POINT" => vertex(id, params),
        "EDGE_CURVE" => edge(id, params),
        "ORIENTED_EDGE" => oriented(id, params, 5, |edge, orientation| Entity::OrientedEdge {
            edge,
            orientation,
        }),
        "EDGE_LOOP" => ids(id, params).map(|e| Entity::Loop(Loop::Edges(e))),
        "VERTEX_LOOP" => one(id, params).map(|v| Entity::Loop(Loop::Vertex(v))),
        "POLY_LOOP" => ids(id, params).map(|p| Entity::Loop(Loop::Poly(p))),
        "FACE_BOUND" => bound(id, params, false),
        "FACE_OUTER_BOUND" => bound(id, params, true),
        "ADVANCED_FACE" | "FACE_SURFACE" => face(id, params, true),
        "FACE" => face(id, params, false),
        "ORIENTED_FACE" => oriented(id, params, 4, |face, orientation| Entity::OrientedFace {
            face,
            orientation,
        }),
        "CLOSED_SHELL" => ids(id, params).map(|faces| Entity::Shell {
            faces,
            closed: true,
        }),
        "OPEN_SHELL" => ids(id, params).map(|faces| Entity::Shell {
            faces,
            closed: false,
        }),
        "ORIENTED_CLOSED_SHELL" | "ORIENTED_OPEN_SHELL" => {
            let closed = name == "ORIENTED_CLOSED_SHELL";
            oriented(id, params, 4, |shell, orientation| Entity::OrientedShell {
                shell,
                orientation,
                closed,
            })
        }
        "MANIFOLD_SOLID_BREP" => one(id, params).map(|outer| solid(outer, vec![], false)),
        "FACETED_BREP" => one(id, params).map(|outer| solid(outer, vec![], true)),
        "BREP_WITH_VOIDS" => voids(id, params),
        "SHELL_BASED_SURFACE_MODEL" => ids(id, params).map(|s| Entity::Brep(Brep::Shells(s))),
        _ => return None,
    })
}

fn solid(outer: Id, voids: Vec<Id>, faceted: bool) -> Entity {
    Entity::Brep(Brep::Solid {
        outer,
        voids,
        faceted,
    })
}

/// `NAME(name, reference)`.
fn one(id: Id, params: &[Param]) -> Result<Id, ImportError> {
    let mut a = Args::new(id, params, 2)?;
    a.skip();
    a.id()
}

/// `NAME(name, (references))`, at least one.
fn ids(id: Id, params: &[Param]) -> Result<Vec<Id>, ImportError> {
    let mut a = Args::new(id, params, 2)?;
    a.skip();
    let ids = a.ids()?;
    if ids.is_empty() {
        return Err(a.fault("expected at least one reference"));
    }
    Ok(ids)
}

/// `VERTEX_POINT(name, vertex_geometry)`.
fn vertex(id: Id, params: &[Param]) -> Result<Entity, ImportError> {
    one(id, params).map(|point| Entity::Vertex { point })
}

/// `EDGE_CURVE(name, edge_start, edge_end, edge_geometry, same_sense)`.
fn edge(id: Id, params: &[Param]) -> Result<Entity, ImportError> {
    let mut a = Args::new(id, params, 5)?;
    a.skip();
    Ok(Entity::Edge(Edge {
        start: a.id()?,
        end: a.id()?,
        curve: a.id()?,
        same_sense: a.boolean()?,
    }))
}

/// An oriented entity of `count` attributes: the name, derived ones, then
/// the element and its orientation.
fn oriented(
    id: Id,
    params: &[Param],
    count: usize,
    make: impl Fn(Id, bool) -> Entity,
) -> Result<Entity, ImportError> {
    let mut a = Args::new(id, params, count)?;
    for _ in 0..count - 2 {
        a.skip();
    }
    let element = a.id()?;
    let orientation = a.boolean()?;
    Ok(make(element, orientation))
}

/// `FACE_BOUND(name, bound, orientation)` or `FACE_OUTER_BOUND`.
fn bound(id: Id, params: &[Param], outer: bool) -> Result<Entity, ImportError> {
    let mut a = Args::new(id, params, 3)?;
    a.skip();
    Ok(Entity::Bound {
        bound: a.id()?,
        orientation: a.boolean()?,
        outer,
    })
}

/// `ADVANCED_FACE(name, bounds, face_geometry, same_sense)`, or
/// `FACE(name, bounds)`.
fn face(id: Id, params: &[Param], with_surface: bool) -> Result<Entity, ImportError> {
    let mut a = Args::new(id, params, if with_surface { 4 } else { 2 })?;
    a.skip();
    let bounds = a.ids()?;
    let (surface, same_sense) = if with_surface {
        (Some(a.id()?), a.boolean()?)
    } else {
        (None, true)
    };
    Ok(Entity::Face(Face {
        bounds,
        surface,
        same_sense,
    }))
}

/// `BREP_WITH_VOIDS(name, outer, voids)`.
fn voids(id: Id, params: &[Param]) -> Result<Entity, ImportError> {
    let mut a = Args::new(id, params, 3)?;
    a.skip();
    let outer = a.id()?;
    let voids = a.ids()?;
    if voids.is_empty() {
        return Err(a.fault("expected at least one void"));
    }
    Ok(solid(outer, voids, false))
}

#[cfg(test)]
mod tests {
    use super::super::tests::entities;
    use super::*;

    #[test]
    fn vertices_edges_and_loops() {
        let e = entities(
            "#1=VERTEX_POINT('',#9);#2=EDGE_CURVE('',#1,#3,#10,.F.);\
             #4=ORIENTED_EDGE('',*,*,#2,.T.);#5=EDGE_LOOP('',(#4,#6));\
             #7=VERTEX_LOOP('',#1);#8=POLY_LOOP('',(#9,#11,#12));#13=EDGE_LOOP('',());",
        );
        let get = |id| e.get(0, id).unwrap().clone();
        assert_eq!(get(1), Entity::Vertex { point: 9 });
        assert_eq!(
            get(2),
            Entity::Edge(Edge {
                start: 1,
                end: 3,
                curve: 10,
                same_sense: false
            })
        );
        assert_eq!(
            get(4),
            Entity::OrientedEdge {
                edge: 2,
                orientation: true
            }
        );
        assert_eq!(get(5), Entity::Loop(Loop::Edges(vec![4, 6])));
        assert_eq!(get(7), Entity::Loop(Loop::Vertex(1)));
        assert_eq!(get(8), Entity::Loop(Loop::Poly(vec![9, 11, 12])));
        assert_eq!(get(8).kind(), "POLY_LOOP");
        assert!(matches!(
            e.get(0, 13),
            Err(ImportError::Malformed {
                what: "expected at least one reference",
                ..
            })
        ));
    }

    #[test]
    fn faces_and_shells() {
        let e = entities(
            "#1=FACE_OUTER_BOUND('',#9,.T.);#2=FACE_BOUND('',#9,.F.);\
             #3=ADVANCED_FACE('',(#1,#2),#10,.F.);#4=FACE('',(#1));\
             #5=ORIENTED_FACE('',*,#3,.F.);#6=CLOSED_SHELL('',(#3,#4));\
             #7=OPEN_SHELL('',(#5));#8=ORIENTED_CLOSED_SHELL('',*,#6,.F.);",
        );
        let get = |id| e.get(0, id).unwrap().clone();
        assert_eq!(
            get(1),
            Entity::Bound {
                bound: 9,
                orientation: true,
                outer: true
            }
        );
        assert_eq!(get(2).kind(), "FACE_BOUND");
        assert_eq!(
            get(3),
            Entity::Face(Face {
                bounds: vec![1, 2],
                surface: Some(10),
                same_sense: false
            })
        );
        assert_eq!(
            get(4),
            Entity::Face(Face {
                bounds: vec![1],
                surface: None,
                same_sense: true
            })
        );
        assert_eq!(
            get(5),
            Entity::OrientedFace {
                face: 3,
                orientation: false
            }
        );
        assert_eq!(
            get(6),
            Entity::Shell {
                faces: vec![3, 4],
                closed: true
            }
        );
        assert_eq!(get(7).kind(), "OPEN_SHELL");
        assert_eq!(
            get(8),
            Entity::OrientedShell {
                shell: 6,
                orientation: false,
                closed: true
            }
        );
    }

    #[test]
    fn breps() {
        let e = entities(
            "#1=MANIFOLD_SOLID_BREP('',#9);#2=BREP_WITH_VOIDS('',#9,(#10,#11));\
             #3=FACETED_BREP('',#9);#4=SHELL_BASED_SURFACE_MODEL('',(#12,#13));\
             #5=BREP_WITH_VOIDS('',#9,());",
        );
        let get = |id| e.get(0, id).unwrap().clone();
        assert_eq!(get(1), solid(9, vec![], false));
        assert_eq!(get(2), solid(9, vec![10, 11], false));
        assert_eq!(get(3), solid(9, vec![], true));
        assert_eq!(get(4), Entity::Brep(Brep::Shells(vec![12, 13])));
        let kinds: Vec<String> = (1..=4).map(|k| get(k).kind().to_owned()).collect();
        assert_eq!(
            kinds,
            [
                "MANIFOLD_SOLID_BREP",
                "BREP_WITH_VOIDS",
                "FACETED_BREP",
                "SHELL_BASED_SURFACE_MODEL"
            ]
        );
        assert!(e.get(0, 5).is_err());
    }
}
