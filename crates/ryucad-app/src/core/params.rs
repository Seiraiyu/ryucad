//! A part's parameters in the parameter panel: each as written, what it
//! evaluates to and what reads it, editable in place. A save is an
//! ordinary `param.set` with the revision `param.list` read, so a stale
//! save fails with `conflict` like an agent's; the panel then reloads and
//! lists what changed instead of overwriting it.

use crate::core::edit::{Changed, Field};
use ryucad::error::Error;
use ryucad::proto::Request;
use serde_json::{Map, Value, json};

/// A parameter: its expression as written (editable), its value, and what
/// reads it.
#[derive(Clone, Debug, PartialEq)]
pub struct Param {
    pub field: Field,
    /// Its value for people: `20 mm`, `15 deg`, `3`; empty when it fails.
    pub value: String,
    /// The features and parameters that read it.
    pub used_by: Vec<String>,
    /// Why it doesn't evaluate.
    pub error: Option<String>,
}

/// The parameters of one part.
#[derive(Clone, Debug, PartialEq)]
pub struct Params {
    pub part: String,
    /// The part revision they were read at.
    pub revision: String,
    pub params: Vec<Param>,
    /// A parameter being added: its name and expression.
    pub new_name: String,
    pub new_value: String,
    /// What changed under a save that conflicted.
    pub changed: Vec<Changed>,
    /// Why the last save failed, other than a conflict.
    pub error: Option<Error>,
    pub saving: bool,
}

fn text(v: &Value) -> String {
    v.as_str().unwrap_or_default().to_owned()
}

/// A parameter's value for people, in the project's default `unit`.
pub(crate) fn value(p: &Value, unit: &str) -> String {
    let n = ryucad::sketch_view::number;
    if let Some(m) = p["m"].as_f64() {
        let factor = ryucad::units::Unit::parse(unit).map_or(0.001, |u| u.factor());
        format!("{} {unit}", n(m / factor))
    } else if let Some(r) = p["rad"].as_f64() {
        format!("{} deg", n(r * 180.0 / core::f64::consts::PI))
    } else if let Some(x) = p["n"].as_f64() {
        n(x)
    } else {
        String::new()
    }
}

impl Params {
    /// The panel for a `param.list` answer, values shown in `unit`.
    pub fn from_list(v: &Value, unit: &str) -> Params {
        let params = v["parameters"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|p| {
                let used = &p["used_by"];
                let used_by = used["features"]
                    .as_array()
                    .into_iter()
                    .chain(used["parameters"].as_array())
                    .flatten()
                    .map(text)
                    .collect();
                Param {
                    field: Field::new(&text(&p["name"]), text(&p["value"])),
                    value: value(p, unit),
                    used_by,
                    error: p.get("error").map(|e| text(&e["message"])),
                }
            })
            .collect();
        Params {
            part: text(&v["part"]),
            revision: text(&v["revision"]),
            params,
            new_name: String::new(),
            new_value: String::new(),
            changed: Vec::new(),
            error: None,
            saving: false,
        }
    }

    /// Whether anything was typed, a new parameter included.
    pub fn dirty(&self) -> bool {
        self.params.iter().any(|p| p.field.dirty())
            || !self.new_name.trim().is_empty()
            || !self.new_value.trim().is_empty()
    }

    /// The `param.set` that saves what was typed, guarded by the revision
    /// the panel was read at.
    pub fn request(&self) -> Request {
        let mut parameters: Map<String, Value> = self
            .params
            .iter()
            .filter(|p| p.field.dirty())
            .map(|p| (p.field.name.clone(), json!(p.field.text.trim())))
            .collect();
        if !self.new_name.trim().is_empty() {
            parameters.insert(
                self.new_name.trim().to_owned(),
                json!(self.new_value.trim()),
            );
        }
        Request::ParamSet {
            part: Some(self.part.clone()),
            scope: None,
            parameters,
            expected_revision: Some(self.revision.clone()),
        }
    }

    /// The panel read again after a conflict: the new values, with what
    /// changed under what was typed listed, and the new parameter kept.
    pub fn reloaded(&self, v: &Value, unit: &str) -> Params {
        let mut fresh = Params::from_list(v, unit);
        fresh.changed = self
            .params
            .iter()
            .filter(|p| p.field.dirty())
            .filter_map(|mine| {
                let now = fresh
                    .params
                    .iter()
                    .find(|p| p.field.name == mine.field.name);
                let theirs = now.map_or("(deleted)".to_owned(), |p| p.field.read.clone());
                (theirs != mine.field.read).then(|| Changed {
                    name: mine.field.name.clone(),
                    theirs,
                    yours: mine.field.text.trim().to_owned(),
                })
            })
            .collect();
        fresh.new_name.clone_from(&self.new_name);
        fresh.new_value.clone_from(&self.new_value);
        fresh
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listed(width: &str, revision: &str) -> Value {
        json!({"part": "p", "revision": revision, "parameters": [
            {"name": "hole", "value": "width / 4", "m": 0.005, "expr": true,
             "used_by": {"features": ["s"], "parameters": []}},
            {"name": "tilt", "value": "15 deg", "rad": 0.2617993877991494,
             "used_by": {"features": [], "parameters": []}},
            {"name": "width", "value": width, "m": 0.02,
             "used_by": {"features": ["s"], "parameters": ["hole"]}}
        ]})
    }

    #[test]
    fn the_panel_lists_values_and_readers() {
        let p = Params::from_list(&listed("20 mm", "r1"), "mm");
        assert_eq!(p.part, "p");
        let rows: Vec<(&str, &str, &str)> = p
            .params
            .iter()
            .map(|r| {
                (
                    r.field.name.as_str(),
                    r.field.read.as_str(),
                    r.value.as_str(),
                )
            })
            .collect();
        assert_eq!(
            rows,
            [
                ("hole", "width / 4", "5 mm"),
                ("tilt", "15 deg", "15 deg"),
                ("width", "20 mm", "20 mm")
            ]
        );
        assert_eq!(p.params[2].used_by, ["s", "hole"]);
        assert_eq!(
            Params::from_list(&listed("20 mm", "r1"), "cm").params[0].value,
            "0.5 cm"
        );
        assert!(!p.dirty());
    }

    #[test]
    fn a_save_sets_what_was_typed() {
        let mut p = Params::from_list(&listed("20 mm", "r1"), "mm");
        p.params[2].field.text = "25 mm".into();
        p.new_name = " depth ".into();
        p.new_value = "width / 2".into();
        assert!(p.dirty());
        let Request::ParamSet {
            part,
            parameters,
            expected_revision,
            ..
        } = p.request()
        else {
            panic!()
        };
        assert_eq!(part.as_deref(), Some("p"));
        assert_eq!(
            Value::Object(parameters),
            json!({"width": "25 mm", "depth": "width / 2"})
        );
        assert_eq!(expected_revision.as_deref(), Some("r1"));
        // Someone set the width first: listed, and the new one kept.
        let again = p.reloaded(&listed("30 mm", "r2"), "mm");
        assert_eq!(again.revision, "r2");
        assert_eq!(
            again.changed,
            [Changed {
                name: "width".into(),
                theirs: "30 mm".into(),
                yours: "25 mm".into()
            }]
        );
        assert_eq!(again.new_name, " depth ");
    }
}
