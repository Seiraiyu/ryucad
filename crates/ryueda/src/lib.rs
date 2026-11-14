//! ryueda: electronics CAD for agents to drive. A project is a folder of
//! canonical JSON files in Git: a component library and a design's
//! connectivity. The CLI and the project's daemon answer the same
//! requests, over `ryu-session`'s plumbing, as ryucad's do.
//!
//! It depends on no kernel and on nothing of ryucad's: the file
//! vocabulary is `ryu-doc`'s, the daemon's plumbing `ryu-session`'s.

pub use ryu_doc::{canonical, error, expr, id, source, units};

pub mod activity;
pub mod annotate;
pub mod cascade;
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
pub mod git;
pub mod kicad;
pub mod libcmd;
pub mod libcomp;
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
pub mod project;
pub mod props;
pub mod proto;
pub mod refs;
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

    /// The daemon's own keys, and the window's (plan 3.D): `ui.*` and
    /// `app.window`, which only the app reads.
    fn accepts(name: &str, value: &serde_json::Value) -> bool {
        let one_of = |words: &[&str]| value.as_str().is_some_and(|s| words.contains(&s));
        match name {
            "daemon.poll_ms" => value.as_u64().is_some_and(|n| (10..=10_000).contains(&n)),
            // Seconds a downloaded catalogue serves before searches warn:
            // an hour to a year.
            "parts.max_age" => value
                .as_u64()
                .is_some_and(|n| (3_600..=365 * 86_400).contains(&n)),
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
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use ryu_session::prefs::Schema;
    use serde_json::json;

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
