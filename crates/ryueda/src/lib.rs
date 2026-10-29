//! ryueda: electronics CAD for agents to drive. A project is a folder of
//! canonical JSON files in Git: a component library and a design's
//! connectivity. The CLI and the project's daemon answer the same
//! requests, over `ryu-session`'s plumbing, as ryucad's do.
//!
//! It depends on no kernel and on nothing of ryucad's: the file
//! vocabulary is `ryu-doc`'s, the daemon's plumbing `ryu-session`'s.

pub use ryu_doc::{canonical, error, expr, id, source, units};

pub mod activity;
pub mod client;
pub mod conn;
pub mod coord;
pub mod daemon;
pub mod design;
pub mod doc;
pub mod edit;
pub mod erc;
pub mod fault;
pub mod footprint;
pub mod kicad;
pub mod libcmd;
pub mod libcomp;
pub mod library;
pub mod name;
pub mod netlist;
pub mod notify;
pub mod partcmd;
pub mod project;
pub mod props;
pub mod proto;
pub mod server;
pub mod session;
pub mod symbol;
pub mod topcmd;
pub mod write;

/// ryueda as a product of `ryu-session`: its daemons, clients and live
/// sessions are that crate's, named for ryueda by this marker.
pub struct Eda;

impl ryu_session::Product for Eda {
    const NAME: &'static str = "ryueda";
    const RUNTIME_DIR_VAR: &'static str = "RYUEDA_RUNTIME_DIR";
    /// STEP models a footprint shows: committed by their bytes.
    const BULKY: &'static [&'static str] = &["library/models/"];
    /// Participants point at objects by reference path
    /// (`lib/self/symbol/r`); ryueda uses selections from stage 3.
    type Target = String;
    /// Nothing else is shown until there is an app (stage 3).
    type View = ();
    /// The revisions subscribers were last told of.
    type Seen = notify::Seen;
    type Topic = notify::Topic;

    fn view_json(_: &(), _: &mut serde_json::Map<String, serde_json::Value>) {}
}

impl ryu_session::prefs::Schema for Eda {
    const CONFIG_DIR_VAR: &'static str = "RYUEDA_CONFIG_DIR";

    /// The one preference so far: how often a watched daemon checks the
    /// files.
    fn accepts(name: &str, value: &serde_json::Value) -> bool {
        name == "daemon.poll_ms" && value.as_u64().is_some_and(|n| (10..=10_000).contains(&n))
    }

    fn default_of(name: &str) -> Option<serde_json::Value> {
        (name == "daemon.poll_ms").then(|| serde_json::json!(250))
    }
}
