//! Inserts and deletes in the window (design 521, The app): what a chosen
//! insert paints, a chosen delete's ghosts, an insert update's preview,
//! and a part's official labels. No egui.

use crate::core::model::{BodyKey, Part};
use ryucad::mesh::WireBody;
use std::collections::BTreeMap;
use std::sync::Arc;

/// The bodies a chosen insert paints whole: every solid it founds that
/// the model shows. None for any other feature.
pub fn painted(part: &Part, chosen: &BodyKey) -> Vec<BodyKey> {
    if chosen.0 != part.id
        || part
            .feature(&chosen.1)
            .is_none_or(|f| f.insert.is_none())
    {
        return Vec::new();
    }
    part.inserted(&chosen.1)
        .into_iter()
        .map(|s| (part.id.clone(), s.id.clone()))
        .collect()
}

/// What a chosen delete_body ghosts: each solid it takes out, as the
/// delete read it (`mesh.get` with `before`), where it was.
#[derive(Clone, Debug, Default)]
pub struct Deleted {
    /// The delete: its part and ID.
    pub feature: BodyKey,
    /// Each solid's mesh as it came, by its key.
    pub ghosts: BTreeMap<BodyKey, Arc<WireBody>>,
    /// Bumped as each comes, for the scene to upload them again.
    pub generation: u64,
}

impl Deleted {
    /// For delete `feature` of `part`, with the solids to ask for; `None`
    /// for a feature that isn't a delete.
    pub fn of(part: &Part, feature: &str) -> Option<(Deleted, Vec<BodyKey>)> {
        let d = part.feature(feature)?.delete.as_ref()?;
        let asked = d
            .solids
            .iter()
            .map(|s| (part.id.clone(), s.clone()))
            .collect();
        let me = Deleted {
            feature: (part.id.clone(), feature.to_owned()),
            ..Deleted::default()
        };
        Some((me, asked))
    }

    /// Keeps a ghost that came for this delete; false for another's.
    pub fn take(&mut self, feature: &BodyKey, body: WireBody) -> bool {
        if *feature != self.feature {
            return false;
        }
        let key = (self.feature.0.clone(), body.feature.clone());
        self.ghosts.insert(key, Arc::new(body));
        self.generation += 1;
        true
    }
}

/// A part's official release labels from `release.list` (its records)
/// or `deps.parts` (a dependency's part's labels), in the order given.
/// With `part` empty, `deps.parts`' every listed part's.
pub fn official_labels(v: &serde_json::Value, part: &str) -> Vec<String> {
    let label = |x: &serde_json::Value| x["label"].as_str().map(str::to_owned);
    if let Some(records) = v["records"].as_array() {
        return records
            .iter()
            .filter(|r| r["part"] == part && r["status"] == "official")
            .filter_map(label)
            .collect();
    }
    v["parts"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|p| part.is_empty() || p["part"] == part || p.get("part").is_none())
        .flat_map(|p| {
            p["labels"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(label)
        })
        .collect()
}

/// What moving an insert's pin changes, from `insert.update`'s dry run.
#[derive(Clone, Debug, PartialEq)]
pub struct Moved {
    pub added: usize,
    pub removed: usize,
    pub changed: usize,
    /// Each feature that would fail after it, with its code.
    pub failing: Vec<(String, String)>,
}

impl Moved {
    pub fn from_answer(v: &serde_json::Value) -> Moved {
        let n = |k: &str| v["solids"][k].as_array().map_or(0, Vec::len);
        Moved {
            added: n("added"),
            removed: n("removed"),
            changed: n("changed"),
            failing: v["failing"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|f| {
                    (
                        f["feature"].as_str().unwrap_or_default().to_owned(),
                        f["error"]["code"].as_str().unwrap_or("failed").to_owned(),
                    )
                })
                .collect(),
        }
    }

    /// Two lines: what moving the pin to `to` changes of the solids, and
    /// what would fail after it.
    pub fn lines(&self, to: &str) -> Vec<String> {
        let said: Vec<String> = [
            (self.changed, "changed"),
            (self.added, "added"),
            (self.removed, "removed"),
        ]
        .into_iter()
        .filter(|(n, _)| *n > 0)
        .map(|(n, w)| format!("{n} solid{} {w}", if n == 1 { "" } else { "s" }))
        .collect();
        let solids = if said.is_empty() {
            "no solid changes".to_owned()
        } else {
            said.join(", ")
        };
        let fails = if self.failing.is_empty() {
            "Nothing fails after it".to_owned()
        } else {
            let list: Vec<String> = self
                .failing
                .iter()
                .map(|(f, c)| format!("{f} ({c})"))
                .collect();
            format!("Fails after it: {}", list.join(", "))
        };
        vec![format!("Moving the pin to {to}: {solids}"), fails]
    }
}

/// An insert update's dry run, for a form's chosen pin: its part, insert
/// and `to` (`working`, `label:B`), and its answer once it came.
#[derive(Clone, Debug)]
pub struct Preview {
    pub key: (String, String, String),
    pub answer: Option<Result<Moved, ryucad::error::Error>>,
}

impl Preview {
    pub fn asked(key: (String, String, String)) -> Preview {
        Preview { key, answer: None }
    }

    /// The pin it moves to, for people: `working`, `label B`.
    pub fn to_text(&self) -> String {
        match self.key.2.strip_prefix("label:") {
            Some(l) => format!("label {l}"),
            None => self.key.2.clone(),
        }
    }

    /// What the form shows of it.
    pub fn lines(&self) -> Vec<String> {
        let to = self.to_text();
        match &self.answer {
            None => vec![format!("Reading what moving the pin to {to} changes\u{2026}")],
            Some(Err(e)) => vec![format!("Moving the pin to {to} is refused: {}", e.message)],
            Some(Ok(m)) => m.lines(&to),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model::Model;
    use serde_json::json;

    pub(crate) fn inserted() -> Model {
        let mut m = Model::default();
        m.apply_part(&json!({"part": "e", "revision": "r", "features": [
            {"id": "brd", "type": "insert", "ok": true, "hash": "1",
             "solids": ["brd"], "insert": {"of": "pcb", "rev": "working", "count": 2}},
            {"id": "lid", "type": "box", "ok": true, "hash": "2", "solids": ["lid"]},
            {"id": "d", "type": "delete_body", "ok": true, "hash": "3",
             "delete_body": {"solids": ["brd/boss"]}}],
          "solids": [
            {"solid": "brd/base", "ok": true, "features": ["brd"], "hash": "4"},
            {"solid": "lid", "ok": true, "features": ["lid"], "hash": "2"}]}));
        m
    }

    fn key(f: &str) -> BodyKey {
        ("e".to_owned(), f.to_owned())
    }

    #[test]
    fn a_chosen_insert_paints_its_solids_and_nothing_else_does() {
        let m = inserted();
        let p = &m.parts["e"];
        assert_eq!(painted(p, &key("brd")), [key("brd/base")]);
        assert!(painted(p, &key("lid")).is_empty());
        assert!(painted(p, &("other".to_owned(), "brd".to_owned())).is_empty());
    }

    #[test]
    fn a_chosen_delete_asks_for_each_solid_and_keeps_only_its_ghosts() {
        let m = inserted();
        let p = &m.parts["e"];
        assert!(Deleted::of(p, "lid").is_none());
        let (mut d, asked) = Deleted::of(p, "d").unwrap();
        assert_eq!(asked, [key("brd/boss")]);
        let body = |f: &str| ryucad::mesh::WireBody {
            feature: f.to_owned(),
            hash: "h".to_owned(),
            mesh: Err(json!(null)),
        };
        assert!(d.take(&key("d"), body("brd/boss")));
        assert!(!d.take(&key("other"), body("brd/boss")));
        assert_eq!(
            d.ghosts.keys().cloned().collect::<Vec<_>>(),
            [key("brd/boss")]
        );
        assert_eq!(d.generation, 1);
    }

    #[test]
    fn official_labels_come_from_either_list() {
        let released = json!({"records": [
            {"part": "pcb", "label": "A", "status": "official"},
            {"part": "pcb", "label": "B", "status": "official"},
            {"part": "pcb", "label": "C", "status": "proposed"},
            {"part": "lid", "label": "A", "status": "official"}]});
        assert_eq!(official_labels(&released, "pcb"), ["A", "B"]);
        let deps = json!({"parts": [{"part": "pcb", "labels": [{"label": "B"}, {"label": "A"}]}]});
        assert_eq!(official_labels(&deps, "pcb"), ["B", "A"]);
    }

    #[test]
    fn an_updates_preview_says_what_changes_and_what_fails() {
        let v = json!({"from": {"label": "A"}, "to": {"label": "B"},
                       "solids": {"added": [], "removed": [], "changed": ["brd/base"]},
                       "failing": [{"feature": "cut", "error": {"code": "reference_unresolved"}}],
                       "written": false});
        let m = Moved::from_answer(&v);
        assert_eq!((m.changed, m.added, m.removed), (1, 0, 0));
        assert_eq!(
            m.lines("label B"),
            [
                "Moving the pin to label B: 1 solid changed",
                "Fails after it: cut (reference_unresolved)"
            ]
        );
        let none = Moved::from_answer(
            &json!({"solids": {"added": [], "removed": [], "changed": []}, "failing": []}),
        );
        assert_eq!(
            none.lines("working"),
            [
                "Moving the pin to working: no solid changes",
                "Nothing fails after it"
            ]
        );
        let mut p = Preview::asked(("e".into(), "brd".into(), "label:B".into()));
        assert_eq!(
            p.lines(),
            ["Reading what moving the pin to label B changes\u{2026}"]
        );
        p.answer = Some(Err(ryucad::error::Error::new(
            ryucad::error::Code::LabelNotFound,
            "pcb has no label Z",
        )));
        assert_eq!(
            p.lines(),
            ["Moving the pin to label B is refused: pcb has no label Z"]
        );
    }
}
