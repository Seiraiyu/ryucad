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

/// ryucad as a product of `ryu-session`: its daemons, clients and live
/// sessions are that crate's, named for ryucad by this marker
/// ([`endpoint::Endpoint`] is `ryu_session::endpoint::Endpoint<Cad>`).
pub struct Cad;

impl ryu_session::Product for Cad {
    const NAME: &'static str = "ryucad";
    const RUNTIME_DIR_VAR: &'static str = "RYUCAD_RUNTIME_DIR";
    /// Imported STEP files: a re-import holds both versions.
    const BULKY: &'static [&'static str] = &["imports/"];
    type Target = entity::EntityRef;
    type View = live::View;
    type Seen = live::Seen;
    type Topic = notify::Topic;

    fn view_json(view: &live::View, into: &mut serde_json::Map<String, serde_json::Value>) {
        use serde_json::json;
        into.insert("camera".into(), json!(view.camera));
        into.insert(
            "compare".into(),
            json!(view.compare.as_ref().map(|rev| json!({ "rev": rev }))),
        );
        into.insert("focus".into(), json!(view.focus));
        into.insert("hidden".into(), json!(view.hidden));
    }
}

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
pub mod export;
pub mod git;
pub mod graph;
pub mod hole;
pub mod import;
pub mod inspect;
pub mod interference;
pub mod live;
pub mod mate;
pub mod materials;
pub mod measure;
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
