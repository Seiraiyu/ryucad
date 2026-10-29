//! ryudfm: design for manufacturing of printed circuit boards.
//!
//! `odb` reads ODB++ jobs (folders and archives, versions 7.0 to 8.1) into
//! `ryu_board`'s model, with faults and warnings for what it couldn't
//! read. A project is a folder of canonical JSON in Git holding boards as
//! committed inputs, byte for byte (`project`, `input`); the CLI and the
//! project's daemon answer the same requests over `ryu-session`'s
//! plumbing, as ryucad's and ryueda's do (`proto`, `server`). `inspect`,
//! `render` and `check` read a board of a project or, in path mode, a job
//! anywhere.
//!
//! It depends on no kernel and no other product: the board model is
//! `ryu-board`'s, the file vocabulary `ryu-doc`'s, the daemon's plumbing
//! `ryu-session`'s.

pub use ryu_doc::{canonical, error, id, source, units};

pub mod activity;
pub mod boards;
pub mod check;
pub mod client;
pub mod codes;
pub mod daemon;
pub mod doc;
pub mod import;
pub mod input;
pub mod inspect;
pub mod notify;
pub mod odb;
pub mod project;
pub mod proto;
pub mod quantity;
pub mod render;
pub mod server;
pub mod session;
#[cfg(feature = "testing")]
pub mod testing;

/// ryudfm as a product of `ryu-session`: its daemons, clients and live
/// sessions are that crate's, named for ryudfm by this marker.
pub struct Dfm;

impl ryu_session::Product for Dfm {
    const NAME: &'static str = "ryudfm";
    const RUNTIME_DIR_VAR: &'static str = "RYUDFM_RUNTIME_DIR";
    /// Boards' inputs, committed by their bytes: jobs of tens of
    /// megabytes.
    const BULKY: &'static [&'static str] = &["boards/"];
    /// Participants will point at boards and what is on them by reference
    /// (`<board>/<step>/<layer>/<index>`); selections come with the app
    /// (stage 7).
    type Target = String;
    /// Nothing else is shown until there is an app (stage 7).
    type View = ();
    /// The revisions subscribers were last told of.
    type Seen = notify::Seen;
    type Topic = notify::Topic;

    fn view_json(_: &(), _: &mut serde_json::Map<String, serde_json::Value>) {}
}

impl ryu_session::prefs::Schema for Dfm {
    const CONFIG_DIR_VAR: &'static str = "RYUDFM_CONFIG_DIR";

    /// The one preference so far: how often a watched daemon checks the
    /// files.
    fn accepts(name: &str, value: &serde_json::Value) -> bool {
        name == "daemon.poll_ms" && value.as_u64().is_some_and(|n| (10..=10_000).contains(&n))
    }

    fn default_of(name: &str) -> Option<serde_json::Value> {
        (name == "daemon.poll_ms").then(|| serde_json::json!(250))
    }
}
