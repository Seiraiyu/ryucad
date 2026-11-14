//! The `ryueda.content.1` hash of a design or a library component
//! (design 629): what a release record says was released.

use crate::error::{Code, Error, Result};
use crate::id::Id;
use crate::source::Source;

/// The content hash of `part` in the project `src` holds.
pub fn hash(src: &dyn Source, part: &Id) -> Result<String> {
    let _ = src;
    Err(Error::new(
        Code::Internal,
        format!("no content hash for {part} yet"),
    ))
}
