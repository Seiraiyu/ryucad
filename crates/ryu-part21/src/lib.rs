//! The exchange structure (ISO 10303-21): a header, then a table of entity
//! instances, each a record of typed parameters.
//!
//! Nothing here knows any schema: an instance is a name and its values, and
//! references are instance numbers, resolved by whoever reads them. It
//! knows no kernel either, so a product that only checks a STEP file
//! parses (ryueda's 3D models) needs nothing else. ryustep re-exports it
//! as `ryustep::part21`.

mod error;
mod file;
mod lex;
mod text;
mod write;

pub use error::{StepError, WriteError};
pub use file::{File, Header, Instance, Record, parse};
pub use write::write;

/// One parameter of a record.
#[derive(Clone, Debug, PartialEq)]
pub enum Param {
    /// `$`: no value.
    Unset,
    /// `*`: a value the schema derives.
    Derived,
    Int(i64),
    /// Always finite.
    Real(f64),
    Str(Box<str>),
    /// `.NAME.`, held without its dots. Booleans and logicals are `T`, `F`
    /// and `U`.
    Enum(Box<str>),
    /// `"…"`, held as written: its first digit says how many bits of the
    /// first hex digit are unused.
    Binary(Box<str>),
    /// `#n`: another instance.
    Ref(u64),
    List(Vec<Param>),
    /// `NAME(value)`: a value of a defined type, such as
    /// `LENGTH_MEASURE(1.E-07)`.
    Typed(Box<str>, Box<Param>),
}

/// An entity's name and its parameters, as one record holds them.
#[derive(Clone, Debug, PartialEq)]
pub struct Part {
    /// Upper case; a user-defined name keeps its `!`.
    pub name: Box<str>,
    pub params: Vec<Param>,
}
