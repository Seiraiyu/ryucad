//! What an error card says (design 199): a short title for each error
//! code, the message, the entities its details name as chips, `removed_by`
//! and `was` in words, and the error's JSON for Copy details. Pure:
//! `ui::feedback` draws it.

use ryucad::error::{Code, Error};
use serde_json::Value;

/// Words written their own way in titles.
const WORDS: &[(&str, &str)] = &[
    ("json", "JSON"),
    ("id", "ID"),
    ("ids", "IDs"),
    ("bom", "BOM"),
    ("pdm", "PDM"),
    ("odb", "ODB++"),
    ("kicad", "KiCad"),
    ("eda", "EDA"),
    ("step", "STEP"),
    ("git", "Git"),
    ("github", "GitHub"),
    ("erc", "ERC"),
    ("nc", "No-connect"),
    ("refdes", "Reference designator"),
    ("demorgan", "De Morgan"),
    ("io", "I/O"),
];

/// Titles that read better than their codes' words.
const TITLES: &[(Code, &str)] = &[
    (Code::Io, "Couldn't read or write a file"),
    (Code::Usage, "Bad arguments"),
    (Code::Daemon, "The daemon didn't answer"),
    (Code::Internal, "Internal error"),
    (Code::NotFound, "Not found"),
    (Code::Conflict, "Changed since you read it"),
    (Code::NotAGitRepo, "Not a Git repository"),
    (Code::NothingToUndo, "Nothing to undo"),
    (Code::UndoConflict, "Can't undo: the files changed since"),
    (Code::Cancelled, "Cancelled"),
    (Code::CheckFailed, "Check failed"),
    (Code::RebuildFailed, "Rebuild failed"),
    (Code::BlendFailed, "Fillet or chamfer failed"),
    (Code::BooleanFailed, "Boolean failed"),
    (Code::ReferenceUnresolved, "A reference no longer resolves"),
    (Code::ReferenceForward, "A reference to a later feature"),
    (Code::FormatTooNew, "Written by a newer ryucad"),
    (Code::AppNotInstalled, "The app isn't installed"),
];

/// A short title for `code`: its own from [`TITLES`], else its words
/// (`unit_mismatch` is `Unit mismatch`), names written as they are
/// (`invalid_json` is `Invalid JSON`).
pub fn title(code: Code) -> String {
    if let Some((_, t)) = TITLES.iter().find(|(c, _)| *c == code) {
        return (*t).to_owned();
    }
    let name = serde_json::to_value(code)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default();
    let words: Vec<String> = name
        .split('_')
        .map(|w| match WORDS.iter().find(|(from, _)| *from == w) {
            Some((_, to)) => (*to).to_owned(),
            None => w.to_owned(),
        })
        .collect();
    let mut s = words.join(" ");
    if let Some(first) = s.get(..1) {
        s = first.to_uppercase() + &s[1..];
    }
    s
}

/// An entity an error names, with Show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chip {
    /// `<part>/<solid>/<name>`, `<part>/<feature>`, a STEP instance.
    pub reference: String,
    /// What it is to the error: `edge, listed`, `face it overflows onto`.
    pub what: String,
    /// Whether Show can go to it (a STEP instance can't).
    pub shows: bool,
}

/// An error as its card shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct Card {
    pub title: String,
    pub code: String,
    pub message: String,
    /// `reason radius_too_large`, `feature bracket/f1`.
    pub facts: Vec<String>,
    pub chips: Vec<Chip>,
    /// `removed_by` and `was` in words.
    pub history: Option<String>,
    /// The error's JSON, for Copy details.
    pub json: String,
}

/// The details keys whose values name entities.
const NAMING: [&str; 6] = [
    "entities",
    "references",
    "faces",
    "step",
    "failing",
    "listed",
];
/// The details keys shown as plain facts.
const FACTS: [&str; 5] = ["reason", "part", "feature", "solid", "rev"];

impl Card {
    pub fn of(e: &Error) -> Card {
        let d = &e.details;
        let facts = FACTS
            .iter()
            .filter_map(|k| Some(format!("{k} {}", d[*k].as_str()?)))
            .collect();
        let mut chips = Vec::new();
        for k in NAMING {
            for v in d[k].as_array().into_iter().flatten() {
                chips.extend(chip(k, v));
            }
        }
        Card {
            title: title(e.code),
            code: serde_json::to_value(e.code)
                .ok()
                .and_then(|v| v.as_str().map(str::to_owned))
                .unwrap_or_default(),
            message: e.message.clone(),
            facts,
            chips,
            history: history(d),
            json: serde_json::to_string_pretty(e).unwrap_or_default(),
        }
    }
}

/// A chip for one value of `key`: a string, or an object with its
/// `reference` (else `part`/`feature`), described by its kind and role.
fn chip(key: &str, v: &Value) -> Option<Chip> {
    if let Some(s) = v.as_str() {
        return Some(Chip {
            reference: s.to_owned(),
            what: String::new(),
            shows: key != "step",
        });
    }
    let reference = match (
        v["reference"].as_str(),
        v["part"].as_str(),
        v["feature"].as_str(),
    ) {
        (Some(r), ..) => r.to_owned(),
        (None, Some(p), Some(f)) => format!("{p}/{f}"),
        _ => return None,
    };
    let what: Vec<&str> = ["kind", "role", "code"]
        .iter()
        .filter_map(|k| v[*k].as_str())
        .collect();
    let mut what = what.join(", ");
    if v["listed"].is_string() {
        what.push_str(", listed");
    }
    Some(Chip {
        reference,
        what,
        shows: true,
    })
}

/// `removed_by` and `was`: `It was an edge made by e1, and is gone since
/// s1 changed.`
fn history(d: &Value) -> Option<String> {
    let removed = d["removed_by"].as_str();
    let a = |k: &str| match k.starts_with(['a', 'e', 'i', 'o', 'u']) {
        true => format!("an {k}"),
        false => format!("a {k}"),
    };
    let was = d["was"]["kind"].as_str().map(a);
    let from = d["was"]["from"].as_str();
    match (was, from, removed) {
        (Some(k), Some(f), Some(r)) => Some(format!("It was {k} made by {f}; {r} took it away.")),
        (Some(k), _, Some(r)) => Some(format!("It was {k}; {r} took it away.")),
        (Some(k), Some(f), None) => Some(format!("It was {k} made by {f}.")),
        (Some(k), None, None) => Some(format!("It was {k}.")),
        (None, _, Some(r)) => Some(format!("{r} took it away.")),
        (None, _, None) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Every code in `ryu-doc`'s list, read from its source as written.
    fn every_code() -> Vec<Code> {
        let src = include_str!("../../../ryu-doc/src/error.rs");
        let body = src
            .split("pub enum Code {")
            .nth(1)
            .and_then(|s| s.split("\n}").next())
            .expect("the enum");
        body.lines()
            .map(str::trim)
            .filter(|l| !l.starts_with("//") && l.ends_with(','))
            .map(|l| {
                let name = l.trim_end_matches(',');
                let mut snake = String::new();
                for (i, c) in name.chars().enumerate() {
                    if c.is_uppercase() && i > 0 {
                        snake.push('_');
                    }
                    snake.push(c.to_ascii_lowercase());
                }
                serde_json::from_value(json!(snake)).unwrap_or_else(|_| panic!("{name}"))
            })
            .collect()
    }

    #[test]
    fn every_code_has_a_title() {
        let codes = every_code();
        assert!(codes.len() > 150, "{}", codes.len());
        for c in codes {
            let t = title(c);
            assert!(!t.is_empty() && !t.contains('_'), "{c:?}: {t}");
            assert!(t.starts_with(char::is_uppercase), "{c:?}: {t}");
        }
        assert_eq!(title(Code::UnitMismatch), "Unit mismatch");
        assert_eq!(title(Code::InvalidJson), "Invalid JSON");
        assert_eq!(title(Code::OdbLayerUnreadable), "ODB++ layer unreadable");
        assert_eq!(title(Code::BomItemDuplicate), "BOM item duplicate");
        assert_eq!(title(Code::Cancelled), "Cancelled");
    }

    #[test]
    fn a_blend_failure_names_its_entities_and_what_happened() {
        let e = Error::new(Code::BlendFailed, "fillet f1: the radius is too large")
            .with("reason", "radius_too_large")
            .with("feature", "f1")
            .with(
                "entities",
                json!([
                    {"reference": "bracket/base/3c1f", "kind": "edge", "listed": "bracket/base/3c1f"},
                    {"reference": "bracket/base/9e2a", "kind": "face", "role": "overflows onto"},
                ]),
            );
        let c = Card::of(&e);
        assert_eq!(c.title, "Fillet or chamfer failed");
        assert_eq!(c.code, "blend_failed");
        assert_eq!(c.facts, ["reason radius_too_large", "feature f1"]);
        assert_eq!(c.chips[0].what, "edge, listed");
        assert_eq!(c.chips[1].what, "face, overflows onto");
        assert!(c.json.contains("radius_too_large"));
    }

    #[test]
    fn a_gone_reference_says_what_it_was_and_what_took_it() {
        let e = Error::new(Code::ReferenceUnresolved, "gone")
            .with("removed_by", "s1")
            .with("was", json!({"kind": "edge", "from": "e1"}))
            .with("step", json!(["#12"]));
        let c = Card::of(&e);
        assert_eq!(
            c.history.as_deref(),
            Some("It was an edge made by e1; s1 took it away.")
        );
        assert!(!c.chips[0].shows);
    }
}
