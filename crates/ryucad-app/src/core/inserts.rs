//! Inserts and deletes in the window (design 521, The app): what a chosen
//! insert paints, a chosen delete's ghosts, an insert update's preview,
//! and a part's official labels. No egui.

use crate::core::model::{BodyKey, Part};

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
}
