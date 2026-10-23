//! A part's release state, derived from its records, the release branch
//! and its working files, never stored:
//!
//! - `in_work`: no official release;
//! - `released <label>`: the part is its latest official release (same
//!   content, same revising properties);
//! - `modified <label>`: it has moved on since; work towards the next one;
//! - `obsolete <label>`: its latest official release has an official
//!   obsolete record;
//! - `unknown`: a record of it can't be read (broken, or a format this
//!   build doesn't know), so nothing is claimed.
//!
//! Each state also names the latest record that isn't official yet, if
//! any (`proposed`), and the label the next release takes.

use crate::config::{Config, Scheme};
use crate::history::Releases;
use crate::label;
use crate::props;
use crate::record::{Entry, Kind, Record, Records};
use ryucad_doc::doc::id::Id;
use ryucad_doc::doc::source::Source;
use ryucad_doc::error::{Code, Error, Result};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// Where a part stands.
#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    InWork,
    Released(String),
    Modified(String),
    Obsolete(String),
    Unknown(Error),
}

impl Status {
    pub fn name(&self) -> &'static str {
        match self {
            Status::InWork => "in_work",
            Status::Released(_) => "released",
            Status::Modified(_) => "modified",
            Status::Obsolete(_) => "obsolete",
            Status::Unknown(_) => "unknown",
        }
    }

    pub fn label(&self) -> Option<&str> {
        match self {
            Status::Released(l) | Status::Modified(l) | Status::Obsolete(l) => Some(l),
            Status::InWork | Status::Unknown(_) => None,
        }
    }
}

/// A part's state.
#[derive(Clone, Debug, PartialEq)]
pub struct State {
    pub status: Status,
    /// The latest record not yet official: its label and kind.
    pub proposed: Option<(String, Kind)>,
    /// The label the next release takes; `None` when the state is unknown.
    pub next: Option<String>,
    /// What differs from the latest official release, when modified:
    /// `content`, then revising properties by name.
    pub changes: Vec<String>,
}

impl State {
    /// As people read it: `in work`, `released B`, `B modified`,
    /// `B obsolete`, `unknown`, then `, C proposed`.
    pub fn text(&self) -> String {
        let mut s = match &self.status {
            Status::InWork => "in work".to_owned(),
            Status::Released(l) => format!("released {l}"),
            Status::Modified(l) => format!("{l} modified"),
            Status::Obsolete(l) => format!("{l} obsolete"),
            Status::Unknown(_) => "unknown".to_owned(),
        };
        if let Some((l, k)) = &self.proposed {
            match k {
                Kind::Release => s.push_str(&format!(", {l} proposed")),
                Kind::Obsolete => s.push_str(&format!(", {l} obsolete proposed")),
            }
        }
        s
    }

    /// `{state, label?, proposed?, next?, changes?, reason?, text}`.
    pub fn to_json(&self) -> Value {
        let mut v = json!({"state": self.status.name(), "text": self.text()});
        if let Some(l) = self.status.label() {
            v["label"] = json!(l);
        }
        if let Some((l, k)) = &self.proposed {
            v["proposed"] = json!({"label": l, "kind": k.as_str()});
        }
        if let Some(n) = &self.next {
            v["next"] = json!(n);
        }
        if !self.changes.is_empty() {
            v["changes"] = json!(self.changes);
        }
        if let Status::Unknown(e) = &self.status {
            v["reason"] = json!(e);
        }
        v
    }
}

/// Where `label` falls in `scheme`, or why it can't be placed.
pub(crate) fn place(scheme: Scheme, e: &Entry) -> Result<u64> {
    label::index(scheme, &e.label).ok_or_else(|| {
        Error::new(
            Code::InvalidDocument,
            format!("{}: {} isn't a {} label", e.path, e.label, scheme.as_str()),
        )
        .with("path", &e.path)
        .with("pointer", "/label")
    })
}

/// A part's records in order: by label, a release before its obsolete
/// record. Fails on the first record that can't be read or placed.
pub fn ordered<'a>(
    scheme: Scheme,
    records: &'a Records,
    part: &'a Id,
) -> Result<Vec<(&'a Entry, &'a Record)>> {
    let mut out = Vec::new();
    for e in records.of(part) {
        let r = e.record.as_ref().map_err(Clone::clone)?;
        out.push(((place(scheme, e)?, e.kind), (e, r)));
    }
    out.sort_by_key(|(k, _)| *k);
    Ok(out.into_iter().map(|(_, x)| x).collect())
}

/// The state of `part` in the project `src` holds, the release branch
/// read into `releases` (`Releases::default()` outside a repository: then
/// nothing is official).
pub fn derive(src: &dyn Source, part: &Id, releases: &Releases) -> State {
    let config = Config::read(src);
    let records = Records::read(src);
    match (config, records) {
        (Ok(c), Ok(r)) => of(src, &c, &r, part, releases),
        (Err(e), _) | (_, Err(e)) => unknown(e),
    }
}

/// [`derive()`] for every part the project has or a record names.
pub fn derive_all(src: &dyn Source, releases: &Releases) -> BTreeMap<Id, State> {
    let config = Config::read(src);
    let records = Records::read(src);
    let mut parts: Vec<Id> = src
        .list("parts")
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(n, k)| {
            (k == ryucad_doc::doc::source::Kind::Dir)
                .then(|| Id::parse(&n, "part").ok())
                .flatten()
        })
        .collect();
    if let Ok(r) = &records {
        parts.extend(r.parts());
    }
    parts.sort();
    parts.dedup();
    parts
        .into_iter()
        .map(|p| {
            let s = match (&config, &records) {
                (Ok(c), Ok(r)) => of(src, c, r, &p, releases),
                (Err(e), _) | (_, Err(e)) => unknown(e.clone()),
            };
            (p, s)
        })
        .collect()
}

fn unknown(e: Error) -> State {
    State {
        status: Status::Unknown(e),
        proposed: None,
        next: None,
        changes: Vec::new(),
    }
}

/// The state of `part` with the project's settings and records read.
pub fn of(
    src: &dyn Source,
    config: &Config,
    records: &Records,
    part: &Id,
    releases: &Releases,
) -> State {
    let list = match ordered(config.revisions, records, part) {
        Ok(l) => l,
        Err(e) => return unknown(e),
    };
    let official = |e: &Entry| releases.official.contains_key(&e.path);
    let next = Some(match list.last() {
        Some((e, _)) => label::label(
            config.revisions,
            label::index(config.revisions, &e.label).expect("placed") + 1,
        ),
        None => label::first(config.revisions),
    });
    let proposed = list
        .iter()
        .rev()
        .find(|(e, _)| !official(e))
        .map(|(e, _)| (e.label.clone(), e.kind));
    let latest = list
        .iter()
        .rev()
        .find(|(e, _)| official(e) && e.kind == Kind::Release);
    let Some((latest, Record::Release(release))) = latest else {
        return State {
            status: Status::InWork,
            proposed,
            next,
            changes: Vec::new(),
        };
    };
    let obsoleted = list
        .iter()
        .any(|(e, _)| official(e) && e.kind == Kind::Obsolete && e.label == latest.label);
    if obsoleted {
        return State {
            status: Status::Obsolete(latest.label.clone()),
            proposed,
            next,
            changes: Vec::new(),
        };
    }
    let changes = match (crate::content::hash(src, part), props::read(src, part)) {
        (Ok(content), Ok(p)) => release.changes(&content, config, &p),
        // A part that can't be read isn't what was released.
        _ => vec!["content".to_owned()],
    };
    let status = if changes.is_empty() {
        Status::Released(latest.label.clone())
    } else {
        Status::Modified(latest.label.clone())
    };
    State {
        status,
        proposed,
        next,
        changes,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::history::Official;
    use crate::record::{BuiltWith, Obsolete, Release, ReleasedBy};
    use ryucad_doc::doc::canonical;
    use ryucad_doc::doc::id::ProjectId;
    use std::collections::BTreeMap;

    pub(crate) const PROJECT: &str = "0123456789abcdef0123456789abcdef";

    pub(crate) fn id(s: &str) -> Id {
        Id::parse(s, "part").unwrap()
    }

    pub(crate) fn by() -> ReleasedBy {
        ReleasedBy {
            name: None,
            email: None,
            participant: "agent".into(),
        }
    }

    /// A project with part `p` (one feature) and `number` 100.
    pub(crate) fn project() -> BTreeMap<String, Vec<u8>> {
        let mut f = BTreeMap::new();
        let mut put = |p: &str, v: Value| {
            f.insert(p.to_owned(), canonical::to_string(&v).into_bytes());
        };
        put(
            "ryucad.json",
            json!({"default_unit": "mm", "format": 2, "kind": "ryucad.project",
                   "project": PROJECT}),
        );
        put(
            "parts/p/part.json",
            json!({"features": ["a"], "format": 1, "kind": "ryucad.part"}),
        );
        put("parts/p/features/a.json", json!({"x": 1}));
        put(
            "parts/p/properties.json",
            json!({"format": 1, "kind": "ryupdm.properties", "number": "100", "name": "P"}),
        );
        f
    }

    /// The release of `p`'s current files as `label`.
    pub(crate) fn release_now(
        f: &BTreeMap<String, Vec<u8>>,
        label: &str,
        supersedes: Option<&str>,
    ) -> Record {
        let config = Config::read(f).unwrap();
        let props = props::read(f, &id("p")).unwrap();
        Record::Release(Release {
            project: ProjectId::parse(PROJECT).unwrap(),
            part: id("p"),
            label: label.into(),
            content: crate::content::hash(f, &id("p")).unwrap(),
            properties: crate::record::revising(&config, &props),
            results: BTreeMap::new(),
            built_with: BuiltWith {
                ryucad: "0".into(),
                ryukei: "0".into(),
                commit: "c".into(),
            },
            released_by: by(),
            supersedes: supersedes.map(str::to_owned),
            note: None,
        })
    }

    pub(crate) fn obsolete(label: &str) -> Record {
        Record::Obsolete(Obsolete {
            project: ProjectId::parse(PROJECT).unwrap(),
            part: id("p"),
            label: label.into(),
            released_by: by(),
            note: None,
        })
    }

    pub(crate) fn add(f: &mut BTreeMap<String, Vec<u8>>, r: &Record) {
        f.insert(r.path(), canonical::to_string(&r.to_json()).into_bytes());
    }

    /// History in which the records at `paths` are official.
    pub(crate) fn official(paths: &[&str]) -> Releases {
        Releases {
            official: paths
                .iter()
                .map(|p| {
                    (
                        (*p).to_owned(),
                        Official {
                            commit: "c0ffee".into(),
                            blob: gix::ObjectId::null(gix::hash::Kind::Sha1),
                            shallow: false,
                        },
                    )
                })
                .collect(),
            ..Releases::default()
        }
    }

    fn text(f: &BTreeMap<String, Vec<u8>>, r: &Releases) -> String {
        derive(f, &id("p"), r).text()
    }

    #[test]
    fn a_part_moves_from_work_to_released_to_modified() {
        let mut f = project();
        let none = Releases::default();
        let s = derive(&f, &id("p"), &none);
        assert_eq!((s.text(), s.next.as_deref()), ("in work".into(), Some("A")));
        let r = release_now(&f, "A", None);
        add(&mut f, &r);
        assert_eq!(text(&f, &none), "in work, A proposed");
        let a = official(&["releases/p/A.json"]);
        let s = derive(&f, &id("p"), &a);
        assert_eq!(
            s.to_json(),
            json!({"state": "released", "label": "A", "next": "B", "text": "released A"})
        );
        // A name isn't revising; the number is, and the model is.
        f.insert(
            "parts/p/properties.json".into(),
            canonical::to_string(
                &json!({"format": 1, "kind": "ryupdm.properties", "number": "100", "name": "Q"}),
            )
            .into_bytes(),
        );
        assert_eq!(text(&f, &a), "released A");
        f.insert("parts/p/features/a.json".into(), b"{\"x\": 2}".to_vec());
        let s = derive(&f, &id("p"), &a);
        assert_eq!(
            (s.text(), s.changes.clone()),
            ("A modified".into(), vec!["content".into()])
        );
        let r = release_now(&f, "B", Some("A"));
        add(&mut f, &r);
        assert_eq!(text(&f, &a), "A modified, B proposed");
        let ab = official(&["releases/p/A.json", "releases/p/B.json"]);
        assert_eq!(text(&f, &ab), "released B");
        assert_eq!(derive(&f, &id("p"), &ab).next.as_deref(), Some("C"));
    }

    #[test]
    fn obsolete_until_released_again() {
        let mut f = project();
        let r = release_now(&f, "A", None);
        add(&mut f, &r);
        add(&mut f, &obsolete("A"));
        let a = official(&["releases/p/A.json"]);
        assert_eq!(text(&f, &a), "released A, A obsolete proposed");
        let both = official(&["releases/p/A.json", "releases/p/A.obsolete.json"]);
        let s = derive(&f, &id("p"), &both);
        assert_eq!(
            (s.text(), s.next.as_deref()),
            ("A obsolete".into(), Some("B"))
        );
        let r = release_now(&f, "B", Some("A"));
        add(&mut f, &r);
        let all = official(&[
            "releases/p/A.json",
            "releases/p/A.obsolete.json",
            "releases/p/B.json",
        ]);
        assert_eq!(text(&f, &all), "released B");
    }

    #[test]
    fn a_record_that_cant_be_read_makes_it_unknown() {
        let mut f = project();
        let r = release_now(&f, "A", None);
        add(&mut f, &r);
        let mut newer = release_now(&f, "B", Some("A")).to_json();
        newer["format"] = json!(2);
        f.insert(
            "releases/p/B.json".into(),
            canonical::to_string(&newer).into_bytes(),
        );
        let s = derive(&f, &id("p"), &official(&["releases/p/A.json"]));
        assert_eq!(s.status.name(), "unknown");
        let Status::Unknown(e) = &s.status else {
            unreachable!()
        };
        assert_eq!(e.code, Code::FormatTooNew);
        assert_eq!(s.next, None);
        assert_eq!(s.to_json()["reason"]["code"], "format_too_new");
        // A label the scheme doesn't write can't be placed either.
        let mut f = project();
        let mut r = release_now(&f, "A", None);
        if let Record::Release(x) = &mut r {
            x.label = "1".into();
        }
        add(&mut f, &r);
        let s = derive(&f, &id("p"), &Releases::default());
        let Status::Unknown(e) = &s.status else {
            panic!("{s:?}")
        };
        assert_eq!(e.details["pointer"], "/label");
    }

    #[test]
    fn every_part_and_every_record_has_a_state() {
        let mut f = project();
        let mut q = obsolete("A");
        if let Record::Obsolete(o) = &mut q {
            o.part = id("gone");
        }
        add(&mut f, &q);
        let all = derive_all(&f, &Releases::default());
        let texts: Vec<(String, String)> =
            all.iter().map(|(p, s)| (p.to_string(), s.text())).collect();
        assert_eq!(
            texts,
            [
                ("gone".into(), "in work, A obsolete proposed".into()),
                ("p".into(), "in work".into())
            ]
        );
    }
}
