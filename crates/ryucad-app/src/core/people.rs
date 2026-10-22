//! Who else is attached, and what each of them selects, looks at,
//! highlights and says they are doing: the session as the window knows
//! it.

use ryucad::live::{Kind, Style};
use ryucad::view::Camera;
use serde_json::Value;
use std::collections::BTreeMap;

/// A group of highlighted entities.
#[derive(Clone, Debug, PartialEq)]
pub struct Highlight {
    pub style: Style,
    pub label: Option<String>,
    pub entities: Vec<String>,
}

/// One participant.
#[derive(Clone, Debug, PartialEq)]
pub struct Person {
    pub id: String,
    pub kind: Kind,
    pub name: String,
    /// What they say they are doing, and the entities it is about.
    pub status: Option<(String, Vec<String>)>,
    pub selection: Vec<String>,
    pub camera: Option<Camera>,
    pub highlights: Vec<Highlight>,
    /// The Git revision they compare the working files against.
    pub compare: Option<String>,
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| s.as_str().map(str::to_owned))
        .collect()
}

impl Person {
    /// A participant as `participant.list` and `participant.changed` give
    /// it; `None` if it isn't one.
    pub fn from_json(v: &Value) -> Option<Person> {
        let status = v["status"].is_object().then(|| {
            (
                v["status"]["text"].as_str().unwrap_or_default().to_owned(),
                strings(&v["status"]["entities"]),
            )
        });
        let highlights = v["highlights"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|h| {
                Some(Highlight {
                    style: serde_json::from_value(h["style"].clone()).ok()?,
                    label: h["label"].as_str().map(str::to_owned),
                    entities: strings(&h["entities"]),
                })
            })
            .collect();
        Some(Person {
            id: v["id"].as_str()?.to_owned(),
            kind: serde_json::from_value(v["kind"].clone()).ok()?,
            name: v["name"].as_str()?.to_owned(),
            status,
            selection: strings(&v["selection"]),
            camera: serde_json::from_value(v["camera"].clone()).ok(),
            highlights,
            compare: v["compare"]["rev"].as_str().map(str::to_owned),
        })
    }
}

/// Everyone attached, by ID.
#[derive(Debug, Default)]
pub struct People {
    pub all: BTreeMap<String, Person>,
    /// Bumped whenever anyone changes, so views know to redraw.
    pub generation: u64,
}

impl People {
    /// Replaces everyone from a `participants` list.
    pub fn reset(&mut self, list: &Value) {
        self.all = list
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Person::from_json)
            .map(|p| (p.id.clone(), p))
            .collect();
        self.generation += 1;
    }

    /// Takes a `participant.changed` notification.
    pub fn changed(&mut self, params: &Value) {
        let id = params["participant"]["id"].as_str().unwrap_or_default();
        if params["left"] == true {
            self.all.remove(id);
        } else if let Some(p) = Person::from_json(&params["participant"]) {
            self.all.insert(p.id.clone(), p);
        }
        self.generation += 1;
    }

    pub fn get(&self, id: &str) -> Option<&Person> {
        self.all.get(id)
    }

    /// Everyone but `me`.
    pub fn others<'a>(&'a self, me: &'a str) -> impl Iterator<Item = &'a Person> {
        self.all.values().filter(move |p| p.id != me)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn notifications_carry_whole_participants() {
        let mut people = People::default();
        people.reset(&json!([
            {"id": "p-00000001", "kind": "human", "name": "Dana", "status": null,
             "selection": [], "camera": null, "highlights": []},
        ]));
        people.changed(&json!({"participant": {
            "id": "p-00000002", "kind": "agent", "name": "agent",
            "status": {"text": "drilling", "entities": ["p/a/0123"]},
            "selection": ["p/a/0123"],
            "camera": {"eye": [1.0, 0.0, 0.0], "target": [0.0, 0.0, 0.0], "up": [0.0, 0.0, 1.0],
                       "projection": {"perspective": {"fov_deg": 30.0}}},
            "highlights": [{"style": "pending", "label": "here", "entities": ["p/a/0123"]}],
        }}));
        let agent = people.get("p-00000002").unwrap();
        assert_eq!(
            agent.status,
            Some(("drilling".into(), vec!["p/a/0123".into()]))
        );
        assert_eq!(agent.highlights[0].style, Style::Pending);
        assert_eq!(agent.camera.as_ref().unwrap().eye, [1.0, 0.0, 0.0]);
        assert_eq!(people.others("p-00000001").count(), 1);
        people.changed(&json!({"participant": {"id": "p-00000002"}, "left": true}));
        assert!(people.get("p-00000002").is_none());
        assert_eq!(people.generation, 3);
    }
}
