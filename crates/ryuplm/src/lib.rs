//! ryuplm: product lifecycle management (design 734). A catalogue is a
//! folder of items in Git (`ryuplm.json`, `items/<ab>/<id>.json`), each
//! with a part number, a description, an AML, documents and a BOM,
//! whether or not any authoring tool ever touches it. Items are
//! ryupdm parts of kind `item` ([`pdm`]), so revisions, releases, states
//! and the commit guard are ryupdm's.

pub use ryu_doc::{canonical, error, id, source, units};

/// The version of the shape of the JSON that the hosted service reads
/// (design 938): `info`, `check`, `diff`, `item list`, `item show`,
/// `release list`, `merge status`, `git attributes`, `reps`, `index` and
/// `resolve` say it as `"schema"`; `bom show`, `bom diff`, `where-used`
/// and `release check` give theirs by the same rule. Adding a field or a
/// code doesn't change it; changing or removing one does. `version`
/// answers it as `schema_outputs`, so a tool can ask once.
pub const OUTPUT_SCHEMA: u64 = 1;

pub mod activity;
pub mod aml;
pub mod bom;
pub mod bom_diff;
pub mod catalogue;
pub mod check;
pub mod client;
pub mod daemon;
pub mod diff;
pub mod drivers;
pub mod fault;
pub mod ids;
pub mod import;
pub mod item;
pub mod items;
pub mod judge;
pub mod links;
pub mod load;
pub mod merge;
pub mod notify;
pub mod numbers;
pub mod pdm;
pub mod proto;
pub mod refs;
pub mod release;
pub mod reps;
pub mod server;
pub mod session;
pub mod settings;
#[cfg(feature = "testing")]
pub mod testing;
pub mod textconv;
pub mod where_used;
pub mod write;

/// ryuplm, as ryupdm, ryu-git and ryu-session know it.
pub struct Plm;

impl ryu_session::Product for Plm {
    const NAME: &'static str = "ryuplm";
    const RUNTIME_DIR_VAR: &'static str = "RYUPLM_RUNTIME_DIR";
    /// Nothing bulky: items are small files, kept in the log inline.
    const BULKY: &'static [&'static str] = &[];
    type Target = String;
    type View = ();
    type Seen = notify::Seen;
    type Topic = notify::Topic;
    fn view_json(_: &(), _: &mut serde_json::Map<String, serde_json::Value>) {}
}

impl ryu_session::prefs::Schema for Plm {
    const CONFIG_DIR_VAR: &'static str = "RYUPLM_CONFIG_DIR";
    /// `daemon.poll_ms`, and `pdm.dependents`: folders searched for other
    /// catalogues (numbers elsewhere), as ryucad's and ryueda's.
    fn accepts(name: &str, value: &serde_json::Value) -> bool {
        match name {
            "daemon.poll_ms" => value.as_u64().is_some_and(|n| (10..=10_000).contains(&n)),
            "pdm.dependents" => value
                .as_array()
                .is_some_and(|a| a.iter().all(serde_json::Value::is_string)),
            _ => false,
        }
    }
    fn default_of(name: &str) -> Option<serde_json::Value> {
        match name {
            "daemon.poll_ms" => Some(serde_json::json!(250)),
            "pdm.dependents" => Some(serde_json::json!([])),
            _ => None,
        }
    }
}
