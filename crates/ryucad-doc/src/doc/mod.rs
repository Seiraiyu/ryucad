//! The project on disk: canonical JSON files for the project, its parts
//! and their features. Pure data; nothing here calls ryukei.

pub mod assembly;
pub mod blend;
pub mod canonical;
pub mod files;
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub mod fixtures;
pub mod hole;
pub mod id;
pub mod mate;
pub mod pattern;
pub mod project;
pub mod reference;
pub mod sketch;
pub mod solid;
pub mod source;
pub mod sweep;
