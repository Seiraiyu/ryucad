//! ryucad: the application on top of ryukei. Projects of feature files,
//! deterministic rebuild, the agent CLI and the session daemon.

// The file format and its vocabulary live in ryucad-doc, under the same
// paths, so ryupdm can read projects without the rest of ryucad.
pub use ryucad_doc::{doc, error, expr, units};

/// The version of the shape of the JSON that `check`, `diff`, `inspect`,
/// `release list` and `mesh` give (`"schema"` in each). Adding a field or
/// a code doesn't change it; changing or removing one does. Other tools,
/// the hosted review service among them, read these.
pub const OUTPUT_SCHEMA: u64 = 1;

pub mod activity;
pub mod assembly;
pub mod cache;
pub mod check;
pub mod client;
pub mod config;
pub mod connector;
pub mod daemon;
pub mod diff;
pub mod doctor;
pub mod drivers;
pub mod endpoint;
pub mod entity;
pub mod git;
pub mod graph;
pub mod hole;
pub mod inspect;
pub mod interference;
pub mod live;
pub mod mate;
pub mod merge;
pub mod mesh;
pub mod notify;
pub mod param;
pub mod pattern;
pub mod pdm;
pub mod profile;
pub mod proto;
pub mod rebuild;
pub mod reference;
pub mod release;
pub mod render;
pub mod renumber;
pub mod review;
pub mod server;
pub mod session;
pub mod sketch;
pub mod sketch_view;
pub mod solid;
pub mod validate;
pub mod view;
pub mod visibility;
