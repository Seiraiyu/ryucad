//! Product structure: which parts an assembly places, at which
//! revisions and how many times, read from files alone. An instance names
//! a part at `working` or at a release label; a label resolves through its
//! record to the files at the commit where the record became official
//! ([`pin`]), the one rule ryucad's rebuild and the BOM share.

use crate::content;
use crate::history::Releases;
use crate::props::Properties;
use crate::record::{Kind, Records};
use ryucad_doc::doc::assembly::Rev;
use ryucad_doc::doc::files::{Feature, PartKind};
use ryucad_doc::doc::id::Id;
use ryucad_doc::doc::project::{Part, Project};
use ryucad_doc::doc::source::Source;
use ryucad_doc::error::{Code, Error, Result};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

/// Where a release label's files are.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum At {
    /// The commit where its record became official.
    Commit(String),
    /// The working files, which its proposed record describes.
    Working,
}

/// A release label resolved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pin {
    pub at: At,
    /// An obsolete record names the label too. It still resolves.
    pub obsolete: bool,
}

/// A label's record, read before the release branch is: what it released
/// and whether it is obsolete.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lookup {
    /// The record's path, relative to the project.
    pub path: String,
    /// The content hash it released.
    pub content: String,
    pub obsolete: bool,
}

/// The record of `part`'s release `label` in the project `src` holds:
/// `label_not_found` without one; a record that can't be read gives its
/// own error.
pub fn lookup(src: &dyn Source, part: &Id, label: &str) -> Result<Lookup> {
    let path = crate::record::path(part, label, Kind::Release);
    let records = Records::read(src)?;
    let Some(entry) = records.at(&path) else {
        return Err(Error::new(
            Code::LabelNotFound,
            format!("{part} has no release {label}: {path} doesn't exist"),
        )
        .with("part", part)
        .with("label", label));
    };
    let record = entry.record.clone()?;
    let content = record
        .release()
        .map(|r| r.content.clone())
        .unwrap_or_default();
    let obsolete = records
        .at(&crate::record::path(part, label, Kind::Obsolete))
        .is_some();
    Ok(Lookup {
        path,
        content,
        obsolete,
    })
}

/// Where `part`'s release `label` (its record `found`) is: the commit
/// where the release branch made it official (`releases`, `None` outside
/// a repository), else the working files `src` holds while the proposed
/// record's content hash describes them; `label_not_official` otherwise.
pub fn pin(
    src: &dyn Source,
    found: &Lookup,
    releases: Option<&Releases>,
    part: &Id,
    label: &str,
) -> Result<Pin> {
    if let Some(o) = releases.and_then(|r| r.official.get(&found.path)) {
        return Ok(Pin {
            at: At::Commit(o.commit.clone()),
            obsolete: found.obsolete,
        });
    }
    if content::hash(src, part).is_ok_and(|h| h == found.content) {
        return Ok(Pin {
            at: At::Working,
            obsolete: found.obsolete,
        });
    }
    Err(Error::new(
        Code::LabelNotOfficial,
        format!(
            "{part}'s release {label} isn't official yet, and {part} has changed since it was proposed"
        ),
    )
    .with("part", part)
    .with("label", label))
}

/// A part at a revision, its files read.
#[derive(Clone, Debug)]
pub struct Node {
    pub part: Part,
    /// Its properties as written; none when it has no file.
    pub properties: Result<Properties>,
    /// The commit its files were read at: `None` for the files the reader
    /// reads as they are (the working files, or one Git revision's).
    pub commit: Option<String>,
    /// Its label has an obsolete record too.
    pub obsolete: bool,
}

impl Node {
    /// What its part file says it is; `None` when the file can't be read.
    pub fn kind(&self) -> Option<PartKind> {
        self.part.file.as_ref().ok().map(|f| f.kind)
    }

    /// A property's value as written, when it is a string.
    pub fn text(&self, name: &str) -> Option<&str> {
        self.properties
            .as_ref()
            .ok()
            .and_then(|p| p.values.get(name))
            .and_then(|v| v.as_str())
    }
}

/// Nodes read at commits, by commit and part: a commit's files never
/// change, so a session keeps this across requests.
pub type Cache = BTreeMap<(String, Id), Arc<Node>>;

/// The repository a project lies in, and where in it.
#[derive(Clone, Copy)]
pub struct Repo<'a> {
    pub git: &'a gix::Repository,
    /// The project's folder in the repository, ending in `/` unless it is
    /// the root.
    pub prefix: &'a str,
}

/// Reads nodes: a part at `working` from the files `src` holds (loaded
/// into `project`), at a label from its official commit through `repo`
/// and `releases` (the release branch as read; `None` outside a
/// repository), or from `src` while its proposed record describes them.
pub struct Reader<'a> {
    pub src: &'a dyn Source,
    pub project: &'a Project,
    pub repo: Option<Repo<'a>>,
    pub releases: Option<&'a Releases>,
    pub cache: &'a mut Cache,
    here: BTreeMap<Id, Arc<Node>>,
}

impl<'a> Reader<'a> {
    pub fn new(
        src: &'a dyn Source,
        project: &'a Project,
        repo: Option<Repo<'a>>,
        releases: Option<&'a Releases>,
        cache: &'a mut Cache,
    ) -> Reader<'a> {
        Reader {
            src,
            project,
            repo,
            releases,
            cache,
            here: BTreeMap::new(),
        }
    }

    /// `part` at `rev`; `not_found` when the part isn't there, a label's
    /// own error when it doesn't resolve.
    pub fn node(&mut self, part: &Id, rev: &Rev) -> Result<Arc<Node>> {
        match rev {
            Rev::Working => self.here(part, false),
            Rev::Label(label) => {
                let found = lookup(self.src, part, label)?;
                let pin = pin(self.src, &found, self.releases, part, label)?;
                match (pin.at, self.repo) {
                    (At::Commit(c), Some(repo)) => self.at(repo, &c, part, pin.obsolete),
                    _ => self.here(part, pin.obsolete),
                }
            }
        }
    }

    fn here(&mut self, part: &Id, obsolete: bool) -> Result<Arc<Node>> {
        if let Some(n) = self.here.get(part) {
            return Ok(Arc::new(Node {
                obsolete,
                ..(**n).clone()
            }));
        }
        let node = Arc::new(Node {
            part: self.project.part(part)?.clone(),
            properties: crate::props::read(self.src, part),
            commit: None,
            obsolete: false,
        });
        self.here.insert(part.clone(), node.clone());
        self.here(part, obsolete)
    }

    fn at(&mut self, repo: Repo, commit: &str, part: &Id, obsolete: bool) -> Result<Arc<Node>> {
        let key = (commit.to_owned(), part.clone());
        if let Some(n) = self.cache.get(&key) {
            return Ok(Arc::new(Node {
                obsolete,
                ..(**n).clone()
            }));
        }
        let files = crate::history::part_files(repo.git, commit, repo.prefix, part)?;
        let mut project = Project::load_from(Path::new(""), &files)?;
        let p = project.parts.remove(part).ok_or_else(|| {
            Error::new(
                Code::NotFound,
                format!("{part} isn't in the project at {commit}"),
            )
            .with("part", part)
            .with("commit", commit)
        })?;
        let node = Arc::new(Node {
            part: p,
            properties: crate::props::read(&files, part),
            commit: Some(commit.to_owned()),
            obsolete: false,
        });
        self.cache.insert(key, node);
        self.at(repo, commit, part, obsolete)
    }
}

/// One use of a part in an assembly: an instance, or a copy an instance
/// pattern makes of one.
#[derive(Clone, Debug, PartialEq)]
pub struct Occurrence {
    /// The instance's ID, or a copy's path `<pattern>:<i>/<seed>`
    /// (`<pattern>:<i>:<j>/<seed>` past the first row).
    pub path: String,
    pub part: Id,
    pub rev: Rev,
}

/// What assembly `part` places, in list order: each instance, then each
/// instance pattern's copies of each of its seeds, the seed's own place
/// and every skipped copy left out. Features that can't be read (and a
/// pattern whose seed can't) are the second list's, with the feature's ID
/// in `feature`; mates place nothing.
pub fn occurrences(part: &Part) -> (Vec<Occurrence>, Vec<Error>) {
    let mut out = Vec::new();
    let mut problems = Vec::new();
    for fid in part.order() {
        match part.features.get(fid) {
            Some(Ok(Feature::Instance(i))) => out.push(Occurrence {
                path: fid.to_string(),
                part: i.part.clone(),
                rev: i.rev.clone(),
            }),
            Some(Ok(Feature::InstancePattern(p))) => {
                let [n, m] = p.form.counts();
                for seed in &p.instances {
                    let Some(Ok(Feature::Instance(i))) = part.features.get(seed) else {
                        problems.push(
                            Error::new(
                                Code::DependencyFailed,
                                format!("{fid} copies {seed}, which isn't an instance that reads"),
                            )
                            .with("feature", fid),
                        );
                        continue;
                    };
                    for j in 0..m {
                        for k in 0..n {
                            let index = [k, j];
                            if index == [0, 0] || p.skip.contains(&index) {
                                continue;
                            }
                            out.push(Occurrence {
                                path: format!("{}/{seed}", Id::copy(fid, index)),
                                part: i.part.clone(),
                                rev: i.rev.clone(),
                            });
                        }
                    }
                }
            }
            Some(Err(e)) => problems.push(e.clone().with("feature", fid)),
            _ => {}
        }
    }
    (out, problems)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::state::tests::{add, id, obsolete, official, project, release_now};

    #[test]
    fn a_label_pins_its_official_commit_else_the_working_files_it_describes() {
        let mut f = project();
        let e = lookup(&f, &id("p"), "A").unwrap_err();
        assert_eq!(e.code, Code::LabelNotFound);
        let a = release_now(&f, "A", None);
        add(&mut f, &a);
        let found = lookup(&f, &id("p"), "A").unwrap();
        assert_eq!(found.path, "releases/p/A.json");
        assert!(!found.obsolete);
        // Official: its commit, whatever the files say now.
        let r = official(&["releases/p/A.json"]);
        let p = pin(&f, &found, Some(&r), &id("p"), "A").unwrap();
        assert_eq!(p.at, At::Commit("c0ffee".into()));
        // Proposed: the working files while they are what it released.
        let p = pin(&f, &found, None, &id("p"), "A").unwrap();
        assert_eq!(p.at, At::Working);
        f.insert("parts/p/features/a.json".into(), b"{\"x\": 2}\n".to_vec());
        let e = pin(&f, &found, None, &id("p"), "A").unwrap_err();
        assert_eq!(e.code, Code::LabelNotOfficial);
        // Obsolete still resolves, and says so.
        add(&mut f, &obsolete("A"));
        let found = lookup(&f, &id("p"), "A").unwrap();
        assert!(found.obsolete);
        assert!(pin(&f, &found, Some(&r), &id("p"), "A").unwrap().obsolete);
    }

    /// Files for a project: `ryucad.json`, then each part's `part.json`
    /// (an assembly when it has instances) and features.
    pub(crate) fn files(
        parts: &[(&str, &[(&str, serde_json::Value)])],
    ) -> BTreeMap<String, Vec<u8>> {
        use ryucad_doc::doc::canonical;
        use serde_json::json;
        let mut f = BTreeMap::new();
        let mut put = |p: &str, v: serde_json::Value| {
            f.insert(p.to_owned(), canonical::to_string(&v).into_bytes());
        };
        put(
            "ryucad.json",
            json!({"default_unit": "mm", "format": 1, "kind": "ryucad.project"}),
        );
        for (part, features) in parts {
            let assembly = features
                .iter()
                .any(|(_, v)| v["type"] == "instance" || v["type"] == "instance_pattern");
            let ids: Vec<&str> = features.iter().map(|(id, _)| *id).collect();
            let mut list = json!({"features": ids, "format": 1, "kind": "ryucad.part"});
            if assembly {
                list["format"] = json!(2);
                list["part_kind"] = json!("assembly");
            }
            put(&format!("parts/{part}/part.json"), list);
            for (id, v) in *features {
                let mut v = v.clone();
                v["kind"] = json!("ryucad.feature");
                if assembly {
                    v["format"] = json!(4);
                } else {
                    v["format"] = json!(1);
                }
                put(&format!("parts/{part}/features/{id}.json"), v);
            }
        }
        f
    }

    pub(crate) fn instance(part: &str, rev: serde_json::Value) -> serde_json::Value {
        serde_json::json!({"type": "instance", "part": part, "rev": rev,
                           "pose": {"origin_m": [0, 0, 0], "z": [0, 0, 1], "x": [1, 0, 0]}})
    }

    pub(crate) fn block() -> serde_json::Value {
        serde_json::json!({"type": "box", "x": "1", "y": "1", "z": "1"})
    }

    #[test]
    fn instances_and_pattern_copies_are_occurrences() {
        use serde_json::json;
        let f = files(&[
            ("bolt", &[("b", block())]),
            ("plate", &[("b", block())]),
            (
                "frame",
                &[
                    ("base", instance("plate", json!("working"))),
                    ("b1", instance("bolt", json!("working"))),
                    ("broken", json!({"type": "instance"})),
                    (
                        "row",
                        json!({"type": "instance_pattern", "form": "linear", "instances": ["b1"],
                               "direction": "x", "spacing": "10 mm", "count": 4, "skip": [2]}),
                    ),
                ],
            ),
        ]);
        let project = Project::load_from(Path::new(""), &f).unwrap();
        let (occ, problems) = occurrences(project.part(&id("frame")).unwrap());
        let paths: Vec<&str> = occ.iter().map(|o| o.path.as_str()).collect();
        assert_eq!(paths, ["base", "b1", "row:1/b1", "row:3/b1"]);
        assert_eq!(occ[2].part, id("bolt"));
        assert_eq!(occ[2].rev, Rev::Working);
        assert_eq!(problems.len(), 1);
        assert_eq!(problems[0].details["feature"], "broken");
    }

    #[test]
    fn a_reader_reads_working_parts_and_labels_where_they_are() {
        let mut f = project();
        let a = release_now(&f, "A", None);
        add(&mut f, &a);
        let project = Project::load_from(Path::new(""), &f).unwrap();
        let mut cache = Cache::new();
        let mut r = Reader::new(&f, &project, None, None, &mut cache);
        let n = r.node(&id("p"), &Rev::Working).unwrap();
        assert_eq!(n.text("number"), Some("100"));
        assert_eq!(n.kind(), Some(PartKind::Part));
        assert!(n.commit.is_none());
        // Proposed and describing the files: read from them.
        let n = r.node(&id("p"), &Rev::Label("A".into())).unwrap();
        assert!(n.commit.is_none());
        let e = r.node(&id("p"), &Rev::Label("B".into())).unwrap_err();
        assert_eq!(e.code, Code::LabelNotFound);
        let e = r.node(&id("q"), &Rev::Working).unwrap_err();
        assert_eq!(e.code, Code::NotFound);
    }
}
