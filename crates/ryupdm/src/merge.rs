//! Merge validation for PDM: the faults a merge's result has that neither
//! of its sides had, which `merge.status` reports as `pdm_broke`. Each
//! side's files may be right on their own and wrong together:
//!
//! - `part_number_duplicate`: each side numbered a different part the
//!   same, or one gave a part the number the other gave another;
//! - `property_unknown`, `property_invalid`, `part_number_invalid`: one
//!   side changed the settings (a declaration, the pattern) and the other
//!   set a value they no longer take;
//! - `release_stale`: one side proposed a release and the other changed
//!   the part;
//! - `release_sequence`: the sides changed a part's records so that a
//!   proposal no longer follows the release before it (one withdrew the
//!   proposal the other's supersedes).
//!
//! A fault is blamed on the side whose change completes it: the side that
//! changed what the faulty file depends on (the settings, another part's
//! number, the part a proposal describes, the part's other records), not
//! counting a side that holds the faulty file as merged (the file came
//! from there, and was right there); `both` when both did, or neither. A proposal that is stale or out of
//! sequence is settled by withdrawing it (deleting its record), so the
//! merged part can be proposed again. Faults a side already had, and files
//! a merge left conflicted (the structural conflicts report those), are
//! left out.

use crate::check::sequence;
use crate::config::Config;
use crate::history::Releases;
use crate::props::Pdm;
use crate::record::{Entry, Record, Records};
use ryucad_doc::doc::files::Manifest;
use ryucad_doc::doc::id::Id;
use ryucad_doc::doc::project::{MANIFEST, PDM_CONFIG, RELEASES, without_conflicts};
use ryucad_doc::doc::source::{self, Source, join};
use ryucad_doc::error::Code;
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// The side a fault is blamed on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cause {
    Ours,
    Theirs,
    Both,
}

impl Cause {
    /// `ours`, `theirs` or `both`, as `merge.status` writes it.
    pub fn as_str(self) -> &'static str {
        match self {
            Cause::Ours => "ours",
            Cause::Theirs => "theirs",
            Cause::Both => "both",
        }
    }
}

/// A PDM fault the merge introduced.
#[derive(Clone, Debug, PartialEq)]
pub struct Broke {
    /// The fault's code: a property fault's error code, or a release
    /// check's.
    pub code: String,
    pub part: Id,
    /// The file the fault is in: the part's properties, or a record.
    pub path: String,
    /// The property, for a property fault.
    pub property: Option<String>,
    /// The record's label, for a release fault.
    pub label: Option<String>,
    pub message: String,
    /// The fault's details, as the property fault or the check finding
    /// gives them.
    pub details: Value,
    pub cause: Cause,
    /// `path`, then the files it depends on that a side changed, sorted.
    pub files: Vec<String>,
    /// Whether withdrawing the record at `path`, a proposal, settles it.
    pub withdraw: bool,
}

/// What a faulty file depends on: a file, or every file under a folder.
enum Dep {
    File(String),
    Under(String),
}

/// One fault on one side, before blame.
struct Fault {
    code: String,
    part: Id,
    path: String,
    property: Option<String>,
    label: Option<String>,
    message: String,
    details: Value,
    withdraw: bool,
    depends: Vec<Dep>,
}

/// What makes two faults on different sides the same: the code, the part
/// and the property or label.
type Key = (String, Id, String);

/// The parts `src` has folders for.
fn parts(src: &dyn Source) -> Vec<Id> {
    src.list("parts")
        .unwrap_or_default()
        .into_iter()
        .filter(|(_, k)| *k == source::Kind::Dir)
        .filter_map(|(n, _)| Id::parse(&n, "part").ok())
        .collect()
}

/// Every PDM fault in the project `src` holds that a merge can introduce,
/// with `releases` saying which records are official. None when the
/// settings can't be read: a conflicted `ryupdm.json` is a structural
/// conflict, and nothing can be judged against it.
fn faults(src: &dyn Source, releases: &Releases) -> BTreeMap<Key, Fault> {
    let mut out = BTreeMap::new();
    let Ok(config) = Config::read(src) else {
        return out;
    };
    let unit = crate::read_json(src, MANIFEST)
        .ok()
        .flatten()
        .and_then(|v| Manifest::from_json(&without_conflicts(&v)).ok())
        .map(|m| m.default_unit);
    if let Some(unit) = unit {
        let pdm = Pdm::of(src, parts(src), unit);
        for (part, pp) in &pdm.parts {
            for e in &pp.faults {
                let property = e.details["property"].as_str().unwrap_or_default();
                let code = json!(e.code).as_str().unwrap_or_default().to_owned();
                let depends = if e.code == Code::PartNumberDuplicate {
                    e.details["parts"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|p| Id::parse(p.as_str()?, "part").ok())
                        .map(|p| Dep::File(crate::props::path(&p)))
                        .collect()
                } else {
                    vec![
                        Dep::File(PDM_CONFIG.to_owned()),
                        Dep::File(MANIFEST.to_owned()),
                    ]
                };
                out.insert(
                    (code.clone(), part.clone(), property.to_owned()),
                    Fault {
                        code,
                        part: part.clone(),
                        path: crate::props::path(part),
                        property: Some(property.to_owned()),
                        label: None,
                        message: e.message.clone(),
                        details: e.details.clone(),
                        withdraw: false,
                        depends,
                    },
                );
            }
        }
    }
    let Ok(records) = Records::read(src) else {
        return out;
    };
    let official = |path: &str| releases.official.contains_key(path);
    for part in records.parts() {
        let entries: Vec<(&Entry, &Record)> = records
            .of(&part)
            .filter_map(|e| Some((e, e.record.as_ref().ok()?)))
            .collect();
        let others = join(RELEASES, part.as_str());
        for f in sequence(&config, &part, &entries) {
            let path = f.details["path"].as_str().unwrap_or_default().to_owned();
            let label = f.label.clone().unwrap_or_default();
            out.insert(
                (f.code.to_owned(), part.clone(), label.clone()),
                Fault {
                    code: f.code.to_owned(),
                    part: part.clone(),
                    withdraw: !official(&path),
                    path,
                    property: None,
                    label: Some(label),
                    message: f.message,
                    details: f.details,
                    depends: vec![Dep::Under(others.clone()), Dep::File(PDM_CONFIG.to_owned())],
                },
            );
        }
        // A part a merge left unreadable is a structural conflict.
        let (Ok(content), Ok(props)) = (
            crate::content::hash(src, &part),
            crate::props::read(src, &part),
        ) else {
            continue;
        };
        for (e, r) in &entries {
            let Some(rel) = r.release() else { continue };
            if official(&e.path) {
                continue;
            }
            let changes = rel.changes(&content, &config, &props);
            if changes.is_empty() {
                continue;
            }
            out.insert(
                ("release_stale".to_owned(), part.clone(), e.label.clone()),
                Fault {
                    code: "release_stale".to_owned(),
                    part: part.clone(),
                    path: e.path.clone(),
                    property: None,
                    label: Some(e.label.clone()),
                    message: format!(
                        "{} proposes {part} {} but the merged part isn't what it describes ({}): withdraw it and propose the part again",
                        e.path,
                        e.label,
                        changes.join(", ")
                    ),
                    details: json!({"path": e.path, "changes": changes}),
                    withdraw: true,
                    depends: vec![
                        Dep::Under(join("parts", part.as_str())),
                        Dep::File(MANIFEST.to_owned()),
                        Dep::File(PDM_CONFIG.to_owned()),
                    ],
                },
            );
        }
    }
    out
}

/// Every file under `dir` in `src`, by path.
fn under(src: &dyn Source, dir: &str, out: &mut BTreeMap<String, Vec<u8>>) {
    for (name, kind) in src.list(dir).unwrap_or_default() {
        let path = join(dir, &name);
        match kind {
            source::Kind::Dir => under(src, &path, out),
            source::Kind::File => {
                if let Ok(Some(b)) = src.read(&path) {
                    out.insert(path, b);
                }
            }
            source::Kind::Other => {}
        }
    }
}

/// The files of `deps` that `side` changed from `base`, but `skip`.
fn changed(side: &dyn Source, base: &dyn Source, deps: &[Dep], skip: &str) -> Vec<String> {
    let mut out = Vec::new();
    for d in deps {
        match d {
            Dep::File(p) => {
                if p != skip && side.read(p).ok().flatten() != base.read(p).ok().flatten() {
                    out.push(p.clone());
                }
            }
            Dep::Under(dir) => {
                let (mut a, mut b) = (BTreeMap::new(), BTreeMap::new());
                under(side, dir, &mut a);
                under(base, dir, &mut b);
                for p in a.keys().chain(b.keys()) {
                    if p != skip && a.get(p) != b.get(p) && !out.contains(p) {
                        out.push(p.clone());
                    }
                }
            }
        }
    }
    out
}

/// The PDM faults of the merge `merged` that neither `ours` nor `theirs`
/// has, each blamed on the side whose change completes it (see the module
/// docs). `base` is the merge base (an empty source when there is none);
/// `releases` the release branch as read, which says which records are
/// official. In the order of their code, part, and property or label.
pub fn validate(
    base: &dyn Source,
    ours: &dyn Source,
    theirs: &dyn Source,
    merged: &dyn Source,
    releases: &Releases,
) -> Vec<Broke> {
    let (o, t) = (faults(ours, releases), faults(theirs, releases));
    let mut out = Vec::new();
    for (key, f) in faults(merged, releases) {
        if o.contains_key(&key) || t.contains_key(&key) {
            continue;
        }
        // A side holding the faulty file as merged is where it came from,
        // right there: the other side's changes complete the fault.
        let read = |s: &dyn Source| s.read(&f.path).ok().flatten();
        let blamed = |s: &dyn Source| {
            if read(s) == read(merged) {
                Vec::new()
            } else {
                changed(s, base, &f.depends, &f.path)
            }
        };
        let (mut by_ours, mut by_theirs) = (blamed(ours), blamed(theirs));
        let cause = match (by_ours.is_empty(), by_theirs.is_empty()) {
            (false, true) => Cause::Ours,
            (true, false) => Cause::Theirs,
            (false, false) => Cause::Both,
            (true, true) => {
                by_ours = changed(ours, base, &f.depends, &f.path);
                by_theirs = changed(theirs, base, &f.depends, &f.path);
                Cause::Both
            }
        };
        let mut rest: Vec<String> = by_ours.into_iter().chain(by_theirs).collect();
        rest.sort();
        rest.dedup();
        let mut files = vec![f.path.clone()];
        files.extend(rest);
        out.push(Broke {
            code: f.code,
            part: f.part,
            path: f.path,
            property: f.property,
            label: f.label,
            message: f.message,
            details: f.details,
            cause,
            files,
            withdraw: f.withdraw,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::tests::{add, official, project, release_now};
    use ryucad_doc::doc::canonical;

    type Files = BTreeMap<String, Vec<u8>>;

    fn put(f: &mut Files, path: &str, v: Value) {
        f.insert(path.to_owned(), canonical::to_string(&v).into_bytes());
    }

    fn props(f: &mut Files, part: &str, values: Value) {
        let mut v = json!({"format": 1, "kind": "ryupdm.properties"});
        for (k, x) in values.as_object().unwrap() {
            v[k] = x.clone();
        }
        put(f, &format!("parts/{part}/properties.json"), v);
    }

    /// `project()` with a second part, `q`, unnumbered.
    fn two_parts() -> Files {
        let mut f = project();
        put(
            &mut f,
            "parts/q/part.json",
            json!({"features": [], "format": 1, "kind": "ryucad.part"}),
        );
        f
    }

    fn summary(b: &[Broke]) -> Vec<(String, String, &'static str, Vec<String>, bool)> {
        b.iter()
            .map(|b| {
                (
                    b.code.clone(),
                    b.part.to_string(),
                    b.cause.as_str(),
                    b.files.clone(),
                    b.withdraw,
                )
            })
            .collect()
    }

    #[test]
    fn numbers_given_on_each_side_collide() {
        let base = two_parts();
        let (mut ours, mut theirs) = (base.clone(), base.clone());
        props(&mut ours, "p", json!({"number": "300"}));
        props(&mut theirs, "q", json!({"number": "300"}));
        let mut merged = ours.clone();
        props(&mut merged, "q", json!({"number": "300"}));
        let none = Releases::default();
        let b = validate(&base, &ours, &theirs, &merged, &none);
        // Each part's number was fine on its side: the other side's number
        // completes the collision.
        assert_eq!(
            summary(&b),
            [
                (
                    "part_number_duplicate".into(),
                    "p".into(),
                    "theirs",
                    vec![
                        "parts/p/properties.json".into(),
                        "parts/q/properties.json".into()
                    ],
                    false
                ),
                (
                    "part_number_duplicate".into(),
                    "q".into(),
                    "ours",
                    vec![
                        "parts/q/properties.json".into(),
                        "parts/p/properties.json".into()
                    ],
                    false
                ),
            ]
        );
        assert_eq!(b[0].property.as_deref(), Some("number"));
        assert_eq!(b[0].details["parts"], json!(["q"]));
        // A collision a side already had isn't the merge's.
        let b = validate(&base, &merged, &theirs, &merged, &none);
        assert_eq!(b, []);
    }

    #[test]
    fn a_value_the_other_sides_settings_refuse_is_the_settings_side() {
        let mut base = two_parts();
        put(
            &mut base,
            "ryupdm.json",
            json!({"format": 1, "kind": "ryupdm.config",
                   "properties": {"finish": {"type": "string"}}}),
        );
        let (mut ours, mut theirs) = (base.clone(), base.clone());
        // Ours makes finish a choice, and has q's finish wrong already.
        put(
            &mut ours,
            "ryupdm.json",
            json!({"format": 1, "kind": "ryupdm.config",
                   "properties": {"finish": {"type": "enum", "values": ["anodized"]}}}),
        );
        props(&mut ours, "q", json!({"finish": "blue"}));
        props(&mut theirs, "p", json!({"number": "100", "finish": "red"}));
        let mut merged = ours.clone();
        props(&mut merged, "p", json!({"number": "100", "finish": "red"}));
        let b = validate(&base, &ours, &theirs, &merged, &Releases::default());
        assert_eq!(
            summary(&b),
            [(
                "property_invalid".into(),
                "p".into(),
                "ours",
                vec!["parts/p/properties.json".into(), "ryupdm.json".into()],
                false
            )]
        );
        assert_eq!(b[0].property.as_deref(), Some("finish"));
        assert_eq!(b[0].details["value"], "red");
    }

    #[test]
    fn a_proposal_the_other_side_changed_the_part_under_is_withdrawn() {
        let base = project();
        let mut theirs = base.clone();
        let r = release_now(&theirs, "A", None);
        add(&mut theirs, &r);
        let mut ours = base.clone();
        ours.insert("parts/p/features/a.json".into(), b"{\"x\": 2}".to_vec());
        let mut merged = ours.clone();
        merged.insert(
            "releases/p/A.json".into(),
            theirs["releases/p/A.json"].clone(),
        );
        let b = validate(&base, &ours, &theirs, &merged, &Releases::default());
        assert_eq!(
            summary(&b),
            [(
                "release_stale".into(),
                "p".into(),
                "ours",
                vec!["releases/p/A.json".into(), "parts/p/features/a.json".into()],
                true
            )]
        );
        assert_eq!(b[0].label.as_deref(), Some("A"));
        assert_eq!(b[0].details["changes"], json!(["content"]));
        // An official record isn't stale: its part is modified.
        let a = official(&["releases/p/A.json"]);
        assert_eq!(validate(&base, &ours, &theirs, &merged, &a), []);
    }

    #[test]
    fn a_proposal_whose_predecessor_was_withdrawn_is_out_of_sequence() {
        let mut base = project();
        let r = release_now(&base, "A", None);
        add(&mut base, &r);
        base.insert("parts/p/features/a.json".into(), b"{\"x\": 2}".to_vec());
        let r = release_now(&base, "B", Some("A"));
        add(&mut base, &r);
        let a = official(&["releases/p/A.json"]);
        // Ours withdraws B; theirs proposes C after it.
        let mut ours = base.clone();
        ours.remove("releases/p/B.json");
        let mut theirs = base.clone();
        let r = release_now(&theirs, "C", Some("B"));
        add(&mut theirs, &r);
        let mut merged = ours.clone();
        merged.insert(
            "releases/p/C.json".into(),
            theirs["releases/p/C.json"].clone(),
        );
        let b = validate(&base, &ours, &theirs, &merged, &a);
        assert_eq!(
            summary(&b),
            [(
                "release_sequence".into(),
                "p".into(),
                "ours",
                vec!["releases/p/C.json".into(), "releases/p/B.json".into()],
                true
            )]
        );
        assert_eq!(b[0].label.as_deref(), Some("C"));
    }

    #[test]
    fn conflicted_settings_judge_nothing() {
        let base = two_parts();
        let (mut ours, mut theirs) = (base.clone(), base.clone());
        props(&mut ours, "p", json!({"number": "300"}));
        props(&mut theirs, "q", json!({"number": "300"}));
        let mut merged = ours.clone();
        props(&mut merged, "q", json!({"number": "300"}));
        put(
            &mut merged,
            "ryupdm.json",
            json!({"format": 1, "kind": "ryupdm.config",
                   "conflicts": {"/revisions": {"kind": "value", "ours": "letters", "theirs": "numeric"}}}),
        );
        assert_eq!(
            validate(&base, &ours, &theirs, &merged, &Releases::default()),
            []
        );
    }
}
