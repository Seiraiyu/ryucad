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
}
