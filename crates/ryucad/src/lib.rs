//! ryucad: the application on top of ryukei. Projects of feature files,
//! deterministic rebuild, the agent CLI and the session daemon.

// The file format and its vocabulary live in ryucad-doc, under the same
// paths, so ryupdm can read projects without the rest of ryucad.
pub use ryucad_doc::{doc, error, expr, units};

pub mod activity;
pub mod check;
pub mod client;
pub mod config;
pub mod daemon;
pub mod diff;
pub mod doctor;
pub mod drivers;
pub mod endpoint;
pub mod entity;
pub mod git;
pub mod graph;
pub mod inspect;
pub mod live;
pub mod merge;
pub mod mesh;
pub mod notify;
pub mod param;
pub mod profile;
pub mod proto;
pub mod rebuild;
pub mod reference;
pub mod render;
pub mod renumber;
pub mod server;
pub mod session;
pub mod sketch;
pub mod sketch_view;
pub mod validate;
pub mod view;
