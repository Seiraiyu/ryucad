//! ryustep: STEP (ISO 10303) files for the ryukei kernel.
//!
//! [`part21`] reads and writes the exchange structure itself (ISO 10303-21):
//! a header and a table of entity instances, parsed in parallel and written
//! back deterministically.
//!
//! It knows nothing of documents or projects: ryucad decides where a file
//! comes from and what its bodies become.

mod error;
pub mod part21;

pub use error::{StepError, WriteError};
