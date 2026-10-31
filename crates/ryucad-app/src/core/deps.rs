//! The Dependencies tab, the picker and the marks (design 233, The app):
//! what `deps.list`, `deps.outdated`, a `deps.update` dry run and
//! `deps.parts` answer, as the window shows them. Nothing here sends a
//! request: the workspace does, as an agent would.

use ryucad::error::Error;
use serde_json::Value;
use std::collections::BTreeMap;

fn text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        v => v.to_string(),
    }
}

/// An error's code as JSON writes it: `dependency_fetch_failed`.
pub fn code(e: &Error) -> String {
    serde_json::to_value(e.code)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

/// A label the lock holds, and the instances placing it.
#[derive(Clone, Debug, PartialEq)]
pub struct Locked {
    pub part: String,
    pub label: String,
    /// `<assembly>/<instance>` for each.
    pub instances: Vec<String>,
    /// The newer official label, when `deps.outdated` says there is one.
    pub outdated: Option<String>,
}

impl Locked {
    /// `m6-bolt B (frame/b1)`, then `, C released` when outdated.
    pub fn text(&self) -> String {
        let mut s = format!("{} {}", self.part, self.label);
        if !self.instances.is_empty() {
            s.push_str(&format!(" ({})", self.instances.join(", ")));
        }
        if let Some(l) = &self.outdated {
            s.push_str(&format!(", {l} released"));
        }
        s
    }
}

/// One dependency, or a project the lock reaches through one.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    /// Its name in the manifest; none for one only the lock reaches.
    pub name: Option<String>,
    pub project: String,
    pub url: String,
    pub path: String,
    /// The names (or IDs) of the dependencies it is reached through.
    pub via: Vec<String>,
    pub fetched: bool,
    pub tip: Option<String>,
    pub labels: Vec<Locked>,
    /// Its faults' messages (a bad URL, an ID listed twice).
    pub faults: Vec<String>,
}

impl Row {
    /// Its name, else its ID and what reaches it: `bb (through fasteners)`.
    pub fn title(&self) -> String {
        match &self.name {
            Some(n) => n.clone(),
            None if self.via.is_empty() => self.project.clone(),
            None => format!("{} (through {})", self.project, self.via.join(", ")),
        }
    }

    /// `fetched at <10 hex>`, or why not.
    pub fn state(&self) -> String {
        match (&self.tip, self.fetched) {
            (Some(t), true) => format!("fetched at {}", &t[..t.len().min(10)]),
            (None, true) => "fetched: no release branch yet".to_owned(),
            _ => "not fetched: Fetch reads it".to_owned(),
        }
    }

    /// Whether the person may remove it here (a transitive project goes
    /// with what reaches it).
    pub fn removable(&self) -> bool {
        self.name.is_some()
    }
}

/// What the window knows of the project's dependencies.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Deps {
    pub rows: Vec<Row>,
    /// The project revision `deps.list` was read at: writes are guarded
    /// by it.
    pub revision: Option<String>,
    /// Why the lock doesn't read, if it doesn't.
    pub lock_fault: Option<String>,
    /// Every instance at a label older than its part's latest, by
    /// assembly and instance: the latest label.
    pub stale: BTreeMap<(String, String), String>,
    /// Why `deps.list` couldn't be read.
    pub error: Option<Error>,
    /// The URL, folder and name typed into Add dependency.
    pub add_url: String,
    pub add_path: String,
    pub add_name: String,
    /// An update's dry run, shown before it is written.
    pub update: Option<Update>,
    /// A write or fetch on its way, and the last one's failure.
    pub busy: bool,
    pub failed: Option<Error>,
    /// The last fetch's summary: `fetched 2 repositories, 1 changed`.
    pub fetched: Option<String>,
}

impl Deps {
    /// Takes a `deps.list` answer.
    pub fn listed(&mut self, v: &Value) {
        self.error = None;
        self.revision = v["project_revision"].as_str().map(str::to_owned);
        self.lock_fault = v["lock_fault"]["message"].as_str().map(str::to_owned);
        let names: BTreeMap<String, String> = v["dependencies"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|d| Some((text(&d["project"]), d["name"].as_str()?.to_owned())))
            .collect();
        self.rows = v["dependencies"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|d| Row {
                name: d["name"].as_str().map(str::to_owned),
                project: text(&d["project"]),
                url: text(&d["url"]),
                path: text(&d["path"]),
                via: d["via"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|p| names.get(&text(p)).cloned().unwrap_or_else(|| text(p)))
                    .collect(),
                fetched: d["fetched"] == true,
                tip: d["tip"].as_str().map(str::to_owned),
                labels: d["labels"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|l| Locked {
                        part: text(&l["part"]),
                        label: text(&l["label"]),
                        instances: l["instances"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .map(|i| format!("{}/{}", text(&i["assembly"]), text(&i["instance"])))
                            .collect(),
                        outdated: None,
                    })
                    .collect(),
                faults: d["faults"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|f| text(&f["message"]))
                    .collect(),
            })
            .collect();
        self.mark_rows();
    }

    /// Takes a `deps.outdated` answer.
    pub fn outdated(&mut self, v: &Value) {
        self.stale = v["outdated"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|o| {
                (
                    (text(&o["assembly"]), text(&o["instance"])),
                    text(&o["latest"]),
                )
            })
            .collect();
        self.mark_rows();
    }

    fn mark_rows(&mut self) {
        for r in &mut self.rows {
            for l in &mut r.labels {
                l.outdated = l.instances.iter().find_map(|i| {
                    let (a, f) = i.split_once('/')?;
                    self.stale.get(&(a.to_owned(), f.to_owned())).cloned()
                });
            }
        }
    }

    /// What the tree and the property panel mark with.
    pub fn marks(&self) -> Marks {
        Marks {
            names: self
                .rows
                .iter()
                .filter_map(|r| Some((r.project.clone(), r.name.clone()?)))
                .collect(),
            stale: self.stale.clone(),
        }
    }

    /// The dependency named `name`, if listed.
    pub fn row(&self, name: &str) -> Option<&Row> {
        self.rows.iter().find(|r| r.name.as_deref() == Some(name))
    }
}

/// Dependencies' names by project ID, and outdated instances: what
/// [`crate::core::model::Model`] marks instance rows with.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Marks {
    names: BTreeMap<String, String>,
    stale: BTreeMap<(String, String), String>,
}

impl Marks {
    /// A project's dependency name, else its ID.
    pub fn name(&self, project: &str) -> String {
        self.names
            .get(project)
            .cloned()
            .unwrap_or_else(|| project.to_owned())
    }

    /// The newer label instance `instance` of `assembly` could take.
    pub fn outdated(&self, assembly: &str, instance: &str) -> Option<&str> {
        self.stale
            .get(&(assembly.to_owned(), instance.to_owned()))
            .map(String::as_str)
    }
}

/// A `deps.update` dry run, as Update shows it before it is written.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Update {
    /// The names it was asked for (empty: every dependency) and the part.
    pub names: Vec<String>,
    pub part: Option<String>,
    /// `frame/b1: fasteners:m6-bolt B → C`.
    pub moved: Vec<String>,
    /// `frame/m1 would fail: … (code)`.
    pub failing: Vec<String>,
}

impl Update {
    pub fn from_answer(v: &Value) -> Update {
        Update {
            moved: v["moved"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|m| {
                    let of = match m["dependency"].as_str() {
                        Some(d) => format!("{d}:{}", text(&m["part"])),
                        None => text(&m["part"]),
                    };
                    format!(
                        "{}/{}: {of} {} → {}",
                        text(&m["assembly"]),
                        text(&m["instance"]),
                        text(&m["from"]),
                        text(&m["to"])
                    )
                })
                .collect(),
            failing: v["failing"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|f| {
                    format!(
                        "{}/{} would fail: {} ({})",
                        text(&f["part"]),
                        text(&f["feature"]),
                        text(&f["error"]["message"]),
                        text(&f["error"]["code"])
                    )
                })
                .collect(),
            ..Update::default()
        }
    }

    /// Nothing would move.
    pub fn is_empty(&self) -> bool {
        self.moved.is_empty()
    }
}

/// A part a dependency released, as the picker lists it.
#[derive(Clone, Debug, PartialEq)]
pub struct Released {
    pub part: String,
    /// `<dependency>:<part>`, what `feature.add` takes as `of`.
    pub of: String,
    pub number: Option<String>,
    pub name: Option<String>,
    pub kind: String,
    /// Latest first, with their states.
    pub labels: Vec<(String, String)>,
}

impl Released {
    /// `B-6 M6 bolt (m6-bolt): C released`, or `kit, assembly: A obsolete`.
    pub fn text(&self) -> String {
        let who = match (&self.number, &self.name) {
            (Some(n), Some(m)) => format!("{n} {m} ({})", self.part),
            (Some(n), None) => format!("{n} ({})", self.part),
            (None, Some(m)) => format!("{m} ({})", self.part),
            (None, None) => self.part.clone(),
        };
        let kind = if self.kind == "assembly" {
            ", assembly"
        } else {
            ""
        };
        match self.labels.first() {
            Some((l, s)) => format!("{who}{kind}: {l} {s}"),
            None => format!("{who}{kind}: nothing released"),
        }
    }

    /// The label offered first: the latest that isn't obsolete, else the
    /// latest.
    pub fn first_label(&self) -> Option<&str> {
        self.labels
            .iter()
            .find(|(_, s)| s != "obsolete")
            .or(self.labels.first())
            .map(|(l, _)| l.as_str())
    }
}

/// Place from dependency…, while open: the assembly it places into, the
/// dependency, its released parts and the one and label chosen.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Picker {
    pub assembly: String,
    pub dependency: Option<String>,
    pub parts: Vec<Released>,
    pub chosen: Option<usize>,
    pub label: Option<String>,
    /// `deps.parts` on its way, or its refusal; the placing's refusal.
    pub reading: bool,
    pub error: Option<Error>,
    pub placing: bool,
}

impl Picker {
    pub fn new(assembly: &str) -> Picker {
        Picker {
            assembly: assembly.to_owned(),
            ..Picker::default()
        }
    }

    /// Takes a `deps.parts` answer for the chosen dependency.
    pub fn parts_read(&mut self, v: &Value) {
        self.reading = false;
        self.error = None;
        self.chosen = None;
        self.label = None;
        self.parts = v["parts"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|p| Released {
                part: text(&p["part"]),
                of: text(&p["of"]),
                number: p["number"].as_str().map(str::to_owned),
                name: p["name"].as_str().map(str::to_owned),
                kind: text(&p["kind"]),
                labels: p["labels"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|l| (text(&l["label"]), text(&l["state"])))
                    .collect(),
            })
            .collect();
    }

    /// Chooses the `i`th part, offering its first label.
    pub fn choose(&mut self, i: usize) {
        self.chosen = Some(i);
        self.label = self
            .parts
            .get(i)
            .and_then(|p| p.first_label())
            .map(str::to_owned);
    }

    /// The chosen part's `of`.
    pub fn of(&self) -> Option<String> {
        Some(self.parts.get(self.chosen?)?.of.clone())
    }

    /// The chosen part, if any.
    pub fn part(&self) -> Option<&Released> {
        self.parts.get(self.chosen?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn list() -> Value {
        json!({"schema": 1, "project_revision": "r1", "dependencies": [
            {"name": "fasteners", "project": "aa", "url": "file:///f", "path": "",
             "direct": true, "via": [], "release_branch": "main", "fetched": true,
             "tip": "0123456789abcdef0123", "faults": [],
             "labels": [{"part": "m6-bolt", "label": "B", "commit": "c", "content": "h",
                         "instances": [{"assembly": "frame", "instance": "b1"}]}]},
            {"name": null, "project": "bb", "url": "file:///w", "path": "lib",
             "direct": false, "via": ["aa"], "release_branch": null, "fetched": false,
             "tip": null, "faults": [], "labels": []}]})
    }

    fn outdated() -> Value {
        json!({"schema": 1, "outdated": [
            {"assembly": "frame", "instance": "b1", "part": "m6-bolt", "label": "B",
             "latest": "C", "project": "aa", "dependency": "fasteners"},
            {"assembly": "frame", "instance": "own", "part": "plate", "label": "A",
             "latest": "B"}]})
    }

    #[test]
    fn dependencies_read_as_people_say_them() {
        let mut d = Deps::default();
        d.listed(&list());
        assert_eq!(d.rows.len(), 2);
        let f = &d.rows[0];
        assert_eq!(f.title(), "fasteners");
        assert_eq!(f.state(), "fetched at 0123456789");
        assert_eq!(f.labels[0].text(), "m6-bolt B (frame/b1)");
        let w = &d.rows[1];
        assert_eq!(w.title(), "bb (through fasteners)");
        assert_eq!(w.state(), "not fetched: Fetch reads it");
        assert_eq!(d.revision.as_deref(), Some("r1"));
        d.outdated(&outdated());
        assert_eq!(d.rows[0].labels[0].outdated.as_deref(), Some("C"));
        assert_eq!(
            d.rows[0].labels[0].text(),
            "m6-bolt B (frame/b1), C released"
        );
    }

    #[test]
    fn marks_name_foreign_instances_and_what_is_outdated() {
        let mut d = Deps::default();
        d.listed(&list());
        d.outdated(&outdated());
        let m = d.marks();
        assert_eq!(m.name("aa"), "fasteners");
        assert_eq!(m.name("zz"), "zz");
        assert_eq!(m.outdated("frame", "b1"), Some("C"));
        assert_eq!(m.outdated("frame", "own"), Some("B"));
        assert_eq!(m.outdated("frame", "other"), None);
    }

    #[test]
    fn an_update_plan_says_what_moves_and_what_breaks() {
        let v = json!({"schema": 1, "dry_run": true, "lock_changed": true,
            "moved": [{"assembly": "frame", "instance": "b1", "part": "m6-bolt",
                       "dependency": "fasteners", "project": "aa", "from": "B", "to": "C"}],
            "failing": [{"part": "frame", "feature": "m1",
                         "error": {"code": "reference_unresolved", "message": "gone"}}]});
        let u = Update::from_answer(&v);
        assert_eq!(u.moved, ["frame/b1: fasteners:m6-bolt B → C"]);
        assert_eq!(
            u.failing,
            ["frame/m1 would fail: gone (reference_unresolved)"]
        );
        assert!(!u.is_empty());
        let none = Update::from_answer(&json!({"moved": [], "failing": []}));
        assert!(none.is_empty());
    }

    #[test]
    fn the_picker_offers_released_parts_and_their_labels() {
        let v = json!({"schema": 1, "dependency": {"name": "fasteners", "project": "aa"},
            "release_branch": "main", "tip": "t", "parts": [
            {"part": "m6-bolt", "of": "fasteners:m6-bolt", "number": "B-6", "name": "M6 bolt",
             "kind": "part", "labels": [{"label": "C", "state": "released", "commit": "c2"},
                                        {"label": "B", "state": "superseded", "commit": "c1"}]},
            {"part": "kit", "of": "fasteners:kit", "number": null, "name": null,
             "kind": "assembly", "labels": [{"label": "A", "state": "obsolete", "commit": "c"}]}]});
        let mut p = Picker::new("frame");
        p.dependency = Some("fasteners".into());
        p.parts_read(&v);
        assert_eq!(p.parts[0].text(), "B-6 M6 bolt (m6-bolt): C released");
        assert_eq!(p.parts[1].text(), "kit, assembly: A obsolete");
        assert_eq!(p.label, None);
        p.choose(0);
        assert_eq!(p.label.as_deref(), Some("C"));
        assert_eq!(p.of().as_deref(), Some("fasteners:m6-bolt"));
        // An obsolete label still places, as within a repo, but isn't the
        // first offered.
        p.choose(1);
        assert_eq!(p.label.as_deref(), Some("A"));
    }
}
