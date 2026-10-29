//! ryustep: STEP (ISO 10303) files for the ryukei kernel.
//!
//! [`part21`] reads and writes the exchange structure itself (ISO 10303-21):
//! a header and a table of entity instances, parsed in parallel and written
//! back deterministically. [`schema`] types the entities of AP203, AP214 and
//! AP242 that B-rep solids, units and product structure use, and
//! [`products`] reads the product tree from them. [`read`] makes ryukei
//! bodies of a file's solids, and [`write`] writes bodies and a product
//! tree as a file.
//!
//! It knows nothing of documents or projects: ryucad decides where a file
//! comes from and what its bodies become.

mod error;
pub use ryu_part21 as part21;
pub mod products;
pub mod read;
pub mod schema;
pub mod write;

pub use error::{ImportError, StepError, WriteError};
pub use read::{Held, Imported, Model, Opened, ReadOptions, open, read};
pub use write::{Child, LengthUnit, Product, WriteModel, WriteOptions, write};

/// This crate's version, which a released ryucad's cache identity names.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
