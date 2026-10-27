//! Products and their shapes (ISO 10303-41 and -44): what the product
//! tree is made of.

use super::Entity;
use super::args::{Args, Id};
use crate::ImportError;
use crate::part21::Param;

/// A simple record of a product entity, or `None`.
pub(super) fn simple(id: Id, name: &str, params: &[Param]) -> Option<Result<Entity, ImportError>> {
    Some(match name {
        "PRODUCT" => (|| {
            let mut a = Args::new(id, params, 4)?;
            let id = a.string()?;
            let name = a.string()?;
            // A description that isn't a string is no description: it
            // names nothing the tree needs.
            let description = a.string().unwrap_or_default();
            Ok(Entity::Product {
                id,
                name,
                description,
            })
        })(),
        "PRODUCT_DEFINITION_FORMATION" => formation(id, params, 3),
        "PRODUCT_DEFINITION_FORMATION_WITH_SPECIFIED_SOURCE" => formation(id, params, 4),
        "PRODUCT_DEFINITION" => definition(id, params, 4),
        "PRODUCT_DEFINITION_WITH_ASSOCIATED_DOCUMENTS" => definition(id, params, 5),
        "PRODUCT_DEFINITION_SHAPE" => (|| {
            let mut a = Args::new(id, params, 3)?;
            a.skip();
            a.skip();
            Ok(Entity::DefinitionShape {
                definition: a.id()?,
            })
        })(),
        "SHAPE_DEFINITION_REPRESENTATION" => (|| {
            let mut a = Args::new(id, params, 2)?;
            Ok(Entity::ShapeDefinition {
                definition: a.id()?,
                representation: a.id()?,
            })
        })(),
        "NEXT_ASSEMBLY_USAGE_OCCURRENCE" => (|| {
            let mut a = Args::new(id, params, 6)?;
            let usage_id = a.string()?;
            let name = a.string()?;
            a.skip();
            Ok(Entity::Usage {
                id: usage_id,
                name,
                parent: a.id()?,
                child: a.id()?,
            })
        })(),
        "CONTEXT_DEPENDENT_SHAPE_REPRESENTATION" => (|| {
            let mut a = Args::new(id, params, 2)?;
            Ok(Entity::ContextDependentShape {
                relationship: a.id()?,
                product_shape: a.id()?,
            })
        })(),
        _ => return None,
    })
}

/// `PRODUCT_DEFINITION_FORMATION(id, description, of_product)`, and the
/// subtype with `make_or_buy` after it.
fn formation(id: Id, params: &[Param], count: usize) -> Result<Entity, ImportError> {
    let mut a = Args::new(id, params, count)?;
    let formation_id = a.string()?;
    a.skip();
    Ok(Entity::Formation {
        id: formation_id,
        product: a.id()?,
    })
}

/// `PRODUCT_DEFINITION(id, description, formation, frame_of_reference)`,
/// and the subtype with `documentation_ids` after it.
fn definition(id: Id, params: &[Param], count: usize) -> Result<Entity, ImportError> {
    let mut a = Args::new(id, params, count)?;
    let definition_id = a.string()?;
    a.skip();
    Ok(Entity::Definition {
        id: definition_id,
        formation: a.id()?,
    })
}

#[cfg(test)]
mod tests {
    use super::super::tests::entities;
    use super::*;

    #[test]
    fn product_entities() {
        let e = entities(
            "#3=SHAPE_DEFINITION_REPRESENTATION(#4,#10);\
             #4=PRODUCT_DEFINITION_SHAPE('','',#5);#5=PRODUCT_DEFINITION('design','',#6,#9);\
             #6=PRODUCT_DEFINITION_FORMATION('','',#7);\
             #7=PRODUCT('block','Block','',(#8));\
             #11=PRODUCT_DEFINITION_FORMATION_WITH_SPECIFIED_SOURCE('','',#7,.NOT_KNOWN.);\
             #12=NEXT_ASSEMBLY_USAGE_OCCURRENCE('1','=>[0:1:1:2]','',#5,#35,$);\
             #13=CONTEXT_DEPENDENT_SHAPE_REPRESENTATION(#377,#379);\
             #14=PRODUCT_DEFINITION_WITH_ASSOCIATED_DOCUMENTS('d','',#6,#9,(#20));",
        );
        let get = |id| e.get(0, id).unwrap().clone();
        assert_eq!(
            get(3),
            Entity::ShapeDefinition {
                definition: 4,
                representation: 10
            }
        );
        assert_eq!(get(4), Entity::DefinitionShape { definition: 5 });
        assert_eq!(
            get(5),
            Entity::Definition {
                id: "design".into(),
                formation: 6
            }
        );
        assert_eq!(
            get(6),
            Entity::Formation {
                id: "".into(),
                product: 7
            }
        );
        assert_eq!(
            get(7),
            Entity::Product {
                id: "block".into(),
                name: "Block".into(),
                description: "".into()
            }
        );
        assert_eq!(
            get(11),
            Entity::Formation {
                id: "".into(),
                product: 7
            }
        );
        assert_eq!(
            get(12),
            Entity::Usage {
                id: "1".into(),
                name: "=>[0:1:1:2]".into(),
                parent: 5,
                child: 35
            }
        );
        assert_eq!(
            get(13),
            Entity::ContextDependentShape {
                relationship: 377,
                product_shape: 379
            }
        );
        assert_eq!(get(14).kind(), "PRODUCT_DEFINITION");
    }
}
