//! Representations (ISO 10303-43): what holds items in a context, maps one
//! into another, and relates two by a transformation.

use super::Entity;
use super::args::{Args, Id};
use crate::ImportError;
use crate::part21::{Param, Part};

/// The representation entities read as [`Entity::Representation`]: any of
/// them holds `(name, items, context_of_items)`.
const REPRESENTATIONS: [&str; 13] = [
    "REPRESENTATION",
    "SHAPE_REPRESENTATION",
    "ADVANCED_BREP_SHAPE_REPRESENTATION",
    "MANIFOLD_SURFACE_SHAPE_REPRESENTATION",
    "FACETED_BREP_SHAPE_REPRESENTATION",
    "NON_MANIFOLD_SURFACE_SHAPE_REPRESENTATION",
    "GEOMETRICALLY_BOUNDED_SURFACE_SHAPE_REPRESENTATION",
    "GEOMETRICALLY_BOUNDED_WIREFRAME_SHAPE_REPRESENTATION",
    "EDGE_BASED_WIREFRAME_SHAPE_REPRESENTATION",
    "SHELL_BASED_WIREFRAME_SHAPE_REPRESENTATION",
    "SHAPE_REPRESENTATION_WITH_PARAMETERS",
    "TESSELLATED_SHAPE_REPRESENTATION",
    "DEFINITIONAL_REPRESENTATION",
];

/// A simple record of a representation entity, or `None`.
pub(super) fn simple(id: Id, name: &str, params: &[Param]) -> Option<Result<Entity, ImportError>> {
    if REPRESENTATIONS.contains(&name) {
        return Some(representation(id, name, params));
    }
    Some(match name {
        "MAPPED_ITEM" => mapped_item(id, params),
        "REPRESENTATION_MAP" => representation_map(id, params),
        "REPRESENTATION_RELATIONSHIP" | "SHAPE_REPRESENTATION_RELATIONSHIP" => {
            relationship(id, params).map(|(rep_1, rep_2)| Entity::RepresentationRelationship {
                rep_1,
                rep_2,
                transformation: None,
            })
        }
        "ITEM_DEFINED_TRANSFORMATION" => item_transformation(id, params),
        "CARTESIAN_TRANSFORMATION_OPERATOR_3D" => operator(id, params),
        "REPRESENTATION_CONTEXT" => context_simple(id, params, 2),
        "GEOMETRIC_REPRESENTATION_CONTEXT" => context_simple(id, params, 3),
        _ => return None,
    })
}

/// A complex instance of a representation, a relationship with a
/// transformation, or a context, or `None`.
pub(super) fn complex(id: Id, parts: &[Part]) -> Option<Result<Entity, ImportError>> {
    let part = |name: &str| parts.iter().find(|p| &*p.name == name);
    if let Some(r) = part("REPRESENTATION_RELATIONSHIP") {
        return Some((|| {
            let (rep_1, rep_2) = relationship(id, &r.params)?;
            let transformation = match part("REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION") {
                Some(t) => {
                    let mut a = Args::new(id, &t.params, 1)?;
                    Some(a.id()?)
                }
                None => None,
            };
            Ok(Entity::RepresentationRelationship {
                rep_1,
                rep_2,
                transformation,
            })
        })());
    }
    if part("REPRESENTATION_CONTEXT").is_some()
        || part("GEOMETRIC_REPRESENTATION_CONTEXT").is_some()
    {
        return Some(context_complex(id, parts));
    }
    if let Some(r) = part("REPRESENTATION") {
        let kind = parts
            .iter()
            .map(|p| &*p.name)
            .find(|n| *n != "REPRESENTATION" && REPRESENTATIONS.contains(n))
            .unwrap_or("REPRESENTATION");
        return Some(representation(id, kind, &r.params));
    }
    None
}

/// `(name, items, context_of_items)`.
fn representation(id: Id, kind: &str, params: &[Param]) -> Result<Entity, ImportError> {
    let mut a = Args::new(id, params, 3)?;
    a.skip();
    Ok(Entity::Representation {
        kind: kind.into(),
        items: a.ids()?,
        context: a.id()?,
    })
}

/// `MAPPED_ITEM(name, mapping_source, mapping_target)`.
fn mapped_item(id: Id, params: &[Param]) -> Result<Entity, ImportError> {
    let mut a = Args::new(id, params, 3)?;
    a.skip();
    Ok(Entity::MappedItem {
        source: a.id()?,
        target: a.id()?,
    })
}

/// `REPRESENTATION_MAP(mapping_origin, mapped_representation)`.
fn representation_map(id: Id, params: &[Param]) -> Result<Entity, ImportError> {
    let mut a = Args::new(id, params, 2)?;
    Ok(Entity::RepresentationMap {
        origin: a.id()?,
        representation: a.id()?,
    })
}

/// `(name, description, rep_1, rep_2)`.
fn relationship(id: Id, params: &[Param]) -> Result<(Id, Id), ImportError> {
    let mut a = Args::new(id, params, 4)?;
    a.skip();
    a.skip();
    Ok((a.id()?, a.id()?))
}

/// `ITEM_DEFINED_TRANSFORMATION(name, description, transform_item_1,
/// transform_item_2)`.
fn item_transformation(id: Id, params: &[Param]) -> Result<Entity, ImportError> {
    let (item_1, item_2) = relationship(id, params)?;
    Ok(Entity::ItemTransformation { item_1, item_2 })
}

/// `CARTESIAN_TRANSFORMATION_OPERATOR_3D(name, [description,] axis1,
/// axis2, local_origin, scale, axis3)`: with the description in AP242's
/// edition of the entity, without it in older ones.
fn operator(id: Id, params: &[Param]) -> Result<Entity, ImportError> {
    let count = if params.len() == 7 { 7 } else { 6 };
    let mut a = Args::new(id, params, count)?;
    a.skip();
    if count == 7 {
        a.skip();
    }
    let axis1 = a.opt_id()?;
    let axis2 = a.opt_id()?;
    let origin = a.id()?;
    let scale = match params[count - 2] {
        Param::Unset => {
            a.skip();
            None
        }
        _ => Some(a.real()?),
    };
    let axis3 = a.opt_id()?;
    Ok(Entity::Operator {
        axis1,
        axis2,
        origin,
        scale,
        axis3,
    })
}

/// `REPRESENTATION_CONTEXT(identifier, type)` or
/// `GEOMETRIC_REPRESENTATION_CONTEXT(identifier, type, dimension)`.
fn context_simple(id: Id, params: &[Param], count: usize) -> Result<Entity, ImportError> {
    let mut a = Args::new(id, params, count)?;
    a.skip();
    a.skip();
    let dimension = if count == 3 { Some(a.int()?) } else { None };
    Ok(Entity::Context {
        dimension,
        units: vec![],
        uncertainty: vec![],
    })
}

/// A context's parts: `GEOMETRIC_REPRESENTATION_CONTEXT(dimension)`,
/// `GLOBAL_UNIT_ASSIGNED_CONTEXT(units)`,
/// `GLOBAL_UNCERTAINTY_ASSIGNED_CONTEXT(uncertainty)`, and others that
/// say nothing about geometry.
fn context_complex(id: Id, parts: &[Part]) -> Result<Entity, ImportError> {
    let mut dimension = None;
    let mut units = vec![];
    let mut uncertainty = vec![];
    for p in parts {
        match &*p.name {
            "GEOMETRIC_REPRESENTATION_CONTEXT" => {
                dimension = Some(Args::new(id, &p.params, 1)?.int()?);
            }
            "GLOBAL_UNIT_ASSIGNED_CONTEXT" => units = Args::new(id, &p.params, 1)?.ids()?,
            "GLOBAL_UNCERTAINTY_ASSIGNED_CONTEXT" => {
                uncertainty = Args::new(id, &p.params, 1)?.ids()?;
            }
            _ => {}
        }
    }
    Ok(Entity::Context {
        dimension,
        units,
        uncertainty,
    })
}

#[cfg(test)]
mod tests {
    use super::super::tests::entities;
    use super::*;

    #[test]
    fn representations() {
        let e = entities(
            "#1=ADVANCED_BREP_SHAPE_REPRESENTATION('',(#11,#15),#27);\
             #2=DEFINITIONAL_REPRESENTATION('',(#50),#54);\
             #3=(REPRESENTATION('',(#11),#27)SHAPE_REPRESENTATION());\
             #4=MAPPED_ITEM('',#5,#11);#5=REPRESENTATION_MAP(#11,#1);\
             #6=SHAPE_REPRESENTATION('',(#11),$);",
        );
        let get = |id| e.get(0, id).unwrap().clone();
        assert_eq!(
            get(1),
            Entity::Representation {
                kind: "ADVANCED_BREP_SHAPE_REPRESENTATION".into(),
                items: vec![11, 15],
                context: 27
            }
        );
        assert_eq!(get(2).kind(), "DEFINITIONAL_REPRESENTATION");
        assert_eq!(get(3).kind(), "SHAPE_REPRESENTATION");
        assert_eq!(
            get(4),
            Entity::MappedItem {
                source: 5,
                target: 11
            }
        );
        assert_eq!(
            get(5),
            Entity::RepresentationMap {
                origin: 11,
                representation: 1
            }
        );
        assert!(e.get(0, 6).is_err());
    }

    #[test]
    fn relationships_and_transformations() {
        let e = entities(
            "#1=( REPRESENTATION_RELATIONSHIP('','',#40,#10) \
             REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION(#2) \
             SHAPE_REPRESENTATION_RELATIONSHIP() );\
             #2=ITEM_DEFINED_TRANSFORMATION('','',#11,#15);\
             #3=SHAPE_REPRESENTATION_RELATIONSHIP('','',#10,#40);\
             #4=CARTESIAN_TRANSFORMATION_OPERATOR_3D('',#20,$,#21,$,#22);\
             #5=CARTESIAN_TRANSFORMATION_OPERATOR_3D('','',$,$,#21,2.,$);",
        );
        let get = |id| e.get(0, id).unwrap().clone();
        assert_eq!(
            get(1),
            Entity::RepresentationRelationship {
                rep_1: 40,
                rep_2: 10,
                transformation: Some(2)
            }
        );
        assert_eq!(
            get(2),
            Entity::ItemTransformation {
                item_1: 11,
                item_2: 15
            }
        );
        assert_eq!(
            get(3),
            Entity::RepresentationRelationship {
                rep_1: 10,
                rep_2: 40,
                transformation: None
            }
        );
        assert_eq!(
            get(4),
            Entity::Operator {
                axis1: Some(20),
                axis2: None,
                origin: 21,
                scale: None,
                axis3: Some(22)
            }
        );
        assert_eq!(
            get(5),
            Entity::Operator {
                axis1: None,
                axis2: None,
                origin: 21,
                scale: Some(2.0),
                axis3: None
            }
        );
    }

    #[test]
    fn contexts() {
        let e = entities(
            "#27=( GEOMETRIC_REPRESENTATION_CONTEXT(3) \
             GLOBAL_UNCERTAINTY_ASSIGNED_CONTEXT((#31)) GLOBAL_UNIT_ASSIGNED_CONTEXT(\
             (#28,#29,#30)) REPRESENTATION_CONTEXT('Context #1',\
             '3D Context with UNIT and UNCERTAINTY') );\
             #54=( GEOMETRIC_REPRESENTATION_CONTEXT(2) PARAMETRIC_REPRESENTATION_CONTEXT() \
             REPRESENTATION_CONTEXT('2D SPACE','') );\
             #55=GEOMETRIC_REPRESENTATION_CONTEXT('','',3);\
             #56=(GEOMETRIC_REPRESENTATION_CONTEXT(3.5)REPRESENTATION_CONTEXT('',''));",
        );
        let get = |id| e.get(0, id).unwrap().clone();
        assert_eq!(
            get(27),
            Entity::Context {
                dimension: Some(3),
                units: vec![28, 29, 30],
                uncertainty: vec![31]
            }
        );
        assert_eq!(
            get(54),
            Entity::Context {
                dimension: Some(2),
                units: vec![],
                uncertainty: vec![]
            }
        );
        assert_eq!(get(55).kind(), "REPRESENTATION_CONTEXT");
        assert!(e.get(0, 56).is_err());
    }
}
