//! Who else is attached, and what each of them selects, highlights and
//! says they are doing: the session as the window knows it, and the
//! People strip that shows it. What else a participant's window shows is
//! the product's [`View`]. Moved from ryucad-app by ryueda 3.C.

use crate::style;
use ryu_session::live::{Kind, Style};
use serde_json::Value;
use std::collections::BTreeMap;

/// What else a participant's window shows, read from their JSON.
pub trait View: Clone + PartialEq + std::fmt::Debug {
    fn from_json(v: &Value) -> Self;
    /// Whether this window can follow them (ryucad: they have a camera).
    fn followable(&self) -> bool;
}

/// A group of highlighted entities.
#[derive(Clone, Debug, PartialEq)]
pub struct Highlight {
    pub style: Style,
    pub label: Option<String>,
    pub entities: Vec<String>,
}

/// One participant; their window's view reads through (`p.camera`).
#[derive(Clone, Debug, PartialEq)]
pub struct Person<V> {
    pub id: String,
    pub kind: Kind,
    pub name: String,
    /// What they say they are doing, and the entities it is about.
    pub status: Option<(String, Vec<String>)>,
    pub selection: Vec<String>,
    pub highlights: Vec<Highlight>,
    pub view: V,
}

impl<V> std::ops::Deref for Person<V> {
    type Target = V;
    fn deref(&self) -> &V {
        &self.view
    }
}

impl<V> std::ops::DerefMut for Person<V> {
    fn deref_mut(&mut self) -> &mut V {
        &mut self.view
    }
}

/// A JSON list of strings, others skipped.
pub fn strings(v: &Value) -> Vec<String> {
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| s.as_str().map(str::to_owned))
        .collect()
}

impl<V: View> Person<V> {
    /// A participant as `participant.list` and `participant.changed` give
    /// it; `None` if it isn't one.
    pub fn from_json(v: &Value) -> Option<Self> {
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
            highlights,
            view: V::from_json(v),
        })
    }
}

/// Everyone attached, by ID.
#[derive(Debug)]
pub struct People<V> {
    pub all: BTreeMap<String, Person<V>>,
    /// Bumped whenever anyone changes, so views know to redraw.
    pub generation: u64,
}

impl<V> Default for People<V> {
    fn default() -> Self {
        People {
            all: BTreeMap::new(),
            generation: 0,
        }
    }
}

impl<V: View> People<V> {
    /// Replaces everyone from a `participants` list.
    pub fn reset(&mut self, list: &Value) {
        self.all = list
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Person::<V>::from_json)
            .map(|p| (p.id.clone(), p))
            .collect();
        self.generation += 1;
    }

    /// Takes a `participant.changed` notification.
    pub fn changed(&mut self, params: &Value) {
        let id = params["participant"]["id"].as_str().unwrap_or_default();
        if params["left"] == true {
            self.all.remove(id);
        } else if let Some(p) = Person::<V>::from_json(&params["participant"]) {
            self.all.insert(p.id.clone(), p);
        }
        self.generation += 1;
    }

    pub fn get(&self, id: &str) -> Option<&Person<V>> {
        self.all.get(id)
    }

    /// Everyone but `me`.
    pub fn others<'a>(&'a self, me: &'a str) -> impl Iterator<Item = &'a Person<V>> {
        self.all.values().filter(move |p| p.id != me)
    }
}

/// One participant's place in the People strip.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub id: String,
    pub name: String,
    pub agent: bool,
    pub you: bool,
    /// Their mode, said after their name (ryucad: `annotating`).
    pub mark: Option<String>,
    /// What they say they are doing.
    pub status: Option<String>,
    /// A long request of theirs: `check 3/8 parts`.
    pub busy: Option<String>,
    /// Whether Follow is offered.
    pub followable: bool,
}

/// Draws the People strip; the new `following` when Follow was pressed.
pub fn strip(ui: &mut egui::Ui, lines: &[Line], following: Option<&str>) -> Option<Option<String>> {
    // One row whatever the width: where the names don't fit they are cut
    // short (their whole text on hover), never wrapped. Wrapped, at 200 %
    // in a window 1000 points wide the strip broke into a few letters a
    // line and took the whole window's height from the tiles (#508).
    let mut follow = None;
    ui.horizontal(|ui| {
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
        for p in lines {
            let colour = if p.you {
                style::SELECTED
            } else {
                style::participant(&p.id)
            };
            let kind = if p.agent { " [agent]" } else { "" };
            let you = if p.you { " (you)" } else { "" };
            // A mode goes in its person's own label: the strip runs right
            // to left, so a label of its own read before the name.
            let mode = p
                .mark
                .as_ref()
                .map(|m| format!(" · {m}"))
                .unwrap_or_default();
            ui.label(
                egui::RichText::new(format!("{}{kind}{you}{mode}", p.name))
                    .color(style::color32(colour)),
            );
            if let Some(text) = &p.status {
                ui.weak(text);
            }
            if let Some(b) = &p.busy {
                ui.weak(format!("· {b}"));
            }
            if !p.you && p.followable {
                let on = following == Some(p.id.as_str());
                if ui.selectable_label(on, "Follow").clicked() {
                    follow = Some(if on { None } else { Some(p.id.clone()) });
                }
            }
            ui.separator();
        }
    });
    follow
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[derive(Clone, Debug, Default, PartialEq)]
    struct Sheet {
        sheet: Option<String>,
    }

    impl View for Sheet {
        fn from_json(v: &Value) -> Self {
            Sheet {
                sheet: v["view"]["sheet"].as_str().map(str::to_owned),
            }
        }
        fn followable(&self) -> bool {
            self.sheet.is_some()
        }
    }

    #[test]
    fn participants_come_and_go_with_their_product_s_view() {
        let mut p = People::<Sheet>::default();
        p.reset(&json!([
            {"id": "p-1", "kind": "human", "name": "me", "selection": ["power/U1"], "highlights": []},
            {"id": "p-2", "kind": "agent", "name": "agent", "selection": [],
             "status": {"text": "placing", "entities": ["power/U2"]},
             "highlights": [{"style": "changed", "entities": ["power/U2"]}],
             "view": {"sheet": "power/1"}},
        ]));
        let agent = p.get("p-2").unwrap();
        assert_eq!(
            agent.status,
            Some(("placing".into(), vec!["power/U2".into()]))
        );
        assert_eq!(agent.highlights[0].style, Style::Changed);
        assert_eq!(
            agent.sheet.as_deref(),
            Some("power/1"),
            "the view reads through"
        );
        assert!(agent.followable() && !p.get("p-1").unwrap().followable());
        assert_eq!(p.others("p-1").count(), 1);
        let g = p.generation;
        p.changed(&json!({"participant": {"id": "p-2"}, "left": true}));
        assert!(p.get("p-2").is_none() && p.generation > g);
    }
}
