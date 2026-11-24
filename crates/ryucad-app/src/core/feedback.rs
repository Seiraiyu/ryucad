//! What an error card says (design 199): a short title for each error
//! code, the message, the entities its details name as chips, `removed_by`
//! and `was` in words, and the error's JSON for Copy details. Pure:
//! `ui::feedback` draws it.

use ryucad::error::{Code, Error};
use serde_json::Value;

/// A short title for each code: `ryu-doc`'s, beside its declared keys
/// (design 698), so `errors` and the app say the same.
pub use ryucad::error::details::title;

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
    /// What the agent would run to clear it: `ryucad import convert
    /// <part>/<solid>`.
    pub hint: Option<String>,
    /// The import a Convert button converts to a solid, `(part,
    /// feature)`: a `reference_body` refusal's, read from its hint.
    pub convert: Option<(String, String)>,
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
            hint: d["hint"].as_str().map(str::to_owned),
            convert: convert_of(e),
        }
    }
}

/// The import a `reference_body` refusal says to convert, `(part,
/// feature)`: its hint is `ryucad import convert <part>/<solid>`, and a
/// reference's solid is named by the import feature that founds it.
/// `import.convert`'s own refusal (features act on what would become a
/// reference) has no hint, and nothing to convert.
pub fn convert_of(e: &Error) -> Option<(String, String)> {
    if e.code != Code::ReferenceBody {
        return None;
    }
    let target = e.details["hint"]
        .as_str()?
        .strip_prefix("ryucad import convert ")?;
    let (part, feature) = target.split_once('/')?;
    Some((part.to_owned(), feature.to_owned()))
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
    fn a_reference_body_refusal_offers_its_conversion() {
        let e = Error::new(
            Code::ReferenceBody,
            "f1 would change motor, a reference import",
        )
        .with("feature", "f1")
        .with("solid", "motor")
        .with("role", "targets")
        .with("hint", "ryucad import convert p/motor");
        let c = Card::of(&e);
        assert_eq!(c.title, "A reference import can't be changed");
        assert_eq!(c.facts, ["feature f1", "solid motor"]);
        assert_eq!(c.hint.as_deref(), Some("ryucad import convert p/motor"));
        assert_eq!(c.convert, Some(("p".into(), "motor".into())));
        // Convert's own refusal names what acts on it, and offers nothing.
        let own = Error::new(Code::ReferenceBody, "motor can't become a reference")
            .with("feature", "motor")
            .with("acting", json!(["f1"]));
        assert_eq!(Card::of(&own).convert, None);
        assert_eq!(convert_of(&Error::new(Code::NotFound, "x")), None);
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
