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
pub mod digest;
pub mod document;
pub mod documents;
pub mod drivers;
pub mod fault;
pub mod ids;
pub mod import;
pub mod index;
pub mod item;
pub mod items;
pub mod judge;
pub mod ledger;
pub mod links;
pub mod listing;
pub mod load;
pub mod merge;
pub mod notify;
pub mod numbers;
pub mod pdm;
pub mod proto;
pub mod reach;
pub mod refs;
pub mod release;
pub mod reps;
pub mod review;
pub mod server;
pub mod session;
pub mod settings;
#[cfg(feature = "testing")]
pub mod testing;
pub mod textconv;
pub mod view;
pub mod where_used;
pub mod write;

/// ryuplm, as ryupdm, ryu-git and ryu-session know it.
pub struct Plm;

impl ryu_session::Product for Plm {
    const NAME: &'static str = "ryuplm";
    const RUNTIME_DIR_VAR: &'static str = "RYUPLM_RUNTIME_DIR";
    /// Documents' bytes (design 937): kept in the log against the bulky
    /// budget, as ryucad's imported STEP files are. Records are small and
    /// count there too, which is harmless.
    const BULKY: &'static [&'static str] = &["documents/"];
    /// `item/<id>`, `item/<id>/line/<line>` or `import/<name>`
    /// ([`refs::target`]).
    type Target = String;
    /// The open item, its tab and the list's filter (design 938).
    type View = view::View;
    type Seen = notify::Seen;
    type Topic = notify::Topic;
    fn view_json(v: &view::View, into: &mut serde_json::Map<String, serde_json::Value>) {
        if let serde_json::Value::Object(shown) = view::to_json(v) {
            into.extend(shown);
        }
    }
}

impl ryu_session::prefs::Schema for Plm {
    const CONFIG_DIR_VAR: &'static str = "RYUPLM_CONFIG_DIR";
    /// `daemon.poll_ms`, `pdm.dependents` (folders searched for other
    /// catalogues: numbers elsewhere, as ryucad's and ryueda's),
    /// `review.url` (the hosted service: the ledger's, and the one
    /// `ryuplm review` talks to), and the window's keys (design 938):
    /// `ui.*` and `app.window`, which only `ryuplm-app` reads.
    fn accepts(name: &str, value: &serde_json::Value) -> bool {
        let one_of = |words: &[&str]| value.as_str().is_some_and(|s| words.contains(&s));
        match name {
            "daemon.poll_ms" => value.as_u64().is_some_and(|n| (10..=10_000).contains(&n)),
            "pdm.dependents" => value
                .as_array()
                .is_some_and(|a| a.iter().all(serde_json::Value::is_string)),
            // The hosted service the ledger is asked of (design 936).
            "review.url" => value.as_str().is_some_and(ryu_review::url_ok),
            "ui.theme" => one_of(&["system", "light", "dark"]),
            "ui.style" => one_of(&["drafting", "graphite", "studio"]),
            "ui.contrast" => one_of(&["normal", "high"]),
            "ui.notify" => one_of(&["all", "errors", "off"]),
            "ui.layout" | "ui.keys" | "app.window" => value.is_object(),
            "ui.flash_changes" | "ui.follow_shows" => value.is_boolean(),
            _ => false,
        }
    }
    fn default_of(name: &str) -> Option<serde_json::Value> {
        use serde_json::json;
        match name {
            "daemon.poll_ms" => Some(json!(250)),
            "pdm.dependents" => Some(json!([])),
            "review.url" => Some(json!(ryu_review::DEFAULT_URL)),
            "ui.theme" => Some(json!("system")),
            "ui.style" => Some(json!("drafting")),
            "ui.contrast" => Some(json!("normal")),
            "ui.notify" => Some(json!("all")),
            "ui.layout" | "ui.keys" => Some(json!({})),
            "ui.flash_changes" | "ui.follow_shows" => Some(json!(true)),
            _ => None,
        }
    }
}

#[cfg(test)]
mod prefs_tests {
    use ryu_session::prefs::Schema;
    use serde_json::json;

    #[test]
    fn review_url_is_an_https_url_or_this_machine() {
        assert!(crate::Plm::accepts(
            "review.url",
            &json!("https://review.example.com")
        ));
        assert!(crate::Plm::accepts(
            "review.url",
            &json!("http://127.0.0.1:9")
        ));
        assert!(!crate::Plm::accepts(
            "review.url",
            &json!("http://example.com")
        ));
        assert!(!crate::Plm::accepts("review.url", &json!(3)));
        assert_eq!(
            crate::Plm::default_of("review.url"),
            Some(json!(ryu_review::DEFAULT_URL))
        );
        assert_eq!(crate::ledger::PLM.name, "ryuplm");
        assert_eq!(crate::ledger::PLM.token_var, "RYUPLM_REVIEW_TOKEN");
        // What it took before, it still takes.
        assert!(crate::Plm::accepts("daemon.poll_ms", &json!(250)));
        assert!(crate::Plm::accepts("pdm.dependents", &json!(["/x"])));
        // The window's keys (design 938), in the user's file.
        assert!(crate::Plm::accepts("ui.theme", &json!("dark")));
        assert!(!crate::Plm::accepts("ui.theme", &json!("pink")));
        assert!(crate::Plm::accepts("ui.layout", &json!({})));
        assert!(crate::Plm::accepts(
            "app.window",
            &json!({"size": [1280, 800]})
        ));
        assert!(crate::Plm::accepts("ui.follow_shows", &json!(false)));
        assert!(!crate::Plm::accepts("ui.flash_changes", &json!("yes")));
        assert_eq!(
            crate::Plm::default_of("ui.flash_changes"),
            Some(json!(true))
        );
        assert_eq!(crate::Plm::default_of("ui.style"), Some(json!("drafting")));
    }
}
