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
mod geometry;

use rayon::prelude::*;

pub use args::Id;
pub use geometry::{Placement, Point};

use crate::part21::{File, Header, Instance, Param, Record};
use crate::{ImportError, StepError};

/// The application protocol a file follows. Their B-rep, unit and product
/// entities are the same, so the schema decides little but what to write.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Schema {
    /// `CONFIG_CONTROL_DESIGN` (edition 1) or AP203 edition 2.
    Ap203,
    /// `AUTOMOTIVE_DESIGN`.
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
                "AUTOMOTIVE_DESIGN" | "AUTOMOTIVE_DESIGN_CC2" => Some(Schema::Ap214),
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
            Entity::Other(name) => name,
        }
    }
}

/// Every instance of a file, decoded, by instance number.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Entities {
    ids: Vec<Id>,
    items: Vec<Result<Entity, ImportError>>,
}

impl Entities {
    /// Decodes every instance of `file`, in parallel. The result is the
    /// same on any number of threads.
    pub fn decode(file: &File) -> Entities {
        let items = file
            .instances
            .par_iter()
            .with_min_len(256)
            .map(decode)
            .collect();
        Entities {
            ids: file.instances.iter().map(|i| i.id).collect(),
            items,
        }
    }

    pub fn len(&self) -> usize {
        self.ids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    /// Every instance number, ascending.
    pub fn ids(&self) -> &[Id] {
        &self.ids
    }

    /// Instance `id`, which `from` refers to: `DanglingReference` if the
    /// file doesn't define it, or its own fault if it couldn't be decoded.
    pub fn get(&self, from: Id, id: Id) -> Result<&Entity, ImportError> {
        match self.ids.binary_search(&id) {
            Ok(k) => self.items[k].as_ref().map_err(Clone::clone),
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
}

pub(crate) fn wrong(instance: Id, expected: &'static str) -> ImportError {
    ImportError::WrongEntity { instance, expected }
}

fn decode(inst: &Instance) -> Result<Entity, ImportError> {
    let id = inst.id;
    match &inst.record {
        Record::Simple(p) => simple(id, &p.name, &p.params),
        Record::Complex(parts) => {
            let names: Vec<&str> = parts.iter().map(|p| &*p.name).collect();
            Ok(Entity::Other(names.join(" ").into()))
        }
    }
}

/// A simple record, by its entity's name.
fn simple(id: Id, name: &str, params: &[Param]) -> Result<Entity, ImportError> {
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
}
