//! Errors for a file as a whole. A fault in one body is not one of these:
//! it fails that body alone.

use std::fmt;

/// Why a file can't be read at all.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum StepError {
    /// The file doesn't start as an ISO 10303-21 exchange structure.
    NotPart21 { offset: usize },
    /// The exchange structure breaks its grammar. Lines and columns count
    /// from 1; columns are bytes.
    Syntax {
        line: usize,
        column: usize,
        what: &'static str,
    },
    /// Two instances share one instance number.
    DuplicateInstance { id: u64 },
    /// No schema the file names is one this crate reads.
    UnknownSchema(String),
    /// The file has no data section.
    NoData,
}

impl fmt::Display for StepError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StepError::NotPart21 { offset } => {
                write!(f, "not an ISO 10303-21 file (at byte {offset})")
            }
            StepError::Syntax { line, column, what } => {
                write!(f, "syntax error at line {line}, column {column}: {what}")
            }
            StepError::DuplicateInstance { id } => write!(f, "instance #{id} is defined twice"),
            StepError::UnknownSchema(s) => write!(f, "schema {s:?} is not one ryustep reads"),
            StepError::NoData => write!(f, "the file has no data section"),
        }
    }
}

impl std::error::Error for StepError {}

/// Why a table can't be written: something in it has no Part 21 form.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum WriteError {
    /// `instance` is `None` in the header.
    Unwritable {
        instance: Option<u64>,
        what: &'static str,
    },
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
        }
    }
}

impl std::error::Error for WriteError {}

/// Why an entity can't be read. It fails only what needs that entity: the
/// body it belongs to, never the whole file. Instances are named by their
/// numbers in the file.
#[derive(Clone, Debug, PartialEq, Eq)]
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
    /// An attribute (counted from 0, in the order of a simple record's
    /// parameters) doesn't have the type or value the schema requires.
    Malformed {
        instance: u64,
        attribute: usize,
        what: &'static str,
    },
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
            ImportError::Malformed {
                instance,
                attribute,
                what,
            } => write!(f, "#{instance}, attribute {attribute}: {what}"),
        }
    }
}

impl std::error::Error for ImportError {}
