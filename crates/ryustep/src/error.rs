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
