//! ryueda: electronics CAD for agents to drive. A project is a folder of
//! canonical JSON files in Git: a component library and a design's
//! connectivity. The CLI and the project's daemon answer the same
//! requests, over `ryu-session`'s plumbing, as ryucad's do.
//!
//! It depends on no kernel and on nothing of ryucad's: the file
//! vocabulary is `ryu-doc`'s, the daemon's plumbing `ryu-session`'s.

pub use ryu_doc::{canonical, error, expr, id, source, units};

/// The version of the shape of the JSON that the hosted review service
/// reads (design 667): `check`, `diff`, `merge status`, `release list`,
/// `lib uses`, `lib update`, `scene`, `render`, `resolve` and `git
/// attributes` say it as `"schema"`; `bom`, `bom diff`, `where-used` and
/// `release check` give ryupdm's, by the same rule. Adding a field or a
/// code doesn't change it; changing or removing one does. `version`
/// answers it as `schema_outputs`, so a tool can ask once.
pub const OUTPUT_SCHEMA: u64 = 1;

pub mod activity;
pub mod anchor;
pub mod annotate;
pub mod board;
pub mod cascade;
pub mod clash;
pub mod client;
pub mod compare;
pub mod conn;
pub mod coord;
pub mod daemon;
pub mod design;
pub mod diff;
pub mod distributors;
pub mod doc;
pub mod doctor;
pub mod drivers;
pub mod edit;
pub mod erc;
pub mod fault;
pub mod footprint;
pub mod fpedit;
pub mod fplint;
pub mod git;
pub mod kicad;
pub mod libcmd;
pub mod libcomp;
pub mod libdeps;
pub mod library;
pub mod lint;
pub mod merge;
pub mod name;
pub mod net;
pub mod netlist;
pub mod notify;
pub mod output;
pub mod partcmd;
pub mod parts;
pub mod pdf;
pub mod pdm;
pub mod pin;
pub mod profile;
pub mod project;
pub mod props;
pub mod proto;
pub mod refs;
pub mod review;
pub mod scene;
pub mod schem;
pub mod schemcmd;
pub mod server;
pub mod session;
pub mod sourcing;
pub mod standards;
pub mod style;
pub mod stylelint;
pub mod supply;
pub mod symbol;
pub mod symlint;
pub mod template;
pub mod textconv;
pub mod topcmd;
pub mod validate;
pub mod view;
pub mod vocab;
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
    /// What a participant shows besides its selection: its compare (plan
    /// 4.B), and its sheet, layer and camera (plan 3.B).
    type View = view::View;
    /// The revisions subscribers were last told of.
    type Seen = notify::Seen;
    type Topic = notify::Topic;

    fn view_json(v: &view::View, into: &mut serde_json::Map<String, serde_json::Value>) {
        into.insert("compare".into(), serde_json::json!(v.compare));
        if let serde_json::Value::Object(shown) = view::to_json(v) {
            into.extend(shown);
        }
    }
}

impl ryu_session::prefs::Schema for Eda {
    const CONFIG_DIR_VAR: &'static str = "RYUEDA_CONFIG_DIR";

    /// How often a watched daemon checks the files, how long a catalogue
    /// serves, the folders where-used searches, the hosted review service
    /// `ryueda review` talks to, and the window's keys (plan 3.D): `ui.*`
    /// and `app.window`, which only the app reads.
    fn accepts(name: &str, value: &serde_json::Value) -> bool {
        let one_of = |words: &[&str]| value.as_str().is_some_and(|s| words.contains(&s));
        match name {
            "daemon.poll_ms" => value.as_u64().is_some_and(|n| (10..=10_000).contains(&n)),
            // Seconds a downloaded catalogue serves before searches warn:
            // an hour to a year.
            "parts.max_age" => value
                .as_u64()
                .is_some_and(|n| (3_600..=365 * 86_400).contains(&n)),
            // Folders where-used searches for other checkouts (design 629).
            "pdm.dependents" => value
                .as_array()
                .is_some_and(|a| a.iter().all(serde_json::Value::is_string)),
            // The hosted review service `ryueda review` talks to (design 667).
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
            "parts.max_age" => Some(json!(crate::parts::MAX_AGE)),
            "ui.theme" => Some(json!("system")),
            "ui.style" => Some(json!("drafting")),
            "ui.contrast" => Some(json!("normal")),
            "ui.notify" => Some(json!("all")),
            "ui.layout" | "ui.keys" => Some(json!({})),
            "ui.flash_changes" | "ui.follow_shows" => Some(json!(true)),
            "pdm.dependents" => Some(json!([])),
            "review.url" => Some(json!(ryu_review::DEFAULT_URL)),
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
        assert!(crate::Eda::accepts(
            "review.url",
            &json!("https://review.example.com")
        ));
        assert!(crate::Eda::accepts(
            "review.url",
            &json!("http://127.0.0.1:9")
        ));
        assert!(!crate::Eda::accepts(
            "review.url",
            &json!("http://example.com")
        ));
        assert!(!crate::Eda::accepts("review.url", &json!(3)));
        assert_eq!(
            crate::Eda::default_of("review.url"),
            Some(json!(ryu_review::DEFAULT_URL))
        );
    }

    #[test]
    fn the_window_s_preferences_are_accepted_with_their_defaults() {
        for (k, v) in [
            ("ui.theme", json!("dark")),
            ("ui.style", json!("graphite")),
            ("ui.contrast", json!("high")),
            ("ui.layout", json!({"format": 1})),
            ("ui.keys", json!({"view.fit": ["F"]})),
            ("ui.notify", json!("errors")),
            ("ui.flash_changes", json!(false)),
            ("ui.follow_shows", json!(false)),
            ("app.window", json!({"size": [1280, 800]})),
        ] {
            assert!(super::Eda::accepts(k, &v), "{k}");
        }
        assert!(!super::Eda::accepts("ui.theme", &json!("blue")));
        assert!(!super::Eda::accepts("ui.flash_changes", &json!("yes")));
        assert_eq!(super::Eda::default_of("ui.theme"), Some(json!("system")));
        assert_eq!(
            super::Eda::default_of("ui.flash_changes"),
            Some(json!(true))
        );
    }
}
