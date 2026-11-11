//! Documents and properties (ISO 10303-41): what external references
//! are made of, as CAx-IF's recommended practice and OCCT's writer give
//! them (design 448), read by [`crate::products`].

use super::Entity;
use super::args::{Args, Id};
use crate::ImportError;
use crate::part21::Param;

/// A simple record of a document or property entity, or `None`.
pub(super) fn simple(id: Id, name: &str, params: &[Param]) -> Option<Result<Entity, ImportError>> {
    Some(match name {
        // DOCUMENT(id, name, description, kind)
        "DOCUMENT" => document(id, params, 4, false),
        // DOCUMENT_FILE(id, name, description, kind, name, description):
        // a document and a characterized object.
        "DOCUMENT_FILE" => document(id, params, 6, true),
        "DOCUMENT_TYPE" => (|| {
            let mut a = Args::new(id, params, 1)?;
            Ok(Entity::DocumentType { name: a.string()? })
        })(),
        "DOCUMENT_REPRESENTATION_TYPE" => (|| {
            let mut a = Args::new(id, params, 2)?;
            Ok(Entity::DocumentRepresentationType {
                name: a.string()?,
                document: a.id()?,
            })
        })(),
        // APPLIED_DOCUMENT_REFERENCE(assigned_document, source, items)
        "APPLIED_DOCUMENT_REFERENCE" => (|| {
            let mut a = Args::new(id, params, 3)?;
            let document = a.id()?;
            a.skip();
            Ok(Entity::DocumentReference {
                document,
                items: a.ids()?,
            })
        })(),
        // DOCUMENT_PRODUCT_EQUIVALENCE(name, description,
        // relating_document, related_product)
        "DOCUMENT_PRODUCT_EQUIVALENCE" => (|| {
            let mut a = Args::new(id, params, 4)?;
            a.skip();
            a.skip();
            Ok(Entity::DocumentEquivalence {
                document: a.id()?,
                product: a.id()?,
            })
        })(),
        // APPLIED_EXTERNAL_IDENTIFICATION_ASSIGNMENT(assigned_id, role,
        // source, items)
        "APPLIED_EXTERNAL_IDENTIFICATION_ASSIGNMENT" => (|| {
            let mut a = Args::new(id, params, 4)?;
            let assigned = a.string()?;
            a.skip();
            a.skip();
            Ok(Entity::ExternalIdentification {
                id: assigned,
                items: a.ids()?,
            })
        })(),
        // PROPERTY_DEFINITION(name, description, definition)
        "PROPERTY_DEFINITION" => (|| {
            let mut a = Args::new(id, params, 3)?;
            let name = a.string()?;
            a.skip();
            Ok(Entity::PropertyDefinition {
                name,
                definition: a.id()?,
            })
        })(),
        "PROPERTY_DEFINITION_REPRESENTATION" => (|| {
            let mut a = Args::new(id, params, 2)?;
            Ok(Entity::PropertyRepresentation {
                definition: a.id()?,
                representation: a.id()?,
            })
        })(),
        "DESCRIPTIVE_REPRESENTATION_ITEM" => (|| {
            let mut a = Args::new(id, params, 2)?;
            Ok(Entity::Descriptive {
                name: a.string()?,
                description: a.string()?,
            })
        })(),
        _ => return None,
    })
}

fn document(id: Id, params: &[Param], count: usize, file: bool) -> Result<Entity, ImportError> {
    let mut a = Args::new(id, params, count)?;
    let doc_id = a.string()?;
    let name = a.string()?;
    a.skip();
    Ok(Entity::Document {
        id: doc_id,
        name,
        kind: a.opt_id()?,
        file,
    })
}
