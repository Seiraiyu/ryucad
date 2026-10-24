//! A part's properties in the property panel's Part section: every core
//! and declared property with a control for its type, read with
//! `props.get` and saved with `props.set` against the revision it read,
//! so a stale save fails with `conflict` like an agent's; the form then
//! reloads and lists what changed instead of overwriting it.

use crate::core::edit::{Changed, Field};
use ryucad::error::Error;
use ryucad::proto::Request;
use serde_json::{Map, Value, json};

/// How a property is edited.
#[derive(Clone, Debug, PartialEq)]
pub enum Control {
    /// One line (`string`) or several (`text`).
    Text {
        multiline: bool,
    },
    /// A number typed in (`integer`, `number`); the daemon reads it as
    /// the type wants.
    Number,
    /// A quantity typed with its unit (`2.5 mm`).
    Quantity,
    Boolean,
    /// One of the declared values.
    Enum(Vec<String>),
    /// Set but declared nowhere: shown as written, to clear or keep.
    Unknown,
}

/// One property: its control, what the settings say of it, its value as
/// read and as typed (empty when unset), and its faults.
#[derive(Clone, Debug, PartialEq)]
pub struct Prop {
    pub field: Field,
    pub control: Control,
    pub core: bool,
    pub required: bool,
    pub revising: bool,
    pub faults: Vec<String>,
}

/// The Part section's form for one part.
#[derive(Clone, Debug, PartialEq)]
pub struct PartProps {
    pub part: String,
    /// The part revision it was read at.
    pub revision: String,
    pub props: Vec<Prop>,
    /// What changed under a save that conflicted.
    pub changed: Vec<Changed>,
    /// Why the last save failed, other than a conflict.
    pub error: Option<Error>,
    pub saving: bool,
}

/// A value as the form shows it: text as written, numbers and booleans
/// as JSON writes them, nothing for unset.
fn shown(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        v => v.to_string(),
    }
}

fn control(decl: &Value) -> Control {
    match decl["type"].as_str() {
        Some("string") => Control::Text { multiline: false },
        Some("text") => Control::Text { multiline: true },
        Some("integer" | "number") => Control::Number,
        Some("quantity") => Control::Quantity,
        Some("boolean") => Control::Boolean,
        Some("enum") => Control::Enum(
            decl["values"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect(),
        ),
        _ => Control::Unknown,
    }
}

impl PartProps {
    /// The form for a `props.get` answer.
    pub fn from_get(v: &Value) -> PartProps {
        let props = v["properties"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|p| {
                let decl = &p["declaration"];
                Prop {
                    field: Field::new(p["name"].as_str().unwrap_or_default(), shown(&p["value"])),
                    control: control(decl),
                    core: p["core"] == true,
                    required: decl["required"] == true,
                    revising: decl["revising"] == true,
                    faults: p["faults"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|f| f["message"].as_str().map(str::to_owned))
                        .collect(),
                }
            })
            .collect();
        PartProps {
            part: v["part"].as_str().unwrap_or_default().to_owned(),
            revision: v["revision"].as_str().unwrap_or_default().to_owned(),
            props,
            changed: Vec::new(),
            error: None,
            saving: false,
        }
    }

    /// Whether anything was typed or picked.
    pub fn dirty(&self) -> bool {
        self.props.iter().any(|p| p.field.dirty())
    }

    /// The `props.set` that saves what changed, guarded by the revision
    /// the form was read at: emptied fields are cleared, booleans sent as
    /// such, everything else as text the daemon reads by its type.
    pub fn request(&self) -> Request {
        let properties: Map<String, Value> = self
            .props
            .iter()
            .filter(|p| p.field.dirty())
            .map(|p| {
                let text = p.field.text.trim();
                let v = match (&p.control, text) {
                    (_, "") => Value::Null,
                    (Control::Boolean, t) => json!(t == "true"),
                    (_, t) => json!(t),
                };
                (p.field.name.clone(), v)
            })
            .collect();
        Request::PropsSet {
            part: self.part.clone(),
            properties,
            expected_revision: Some(self.revision.clone()),
        }
    }

    /// The form read again after a conflict: the new values, with what
    /// changed under what was typed listed.
    pub fn reloaded(&self, v: &Value) -> PartProps {
        let mut fresh = PartProps::from_get(v);
        fresh.changed = self
            .props
            .iter()
            .filter(|p| p.field.dirty())
            .filter_map(|mine| {
                let now = fresh.props.iter().find(|p| p.field.name == mine.field.name);
                let theirs = now.map_or(String::new(), |p| p.field.read.clone());
                (theirs != mine.field.read).then(|| Changed {
                    name: mine.field.name.clone(),
                    theirs: if theirs.is_empty() {
                        "(not set)".to_owned()
                    } else {
                        theirs
                    },
                    yours: mine.field.text.trim().to_owned(),
                })
            })
            .collect();
        fresh
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn got(number: &str, revision: &str) -> Value {
        json!({"part": "p", "revision": revision, "properties": [
            {"name": "number", "core": true, "value": number, "faults": [],
             "declaration": {"type": "string", "required": true, "revising": true}},
            {"name": "description", "core": true, "faults": [],
             "declaration": {"type": "text", "required": false, "revising": false}},
            {"name": "mass", "core": false, "value": "2 kg", "faults": [],
             "declaration": {"type": "quantity", "dim": "mass", "required": false,
                             "revising": false}},
            {"name": "finish", "core": false, "value": "red",
             "faults": [{"code": "property_invalid", "message": "finish: not one of anodized, bare"}],
             "declaration": {"type": "enum", "values": ["anodized", "bare"],
                             "required": false, "revising": true}},
            {"name": "coated", "core": false, "value": false, "faults": [],
             "declaration": {"type": "boolean", "required": false, "revising": false}},
            {"name": "count", "core": false, "value": 3, "faults": [],
             "declaration": {"type": "integer", "required": false, "revising": false}},
            {"name": "legacy", "core": false, "declaration": null, "value": "x",
             "faults": [{"code": "property_unknown", "message": "legacy is not declared"}]}
        ]})
    }

    #[test]
    fn every_property_gets_a_control_for_its_type() {
        let f = PartProps::from_get(&got("100", "r1"));
        let rows: Vec<(&str, &Control, &str, bool, bool)> = f
            .props
            .iter()
            .map(|p| {
                (
                    p.field.name.as_str(),
                    &p.control,
                    p.field.read.as_str(),
                    p.required,
                    p.revising,
                )
            })
            .collect();
        let anodized = Control::Enum(vec!["anodized".into(), "bare".into()]);
        assert_eq!(
            rows,
            [
                (
                    "number",
                    &Control::Text { multiline: false },
                    "100",
                    true,
                    true
                ),
                (
                    "description",
                    &Control::Text { multiline: true },
                    "",
                    false,
                    false
                ),
                ("mass", &Control::Quantity, "2 kg", false, false),
                ("finish", &anodized, "red", false, true),
                ("coated", &Control::Boolean, "false", false, false),
                ("count", &Control::Number, "3", false, false),
                ("legacy", &Control::Unknown, "x", false, false),
            ]
        );
        assert_eq!(f.props[3].faults, ["finish: not one of anodized, bare"]);
        assert!(f.props[0].core && !f.props[2].core);
        assert!(!f.dirty());
    }

    #[test]
    fn a_save_sets_what_changed_and_clears_what_was_emptied() {
        let mut f = PartProps::from_get(&got("100", "r1"));
        f.props[0].field.text = "101".into();
        f.props[3].field.text = "anodized".into();
        f.props[4].field.text = "true".into();
        f.props[6].field.text = String::new();
        let Request::PropsSet {
            part,
            properties,
            expected_revision,
        } = f.request()
        else {
            panic!()
        };
        assert_eq!(part, "p");
        assert_eq!(
            Value::Object(properties),
            json!({"number": "101", "finish": "anodized", "coated": true, "legacy": null})
        );
        assert_eq!(expected_revision.as_deref(), Some("r1"));
        // Someone renumbered it first: listed, not overwritten.
        let again = f.reloaded(&got("200", "r2"));
        assert_eq!(again.revision, "r2");
        assert_eq!(
            again.changed,
            [Changed {
                name: "number".into(),
                theirs: "200".into(),
                yours: "101".into()
            }]
        );
        assert_eq!(again.props[0].field.text, "200");
    }
}
