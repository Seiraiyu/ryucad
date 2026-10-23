//! Release records: `releases/<part>/<label>.json` (kind `ryupdm.release`)
//! says that a part's content, and its revising properties' values, are
//! revision `<label>`; `releases/<part>/<label>.obsolete.json` (kind
//! `ryupdm.obsolete`) that the revision is obsolete. Records are written
//! once and never changed: a record becomes official when it reaches the
//! release branch ([`crate::history`]).
//!
//! ```json
//! {
//!   "built_with": { "commit": "515f283…", "ryucad": "0.6.0", "ryukei": "0.6.0" },
//!   "content": "<64 hex: the content hash>",
//!   "format": 1,
//!   "kind": "ryupdm.release",
//!   "label": "B",
//!   "note": "Thicker flange for the M5 screws",
//!   "part": "bracket",
//!   "project": "<32 hex>",
//!   "properties": { "material": "6061-T6", "number": "100-0042", "uom": "each" },
//!   "released_by": { "email": "dave@example.com", "name": "David Stonely", "participant": "agent" },
//!   "results": { "base": "<body hash>", "flange": "<body hash>" },
//!   "supersedes": "A"
//! }
//! ```
//!
//! `approvals` and `attestations` are reserved for the format the service
//! will write (format 2): format 1 refuses them, as it refuses any key it
//! doesn't know, and this build reads a format 2 record as
//! `format_too_new`, never as a release it half understands.

use crate::config::Config;
use crate::props::Properties;
use ryucad_doc::doc::files::{Obj, header, invalid};
use ryucad_doc::doc::id::{Id, ProjectId};
use ryucad_doc::doc::project::RELEASES;
use ryucad_doc::doc::source::{self, Source, join};
use ryucad_doc::error::{Code, Error, Result};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

pub const RELEASE_KIND: &str = "ryupdm.release";
pub const OBSOLETE_KIND: &str = "ryupdm.obsolete";
pub const FORMAT: u64 = 1;

/// Keys of a release record that a later format adds, and format 1 refuses.
pub const RESERVED: [&str; 2] = ["approvals", "attestations"];

/// What a record says.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    Release,
    Obsolete,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Release => "release",
            Kind::Obsolete => "obsolete",
        }
    }

    fn file_kind(self) -> &'static str {
        match self {
            Kind::Release => RELEASE_KIND,
            Kind::Obsolete => OBSOLETE_KIND,
        }
    }

    fn suffix(self) -> &'static str {
        match self {
            Kind::Release => ".json",
            Kind::Obsolete => ".obsolete.json",
        }
    }
}

/// Where the record of `part`'s `label` lives, relative to the project.
pub fn path(part: &Id, label: &str, kind: Kind) -> String {
    join(
        &join(RELEASES, part.as_str()),
        &format!("{label}{}", kind.suffix()),
    )
}

/// The label and kind a record's file name gives, if it is a record's
/// name: `<label>.json` or `<label>.obsolete.json`, the label letters and
/// digits only.
pub fn parse_name(file: &str) -> Option<(String, Kind)> {
    let (label, kind) = match file.strip_suffix(Kind::Obsolete.suffix()) {
        Some(l) => (l, Kind::Obsolete),
        None => (file.strip_suffix(Kind::Release.suffix())?, Kind::Release),
    };
    (!label.is_empty() && label.bytes().all(|c| c.is_ascii_alphanumeric()))
        .then(|| (label.to_owned(), kind))
}

/// The ryucad build that rebuilt a release's results.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuiltWith {
    pub ryucad: String,
    pub ryukei: String,
    pub commit: String,
}

/// Who wrote a record: the Git identity, where Git's config has one, and
/// the ryucad participant that asked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReleasedBy {
    pub name: Option<String>,
    pub email: Option<String>,
    pub participant: String,
}

impl ReleasedBy {
    fn to_json(&self) -> Value {
        let mut v = json!({"participant": self.participant});
        if let Some(n) = &self.name {
            v["name"] = json!(n);
        }
        if let Some(e) = &self.email {
            v["email"] = json!(e);
        }
        v
    }

    fn from_json(v: &Value) -> Result<ReleasedBy> {
        let mut o = Obj::new(v, "/released_by")?;
        let participant = text(&o.take("participant")?, "/released_by/participant")?;
        let opt = |o: &mut Obj, key: &str| -> Result<Option<String>> {
            o.take_opt(key)
                .map(|v| text(&v, &format!("/released_by/{key}")))
                .transpose()
        };
        let name = opt(&mut o, "name")?;
        let email = opt(&mut o, "email")?;
        o.finish()?;
        Ok(ReleasedBy {
            name,
            email,
            participant,
        })
    }
}

/// A release record.
#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    pub project: ProjectId,
    pub part: Id,
    pub label: String,
    /// The part's content hash ([`crate::content`]).
    pub content: String,
    /// Every revising property's value, as declared when it was released.
    pub properties: BTreeMap<String, Value>,
    /// The body hash of each feature that makes a body, as the build in
    /// `built_with` rebuilt it: evidence, not identity.
    pub results: BTreeMap<String, String>,
    pub built_with: BuiltWith,
    pub released_by: ReleasedBy,
    /// The label before this one, `None` for a part's first release.
    pub supersedes: Option<String>,
    pub note: Option<String>,
}

/// An obsolete record: the part's revision `label` is obsolete.
#[derive(Clone, Debug, PartialEq)]
pub struct Obsolete {
    pub project: ProjectId,
    pub part: Id,
    pub label: String,
    pub released_by: ReleasedBy,
    pub note: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Record {
    Release(Release),
    Obsolete(Obsolete),
}

fn text(v: &Value, at: &str) -> Result<String> {
    match v.as_str() {
        Some(s) if !s.is_empty() => Ok(s.to_owned()),
        _ => Err(invalid(at, "expected a non-empty string")),
    }
}

fn hex(v: &Value, at: &str, len: usize) -> Result<String> {
    match v.as_str() {
        Some(s) if s.len() == len && s.bytes().all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f')) => {
            Ok(s.to_owned())
        }
        _ => Err(invalid(
            at,
            &format!("expected {len} lowercase hexadecimal digits"),
        )),
    }
}

impl Record {
    pub fn kind(&self) -> Kind {
        match self {
            Record::Release(_) => Kind::Release,
            Record::Obsolete(_) => Kind::Obsolete,
        }
    }

    pub fn project(&self) -> &ProjectId {
        match self {
            Record::Release(r) => &r.project,
            Record::Obsolete(o) => &o.project,
        }
    }

    pub fn part(&self) -> &Id {
        match self {
            Record::Release(r) => &r.part,
            Record::Obsolete(o) => &o.part,
        }
    }

    pub fn label(&self) -> &str {
        match self {
            Record::Release(r) => &r.label,
            Record::Obsolete(o) => &o.label,
        }
    }

    pub fn note(&self) -> Option<&str> {
        match self {
            Record::Release(r) => r.note.as_deref(),
            Record::Obsolete(o) => o.note.as_deref(),
        }
    }

    pub fn released_by(&self) -> &ReleasedBy {
        match self {
            Record::Release(r) => &r.released_by,
            Record::Obsolete(o) => &o.released_by,
        }
    }

    pub fn release(&self) -> Option<&Release> {
        match self {
            Record::Release(r) => Some(r),
            Record::Obsolete(_) => None,
        }
    }

    /// Where the record lives, relative to the project.
    pub fn path(&self) -> String {
        path(self.part(), self.label(), self.kind())
    }

    pub fn to_json(&self) -> Value {
        let mut v = json!({
            "format": FORMAT,
            "kind": self.kind().file_kind(),
            "label": self.label(),
            "part": self.part(),
            "project": self.project(),
            "released_by": self.released_by().to_json(),
        });
        if let Some(n) = self.note() {
            v["note"] = json!(n);
        }
        if let Record::Release(r) = self {
            let props: Map<String, Value> = r.properties.clone().into_iter().collect();
            v["built_with"] = json!({"commit": r.built_with.commit,
                                     "ryucad": r.built_with.ryucad,
                                     "ryukei": r.built_with.ryukei});
            v["content"] = json!(r.content);
            v["properties"] = Value::Object(props);
            v["results"] = json!(r.results);
            v["supersedes"] = json!(r.supersedes);
        }
        v
    }

    /// Reads a record of `kind`. Whether it belongs where it was found is
    /// [`Records::read`]'s to say.
    pub fn from_json(v: &Value, kind: Kind) -> Result<Record> {
        let mut o = header(v, kind.file_kind(), FORMAT)?;
        let project = o.take("project")?;
        let project = project
            .as_str()
            .and_then(|p| ProjectId::parse(p).ok())
            .ok_or_else(|| invalid("/project", "expected a project ID: 32 lowercase hex digits"))?;
        let part = o.take("part")?;
        let part = part
            .as_str()
            .and_then(|p| Id::parse(p, "part").ok())
            .ok_or_else(|| invalid("/part", "expected a part ID"))?;
        let label = o.take("label")?;
        let label = label
            .as_str()
            .filter(|l| parse_name(&format!("{l}.json")).is_some())
            .ok_or_else(|| invalid("/label", "expected a label of letters and digits"))?
            .to_owned();
        let released_by = ReleasedBy::from_json(&o.take("released_by")?)?;
        let note = o.take_opt("note").map(|n| text(&n, "/note")).transpose()?;
        if kind == Kind::Obsolete {
            o.finish()?;
            return Ok(Record::Obsolete(Obsolete {
                project,
                part,
                label,
                released_by,
                note,
            }));
        }
        let content = hex(&o.take("content")?, "/content", 64)?;
        let properties = o.take("properties")?;
        let properties: BTreeMap<String, Value> = properties
            .as_object()
            .ok_or_else(|| invalid("/properties", "expected an object of property values"))?
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let results = o.take("results")?;
        let results = results
            .as_object()
            .ok_or_else(|| invalid("/results", "expected an object of body hashes by feature"))?
            .iter()
            .map(|(k, v)| {
                let at = format!("/results/{k}");
                Id::parse(k, "feature").map_err(|_| invalid(&at, "expected a feature ID"))?;
                Ok((k.clone(), text(v, &at)?))
            })
            .collect::<Result<_>>()?;
        let built = o.take("built_with")?;
        let mut b = Obj::new(&built, "/built_with")?;
        let built_with = BuiltWith {
            commit: text(&b.take("commit")?, "/built_with/commit")?,
            ryucad: text(&b.take("ryucad")?, "/built_with/ryucad")?,
            ryukei: text(&b.take("ryukei")?, "/built_with/ryukei")?,
        };
        b.finish()?;
        let supersedes = match o.take("supersedes")? {
            Value::Null => None,
            s => Some(
                s.as_str()
                    .filter(|l| parse_name(&format!("{l}.json")).is_some())
                    .ok_or_else(|| invalid("/supersedes", "expected a label, or null"))?
                    .to_owned(),
            ),
        };
        o.finish()?;
        Ok(Record::Release(Release {
            project,
            part,
            label,
            content,
            properties,
            results,
            built_with,
            released_by,
            supersedes,
            note,
        }))
    }
}

/// The revising properties `props` sets under `config`: what a release
/// pins.
pub fn revising(config: &Config, props: &Properties) -> BTreeMap<String, Value> {
    props
        .values
        .iter()
        .filter(|(n, _)| config.decl(n).is_some_and(|d| d.revising))
        .map(|(n, v)| (n.clone(), v.clone()))
        .collect()
}

impl Release {
    /// What differs between this release and a part with `content` and
    /// `props`: `content` when the model does, then each property revising
    /// under `config` whose value isn't the one released (set, cleared or
    /// changed). Empty when the part is this release. A property that has
    /// stopped revising no longer counts; one that has started counts if
    /// it is set.
    pub fn changes(&self, content: &str, config: &Config, props: &Properties) -> Vec<String> {
        let mut out = Vec::new();
        if content != self.content {
            out.push("content".to_owned());
        }
        let now = revising(config, props);
        let names: std::collections::BTreeSet<&String> = now
            .keys()
            .chain(
                self.properties
                    .keys()
                    .filter(|n| config.decl(n).is_some_and(|d| d.revising)),
            )
            .collect();
        for n in names {
            if now.get(n) != self.properties.get(n) {
                out.push(n.clone());
            }
        }
        out
    }
}

/// One record file of a project, read.
#[derive(Clone, Debug)]
pub struct Entry {
    /// Relative to the project.
    pub path: String,
    pub part: Id,
    /// The label and kind its name gives.
    pub label: String,
    pub kind: Kind,
    /// The file's bytes, as history compares them.
    pub bytes: Vec<u8>,
    /// The record, or why it can't be used: not JSON, not a record, a
    /// newer format (`format_too_new`), or one that says another part or
    /// label than its name.
    pub record: Result<Record>,
}

/// Every record a project's files hold.
#[derive(Clone, Debug, Default)]
pub struct Records {
    /// By path.
    pub entries: Vec<Entry>,
}

impl Records {
    /// The records under `releases/`. Folders that aren't part IDs and
    /// files that aren't records' names are left out (doctor reports them).
    pub fn read(src: &dyn Source) -> Result<Records> {
        let mut entries = Vec::new();
        for (dir, kind) in src.list(RELEASES)? {
            let (Ok(part), true) = (Id::parse(&dir, "part"), kind == source::Kind::Dir) else {
                continue;
            };
            let at = join(RELEASES, &dir);
            for (file, kind) in src.list(&at)? {
                let (Some((label, rkind)), source::Kind::File) = (parse_name(&file), kind) else {
                    continue;
                };
                let path = join(&at, &file);
                let bytes = src
                    .read(&path)
                    .map_err(|e| e.with("path", &path))?
                    .unwrap_or_default();
                let record = read_one(src, &path, &part, &label, rkind);
                entries.push(Entry {
                    path,
                    part: part.clone(),
                    label,
                    kind: rkind,
                    bytes,
                    record,
                });
            }
        }
        Ok(Records { entries })
    }

    /// The records of `part`, in path order.
    pub fn of<'a>(&'a self, part: &'a Id) -> impl Iterator<Item = &'a Entry> + 'a {
        self.entries.iter().filter(move |e| &e.part == part)
    }

    /// The record at `path`, if there is one.
    pub fn at(&self, path: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.path == path)
    }

    /// Every part some record names, in order.
    pub fn parts(&self) -> Vec<Id> {
        let mut out: Vec<Id> = self.entries.iter().map(|e| e.part.clone()).collect();
        out.dedup();
        out
    }
}

/// The record at `path`, which its name says is `part`'s `label`.
fn read_one(src: &dyn Source, path: &str, part: &Id, label: &str, kind: Kind) -> Result<Record> {
    let v = crate::read_json(src, path)?.unwrap_or_default();
    let r = Record::from_json(&v, kind).map_err(|e| e.with("path", path))?;
    let wrong = |pointer: &str, what: &str| {
        Error::new(
            Code::InvalidDocument,
            format!("{path} names {what}, not the one its path gives"),
        )
        .with("path", path)
        .with("pointer", pointer)
    };
    if r.part() != part {
        return Err(wrong("/part", &format!("part {}", r.part())));
    }
    if r.label() != label {
        return Err(wrong("/label", &format!("label {}", r.label())));
    }
    Ok(r)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ryucad_doc::doc::canonical;

    fn id(s: &str) -> Id {
        Id::parse(s, "part").unwrap()
    }

    const PROJECT: &str = "0123456789abcdef0123456789abcdef";

    pub(crate) fn release(part: &str, label: &str, supersedes: Option<&str>) -> Record {
        Record::Release(Release {
            project: ProjectId::parse(PROJECT).unwrap(),
            part: id(part),
            label: label.to_owned(),
            content: "c".repeat(64),
            properties: [("number".to_owned(), json!("100-0042"))].into(),
            results: [("base".to_owned(), "h".repeat(64))].into(),
            built_with: BuiltWith {
                ryucad: "0.6.0".into(),
                ryukei: "0.6.0".into(),
                commit: "515f283".into(),
            },
            released_by: ReleasedBy {
                name: Some("Dave".into()),
                email: Some("dave@example.com".into()),
                participant: "agent".into(),
            },
            supersedes: supersedes.map(str::to_owned),
            note: Some("Thicker flange".into()),
        })
    }

    fn files(records: &[(&str, &Value)]) -> std::collections::BTreeMap<String, Vec<u8>> {
        records
            .iter()
            .map(|(p, v)| ((*p).to_owned(), canonical::to_string(v).into_bytes()))
            .collect()
    }

    #[test]
    fn records_round_trip() {
        let r = release("bracket", "B", Some("A"));
        let v = r.to_json();
        assert_eq!(v["kind"], "ryupdm.release");
        assert_eq!(v["supersedes"], "A");
        assert_eq!(Record::from_json(&v, Kind::Release).unwrap(), r);
        let first = release("bracket", "A", None);
        assert_eq!(first.to_json()["supersedes"], Value::Null);
        assert_eq!(
            Record::from_json(&first.to_json(), Kind::Release).unwrap(),
            first
        );
        let o = Record::Obsolete(Obsolete {
            project: ProjectId::parse(PROJECT).unwrap(),
            part: id("bracket"),
            label: "B".into(),
            released_by: ReleasedBy {
                name: None,
                email: None,
                participant: "agent".into(),
            },
            note: None,
        });
        assert_eq!(
            o.to_json(),
            json!({"format": 1, "kind": "ryupdm.obsolete", "label": "B", "part": "bracket",
                   "project": PROJECT, "released_by": {"participant": "agent"}})
        );
        assert_eq!(Record::from_json(&o.to_json(), Kind::Obsolete).unwrap(), o);
        assert_eq!(r.path(), "releases/bracket/B.json");
        assert_eq!(o.path(), "releases/bracket/B.obsolete.json");
    }

    #[test]
    fn names_give_label_and_kind() {
        assert_eq!(parse_name("B.json"), Some(("B".into(), Kind::Release)));
        assert_eq!(
            parse_name("02.obsolete.json"),
            Some(("02".into(), Kind::Obsolete))
        );
        for bad in [
            ".json",
            "notes.txt",
            "B.json.bak",
            "a-b.json",
            ".obsolete.json",
        ] {
            assert_eq!(parse_name(bad), None, "{bad}");
        }
    }

    #[test]
    fn faults_point_at_their_field() {
        let good = release("p", "A", None).to_json();
        let cases = [
            ("/project", json!({"project": "xyz"})),
            ("/part", json!({"part": "Not An Id"})),
            ("/label", json!({"label": "a-b"})),
            ("/content", json!({"content": "ABC"})),
            ("/properties", json!({"properties": []})),
            ("/results/X Y", json!({"results": {"X Y": "h"}})),
            ("/results/base", json!({"results": {"base": 1}})),
            (
                "/built_with/commit",
                json!({"built_with": {"ryucad": "1", "ryukei": "1"}}),
            ),
            ("/released_by/participant", json!({"released_by": {}})),
            ("/supersedes", json!({"supersedes": 1})),
            ("/note", json!({"note": ""})),
            ("/approvals", json!({"approvals": []})),
            ("/attestations", json!({"attestations": []})),
        ];
        for (pointer, change) in cases {
            let mut v = good.clone();
            for (k, x) in change.as_object().unwrap() {
                v[k] = x.clone();
            }
            let e = Record::from_json(&v, Kind::Release).unwrap_err();
            assert_eq!(
                (e.code, e.details["pointer"].as_str()),
                (Code::InvalidDocument, Some(pointer)),
                "{v}"
            );
        }
        let mut newer = good.clone();
        newer["format"] = json!(2);
        newer["approvals"] = json!([]);
        assert_eq!(
            Record::from_json(&newer, Kind::Release).unwrap_err().code,
            Code::FormatTooNew
        );
        // A release file holding an obsolete record is the wrong kind.
        assert_eq!(
            Record::from_json(&good, Kind::Obsolete)
                .unwrap_err()
                .details["pointer"],
            "/kind"
        );
    }

    #[test]
    fn a_project_records_are_read_where_they_lie() {
        let a = release("p", "A", None).to_json();
        let b = release("p", "B", Some("A")).to_json();
        let q = release("q", "A", None).to_json();
        let mut elsewhere = q.clone();
        elsewhere["part"] = json!("p");
        let mut src = files(&[
            ("releases/p/A.json", &a),
            ("releases/p/B.json", &b),
            ("releases/p/notes.txt", &json!("x")),
            ("releases/q/A.json", &elsewhere),
            ("releases/q/C.json", &q),
            ("releases/Bad Name/A.json", &a),
        ]);
        src.insert("releases/r/A.json".into(), b"{".to_vec());
        let rs = Records::read(&src).unwrap();
        let paths: Vec<&str> = rs.entries.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(
            paths,
            [
                "releases/p/A.json",
                "releases/p/B.json",
                "releases/q/A.json",
                "releases/q/C.json",
                "releases/r/A.json"
            ]
        );
        assert_eq!(rs.of(&id("p")).count(), 2);
        assert!(rs.entries[1].record.is_ok());
        let pointer =
            |i: usize| rs.entries[i].record.as_ref().unwrap_err().details["pointer"].clone();
        assert_eq!(pointer(2), "/part");
        assert_eq!(pointer(3), "/label");
        assert_eq!(
            rs.entries[4].record.as_ref().unwrap_err().code,
            Code::InvalidJson
        );
        assert_eq!(rs.parts(), [id("p"), id("q"), id("r")]);
        assert_eq!(rs.at("releases/p/B.json").unwrap().label, "B");
        assert!(Records::read(&files(&[])).unwrap().entries.is_empty());
    }

    #[test]
    fn a_release_differs_by_content_and_revising_properties() {
        use crate::config::Decl;
        let Record::Release(r) = release("p", "A", None) else {
            unreachable!()
        };
        let mut config = Config::default();
        let decl = |revising| Decl {
            ty: crate::config::Type::String,
            required: false,
            revising,
        };
        config.properties.insert("finish".into(), decl(true));
        config.properties.insert("supplier".into(), decl(false));
        let props = |pairs: &[(&str, &str)]| Properties {
            values: pairs
                .iter()
                .map(|(k, v)| ((*k).to_owned(), json!(v)))
                .collect(),
        };
        let c = "c".repeat(64);
        let same = props(&[
            ("number", "100-0042"),
            ("name", "Bracket"),
            ("supplier", "X"),
        ]);
        assert!(r.changes(&c, &config, &same).is_empty());
        assert_eq!(r.changes(&"d".repeat(64), &config, &same), ["content"]);
        let renumbered = props(&[("number", "100-0043")]);
        assert_eq!(r.changes(&c, &config, &renumbered), ["number"]);
        assert_eq!(
            r.changes(
                &c,
                &config,
                &props(&[("number", "100-0042"), ("finish", "bare")])
            ),
            ["finish"]
        );
        assert_eq!(r.changes(&c, &config, &props(&[])), ["number"]);
        // Once number stops revising, it no longer counts.
        let mut loose = config.clone();
        loose.properties.clear();
        assert_eq!(
            revising(&loose, &same),
            [("number".to_owned(), json!("100-0042"))].into()
        );
    }
}
