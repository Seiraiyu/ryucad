//! ryupdm in the window: each part's release state, as `release.list`
//! gives it when the window opens and `pdm.changed` announces whenever a
//! record, a part, the release branch or a tag moves it.

use serde_json::Value;
use std::collections::BTreeMap;

/// A part's release state as the tree's badge says it: `in work`, `B`,
/// `B modified`, `B obsolete` or `unknown`, then `, C proposed` (or
/// `, B obsolete proposed`) while a record isn't official yet.
pub fn badge(state: &Value) -> String {
    let label = state["label"].as_str().unwrap_or("?");
    let mut s = match state["state"].as_str() {
        Some("in_work") => "in work".to_owned(),
        Some("released") => label.to_owned(),
        Some("modified") => format!("{label} modified"),
        Some("obsolete") => format!("{label} obsolete"),
        _ => "unknown".to_owned(),
    };
    if let Some(l) = state["proposed"]["label"].as_str() {
        match state["proposed"]["kind"].as_str() {
            Some("obsolete") => s.push_str(&format!(", {l} obsolete proposed")),
            _ => s.push_str(&format!(", {l} proposed")),
        }
    }
    s
}

/// Why a state is unknown (a record this build can't read), for the
/// badge's hover.
pub fn reason(state: &Value) -> Option<&str> {
    state["reason"]["message"].as_str()
}

/// Every part's release state, by part.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct States {
    pub by_part: BTreeMap<String, Value>,
    /// Bumped on every change, for whatever is drawn from them.
    pub generation: u64,
}

impl States {
    /// All of them, from a `release.list` answer (`parts`: each a state
    /// with its `part`).
    pub fn reset(&mut self, list: &Value) {
        self.by_part = list["parts"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|p| Some((p["part"].as_str()?.to_owned(), p.clone())))
            .collect();
        self.generation += 1;
    }

    /// Those a `pdm.changed` note names (`parts`: each `{part, state,
    /// tags}`).
    pub fn changed(&mut self, note: &Value) {
        for p in note["parts"].as_array().into_iter().flatten() {
            if let Some(id) = p["part"].as_str() {
                self.by_part.insert(id.to_owned(), p["state"].clone());
            }
        }
        self.generation += 1;
    }

    /// `part`'s badge, once its state is known.
    pub fn badge(&self, part: &str) -> Option<String> {
        self.by_part.get(part).map(badge)
    }

    pub fn get(&self, part: &str) -> Option<&Value> {
        self.by_part.get(part)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn badges_say_the_state_and_what_is_proposed() {
        for (state, text) in [
            (json!({"state": "in_work"}), "in work"),
            (json!({"state": "released", "label": "B"}), "B"),
            (json!({"state": "modified", "label": "B"}), "B modified"),
            (json!({"state": "obsolete", "label": "B"}), "B obsolete"),
            (
                json!({"state": "unknown", "reason": {"message": "format 2"}}),
                "unknown",
            ),
            (
                json!({"state": "modified", "label": "B",
                       "proposed": {"label": "C", "kind": "release"}}),
                "B modified, C proposed",
            ),
            (
                json!({"state": "released", "label": "B",
                       "proposed": {"label": "B", "kind": "obsolete"}}),
                "B, B obsolete proposed",
            ),
        ] {
            assert_eq!(badge(&state), text, "{state}");
        }
        assert_eq!(
            reason(&json!({"state": "unknown", "reason": {"message": "format 2"}})),
            Some("format 2")
        );
        assert_eq!(reason(&json!({"state": "in_work"})), None);
    }

    #[test]
    fn states_come_from_the_list_then_follow_notes() {
        let mut s = States::default();
        s.reset(&json!({"parts": [
            {"part": "p", "state": "in_work", "text": "in work"},
            {"part": "q", "state": "released", "label": "A", "text": "released A"},
        ]}));
        assert_eq!(s.badge("p").as_deref(), Some("in work"));
        assert_eq!(s.badge("q").as_deref(), Some("A"));
        assert_eq!(s.badge("r"), None);
        s.changed(&json!({"parts": [
            {"part": "p", "state": {"state": "in_work",
                                    "proposed": {"label": "A", "kind": "release"}},
             "tags": []},
        ]}));
        assert_eq!(s.badge("p").as_deref(), Some("in work, A proposed"));
        assert_eq!(s.badge("q").as_deref(), Some("A"));
        assert_eq!(s.generation, 2);
    }
}
