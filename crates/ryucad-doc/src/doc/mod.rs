//! The project on disk: canonical JSON files for the project, its parts
//! and their features. Pure data; nothing here calls ryukei.

pub mod annotation;
pub mod appearance;
pub mod assembly;
pub mod blend;
pub mod body;
pub mod datum;
pub mod deps;
pub mod drawing;
pub mod drawing_annotation;
pub mod drawing_lint;
pub mod drawing_template;
pub mod edit;
pub mod explode;
pub mod files;
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub mod fixtures;
pub mod geometry;
pub mod hole;
pub mod hole_standards;
pub mod import;
pub mod lock;
pub mod mate;
pub mod materials;
pub mod path;
pub mod pattern;
pub mod project;
pub mod reads;
pub mod redline;
pub mod reference;
pub mod sheet_metal;
pub mod sketch;
pub mod solid;
pub mod surface;
pub mod sweep;
pub mod tweak;
pub mod views;

pub use ryu_doc::{canonical, id, source};
