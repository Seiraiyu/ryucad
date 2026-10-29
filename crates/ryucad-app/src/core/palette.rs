//! The command palette's matching (design 199, Command registry >
//! Palette): fuzzy over a command's title, category and id, recently run
//! commands first; and, after `@`, over the model's parts, features,
//! solids and instances by id, type and PDM number or name. Pure: the
//! window draws it (`ui::palette`).

use crate::core::command::{self, COMMANDS};
use crate::core::model::Model;
use std::collections::BTreeMap;

/// How many recently run commands the palette remembers.
pub const RECENT: usize = 8;

/// How well `query` matches `text`: its characters in order, ignoring
/// case, `None` if they aren't all there. Higher is better: runs of
/// characters together, matches at the start of a word, and a match at
/// the very start score more; a long text scores a little less.
pub fn score(query: &str, text: &str) -> Option<i32> {
    let q: Vec<char> = query
        .chars()
        .flat_map(char::to_lowercase)
        .filter(|c| !c.is_whitespace())
        .collect();
    if q.is_empty() {
        return Some(0);
    }
    let t: Vec<char> = text.chars().collect();
    let mut s = 0;
    let mut qi = 0;
    let mut prev: Option<usize> = None;
    for (i, c) in t.iter().enumerate() {
        if qi == q.len() {
            break;
        }
        if c.to_lowercase().eq(std::iter::once(q[qi])) {
            let word_start = i == 0 || !t[i - 1].is_alphanumeric();
            s += 1;
            if word_start {
                s += 8;
            }
            if i == 0 {
                s += 4;
            }
            if prev.is_some_and(|p| p + 1 == i) {
                s += 5;
            }
            prev = Some(i);
            qi += 1;
        }
    }
    if qi < q.len() {
        return None;
    }
    // The query whole, as typed, counts most: at a word's start most.
    let lower: Vec<char> = t.iter().flat_map(|c| c.to_lowercase()).collect();
    if lower.len() == t.len() {
        let at = (0..=lower.len().saturating_sub(q.len())).find(|i| lower[*i..].starts_with(&q));
        if let Some(i) = at {
            s += 20;
            if i == 0 || !t[i - 1].is_alphanumeric() {
                s += 20;
            }
        }
    }
    Some(s * 16 - t.len() as i32)
}

/// A command's best score over its title, category and id.
fn command_score(query: &str, c: &command::Command) -> Option<i32> {
    [
        score(query, c.title).map(|s| s + 2),
        score(query, c.category.title()),
        score(query, c.id),
    ]
    .into_iter()
    .flatten()
    .max()
}

/// The commands matching `query`, best first; recently run commands
/// (`recent`, newest first) before the rest, and with an empty query in
/// that order then the registry's. Disabled commands stay in: the window
/// shows them greyed with their reason.
pub fn commands(query: &str, recent: &[&'static str]) -> Vec<&'static command::Command> {
    let mut found: Vec<(bool, usize, i32, usize, &'static command::Command)> = COMMANDS
        .iter()
        .enumerate()
        .filter(|(_, c)| c.kind == command::Kind::Action || c.kind == command::Kind::Toggle)
        .filter_map(|(i, c)| {
            let s = command_score(query, c)?;
            let r = recent.iter().position(|id| *id == c.id);
            Some((r.is_none(), r.unwrap_or(0), -s, i, c))
        })
        .collect();
    found.sort_by_key(|(not_recent, r, s, i, _)| (*not_recent, *r, *s, *i));
    found.into_iter().map(|x| x.4).collect()
}

/// Notes a command as just run.
pub fn ran(recent: &mut Vec<&'static str>, id: &'static str) {
    recent.retain(|r| *r != id);
    recent.insert(0, id);
    recent.truncate(RECENT);
}

/// Something in the model the palette can go to after `@`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Place {
    /// What it shows: `bracket/e1 (extrude)`.
    pub label: String,
    /// The part, and the feature (`None` for a part itself).
    pub part: String,
    pub feature: Option<String>,
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

/// The model's parts, features, solids and instances matching `query`
/// (the text after `@`), best first: by id, type, and a part's PDM number
/// and name (`pdm`, from [`pdm_names`]).
pub fn places(query: &str, model: &Model, pdm: &BTreeMap<String, String>) -> Vec<Place> {
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

    #[test]
    fn matching_is_in_order_and_ignores_case() {
        assert!(score("zf", "Zoom to fit").is_some());
        assert!(score("fz", "Zoom to fit").is_none());
        assert!(score("ZOOM", "zoom to fit").is_some());
        assert_eq!(score("", "anything"), Some(0));
        // Word starts and runs beat scattered letters.
        assert!(score("zoom", "Zoom to fit") > score("zoom", "Zap of a room"));
        assert!(score("fit", "Zoom to fit") > score("fit", "Find it"));
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
            places(q, &m, &pdm)
                .into_iter()
                .map(|p| p.label)
                .collect::<Vec<_>>()
        };
        assert_eq!(labels("fillet"), ["bracket/f1 (fillet)"]);
        assert_eq!(labels("brk")[0], "bracket (part, BRK-100 Bracket)");
        assert!(labels("assembly").contains(&"frame (assembly)".to_owned()));
        assert_eq!(labels("pin")[0], "frame/pin (instance of bracket)");
        let first = &places("e1", &m, &pdm)[0];
        assert_eq!(
            (first.part.as_str(), first.feature.as_deref()),
            ("bracket", Some("e1"))
        );
    }
}
