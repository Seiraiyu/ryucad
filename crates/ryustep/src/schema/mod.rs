//! The entities of the application protocols this crate reads (AP203,
//! AP214 and AP242), typed: the subset of ISO 10303-41, -42 and -44 that
//! B-rep solids, their units and the product tree use.
//!
//! [`Entities::decode`] decodes every instance of a file in parallel.
//! Entities this crate doesn't read become [`Entity::Other`], and an
//! instance that breaks its entity's rules holds its [`ImportError`]:
//! neither fails anything until a body needs that instance. Values stay as
//! the file wrote them, in its units; references stay instance numbers.

mod args;
mod bspline;
mod curve;
mod derived;
mod document;
mod geometry;
mod product;
mod property;
mod representation;
mod style;
mod surface;
mod topology;
mod units;

use rayon::prelude::*;

pub use args::Id;
pub use bspline::{BSplineCurve, BSplineSurface, Knots};
pub use curve::Curve;
pub use derived::{OnSurface, Trim};
pub use geometry::{Placement, Point};
pub use property::{ItemValue, ValueForm};
pub use style::Side;
pub use surface::Surface;
pub use topology::{Brep, Edge, Face, Loop};
pub use units::{MeasureUnit, Quantity, Unit, Units, scaled};

use crate::part21::{File, Header, Instance, Param, Part, Record};
use crate::{ImportError, StepError};

/// The application protocol a file follows. Their B-rep, unit and product
/// entities are the same, so the schema decides little but what to write.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Schema {
    /// `CONFIG_CONTROL_DESIGN` (edition 1) or AP203 edition 2.
    Ap203,
    /// `AUTOMOTIVE_DESIGN`, and its drafts' `AUTOMOTIVE_DESIGN_CC1` and
    /// `_CC2`.
    Ap214,
    /// `AP242_MANAGED_MODEL_BASED_3D_ENGINEERING_MIM_LF`, any edition.
    Ap242,
}

impl Schema {
    /// The first schema in the header's `FILE_SCHEMA` this crate reads.
    /// A name is compared up to its first character that can't be in an
    /// identifier, so `AUTOMOTIVE_DESIGN { 1 0 10303 214 1 1 1 1 }` and
    /// OCCT's `AP242_…_MIM_LF. {…}` are known.
    pub fn of(header: &Header) -> Result<Schema, StepError> {
        let schemas = header.schemas();
        for s in &schemas {
            let name: String = s
                .trim_start()
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .map(|c| c.to_ascii_uppercase())
                .collect();
            let known = match name.as_str() {
                "CONFIG_CONTROL_DESIGN" => Some(Schema::Ap203),
                "AUTOMOTIVE_DESIGN" | "AUTOMOTIVE_DESIGN_CC1" | "AUTOMOTIVE_DESIGN_CC2" => {
                    Some(Schema::Ap214)
                }
                n if n.starts_with("AP203_CONFIGURATION_CONTROLLED_3D_DESIGN") => {
                    Some(Schema::Ap203)
                }
                n if n.starts_with("AP214_AUTOMOTIVE_DESIGN") => Some(Schema::Ap214),
                n if n.starts_with("AP242_MANAGED_MODEL_BASED_3D_ENGINEERING") => {
                    Some(Schema::Ap242)
                }
                _ => None,
            };
            if let Some(k) = known {
                return Ok(k);
            }
        }
        Err(StepError::UnknownSchema(
            schemas
                .first()
                .map_or_else(String::new, |s| (*s).to_owned()),
        ))
    }
}

/// An instance, typed.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum Entity {
    /// `CARTESIAN_POINT`.
    Point(Point),
    /// `DIRECTION`: not necessarily of unit length.
    Direction(Point),
    /// `VECTOR`: a direction and a length.
    Vector {
        orientation: Id,
        magnitude: f64,
    },
    Placement(Placement),
    Curve(Curve),
    Surface(Surface),
    /// `COMPOSITE_CURVE_SEGMENT`: `parent`, the same way as the composite
    /// curve if `same_sense`.
    Segment {
        same_sense: bool,
        parent: Id,
    },
    /// `VERTEX_POINT`, with its name (`ryukei:seam` marks a vertex a
    /// writer added only to end a seam: see `ryukei::heal::Vertex`).
    Vertex {
        name: Box<str>,
        point: Id,
    },
    Edge(Edge),
    /// `ORIENTED_EDGE`: `edge`, the way it runs if `orientation`.
    OrientedEdge {
        edge: Id,
        orientation: bool,
    },
    Loop(Loop),
    /// `FACE_BOUND`, or `FACE_OUTER_BOUND` if `outer`: a loop, the way it
    /// runs if `orientation`.
    Bound {
        bound: Id,
        orientation: bool,
        outer: bool,
    },
    Face(Face),
    /// `ORIENTED_FACE`.
    OrientedFace {
        face: Id,
        orientation: bool,
    },
    /// `CLOSED_SHELL` or `OPEN_SHELL`.
    Shell {
        faces: Vec<Id>,
        closed: bool,
    },
    /// `CONNECTED_FACE_SET`: faces joined by their edges, in a
    /// `FACE_BASED_SURFACE_MODEL`.
    FaceSet {
        faces: Vec<Id>,
    },
    /// `ORIENTED_CLOSED_SHELL` or `ORIENTED_OPEN_SHELL`.
    OrientedShell {
        shell: Id,
        orientation: bool,
        closed: bool,
    },
    Brep(Brep),
    /// A representation of the kinds in `representation::REPRESENTATIONS`
    /// (`kind` is its entity's name): `items` in `context`.
    Representation {
        kind: Box<str>,
        items: Vec<Id>,
        context: Id,
    },
    /// `MAPPED_ITEM`: the representation `source` maps, placed at
    /// `target`.
    MappedItem {
        source: Id,
        target: Id,
    },
    /// `REPRESENTATION_MAP`: `representation`, with `origin` its placement
    /// that a mapped item's target takes the place of.
    RepresentationMap {
        origin: Id,
        representation: Id,
    },
    /// `REPRESENTATION_RELATIONSHIP` and `SHAPE_REPRESENTATION_RELATIONSHIP`,
    /// with a transformation when complex: `rep_1` in `rep_2`'s place.
    RepresentationRelationship {
        rep_1: Id,
        rep_2: Id,
        transformation: Option<Id>,
    },
    /// `ITEM_DEFINED_TRANSFORMATION`: the placement `item_1` (of the
    /// relationship's `rep_1`) moved onto `item_2` (of `rep_2`).
    ItemTransformation {
        item_1: Id,
        item_2: Id,
    },
    /// `CARTESIAN_TRANSFORMATION_OPERATOR_3D`.
    Operator {
        axis1: Option<Id>,
        axis2: Option<Id>,
        origin: Id,
        scale: Option<f64>,
        axis3: Option<Id>,
    },
    /// A representation context: its dimension, units and uncertainties.
    Context {
        dimension: Option<i64>,
        units: Vec<Id>,
        uncertainty: Vec<Id>,
    },
    Unit(Unit),
    /// `LENGTH_MEASURE_WITH_UNIT`, `PLANE_ANGLE_MEASURE_WITH_UNIT` or
    /// `MEASURE_WITH_UNIT`.
    Measure {
        value: f64,
        unit: Id,
    },
    /// `UNCERTAINTY_MEASURE_WITH_UNIT`: `name` says which
    /// (`distance_accuracy_value`).
    Uncertainty {
        value: f64,
        unit: Id,
        name: Box<str>,
    },
    /// `PRODUCT`: its identifier (often a part number), name and
    /// description (empty when unset).
    Product {
        id: Box<str>,
        name: Box<str>,
        description: Box<str>,
    },
    /// `PRODUCT_DEFINITION_FORMATION` (a version of a product), with or
    /// without a specified source: its identifier (a revision, where
    /// writers give one) and its product.
    Formation {
        id: Box<str>,
        product: Id,
    },
    /// `PRODUCT_DEFINITION` (a view of a version), with or without
    /// associated documents (`documents`, empty without).
    Definition {
        id: Box<str>,
        formation: Id,
        documents: Vec<Id>,
    },
    /// `PRODUCT_DEFINITION_SHAPE`: the shape of a product definition, or
    /// of an assembly usage.
    DefinitionShape {
        definition: Id,
    },
    /// `SHAPE_DEFINITION_REPRESENTATION`: a shape and a representation of
    /// it.
    ShapeDefinition {
        definition: Id,
        representation: Id,
    },
    /// `NEXT_ASSEMBLY_USAGE_OCCURRENCE`: `child` used once in `parent`,
    /// both product definitions.
    Usage {
        id: Box<str>,
        name: Box<str>,
        parent: Id,
        child: Id,
    },
    /// `CONTEXT_DEPENDENT_SHAPE_REPRESENTATION`: where a usage puts its
    /// child, as a relationship between their representations.
    ContextDependentShape {
        relationship: Id,
        product_shape: Id,
    },
    /// `DOCUMENT`, or `DOCUMENT_FILE` (`file`): its identifier, name and
    /// kind (a `DOCUMENT_TYPE`).
    Document {
        id: Box<str>,
        name: Box<str>,
        kind: Option<Id>,
        file: bool,
    },
    /// `DOCUMENT_TYPE`: its product data type (`cad_filename`, or empty).
    DocumentType {
        name: Box<str>,
    },
    /// `DOCUMENT_REPRESENTATION_TYPE`: `digital` or `physical`, of a
    /// document.
    DocumentRepresentationType {
        name: Box<str>,
        document: Id,
    },
    /// `APPLIED_DOCUMENT_REFERENCE`: a document assigned to items.
    DocumentReference {
        document: Id,
        items: Vec<Id>,
    },
    /// `DOCUMENT_PRODUCT_EQUIVALENCE`: a document that is a product, a
    /// formation or a definition.
    DocumentEquivalence {
        document: Id,
        product: Id,
    },
    /// `APPLIED_EXTERNAL_IDENTIFICATION_ASSIGNMENT`: an identifier (a
    /// file's name, for documents) assigned to items.
    ExternalIdentification {
        id: Box<str>,
        items: Vec<Id>,
    },
    /// `PROPERTY_DEFINITION`: its name, description and what it is of.
    PropertyDefinition {
        name: Box<str>,
        description: Box<str>,
        definition: Id,
    },
    /// `PROPERTY_DEFINITION_REPRESENTATION`.
    PropertyRepresentation {
        definition: Id,
        representation: Id,
    },
    /// `DESCRIPTIVE_REPRESENTATION_ITEM`: a name and a text.
    Descriptive {
        name: Box<str>,
        description: Box<str>,
    },
    /// `STYLED_ITEM`, `OVER_RIDING_STYLED_ITEM` (`over_riding`) or
    /// `CONTEXT_DEPENDENT_OVER_RIDING_STYLED_ITEM` (`context`, the
    /// usages or relationships it holds in): its presentation style
    /// assignments on `item`.
    StyledItem {
        styles: Vec<Id>,
        item: Id,
        over_riding: bool,
        context: Option<Vec<Id>>,
    },
    /// `PRESENTATION_STYLE_ASSIGNMENT`: the styles it assigns that are
    /// instances.
    StyleAssignment {
        styles: Vec<Id>,
    },
    /// `PRESENTATION_STYLE_BY_CONTEXT`: an assignment's styles that hold
    /// in `context` (a representation or a representation item).
    StyleByContext {
        styles: Vec<Id>,
        context: Id,
    },
    /// `SURFACE_STYLE_USAGE`: a `SURFACE_SIDE_STYLE` for `side`.
    SurfaceStyleUsage {
        side: Side,
        style: Id,
    },
    /// `SURFACE_SIDE_STYLE`: its style elements.
    SurfaceSideStyle {
        styles: Vec<Id>,
    },
    /// `SURFACE_STYLE_FILL_AREA`: its `FILL_AREA_STYLE`.
    SurfaceFill {
        fill: Id,
    },
    /// `FILL_AREA_STYLE`: its fill styles.
    FillAreaStyle {
        styles: Vec<Id>,
    },
    /// `FILL_AREA_STYLE_COLOUR`: its colour.
    FillColour {
        colour: Id,
    },
    /// `COLOUR_RGB`: red, green and blue as the file gives them, in
    /// `[0, 1]`.
    ColourRgb {
        rgb: [f64; 3],
    },
    /// `DRAUGHTING_PRE_DEFINED_COLOUR`: its name.
    PredefinedColour {
        name: Box<str>,
    },
    /// `SURFACE_STYLE_RENDERING_WITH_PROPERTIES`: its rendering
    /// properties.
    Rendering {
        properties: Vec<Id>,
    },
    /// `SURFACE_STYLE_TRANSPARENT`.
    Transparent {
        transparency: f64,
    },
    /// `SPECIFIED_HIGHER_USAGE_OCCURRENCE`: the usage `next` within the
    /// occurrence `upper` (a usage, or another of these).
    HigherUsage {
        upper: Id,
        next: Id,
    },
    /// `MATERIAL_DESIGNATION`: a material's name and the characterized
    /// definitions (product definitions or their shapes) it is of.
    MaterialDesignation {
        name: Box<str>,
        definitions: Vec<Id>,
    },
    /// `MEASURE_REPRESENTATION_ITEM`, simple or complex: a named measure
    /// and its unit (`None` for `$`).
    MeasureItem {
        name: Box<str>,
        value: ItemValue,
        unit: Option<Id>,
    },
    /// A named value: `VALUE_REPRESENTATION_ITEM`, or AP242's
    /// `INTEGER_`, `REAL_` or `BOOLEAN_REPRESENTATION_ITEM` (`form`).
    ValueItem {
        name: Box<str>,
        value: ItemValue,
        form: ValueForm,
    },
    /// `DERIVED_UNIT`: its elements.
    DerivedUnit {
        elements: Vec<Id>,
    },
    /// `DERIVED_UNIT_ELEMENT`: a named unit to a power.
    UnitElement {
        unit: Id,
        exponent: f64,
    },
    /// An entity this crate doesn't read: its name, or a complex
    /// instance's names joined by spaces.
    Other(Box<str>),
}

impl Entity {
    /// What the entity is, for messages.
    pub fn kind(&self) -> &str {
        match self {
            Entity::Point(_) => "CARTESIAN_POINT",
            Entity::Direction(_) => "DIRECTION",
            Entity::Vector { .. } => "VECTOR",
            Entity::Placement(p) => p.kind(),
            Entity::Curve(c) => c.kind(),
            Entity::Surface(s) => s.kind(),
            Entity::Segment { .. } => "COMPOSITE_CURVE_SEGMENT",
            Entity::Vertex { .. } => "VERTEX_POINT",
            Entity::Edge(_) => "EDGE_CURVE",
            Entity::OrientedEdge { .. } => "ORIENTED_EDGE",
            Entity::Loop(Loop::Edges(_)) => "EDGE_LOOP",
            Entity::Loop(Loop::Vertex(_)) => "VERTEX_LOOP",
            Entity::Loop(Loop::Poly(_)) => "POLY_LOOP",
            Entity::Bound { outer: true, .. } => "FACE_OUTER_BOUND",
            Entity::Bound { .. } => "FACE_BOUND",
            Entity::Face(Face { surface: None, .. }) => "FACE",
            Entity::Face(_) => "ADVANCED_FACE",
            Entity::OrientedFace { .. } => "ORIENTED_FACE",
            Entity::Shell { closed: true, .. } => "CLOSED_SHELL",
            Entity::Shell { .. } => "OPEN_SHELL",
            Entity::FaceSet { .. } => "CONNECTED_FACE_SET",
            Entity::OrientedShell { closed: true, .. } => "ORIENTED_CLOSED_SHELL",
            Entity::OrientedShell { .. } => "ORIENTED_OPEN_SHELL",
            Entity::Brep(Brep::Solid { faceted: true, .. }) => "FACETED_BREP",
            Entity::Brep(Brep::Solid { voids, .. }) if !voids.is_empty() => "BREP_WITH_VOIDS",
            Entity::Brep(Brep::Solid { .. }) => "MANIFOLD_SOLID_BREP",
            Entity::Brep(Brep::Shells(_)) => "SHELL_BASED_SURFACE_MODEL",
            Entity::Brep(Brep::Faces(_)) => "FACE_BASED_SURFACE_MODEL",
            Entity::Representation { kind, .. } => kind,
            Entity::MappedItem { .. } => "MAPPED_ITEM",
            Entity::RepresentationMap { .. } => "REPRESENTATION_MAP",
            Entity::RepresentationRelationship { .. } => "REPRESENTATION_RELATIONSHIP",
            Entity::ItemTransformation { .. } => "ITEM_DEFINED_TRANSFORMATION",
            Entity::Operator { .. } => "CARTESIAN_TRANSFORMATION_OPERATOR_3D",
            Entity::Context { .. } => "REPRESENTATION_CONTEXT",
            Entity::Unit(Unit::Si { .. }) => "SI_UNIT",
            Entity::Unit(Unit::Conversion { .. }) => "CONVERSION_BASED_UNIT",
            Entity::Measure { .. } => "MEASURE_WITH_UNIT",
            Entity::Uncertainty { .. } => "UNCERTAINTY_MEASURE_WITH_UNIT",
            Entity::Product { .. } => "PRODUCT",
            Entity::Formation { .. } => "PRODUCT_DEFINITION_FORMATION",
            Entity::Definition { .. } => "PRODUCT_DEFINITION",
            Entity::DefinitionShape { .. } => "PRODUCT_DEFINITION_SHAPE",
            Entity::ShapeDefinition { .. } => "SHAPE_DEFINITION_REPRESENTATION",
            Entity::Usage { .. } => "NEXT_ASSEMBLY_USAGE_OCCURRENCE",
            Entity::ContextDependentShape { .. } => "CONTEXT_DEPENDENT_SHAPE_REPRESENTATION",
            Entity::Document { file: true, .. } => "DOCUMENT_FILE",
            Entity::Document { .. } => "DOCUMENT",
            Entity::DocumentType { .. } => "DOCUMENT_TYPE",
            Entity::DocumentRepresentationType { .. } => "DOCUMENT_REPRESENTATION_TYPE",
            Entity::DocumentReference { .. } => "APPLIED_DOCUMENT_REFERENCE",
            Entity::DocumentEquivalence { .. } => "DOCUMENT_PRODUCT_EQUIVALENCE",
            Entity::ExternalIdentification { .. } => "APPLIED_EXTERNAL_IDENTIFICATION_ASSIGNMENT",
            Entity::PropertyDefinition { .. } => "PROPERTY_DEFINITION",
            Entity::PropertyRepresentation { .. } => "PROPERTY_DEFINITION_REPRESENTATION",
            Entity::Descriptive { .. } => "DESCRIPTIVE_REPRESENTATION_ITEM",
            Entity::StyledItem {
                context: Some(_), ..
            } => "CONTEXT_DEPENDENT_OVER_RIDING_STYLED_ITEM",
            Entity::StyledItem {
                over_riding: true, ..
            } => "OVER_RIDING_STYLED_ITEM",
            Entity::StyledItem { .. } => "STYLED_ITEM",
            Entity::StyleAssignment { .. } => "PRESENTATION_STYLE_ASSIGNMENT",
            Entity::StyleByContext { .. } => "PRESENTATION_STYLE_BY_CONTEXT",
            Entity::SurfaceStyleUsage { .. } => "SURFACE_STYLE_USAGE",
            Entity::SurfaceSideStyle { .. } => "SURFACE_SIDE_STYLE",
            Entity::SurfaceFill { .. } => "SURFACE_STYLE_FILL_AREA",
            Entity::FillAreaStyle { .. } => "FILL_AREA_STYLE",
            Entity::FillColour { .. } => "FILL_AREA_STYLE_COLOUR",
            Entity::ColourRgb { .. } => "COLOUR_RGB",
            Entity::PredefinedColour { .. } => "DRAUGHTING_PRE_DEFINED_COLOUR",
            Entity::Rendering { .. } => "SURFACE_STYLE_RENDERING_WITH_PROPERTIES",
            Entity::Transparent { .. } => "SURFACE_STYLE_TRANSPARENT",
            Entity::HigherUsage { .. } => "SPECIFIED_HIGHER_USAGE_OCCURRENCE",
            Entity::MaterialDesignation { .. } => "MATERIAL_DESIGNATION",
            Entity::MeasureItem { .. } => "MEASURE_REPRESENTATION_ITEM",
            Entity::ValueItem { form, .. } => form.name(),
            Entity::DerivedUnit { .. } => "DERIVED_UNIT",
            Entity::UnitElement { .. } => "DERIVED_UNIT_ELEMENT",
            Entity::Other(name) => name,
        }
    }
}

/// Every instance of a file, decoded, by instance number.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Entities {
    ids: Vec<Id>,
    /// Decoded instances, [`crate::census::CHUNK`] to a chunk: built a
    /// chunk at a time, never copied into one.
    items: Vec<Vec<Result<Entity, ImportError>>>,
}

impl Entities {
    /// Decodes every instance of `file`, in parallel. The result is the
    /// same on any number of threads.
    pub fn decode(file: &File) -> Entities {
        let items = file
            .instances
            .par_chunks(crate::census::CHUNK)
            .map(|c| c.iter().map(decode).collect())
            .collect();
        Entities {
            ids: file.instances.par_iter().map(|i| i.id).collect(),
            items,
        }
    }

    /// [`Entities::decode`], with the file's [`crate::census::Census`]
    /// taken in the same pass: each chunk of instances decoded and counted
    /// together, while its records are in cache.
    pub fn decode_counted(file: &File) -> (Entities, crate::census::Census) {
        let ids: Vec<Id> = file.instances.par_iter().map(|i| i.id).collect();
        let (items, chunks): (Vec<Vec<_>>, Vec<_>) = file
            .instances
            .par_chunks(crate::census::CHUNK)
            .map(|c| {
                let mut out = Vec::with_capacity(c.len());
                let census = crate::census::chunk_with(c, &ids, |i| out.push(decode(i)));
                (out, census)
            })
            .unzip();
        let census = crate::census::Census::from_chunks(ids.clone(), chunks);
        (Entities { ids, items }, census)
    }

    pub fn len(&self) -> usize {
        self.ids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    /// Every instance number for which `keep` holds of it and what it
    /// decoded to, ascending: the instances scanned in parallel, a chunk
    /// at a time, so a pass over a large file for a few of them costs a
    /// fraction of decoding it. The same on any number of threads.
    pub fn select(
        &self,
        keep: impl Fn(Id, &Result<Entity, ImportError>) -> bool + Sync,
    ) -> Vec<Id> {
        self.ids
            .par_chunks(crate::census::CHUNK)
            .zip(self.items.par_iter())
            .flat_map_iter(|(ids, items)| {
                ids.iter()
                    .zip(items)
                    .filter(|(id, x)| keep(**id, x))
                    .map(|(id, _)| *id)
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    /// Every instance number, ascending.
    pub fn ids(&self) -> &[Id] {
        &self.ids
    }

    /// Instance `id`, which `from` refers to: `DanglingReference` if the
    /// file doesn't define it, or its own fault if it couldn't be decoded.
    pub fn get(&self, from: Id, id: Id) -> Result<&Entity, ImportError> {
        match self.ids.binary_search(&id) {
            Ok(k) => self.items[k / crate::census::CHUNK][k % crate::census::CHUNK]
                .as_ref()
                .map_err(Clone::clone),
            Err(_) => Err(ImportError::DanglingReference {
                instance: from,
                reference: id,
            }),
        }
    }

    /// A `CARTESIAN_POINT`.
    pub fn point(&self, from: Id, id: Id) -> Result<&Point, ImportError> {
        match self.get(from, id)? {
            Entity::Point(p) => Ok(p),
            _ => Err(wrong(id, "a CARTESIAN_POINT")),
        }
    }

    /// A `DIRECTION`.
    pub fn direction(&self, from: Id, id: Id) -> Result<&Point, ImportError> {
        match self.get(from, id)? {
            Entity::Direction(d) => Ok(d),
            _ => Err(wrong(id, "a DIRECTION")),
        }
    }

    /// An axis placement of any kind.
    pub fn placement(&self, from: Id, id: Id) -> Result<&Placement, ImportError> {
        match self.get(from, id)? {
            Entity::Placement(p) => Ok(p),
            _ => Err(wrong(id, "an axis placement")),
        }
    }

    /// A curve of any kind.
    pub fn curve(&self, from: Id, id: Id) -> Result<&Curve, ImportError> {
        match self.get(from, id)? {
            Entity::Curve(c) => Ok(c),
            _ => Err(wrong(id, "a curve")),
        }
    }

    /// A `COMPOSITE_CURVE_SEGMENT`: its sense and its curve.
    pub fn segment(&self, from: Id, id: Id) -> Result<(bool, Id), ImportError> {
        match self.get(from, id)? {
            Entity::Segment { same_sense, parent } => Ok((*same_sense, *parent)),
            _ => Err(wrong(id, "a COMPOSITE_CURVE_SEGMENT")),
        }
    }

    /// A surface of any kind.
    pub fn surface(&self, from: Id, id: Id) -> Result<&Surface, ImportError> {
        match self.get(from, id)? {
            Entity::Surface(s) => Ok(s),
            _ => Err(wrong(id, "a surface")),
        }
    }
}

pub(crate) fn wrong(instance: Id, expected: &'static str) -> ImportError {
    ImportError::WrongEntity { instance, expected }
}

fn decode(inst: &Instance) -> Result<Entity, ImportError> {
    let id = inst.id;
    match &inst.record {
        Record::Simple(p) => simple(id, &p.name, &p.params),
        Record::Complex(parts) => complex(id, parts),
    }
}

/// A simple record, by its entity's name.
fn simple(id: Id, name: &str, params: &[Param]) -> Result<Entity, ImportError> {
    if let Some(t) = topology::simple(id, name, params) {
        return t;
    }
    if let Some(r) = representation::simple(id, name, params) {
        return r;
    }
    if let Some(u) = units::simple(id, name, params) {
        return u;
    }
    if let Some(d) = document::simple(id, name, params) {
        return d;
    }
    if let Some(s) = style::simple(id, name, params) {
        return s;
    }
    if let Some(p) = property::simple(id, name, params) {
        return p;
    }
    if let Some(p) = product::simple(id, name, params) {
        return p;
    }
    if let Some(d) = derived::simple(id, name, params) {
        return d;
    }
    if let Some(c) = curve::simple(id, name, params) {
        return c.map(Entity::Curve);
    }
    if let Some(s) = surface::simple(id, name, params) {
        return s.map(Entity::Surface);
    }
    match name {
        "CARTESIAN_POINT" => geometry::point(id, params).map(Entity::Point),
        "DIRECTION" => geometry::point(id, params).map(Entity::Direction),
        "VECTOR" => geometry::vector(id, params),
        "AXIS1_PLACEMENT" | "AXIS2_PLACEMENT_2D" | "AXIS2_PLACEMENT_3D" => {
            geometry::placement(id, name, params).map(Entity::Placement)
        }
        _ => Ok(Entity::Other(name.into())),
    }
}

/// A complex instance, by the parts it has.
fn complex(id: Id, parts: &[Part]) -> Result<Entity, ImportError> {
    if let Some(p) = property::complex(id, parts) {
        return p;
    }
    if let Some(c) = bspline::complex_curve(id, parts) {
        return c.map(|c| Entity::Curve(Curve::BSpline(c)));
    }
    if let Some(s) = bspline::complex_surface(id, parts) {
        return s.map(|s| Entity::Surface(Surface::BSpline(s)));
    }
    if let Some(r) = representation::complex(id, parts) {
        return r;
    }
    if let Some(u) = units::complex(id, parts) {
        return u;
    }
    let names: Vec<&str> = parts.iter().map(|p| &*p.name).collect();
    Ok(Entity::Other(names.join(" ").into()))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::part21::parse;

    /// The entities of a file whose data section is `data`.
    pub fn entities(data: &str) -> Entities {
        let text = format!(
            "ISO-10303-21;HEADER;FILE_SCHEMA(('CONFIG_CONTROL_DESIGN'));ENDSEC;\
             DATA;{data}ENDSEC;END-ISO-10303-21;"
        );
        Entities::decode(&parse(text.as_bytes()).unwrap())
    }

    fn schema(names: &[&str]) -> Result<Schema, StepError> {
        let list: Vec<String> = names.iter().map(|n| format!("'{n}'")).collect();
        let text = format!(
            "ISO-10303-21;HEADER;FILE_SCHEMA(({}));ENDSEC;DATA;ENDSEC;END-ISO-10303-21;",
            list.join(",")
        );
        Schema::of(&parse(text.as_bytes()).unwrap().header)
    }

    #[test]
    fn schemas() {
        assert_eq!(schema(&["CONFIG_CONTROL_DESIGN"]), Ok(Schema::Ap203));
        assert_eq!(
            schema(&[
                "AP203_CONFIGURATION_CONTROLLED_3D_DESIGN_OF_MECHANICAL_PARTS_AND_ASSEMBLIES_MIM_LF { 1 0 10303 403 3 1 4 }"
            ]),
            Ok(Schema::Ap203)
        );
        assert_eq!(
            schema(&["automotive_design { 1 0 10303 214 1 1 1 1 }"]),
            Ok(Schema::Ap214)
        );
        assert_eq!(
            schema(&["AP242_MANAGED_MODEL_BASED_3D_ENGINEERING_MIM_LF. {1 0 10303 442 1 1 4 }"]),
            Ok(Schema::Ap242)
        );
        assert_eq!(schema(&["IFC4", "AUTOMOTIVE_DESIGN"]), Ok(Schema::Ap214));
        // OCCT's own sample files name AP214's first draft.
        assert_eq!(
            schema(&["AUTOMOTIVE_DESIGN_CC1 { 1 2 10303 214 -1 1 3  2}"]),
            Ok(Schema::Ap214)
        );
        assert_eq!(
            schema(&["IFC4"]),
            Err(StepError::UnknownSchema("IFC4".into()))
        );
        assert_eq!(schema(&[]), Err(StepError::UnknownSchema("".into())));
    }

    #[test]
    fn lookups() {
        let e = entities(
            "#1=CARTESIAN_POINT('',(1.,2.,3.));#2=DIRECTION('',(0.,0.,1));\
             #3=CARTESIAN_POINT('',(1.,2.,3.,4.));#4=APPLICATION_CONTEXT('x');\
             #5=(A()B());",
        );
        assert_eq!(e.len(), 5);
        assert_eq!(e.point(9, 1).unwrap().coords(), &[1.0, 2.0, 3.0]);
        assert_eq!(e.direction(9, 2).unwrap().coords(), &[0.0, 0.0, 1.0]);
        assert_eq!(
            e.point(9, 2),
            Err(ImportError::WrongEntity {
                instance: 2,
                expected: "a CARTESIAN_POINT"
            })
        );
        assert_eq!(
            e.point(9, 3),
            Err(ImportError::Malformed {
                instance: 3,
                attribute: 1,
                what: "expected 1 to 3 coordinates"
            })
        );
        assert_eq!(
            e.get(9, 6),
            Err(ImportError::DanglingReference {
                instance: 9,
                reference: 6
            })
        );
        assert_eq!(
            e.get(9, 4),
            Ok(&Entity::Other("APPLICATION_CONTEXT".into()))
        );
        assert_eq!(e.get(9, 5).unwrap().kind(), "A B");
    }

    #[test]
    fn one_thread_and_many_decode_alike() {
        let mut data = String::new();
        for k in 1..=3000 {
            data.push_str(&format!("#{k}=CARTESIAN_POINT('',({k}.,0.,-1.));"));
        }
        let e = entities(&data);
        let one = rayon::ThreadPoolBuilder::new()
            .num_threads(1)
            .build()
            .unwrap()
            .install(|| entities(&data));
        assert_eq!(e, one);
    }

    #[test]
    fn styles_decode() {
        let e = entities(
            "#1=COLOUR_RGB('',1.,0.5,0);\
             #2=FILL_AREA_STYLE_COLOUR('',#1);\
             #3=FILL_AREA_STYLE('',(#2));\
             #4=SURFACE_STYLE_FILL_AREA(#3);\
             #5=SURFACE_STYLE_TRANSPARENT(0.25);\
             #6=SURFACE_STYLE_RENDERING_WITH_PROPERTIES(.NORMAL_SHADING.,#1,(#5));\
             #7=SURFACE_SIDE_STYLE('',(#4,#6));\
             #8=SURFACE_STYLE_USAGE(.BOTH.,#7);\
             #9=PRESENTATION_STYLE_ASSIGNMENT((#8,NULL_STYLE(.NULL.)));\
             #10=STYLED_ITEM('color',(#9),#30);\
             #11=OVER_RIDING_STYLED_ITEM('',(#9),#30,#10);\
             #12=CONTEXT_DEPENDENT_OVER_RIDING_STYLED_ITEM('',(#9),#30,#10,(#40,#41));\
             #13=DRAUGHTING_PRE_DEFINED_COLOUR('red');\
             #14=SPECIFIED_HIGHER_USAGE_OCCURRENCE('','','',#50,#51,$,#40,#41);\
             #15=SURFACE_STYLE_USAGE(.SIDEWAYS.,#7);\
             #16=PRESENTATION_STYLE_BY_CONTEXT((#8),#31);",
        );
        assert_eq!(
            e.get(0, 1),
            Ok(&Entity::ColourRgb {
                rgb: [1.0, 0.5, 0.0]
            })
        );
        assert_eq!(e.get(0, 2), Ok(&Entity::FillColour { colour: 1 }));
        assert_eq!(e.get(0, 3), Ok(&Entity::FillAreaStyle { styles: vec![2] }));
        assert_eq!(e.get(0, 4), Ok(&Entity::SurfaceFill { fill: 3 }));
        assert_eq!(e.get(0, 5), Ok(&Entity::Transparent { transparency: 0.25 }));
        assert_eq!(
            e.get(0, 6),
            Ok(&Entity::Rendering {
                properties: vec![5]
            })
        );
        assert_eq!(
            e.get(0, 7),
            Ok(&Entity::SurfaceSideStyle { styles: vec![4, 6] })
        );
        assert_eq!(
            e.get(0, 8),
            Ok(&Entity::SurfaceStyleUsage {
                side: Side::Both,
                style: 7
            })
        );
        // NULL_STYLE is a typed value, not an instance: it styles nothing.
        assert_eq!(
            e.get(0, 9),
            Ok(&Entity::StyleAssignment { styles: vec![8] })
        );
        let styled = |over_riding, context| Entity::StyledItem {
            styles: vec![9],
            item: 30,
            over_riding,
            context,
        };
        assert_eq!(e.get(0, 10), Ok(&styled(false, None)));
        assert_eq!(e.get(0, 11), Ok(&styled(true, None)));
        assert_eq!(e.get(0, 12), Ok(&styled(true, Some(vec![40, 41]))));
        assert_eq!(
            e.get(0, 13),
            Ok(&Entity::PredefinedColour { name: "red".into() })
        );
        assert_eq!(
            e.get(0, 14),
            Ok(&Entity::HigherUsage {
                upper: 40,
                next: 41
            })
        );
        assert!(matches!(
            e.get(0, 15),
            Err(ImportError::Malformed { instance: 15, .. })
        ));
        assert_eq!(
            e.get(0, 16),
            Ok(&Entity::StyleByContext {
                styles: vec![8],
                context: 31
            })
        );
        for (id, kind) in [
            (10, "STYLED_ITEM"),
            (11, "OVER_RIDING_STYLED_ITEM"),
            (12, "CONTEXT_DEPENDENT_OVER_RIDING_STYLED_ITEM"),
            (14, "SPECIFIED_HIGHER_USAGE_OCCURRENCE"),
            (6, "SURFACE_STYLE_RENDERING_WITH_PROPERTIES"),
        ] {
            assert_eq!(e.get(0, id).unwrap().kind(), kind);
        }
    }

    #[test]
    fn property_items_decode() {
        let e = entities(
            "#1=MATERIAL_DESIGNATION('6061-T6',#30);\
             #2=MATERIAL_DESIGNATION('Steel',(#30,#31));\
             #3=MEASURE_REPRESENTATION_ITEM('density',POSITIVE_RATIO_MEASURE(2.7),#40);\
             #4=(MEASURE_REPRESENTATION_ITEM()MEASURE_WITH_UNIT(MASS_MEASURE(0.12),#41)REPRESENTATION_ITEM('Net Mass'));\
             #5=VALUE_REPRESENTATION_ITEM('Batch Size',COUNT_MEASURE(250.));\
             #6=VALUE_REPRESENTATION_ITEM('RoHS',BOOLEAN(.T.));\
             #7=INTEGER_REPRESENTATION_ITEM('Revisions',3);\
             #8=REAL_REPRESENTATION_ITEM('Yield',0.95);\
             #9=BOOLEAN_REPRESENTATION_ITEM('Lead Free',.F.);\
             #10=DERIVED_UNIT((#11,#12));\
             #11=DERIVED_UNIT_ELEMENT(#41,1.);\
             #12=DERIVED_UNIT_ELEMENT(#42,-3.);\
             #13=PROPERTY_DEFINITION('material property','density',#30);\
             #14=VALUE_REPRESENTATION_ITEM('Broken',$);\
             #15=(INTEGER_REPRESENTATION_ITEM()INT_LITERAL()LITERAL_NUMBER(7)REPRESENTATION_ITEM('Count'));\
             #16=MEASURE_REPRESENTATION_ITEM('Thickness',LENGTH_MEASURE(2.5),$);",
        );
        assert_eq!(
            e.get(0, 1),
            Ok(&Entity::MaterialDesignation {
                name: "6061-T6".into(),
                definitions: vec![30]
            })
        );
        assert_eq!(
            e.get(0, 2),
            Ok(&Entity::MaterialDesignation {
                name: "Steel".into(),
                definitions: vec![30, 31]
            })
        );
        let real = |m: &str, value: f64| ItemValue::Real {
            measure: m.into(),
            value,
        };
        assert_eq!(
            e.get(0, 3),
            Ok(&Entity::MeasureItem {
                name: "density".into(),
                value: real("POSITIVE_RATIO_MEASURE", 2.7),
                unit: Some(40),
            })
        );
        assert_eq!(
            e.get(0, 4),
            Ok(&Entity::MeasureItem {
                name: "Net Mass".into(),
                value: real("MASS_MEASURE", 0.12),
                unit: Some(41),
            })
        );
        let item = |name: &str, value, form| Entity::ValueItem {
            name: name.into(),
            value,
            form,
        };
        assert_eq!(
            e.get(0, 5),
            Ok(&item(
                "Batch Size",
                real("COUNT_MEASURE", 250.0),
                ValueForm::Value
            ))
        );
        assert_eq!(
            e.get(0, 6),
            Ok(&item(
                "RoHS",
                ItemValue::Logical(Some(true)),
                ValueForm::Value
            ))
        );
        assert_eq!(
            e.get(0, 7),
            Ok(&item(
                "Revisions",
                ItemValue::Integer(3),
                ValueForm::Integer
            ))
        );
        assert_eq!(
            e.get(0, 8),
            Ok(&item("Yield", real("", 0.95), ValueForm::Real))
        );
        assert_eq!(
            e.get(0, 9),
            Ok(&item(
                "Lead Free",
                ItemValue::Logical(Some(false)),
                ValueForm::Boolean
            ))
        );
        assert_eq!(
            e.get(0, 10),
            Ok(&Entity::DerivedUnit {
                elements: vec![11, 12]
            })
        );
        assert_eq!(
            e.get(0, 12),
            Ok(&Entity::UnitElement {
                unit: 42,
                exponent: -3.0
            })
        );
        assert_eq!(
            e.get(0, 13),
            Ok(&Entity::PropertyDefinition {
                name: "material property".into(),
                description: "density".into(),
                definition: 30,
            })
        );
        assert!(e.get(0, 14).is_err(), "$ is no value");
        assert_eq!(
            e.get(0, 15),
            Ok(&item("Count", ItemValue::Integer(7), ValueForm::Integer))
        );
        assert_eq!(
            e.get(0, 16),
            Ok(&Entity::MeasureItem {
                name: "Thickness".into(),
                value: real("LENGTH_MEASURE", 2.5),
                unit: None
            })
        );
        assert_eq!(e.get(0, 9).unwrap().kind(), "BOOLEAN_REPRESENTATION_ITEM");
        assert_eq!(e.get(0, 4).unwrap().kind(), "MEASURE_REPRESENTATION_ITEM");
    }
}
