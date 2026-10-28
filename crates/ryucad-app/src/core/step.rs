//! STEP from the File menu: Import STEP…, Update import… and Export
//! STEP…. The window asks the daemon what an import would do (`import`
//! with `dry_run`), shows that plan, and on confirmation sends the same
//! `import`; an export is `export`'s bytes, written where the person
//! chose. Nothing is worked out here: plans are the daemon's answers, as
//! an agent's are.

use ryucad::error::Error;
use serde_json::Value;

/// What an import or update would do, or did: the daemon's answer, as
/// lines for people.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Plan {
    /// Where the file is (or would be) kept: `imports/<name>.step`, and
    /// whether the same bytes were there already.
    pub stored: String,
    pub reused: bool,
    /// Each part or assembly made or changed: a heading line, then a line
    /// per feature and property.
    pub parts: Vec<(String, Vec<String>)>,
    /// Features that build now and wouldn't after an update.
    pub failing: Vec<String>,
    /// Products an update finds gone from the file, and their parts.
    pub vanished: Vec<String>,
    /// The answer's warnings, as written.
    pub warnings: Vec<String>,
}

fn text(v: &Value) -> &str {
    v.as_str().unwrap_or_default()
}

/// A feature of a planned part as one line: an import's body with its
/// counts and what healing did, or why it doesn't build; an instance what
/// it places.
fn feature_line(f: &Value) -> String {
    let id = text(&f["id"]);
    let what = match text(&f["type"]) {
        "import" => format!("body {}", f["body"].as_u64().unwrap_or_default()),
        "instance" => format!("places {}", text(&f["of"])),
        t => t.to_owned(),
    };
    if f["ok"] == false {
        return format!(
            "{id} ({what}): fails ({}: {})",
            text(&f["error"]["code"]),
            text(&f["error"]["message"])
        );
    }
    let mut s = format!("{id} ({what})");
    if let Some(c) = f["counts"].as_object() {
        let n = |k: &str| c.get(k).and_then(Value::as_u64).unwrap_or_default();
        s.push_str(&format!(
            ": {} faces, {} edges, {} vertices",
            n("faces"),
            n("edges"),
            n("vertices")
        ));
    }
    let healed: Vec<String> = f["heal"]
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(k, _)| k.as_str() != "tolerance_m")
        .filter_map(|(k, n)| Some((k, n.as_u64().filter(|n| *n > 0)?)))
        .map(|(k, n)| format!("{k} {n}"))
        .collect();
    if !healed.is_empty() {
        s.push_str(&format!("; healed: {}", healed.join(", ")));
    }
    s
}

impl Plan {
    /// From an `import` answer, a dry run's or a write's.
    pub fn from_answer(v: &Value) -> Plan {
        let parts = v["parts"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|p| {
                let kind = match (text(&p["kind"]), p["created"] == true) {
                    ("assembly", true) => "new assembly",
                    ("assembly", false) => "assembly",
                    (_, true) => "new part",
                    _ => "part",
                };
                let mut head = format!("{} ({kind}", text(&p["part"]));
                if p["product"].is_object() {
                    head.push_str(&format!(", product {}", text(&p["product"]["id"])));
                }
                head.push(')');
                let mut lines: Vec<String> = p["features"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(feature_line)
                    .collect();
                for r in p["removed"].as_array().into_iter().flatten() {
                    lines.push(format!("{} removed", text(r)));
                }
                for (k, x) in p["properties"].as_object().into_iter().flatten() {
                    lines.push(format!("property {k} = {}", text(x)));
                }
                (head, lines)
            })
            .collect();
        let strings = |v: &Value| -> Vec<String> {
            v.as_array()
                .into_iter()
                .flatten()
                .filter_map(|s| s.as_str().map(str::to_owned))
                .collect()
        };
        Plan {
            stored: text(&v["file"]["path"]).to_owned(),
            reused: v["file"]["reused"] == true,
            parts,
            failing: v["failing"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|f| {
                    format!(
                        "{}/{}: {}",
                        text(&f["part"]),
                        text(&f["feature"]),
                        text(&f["error"]["code"])
                    )
                })
                .collect(),
            vanished: v["vanished"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|x| {
                    format!(
                        "{} (in {})",
                        text(&x["product"]["id"]),
                        strings(&x["parts"]).join(", ")
                    )
                })
                .collect(),
            warnings: strings(&v["warnings"]),
        }
    }
}

/// File > Import STEP… or Update import…: the file to read, the plan its
/// dry run gave, and once confirmed what the import did.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Import {
    /// The file to read, as typed or picked.
    pub path: String,
    /// For an update, the file the project holds that it replaces.
    pub update: Option<String>,
    /// The dry run's plan, for the path it was asked for.
    pub plan: Option<(String, Plan)>,
    /// What the import wrote, once it has.
    pub done: Option<Plan>,
    pub error: Option<Error>,
    /// A dry run or an import is on its way.
    pub waiting: bool,
}

impl Import {
    /// Importing a new file.
    pub fn new() -> Import {
        Import::default()
    }

    /// Updating `stored`, a file the project holds.
    pub fn update(stored: &str) -> Import {
        Import {
            update: Some(stored.to_owned()),
            ..Import::default()
        }
    }

    /// The plan, when it is for the path as typed now.
    pub fn current(&self) -> Option<&Plan> {
        self.plan
            .as_ref()
            .filter(|(p, _)| *p == self.path.trim())
            .map(|(_, plan)| plan)
    }
}

/// The schemas and units an export offers.
pub const SCHEMAS: [&str; 2] = ["ap242", "ap214"];
pub const UNITS: [&str; 2] = ["mm", "m"];

/// File > Export STEP…: the part or assembly exported, how, and where.
#[derive(Clone, Debug, PartialEq)]
pub struct Export {
    pub part: String,
    pub schema: String,
    pub unit: String,
    /// Where to write it, as typed or picked.
    pub path: String,
    /// The file written and how many bytes, once it has been.
    pub done: Option<(String, usize)>,
    pub error: Option<Error>,
    pub waiting: bool,
}

impl Export {
    pub fn new(part: &str) -> Export {
        Export {
            part: part.to_owned(),
            schema: SCHEMAS[0].to_owned(),
            unit: UNITS[0].to_owned(),
            path: String::new(),
            done: None,
            error: None,
            waiting: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_plan_reads_as_lines() {
        let v = json!({
            "dry_run": true,
            "file": {"path": "imports/frame.step", "reused": false},
            "parts": [
                {"part": "plate", "kind": "part", "created": true,
                 "product": {"id": "PLT-1", "name": "Plate"},
                 "features": [
                    {"id": "body", "type": "import", "body": 0, "ok": true,
                     "counts": {"faces": 6, "edges": 12, "vertices": 8},
                     "heal": {"sewn": 2, "seams": 0, "tolerance_m": 1e-7}},
                    {"id": "body-2", "type": "import", "body": 1, "ok": false,
                     "error": {"code": "import_failed", "message": "an open shell"}}],
                 "properties": {"name": "Plate"}},
                {"part": "frame", "kind": "assembly", "created": false,
                 "features": [{"id": "plate", "type": "instance", "of": "plate", "ok": true}],
                 "removed": ["old"]}],
            "failing": [{"part": "plate", "feature": "f1",
                         "error": {"code": "reference_unresolved"}}],
            "vanished": [{"product": {"id": "OLD-1"}, "parts": ["old"]}],
            "warnings": ["import_failed: plate/body-2 doesn't build: an open shell"],
        });
        let p = Plan::from_answer(&v);
        assert_eq!(p.stored, "imports/frame.step");
        assert!(!p.reused);
        assert_eq!(
            p.parts,
            [
                (
                    "plate (new part, product PLT-1)".to_owned(),
                    vec![
                        "body (body 0): 6 faces, 12 edges, 8 vertices; healed: sewn 2".to_owned(),
                        "body-2 (body 1): fails (import_failed: an open shell)".to_owned(),
                        "property name = Plate".to_owned(),
                    ]
                ),
                (
                    "frame (assembly)".to_owned(),
                    vec!["plate (places plate)".to_owned(), "old removed".to_owned()]
                ),
            ]
        );
        assert_eq!(p.failing, ["plate/f1: reference_unresolved"]);
        assert_eq!(p.vanished, ["OLD-1 (in old)"]);
        assert_eq!(p.warnings.len(), 1);
    }

    #[test]
    fn a_plan_belongs_to_the_path_it_was_asked_for() {
        let mut i = Import::new();
        i.path = " /tmp/a.step ".into();
        i.plan = Some(("/tmp/a.step".into(), Plan::default()));
        assert!(i.current().is_some());
        i.path = "/tmp/b.step".into();
        assert!(i.current().is_none());
        assert_eq!(
            Import::update("imports/a.step").update.as_deref(),
            Some("imports/a.step")
        );
        let e = Export::new("frame");
        assert_eq!((e.schema.as_str(), e.unit.as_str()), ("ap242", "mm"));
    }
}
