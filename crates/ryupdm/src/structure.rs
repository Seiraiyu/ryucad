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
use ryucad_doc::doc::id::{Id, ProjectId};
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
    lookup_in(&Records::read(src)?, part, label)
}

/// [`lookup`] in records already read.
pub fn lookup_in(records: &Records, part: &Id, label: &str) -> Result<Lookup> {
    let path = crate::record::path(part, label, Kind::Release);
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
    /// The other project it is a release of, `None` for the reading
    /// project's own. Its instances' revisions are as the reading project
    /// sees them ([`Rev::seen_from`]).
    pub project: Option<ProjectId>,
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

/// Nodes read at commits, by project (`None` for the reading project's
/// own), commit and part: a commit's files never change, so a session
/// keeps this across requests. A fork shares its upstream's commits, so
/// the project is part of the key.
pub type Cache = BTreeMap<(Option<ProjectId>, String, Id), Arc<Node>>;

/// Where a reader finds other projects' releases: the dependency cache
/// through the lock ([`crate::deps::read::Through`]), or files held in
/// memory ([`Fixed`]).
pub trait Others {
    /// `part`'s release `label` of `project`, read and checked: its files
    /// there (its own and its project's root files), the commit and
    /// whether it is obsolete.
    fn label(
        &mut self,
        project: &ProjectId,
        part: &Id,
        label: &str,
    ) -> Result<crate::deps::read::Label>;

    /// The commit `part`'s release `label` of `project` reads, when it is
    /// known without reading it (the lock says): a node already read
    /// there is used as it is.
    fn commit(&self, _project: &ProjectId, _part: &Id, _label: &str) -> Option<String> {
        None
    }

    /// Takes in what a lock read with an assembly's files at a commit
    /// holds: a release placing another project's part names the commit
    /// in the lock committed beside it, which the working files' lock may
    /// no longer hold.
    fn learn(&mut self, _lock: &ryucad_doc::doc::lock::Lock) {}
}

/// Other projects held in memory, each label of a project reading its
/// one set of files: for tests and fuzzing, with no repository.
#[derive(Clone, Debug, Default)]
pub struct Fixed(pub BTreeMap<ProjectId, BTreeMap<String, Vec<u8>>>);

impl Others for Fixed {
    fn commit(&self, project: &ProjectId, _part: &Id, _label: &str) -> Option<String> {
        self.0.contains_key(project).then(|| project.to_string())
    }

    fn label(
        &mut self,
        project: &ProjectId,
        part: &Id,
        label: &str,
    ) -> Result<crate::deps::read::Label> {
        let files = self.0.get(project).ok_or_else(|| {
            Error::new(
                Code::DependencyMissing,
                format!("{part}@{label} is of project {project}, which isn't held"),
            )
            .with("project", project)
            .with("part", part)
            .with("label", label)
        })?;
        Ok(crate::deps::read::Label {
            files: files.clone(),
            commit: format!("{project}"),
            content: String::new(),
            obsolete: false,
        })
    }
}

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
    /// Where other projects' releases are read; without one, a foreign
    /// instance is `dependency_not_fetched`.
    others: Option<&'a mut dyn Others>,
    /// What people call each other project: the manifest's dependency
    /// names, then those other projects' manifests give, first met.
    names: BTreeMap<ProjectId, String>,
    here: BTreeMap<Id, Arc<Node>>,
    /// The release records `src` holds, read at the first label.
    records: Option<Result<Records>>,
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
            others: None,
            names: project
                .manifest
                .dependencies
                .iter()
                .map(|(id, d)| (id.clone(), d.name.clone()))
                .collect(),
            here: BTreeMap::new(),
            records: None,
        }
    }

    /// The reader, reading other projects' releases from `others`.
    pub fn with_others(mut self, others: &'a mut dyn Others) -> Reader<'a> {
        self.others = Some(others);
        self
    }

    /// The reading project's own ID, if it has one.
    pub fn own(&self) -> Option<&ProjectId> {
        self.project.manifest.project.as_ref()
    }

    /// The name people know another project by: its dependency name in the
    /// reading project's manifest, else in the manifest of the first other
    /// project read that names it.
    pub fn name(&self, project: &ProjectId) -> Option<&str> {
        self.names.get(project).map(String::as_str)
    }

    /// `part` at `rev`; `not_found` when the part isn't there, a label's
    /// own error when it doesn't resolve.
    pub fn node(&mut self, part: &Id, rev: &Rev) -> Result<Arc<Node>> {
        match rev {
            Rev::Working => self.here(part, false),
            Rev::Label(label) => {
                let src = self.src;
                let records = self.records.get_or_insert_with(|| Records::read(src));
                let found = lookup_in(records.as_ref().map_err(Clone::clone)?, part, label)?;
                let pin = pin(self.src, &found, self.releases, part, label)?;
                match (pin.at, self.repo) {
                    (At::Commit(c), Some(repo)) => self.at(repo, &c, part, pin.obsolete),
                    _ => self.here(part, pin.obsolete),
                }
            }
            Rev::Foreign { project, label } if Some(project) == self.own() => {
                self.node(part, &Rev::Label(label.clone()))
            }
            Rev::Foreign { project, label } => self.foreign(project, part, label),
        }
    }

    /// `part` at `project`'s release `label`, through [`Others`].
    fn foreign(&mut self, project: &ProjectId, part: &Id, label: &str) -> Result<Arc<Node>> {
        let Some(others) = self.others.as_deref_mut() else {
            return Err(Error::new(
                Code::DependencyNotFetched,
                format!("{part} at label {label} of {project} isn't read across projects here"),
            )
            .with("project", project)
            .with("part", part)
            .with("label", label));
        };
        // Read once per commit: a commit's files never change.
        if let Some(c) = others.commit(project, part, label)
            && let Some(n) = self.cache.get(&(Some(project.clone()), c, part.clone()))
        {
            return Ok(n.clone());
        }
        let read = others.label(project, part, label)?;
        let key = (Some(project.clone()), read.commit.clone(), part.clone());
        if let Some(n) = self.cache.get(&key) {
            return Ok(flagged(n, read.obsolete));
        }
        let mut theirs = Project::load_from(Path::new(""), &read.files)?;
        for (id, d) in &theirs.manifest.dependencies {
            self.names
                .entry(id.clone())
                .or_insert_with(|| d.name.clone());
        }
        let mut p = theirs.parts.remove(part).ok_or_else(|| {
            Error::new(
                Code::NotFound,
                format!("{part} isn't in project {project} at {}", read.commit),
            )
            .with("project", project)
            .with("part", part)
            .with("commit", &read.commit)
        })?;
        let own = self.project.manifest.project.clone();
        for f in p.features.values_mut() {
            if let Ok(Feature::Instance(i)) = f {
                i.rev = i.rev.seen_from(project, own.as_ref());
            }
        }
        let node = Arc::new(Node {
            part: p,
            properties: crate::props::read(&read.files, part),
            commit: Some(read.commit),
            obsolete: read.obsolete,
            project: Some(project.clone()),
        });
        self.cache.insert(key, node.clone());
        Ok(node)
    }

    fn here(&mut self, part: &Id, obsolete: bool) -> Result<Arc<Node>> {
        if let Some(n) = self.here.get(part) {
            return Ok(flagged(n, obsolete));
        }
        let node = Arc::new(Node {
            part: self.project.part(part)?.clone(),
            properties: crate::props::read(self.src, part),
            commit: None,
            obsolete: false,
            project: None,
        });
        self.here.insert(part.clone(), node.clone());
        self.here(part, obsolete)
    }

    fn at(&mut self, repo: Repo, commit: &str, part: &Id, obsolete: bool) -> Result<Arc<Node>> {
        let key = (None, commit.to_owned(), part.clone());
        if let Some(n) = self.cache.get(&key) {
            return Ok(flagged(n, obsolete));
        }
        let files = crate::history::part_files(repo.git, commit, repo.prefix, part)?;
        let mut project = Project::load_from(Path::new(""), &files)?;
        if let (Some(others), Some(Ok(lock))) = (self.others.as_deref_mut(), &project.lock) {
            others.learn(lock);
        }
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
            project: None,
        });
        self.cache.insert(key, node);
        self.at(repo, commit, part, obsolete)
    }
}

/// `n` with its label's obsolete flag: shared as it is when that is how it
/// was read (almost always), else copied.
fn flagged(n: &Arc<Node>, obsolete: bool) -> Arc<Node> {
    if n.obsolete == obsolete {
        return n.clone();
    }
    Arc::new(Node {
        obsolete,
        ..(**n).clone()
    })
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
                if v.get("project").is_some() {
                    v["format"] = json!(6);
                } else if assembly {
                    v["format"] = json!(4);
                } else {
                    v["format"] = json!(1);
                }
                put(&format!("parts/{part}/features/{id}.json"), v);
            }
        }
        f
    }

    /// [`files`] of project `me`, listing `deps` (ID and name) as its
    /// dependencies.
    pub(crate) fn project_files(
        me: &ProjectId,
        deps: &[(&ProjectId, &str)],
        parts: &[(&str, &[(&str, serde_json::Value)])],
    ) -> BTreeMap<String, Vec<u8>> {
        use serde_json::json;
        let mut f = files(parts);
        let mut m = json!({"default_unit": "mm", "format": 2, "kind": "ryucad.project",
                           "project": me});
        if !deps.is_empty() {
            m["format"] = json!(3);
            m["dependencies"] = deps
                .iter()
                .map(|(id, name)| {
                    (
                        id.to_string(),
                        json!({"name": name, "path": "", "url": format!("file:///{name}")}),
                    )
                })
                .collect::<serde_json::Map<_, _>>()
                .into();
        }
        f.insert(
            "ryucad.json".into(),
            ryucad_doc::doc::canonical::to_string(&m).into_bytes(),
        );
        f
    }

    /// An instance of `project`'s `part` at its release `label`.
    pub(crate) fn foreign(project: &ProjectId, part: &str, label: &str) -> serde_json::Value {
        let mut v = instance(part, serde_json::json!({"label": label}));
        v["project"] = serde_json::json!(project);
        v
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

    /// A library (`[1; 16]`, named `lib`) whose `kit` places two of its own
    /// `bolt` at A and one of the reading project's (`[9; 16]`) `plate` at
    /// A, and the reading project's `asm` placing the kit at A.
    pub(crate) fn two_projects() -> (BTreeMap<String, Vec<u8>>, Fixed) {
        use serde_json::json;
        let (lib, me) = (
            ProjectId::from_bytes([1; 16]),
            ProjectId::from_bytes([9; 16]),
        );
        let theirs = project_files(
            &lib,
            &[(&me, "product")],
            &[
                ("bolt", &[("b", block())]),
                (
                    "kit",
                    &[
                        ("b1", instance("bolt", json!({"label": "A"}))),
                        ("b2", instance("bolt", json!({"label": "A"}))),
                        ("pl", foreign(&me, "plate", "A")),
                    ],
                ),
            ],
        );
        let ours = project_files(
            &me,
            &[(&lib, "lib")],
            &[
                ("plate", &[("b", block())]),
                ("asm", &[("k", foreign(&lib, "kit", "A"))]),
            ],
        );
        (ours, Fixed(BTreeMap::from([(lib, theirs)])))
    }

    #[test]
    fn a_reader_follows_other_projects_releases() {
        let (f, mut others) = two_projects();
        let lib = ProjectId::from_bytes([1; 16]);
        let project = Project::load_from(Path::new(""), &f).unwrap();
        let mut cache = Cache::new();
        let kit = Rev::Foreign {
            project: lib.clone(),
            label: "A".into(),
        };
        // Without other projects to read, a foreign instance can't be.
        let mut r = Reader::new(&f, &project, None, None, &mut cache);
        let e = r.node(&id("kit"), &kit).unwrap_err();
        assert_eq!(e.code, Code::DependencyNotFetched);
        let mut r = Reader::new(&f, &project, None, None, &mut cache).with_others(&mut others);
        assert_eq!(r.name(&lib), Some("lib"));
        let n = r.node(&id("kit"), &kit).unwrap();
        assert_eq!(n.project, Some(lib.clone()));
        assert_eq!(n.kind(), Some(PartKind::Assembly));
        // Its own labels are the library's; the reading project's are its own.
        let (occ, _) = occurrences(&n.part);
        let revs: Vec<&Rev> = occ.iter().map(|o| &o.rev).collect();
        assert_eq!(revs, [&kit, &kit, &Rev::Label("A".into())]);
        // A part it doesn't have.
        let e = r.node(&id("nut"), &kit).unwrap_err();
        assert_eq!(e.code, Code::NotFound);
        // Read once, kept by project, commit and part.
        drop(r);
        assert!(cache.contains_key(&(Some(lib.clone()), lib.to_string(), id("kit"))));
        // A project nothing holds.
        let mut none = Fixed::default();
        let mut r = Reader::new(&f, &project, None, None, &mut cache).with_others(&mut none);
        let other = Rev::Foreign {
            project: ProjectId::from_bytes([5; 16]),
            label: "A".into(),
        };
        let e = r.node(&id("kit"), &other).unwrap_err();
        assert_eq!(e.code, Code::DependencyMissing);
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
