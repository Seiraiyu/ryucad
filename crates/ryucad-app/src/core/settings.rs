//! Settings as the window knows them: project settings (in `ryucad.json`)
//! and the user's preferences, each with where its value comes from. Both
//! are read with `config.get` and kept up to date by `config.changed`; the
//! window changes them only through `config.set`, like an agent.

use serde_json::Value;

/// One setting.
#[derive(Clone, Debug, PartialEq)]
pub struct Setting {
    pub key: String,
    /// `project` or `user`.
    pub scope: String,
    pub value: Value,
    /// The value is the default: nobody set it.
    pub default: bool,
    pub about: String,
}

#[derive(Debug, Default)]
pub struct Settings {
    pub all: Vec<Setting>,
    /// The revision project settings were read at: a change sends it.
    pub project_revision: String,
    /// Problems reading the preferences file.
    pub warnings: Vec<String>,
    pub generation: u64,
}

impl Settings {
    /// Replaces everything with a `config.get` answer.
    pub fn reset(&mut self, v: &Value) {
        self.all = v["settings"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|s| Setting {
                key: s["key"].as_str().unwrap_or_default().to_owned(),
                scope: s["scope"].as_str().unwrap_or_default().to_owned(),
                value: s["value"].clone(),
                default: s["default"] == true,
                about: s["about"].as_str().unwrap_or_default().to_owned(),
            })
            .collect();
        self.project_revision = v["project_revision"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        self.warnings = v["warnings"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|w| w.as_str().map(str::to_owned))
            .collect();
        self.generation += 1;
    }

    /// Takes a `config.changed` notification.
    pub fn changed(&mut self, v: &Value) {
        if let Some(s) = self.all.iter_mut().find(|s| v["key"] == s.key.as_str()) {
            s.value = v["value"].clone();
            s.default = v["default"] == true;
            self.generation += 1;
        }
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        self.all.iter().find(|s| s.key == key).map(|s| &s.value)
    }

    /// A text setting, or `fallback` until the settings arrive.
    pub fn text<'a>(&'a self, key: &str, fallback: &'a str) -> &'a str {
        self.get(key).and_then(Value::as_str).unwrap_or(fallback)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn changes_land_on_their_setting() {
        let mut s = Settings::default();
        s.reset(&json!({
            "settings": [
                {"key": "ui.theme", "scope": "user", "value": "system", "default": true, "about": "colours"},
                {"key": "default_unit", "scope": "project", "value": "mm", "default": false, "about": "unit"},
            ],
            "project_revision": "r1",
            "warnings": ["preferences.json is newer"],
        }));
        assert_eq!(s.text("ui.theme", "x"), "system");
        assert_eq!(s.text("ui.navigation", "ryucad"), "ryucad");
        s.changed(&json!({"key": "ui.theme", "scope": "user", "value": "dark"}));
        assert_eq!(s.text("ui.theme", "x"), "dark");
        assert!(!s.all[0].default);
        // Set back to its default, it says so.
        s.changed(&json!({"key": "ui.theme", "scope": "user", "value": "system", "default": true}));
        assert!(s.all[0].default);
        assert_eq!((s.project_revision.as_str(), s.warnings.len()), ("r1", 1));
    }
}
