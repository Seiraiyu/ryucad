//! The command palette's matching (design 199, Command registry >
//! Palette): fuzzy over a command's title, category and id, recently run
//! commands first; and, after `@`, over the model's parts, features,
//! solids and instances by id, type and PDM number or name. Pure: the
//! window draws it (`ui::palette`).

use crate::core::command::{self, COMMANDS};
use crate::core::model::Model;
use std::collections::BTreeMap;

/// What the palette's query starts with to list the other branches
/// (File > Switch branch…): Enter opens the Check out dialog.
pub const BRANCHES: &str = "branch:";

pub use ryu_app::palette::{RECENT, ran, score};

/// ryucad's commands matching `query` ([`ryu_app::palette::commands`]
/// over [`COMMANDS`]).
pub fn commands(query: &str, recent: &[&'static str]) -> Vec<&'static command::Command> {
    ryu_app::palette::commands(COMMANDS, query, recent)
}

/// Something in the model the palette can go to after `@`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Place {
    /// What it shows: `bracket/e1 (extrude)`.
    pub label: String,
    /// The part, and the feature (`None` for a part itself).
    pub part: String,
    pub feature: Option<String>,
    /// A drawing of `part`, opened in the Drawings tile.
    pub drawing: Option<String>,
}

/// Each part's PDM number and name, as far as the window knows them: the
/// BOM tab's lines, once it has read one.
pub fn pdm_names(bom: &crate::core::bom::BomTab) -> BTreeMap<String, String> {
    let col = |name: &str| bom.columns.iter().position(|c| c == name);
    let (number, name) = (col("number"), col("name"));
    let mut out = BTreeMap::new();
    for l in bom.tree.iter().chain(&bom.flat_lines) {
        let cell = |i: Option<usize>| i.and_then(|i| l.cells.get(i)).map_or("", String::as_str);
        let text = format!("{} {}", cell(number), cell(name));
        if !text.trim().is_empty() {
            out.insert(l.part.clone(), text.trim().to_owned());
        }
    }
    out
}

/// The model's parts, features, solids, instances and drawings
/// (`drawings`, each part's `(id, name, failed)`) matching `query` (the
/// text after `@`), best first: by id, type, and a part's PDM number and
/// name (`pdm`, from [`pdm_names`]).
pub fn places(
    query: &str,
    model: &Model,
    pdm: &BTreeMap<String, String>,
    drawings: &BTreeMap<String, Vec<(String, String, bool)>>,
) -> Vec<Place> {
    let mut found: Vec<(i32, Place)> = Vec::new();
    for p in model.parts.values() {
        let kind = if p.assembly { "assembly" } else { "part" };
        let named = pdm.get(&p.id).map_or("", String::as_str);
        if let Some(s) = score(query, &format!("{} {kind} {named}", p.id)) {
            let label = match named {
                "" => format!("{} ({kind})", p.id),
                n => format!("{} ({kind}, {n})", p.id),
            };
            found.push((
                s + 4,
                Place {
                    label,
                    part: p.id.clone(),
                    feature: None,
                    drawing: None,
                },
            ));
        }
        for f in &p.features {
            let ty = f.ty.as_deref().unwrap_or("feature");
            let what = match &f.instance {
                Some(i) => format!("{ty} of {}", i.part),
                None => ty.to_owned(),
            };
            if let Some(s) = score(query, &format!("{}/{} {what}", p.id, f.id)) {
                found.push((
                    s,
                    Place {
                        label: format!("{}/{} ({what})", p.id, f.id),
                        part: p.id.clone(),
                        feature: Some(f.id.clone()),
                        drawing: None,
                    },
                ));
            }
        }
    }
    // Every part's drawings, by ID and name (#748).
    for (part, list) in drawings {
        for (id, name, _) in list {
            if let Some(s) = score(query, &format!("{part}/{id} drawing {name}")) {
                let label = if name.is_empty() {
                    format!("{part}/{id} (drawing)")
                } else {
                    format!("{part}/{id} (drawing, {name})")
                };
                found.push((
                    s,
                    Place {
                        label,
                        part: part.clone(),
                        feature: None,
                        drawing: Some(id.clone()),
                    },
                ));
            }
        }
    }
    found.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.label.cmp(&b.1.label)));
    found.into_iter().map(|(_, p)| p).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Issue #519: Zoom to fit was listed twice, `view.fit` and the tree
    /// rows' hidden `view.fit_row` of the same title.
    #[test]
    fn at_finds_drawings_by_id_and_name() {
        let m = Model::default();
        let drawn = BTreeMap::from([(
            "dw-plate".to_owned(),
            vec![
                ("d".to_owned(), "Plate".to_owned(), false),
                ("big".to_owned(), String::new(), false),
            ],
        )]);
        let found = places("dw-plate/d", &m, &BTreeMap::new(), &drawn);
        assert_eq!(found[0].label, "dw-plate/d (drawing, Plate)");
        assert_eq!(
            (found[0].part.as_str(), found[0].drawing.as_deref()),
            ("dw-plate", Some("d"))
        );
        assert!(
            places("plate", &m, &BTreeMap::new(), &drawn)
                .iter()
                .any(|p| p.drawing.as_deref() == Some("d"))
        );
    }

    #[test]
    fn a_rows_copy_of_a_listed_command_is_listed_once() {
        let found: Vec<&str> = commands("fit", &[])
            .iter()
            .filter(|c| c.title == "Zoom to fit")
            .map(|c| c.id)
            .collect();
        assert_eq!(found, ["view.fit"]);
    }

    #[test]
    fn titles_categories_and_ids_all_match() {
        let ids = |q: &str| commands(q, &[]).iter().map(|c| c.id).collect::<Vec<_>>();
        assert_eq!(ids("zoom to fit")[0], "view.fit");
        assert!(ids("edit.redo").contains(&"edit.redo"));
        // By category: every Tools command.
        assert!(ids("tools").contains(&"measure.tool"));
        // Submenus aren't commands to run from here.
        assert!(!ids("open recent").contains(&"file.open_recent"));
    }

    #[test]
    fn recently_run_commands_come_first() {
        let mut recent = Vec::new();
        ran(&mut recent, "view.fit");
        ran(&mut recent, "edit.undo");
        ran(&mut recent, "view.fit");
        assert_eq!(recent, ["view.fit", "edit.undo"]);
        let all = commands("", &recent);
        assert_eq!(all[0].id, "view.fit");
        assert_eq!(all[1].id, "edit.undo");
        assert_eq!(all.len(), commands("", &[]).len());
        for c in &COMMANDS[..20] {
            ran(&mut recent, c.id);
        }
        assert_eq!(recent.len(), RECENT);
    }

    #[test]
    fn at_finds_parts_features_and_instances() {
        use serde_json::json;
        let mut m = Model::default();
        m.apply_part(&json!({"part": "bracket", "revision": "r", "features": [
            {"id": "e1", "type": "extrude", "ok": true, "hash": "1"},
            {"id": "f1", "type": "fillet", "ok": true, "hash": "2"}],
            "solids": [{"solid": "e1", "features": ["e1", "f1"], "hash": "2"}]}));
        m.apply_part(
            &json!({"part": "frame", "revision": "r", "kind": "assembly",
            "features": [{"id": "pin", "type": "instance", "ok": true,
                "instance": {"part": "bracket", "rev": "working"}}]}),
        );
        let pdm = BTreeMap::from([("bracket".to_owned(), "BRK-100 Bracket".to_owned())]);
        let labels = |q: &str| {
            places(q, &m, &pdm, &BTreeMap::new())
                .into_iter()
                .map(|p| p.label)
                .collect::<Vec<_>>()
        };
        assert_eq!(labels("fillet"), ["bracket/f1 (fillet)"]);
        assert_eq!(labels("brk")[0], "bracket (part, BRK-100 Bracket)");
        assert!(labels("assembly").contains(&"frame (assembly)".to_owned()));
        assert_eq!(labels("pin")[0], "frame/pin (instance of bracket)");
        let first = &places("e1", &m, &pdm, &BTreeMap::new())[0];
        assert_eq!(
            (first.part.as_str(), first.feature.as_deref()),
            ("bracket", Some("e1"))
        );
    }
}
