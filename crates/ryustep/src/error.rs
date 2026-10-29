//! Errors for a file as a whole. A fault in one body is not one of these:
//! it fails that body alone.

use std::fmt;

pub use ryu_part21::StepError;

/// Why a table can't be written: something in it has no Part 21 form.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum WriteError {
    /// `instance` is `None` in the header.
    Unwritable {
        instance: Option<u64>,
        what: &'static str,
    },
    /// A body's face that can't be cut open into a region of its surface
    /// (its loops wind round a torus both ways), or a pipe face that can't
    /// be approximated within the tolerance: named by its stable name.
    NotRepresentable { face: ryukei::name::Name },
}

impl fmt::Display for WriteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WriteError::Unwritable {
                instance: Some(id),
                what,
            } => write!(f, "instance #{id} can't be written: {what}"),
            WriteError::Unwritable {
                instance: None,
                what,
            } => write!(f, "the header can't be written: {what}"),
            WriteError::NotRepresentable { face } => {
                write!(f, "face {face} has no form a file can hold")
            }
        }
    }
}

impl std::error::Error for WriteError {}

impl From<ryu_part21::WriteError> for WriteError {
    fn from(e: ryu_part21::WriteError) -> WriteError {
        let ryu_part21::WriteError::Unwritable { instance, what } = e;
        WriteError::Unwritable { instance, what }
    }
}

/// Why an entity can't be read. It fails only what needs that entity: the
/// body it belongs to, never the whole file. Instances are named by their
/// numbers in the file.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum ImportError {
    /// `instance` refers to `reference`, which the file doesn't define.
    DanglingReference { instance: u64, reference: u64 },
    /// `instance` is not the kind of entity its referrer needs.
    WrongEntity {
        instance: u64,
        expected: &'static str,
    },
    /// Following references from `instance` leads back to it.
    ReferenceCycle { instance: u64 },
    /// An entity this crate doesn't read, where one it does is needed.
    UnsupportedEntity { instance: u64, name: Box<str> },
    /// Representation context `context` has no unit for `quantity`, which
    /// its items need.
    MissingUnit {
        context: u64,
        quantity: &'static str,
    },
    /// An attribute doesn't have the type or value the schema requires.
    /// Attributes count from 0 in a simple record's parameters, or within
    /// a complex instance's part; a fault in the whole entity (lists of
    /// different lengths) is attribute 0.
    Malformed {
        instance: u64,
        attribute: usize,
        what: &'static str,
    },
    /// The kernel refuses `instance`'s geometry as the file gives it: a
    /// spline's knots, a zero radius, a direction of no length.
    InvalidGeometry { instance: u64, error: ryukei::Error },
    /// `instance` lies outside ryukei's size box, in meters.
    OutsideSizeBox { instance: u64 },
    /// An offset curve or surface whose basis has no exact offset (a
    /// spline's or a swept surface's): one within a certified bound comes
    /// with healing.
    NotBounded { instance: u64 },
    /// The solid's shape, read, couldn't be healed into a body.
    Heal(ryukei::HealError),
}

impl fmt::Display for ImportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ImportError::DanglingReference {
                instance,
                reference,
            } => write!(f, "#{instance} refers to #{reference}, which isn't defined"),
            ImportError::WrongEntity { instance, expected } => {
                write!(f, "#{instance} is not {expected}")
            }
            ImportError::ReferenceCycle { instance } => {
                write!(f, "#{instance} refers back to itself")
            }
            ImportError::UnsupportedEntity { instance, name } => {
                write!(f, "#{instance} is a {name}, which ryustep doesn't read")
            }
            ImportError::MissingUnit { context, quantity } => {
                write!(f, "context #{context} has no {quantity} unit")
            }
            ImportError::Malformed {
                instance,
                attribute,
                what,
            } => write!(f, "#{instance}, attribute {attribute}: {what}"),
            ImportError::InvalidGeometry { instance, error } => {
                write!(f, "#{instance} is not valid geometry: {error}")
            }
            ImportError::OutsideSizeBox { instance } => {
                write!(f, "#{instance} lies outside the size box")
            }
            ImportError::NotBounded { instance } => {
                write!(f, "#{instance} is an offset with no exact form")
            }
            ImportError::Heal(h) => write!(f, "can't heal the solid: {h}"),
        }
    }
}

impl std::error::Error for ImportError {}
