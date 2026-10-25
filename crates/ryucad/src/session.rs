//! A loaded project and its rebuilt parts, kept fresh against the files.
//!
//! The files are the truth. Before every request the session checks each
//! file it depends on: an unchanged size and modification time is trusted
//! unless the time is too recent to be reliable; otherwise the content is
//! read again. A real change reloads only the parts it lies in (all of
//! them when the manifest changed). The session keeps every file's bytes,
//! so the activity log's snapshots cost no reading. Rebuilds are cached by
//! part revision, so only parts whose files changed are rebuilt.
//!
//! A request may name a Git revision instead of the working files: the
//! session then answers from a frozen session over that revision's files,
//! one of the few it keeps, which starts from the working files' results,
//! so only what differs is rebuilt.

use crate::activity::Files;
use crate::doc::id::Id;
use crate::doc::project::Project;
use crate::error::{Error, Result};
use crate::rebuild::{Cache, PartResult, rebuild_part_with};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

/// A modification time this close to when it was read may be followed by
/// another write in the same timestamp tick, so it isn't trusted.
const RACY: Duration = Duration::from_secs(2);

/// How many revisions a session keeps loaded beside the working files.
pub const REVISIONS: usize = 4;

#[derive(Clone, Debug, PartialEq)]
struct Stamp {
    modified: Option<SystemTime>,
    len: u64,
    bytes: Arc<[u8]>,
}

pub struct Session {
    project: Project,
    stamps: BTreeMap<PathBuf, Stamp>,
    stamped_at: SystemTime,
    results: BTreeMap<Id, (String, PartResult)>,
    /// Each part's results by feature, keyed by their inputs' hash: a
    /// rebuild reuses every feature whose inputs haven't changed.
    cache: BTreeMap<Id, Cache>,
    /// What each name of a feature's body was, by part and feature, from
    /// every build this session has seen: its kind and, for a sweep, where
    /// it came from. Names are only added, so a reference whose name has
    /// gone can still say what it named.
    known: BTreeMap<(Id, Id), BTreeMap<String, Value>>,
    /// Display meshes of rebuilt bodies.
    pub meshes: crate::mesh::Cache,
    /// A revision's session: its files never change, and it never reads
    /// the disk.
    frozen: bool,
    /// The revisions requests have named, by the project folder's tree,
    /// most recently used first.
    revisions: Vec<(String, Session)>,
    /// The last merge status, by the key of what it was computed from.
    pub(crate) merge_status: Option<(String, serde_json::Value)>,
    /// The release branch as last read, by the key of the refs it was
    /// read from (see [`crate::release::history`]).
    pub(crate) releases: Option<(String, ryupdm::history::Releases)>,
    /// Bodies of released content as this build rebuilds them, by content
    /// hash, for `release_result_changed`.
    pub(crate) release_results: BTreeMap<String, BTreeMap<String, String>>,
}

impl Session {
    /// Loads the project at `root`.
    pub fn open(root: &Path) -> Result<Session> {
        let mut s = Session {
            project: Project::load(root)?,
            stamps: BTreeMap::new(),
            stamped_at: SystemTime::now(),
            results: BTreeMap::new(),
            cache: BTreeMap::new(),
            known: BTreeMap::new(),
            meshes: crate::mesh::Cache::default(),
            frozen: false,
            revisions: Vec::new(),
            merge_status: None,
            releases: None,
            release_results: BTreeMap::new(),
        };
        s.scan()?;
        Ok(s)
    }

    /// A session over `files`, the project as a revision has it, starting
    /// from what `from` has built and learnt.
    fn frozen(from: &Session, files: Files) -> Result<Session> {
        let root = from.root().to_path_buf();
        let project = Project::load_from(&root, &files)?;
        let stamps = files
            .into_iter()
            .map(|(rel, bytes)| {
                let path = rel.split('/').fold(root.clone(), |p, s| p.join(s));
                let stamp = Stamp {
                    modified: None,
                    len: bytes.len() as u64,
                    bytes: bytes.into(),
                };
                (path, stamp)
            })
            .collect();
        Ok(Session {
            project,
            stamps,
            stamped_at: SystemTime::now(),
            results: BTreeMap::new(),
            cache: from.cache.clone(),
            known: from.known.clone(),
            meshes: crate::mesh::Cache::default(),
            frozen: true,
            revisions: Vec::new(),
            merge_status: None,
            releases: None,
            release_results: BTreeMap::new(),
        })
    }

    /// The session to answer from for `rev`: this one for the working
    /// files (no revision, or `working`), else one over the project as that
    /// Git revision has it, with the commit it names.
    pub fn at(&mut self, rev: Option<&str>) -> Result<(Option<String>, &mut Session)> {
        let Some(rev) = rev.filter(|r| *r != "working") else {
            return Ok((None, self));
        };
        let repo = crate::git::repo(self.root())?;
        let (commit, tree) = repo.locate(rev)?;
        if tree.is_empty() {
            return Err(Error::new(
                crate::error::Code::NotFound,
                format!("{rev} has no project at {}", self.root().display()),
            )
            .with("rev", rev)
            .with("commit", &commit));
        }
        let i = match self.revisions.iter().position(|(t, _)| *t == tree) {
            Some(i) => i,
            None => {
                let s = Session::frozen(self, repo.files(&commit)?)?;
                self.revisions.push((tree, s));
                self.revisions.len() - 1
            }
        };
        let used = self.revisions.remove(i);
        self.revisions.insert(0, used);
        self.revisions.truncate(REVISIONS);
        Ok((Some(commit), &mut self.revisions[0].1))
    }

    pub fn root(&self) -> &Path {
        &self.project.root
    }

    pub fn project(&self) -> &Project {
        &self.project
    }

    /// Brings the session up to date with the files. Returns whether the
    /// project was reloaded.
    pub fn refresh(&mut self) -> Result<bool> {
        Ok(!self.refresh_changes()?.is_empty())
    }

    /// [`Session::refresh`], returning the files that changed (relative,
    /// with `/`) and how: `created`, `modified` or `deleted`.
    pub fn refresh_changes(&mut self) -> Result<Vec<(String, &'static str)>> {
        if self.frozen {
            return Ok(Vec::new());
        }
        let changes = self.scan()?;
        if !changes.is_empty() {
            let paths: Vec<String> = changes.iter().map(|(p, _)| p.clone()).collect();
            self.project.reload(&paths)?;
            let live = &self.project.parts;
            self.results
                .retain(|id, (rev, _)| live.get(id).is_some_and(|p| &p.revision == rev));
            self.cache.retain(|id, _| live.contains_key(id));
        }
        Ok(changes)
    }

    /// Takes in what this session just wrote. Every write goes through here
    /// before it answers, so the project, the stamps and the snapshot
    /// agree with the files.
    pub fn reload(&mut self) -> Result<()> {
        self.refresh_changes().map(drop)
    }

    /// Every file the project's state depends on, as last read: what the
    /// activity log keeps of a change.
    pub fn snapshot(&self) -> Files {
        let root = self.root();
        self.stamps
            .iter()
            .map(|(p, s)| (crate::doc::project::relative(root, p), s.bytes.to_vec()))
            .collect()
    }

    /// A hash of every file the project's state depends on, as last read.
    pub fn digest(&self) -> String {
        let mut h = blake3::Hasher::new();
        for (path, s) in &self.stamps {
            let p = path.to_string_lossy();
            h.update(&(p.len() as u64).to_le_bytes());
            h.update(p.as_bytes());
            h.update(&(s.bytes.len() as u64).to_le_bytes());
            h.update(&s.bytes);
        }
        h.finalize().to_hex().to_string()
    }

    /// The rebuild of `part`, from the cache when its files haven't changed.
    pub fn rebuilt(&mut self, part: &Id) -> Result<&PartResult> {
        let p = self.project.part(part)?;
        let fresh = self
            .results
            .get(part)
            .is_some_and(|(rev, _)| *rev == p.revision);
        if !fresh {
            let empty = Cache::new();
            let (result, cache) = rebuild_part_with(p, self.cache.get(part).unwrap_or(&empty));
            self.cache.insert(part.clone(), cache);
            let revision = p.revision.clone();
            self.learn(part, &result);
            self.results.insert(part.clone(), (revision, result));
            // Explained against this rebuild, not the one before: which
            // feature took a name away is read from these results.
            let explained: Vec<(usize, Error)> = self.results[part]
                .1
                .features
                .iter()
                .enumerate()
                .filter_map(|(i, (_, r))| Some((i, self.explain(part, r.as_ref().err()?.clone()))))
                .collect();
            let result = &mut self.results.get_mut(part).expect("just put").1;
            for (i, e) in explained {
                result.features[i].1 = Err(e);
            }
        }
        Ok(&self.results[part].1)
    }

    /// The cached rebuild of `part`, to make it wrong in tests.
    #[cfg(test)]
    pub(crate) fn cached_mut(&mut self, part: &Id) -> Option<&mut PartResult> {
        self.results.get_mut(part).map(|(_, r)| r)
    }

    /// Adds to `reference_unresolved` what its name was when it last
    /// resolved (`was`: `{kind, from}`), if this session saw it, and
    /// which feature of its solid's chain took it away (`removed_by`):
    /// one that dropped it after the feature before it had it, or the
    /// feature that made it (`was.from.feature`) when that one no longer
    /// does (its sketch drawn again with new IDs).
    pub fn explain(&self, part: &Id, e: Error) -> Error {
        let order = self
            .project
            .parts
            .get(part)
            .map(|p| p.order().to_vec())
            .unwrap_or_default();
        self.explain_in(part, &order, e)
    }

    /// [`Session::explain`] for the part's list as `order` has it: as a
    /// write would leave it.
    pub fn explain_in(&self, part: &Id, order: &[Id], mut e: Error) -> Error {
        if e.code != crate::error::Code::ReferenceUnresolved {
            return e;
        }
        let dependency = e.details["dependency"]
            .as_str()
            .and_then(|d| Id::parse(d, "feature").ok());
        let name = e.details["reference"]
            .as_str()
            .and_then(|r| r.rsplit('/').next())
            .map(str::to_owned);
        let name = name.as_deref();
        let was = dependency
            .as_ref()
            .zip(name)
            .and_then(|(d, n)| self.known.get(&(part.clone(), d.clone()))?.get(n));
        if let Some(w) = was {
            e.details["was"] = w.clone();
        }
        let reader = e.details["feature"]
            .as_str()
            .and_then(|f| Id::parse(f, "feature").ok());
        let maker = was
            .and_then(|w| w["from"]["feature"].as_str())
            .and_then(|f| Id::parse(f, "feature").ok());
        let removed = dependency
            .zip(name.and_then(crate::doc::reference::parse_name))
            .zip(reader)
            .and_then(|((solid, name), reader)| {
                self.removed_by(part, order, &solid, name, &reader, maker.as_ref())
            });
        if let Some(by) = removed {
            e.details["removed_by"] = serde_json::json!(by);
        }
        e
    }

    /// The feature of `solid`'s chain, before `reader`, that left the
    /// solid without `name` after the one before it had it, or `maker`
    /// itself when its own result lacks it, in the cached rebuild.
    fn removed_by(
        &self,
        part: &Id,
        order: &[Id],
        solid: &Id,
        name: ryukei::name::Name,
        reader: &Id,
        maker: Option<&Id>,
    ) -> Option<Id> {
        let p = self.project.parts.get(part)?;
        let result = &self.results.get(part)?.1;
        let solids = crate::solid::Solids::of_order(p, order);
        let last = solids.view(solid, reader)?;
        let chain = solids.chain(solid)?;
        let has = |f: &Id| {
            result
                .output(f)
                .and_then(|o| o.solid(solid))
                .map(|b| b.body.by_name(name).is_some())
        };
        let mut had = false;
        for f in chain {
            match has(f) {
                Some(true) => had = true,
                Some(false) if had || maker == Some(f) => return Some(f.clone()),
                _ => {}
            }
            if f == last {
                break;
            }
        }
        None
    }

    /// Notes what every name of the part's solids is, by solid.
    fn learn(&mut self, part: &Id, result: &PartResult) {
        let solids = result
            .features
            .iter()
            .filter_map(|(_, r)| r.as_ref().ok())
            .flat_map(|o| o.solids());
        for (solid, b) in solids {
            let known = self.known.entry((part.clone(), solid.clone())).or_default();
            let body = &b.body;
            let names = body
                .faces()
                .map(|f| (body.face(f).expect("live").name, "face"))
                .chain(
                    body.edges()
                        .map(|e| (body.edge(e).expect("live").name, "edge")),
                )
                .chain(
                    body.vertices()
                        .map(|v| (body.vertex(v).expect("live").name, "vertex")),
                );
            for (name, kind) in names {
                let name = name.to_string();
                let mut was = json!({ "kind": kind });
                if let Some(from) = b.origins.get(&name) {
                    was["from"] = from.clone();
                }
                known.insert(name, was);
            }
        }
    }

    /// Checks every file against its stamp, reading only those whose size
    /// or time moved or whose time is too recent to trust. Returns what
    /// changed, sorted.
    fn scan(&mut self) -> Result<Vec<(String, &'static str)>> {
        let files = listed(self.root())?;
        let trusted_before = self.stamped_at.checked_sub(RACY);
        let mut changes = Vec::new();
        let mut stamps = BTreeMap::new();
        for (path, meta) in files {
            let (modified, len) = (meta.modified().ok(), meta.len());
            let old = self.stamps.get(&path);
            let trusted = match (old, modified, trusted_before) {
                (Some(o), Some(m), Some(t)) => o.modified == Some(m) && o.len == len && m < t,
                _ => false,
            };
            let stamp = match old {
                Some(o) if trusted => o.clone(),
                _ => {
                    let bytes = fs::read(&path).map_err(|e| Error::io(&path, &e))?;
                    Stamp {
                        modified,
                        len,
                        bytes: bytes.into(),
                    }
                }
            };
            match old {
                None => changes.push((path.clone(), "created")),
                Some(o) if o.bytes != stamp.bytes => changes.push((path.clone(), "modified")),
                Some(_) => {}
            }
            stamps.insert(path, stamp);
        }
        for path in self.stamps.keys().filter(|p| !stamps.contains_key(*p)) {
            changes.push((path.clone(), "deleted"));
        }
        self.stamped_at = SystemTime::now();
        self.stamps = stamps;
        let root = self.project.root.clone();
        let mut changes: Vec<(String, &'static str)> = changes
            .into_iter()
            .map(|(p, c)| (crate::doc::project::relative(&root, &p), c))
            .collect();
        changes.sort();
        Ok(changes)
    }
}

/// The files as last read, without copying them: what the session reads
/// on its own while polling (release states, for `pdm.changed`).
impl crate::doc::source::Source for Session {
    fn list(&self, dir: &str) -> Result<Vec<(String, crate::doc::source::Kind)>> {
        use crate::doc::source::Kind;
        let at = dir
            .split('/')
            .filter(|s| !s.is_empty())
            .fold(self.root().to_path_buf(), |p, s| p.join(s));
        let mut out: Vec<(String, Kind)> = Vec::new();
        for path in self.stamps.range(at.clone()..).map(|(p, _)| p) {
            let Ok(rest) = path.strip_prefix(&at) else {
                break;
            };
            let mut parts = rest.components();
            let Some(name) = parts.next() else {
                continue;
            };
            let kind = if parts.next().is_some() {
                Kind::Dir
            } else {
                Kind::File
            };
            let entry = (name.as_os_str().to_string_lossy().into_owned(), kind);
            if !out.contains(&entry) {
                out.push(entry);
            }
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(out)
    }

    fn read(&self, rel: &str) -> Result<Option<Vec<u8>>> {
        let path = rel
            .split('/')
            .filter(|s| !s.is_empty())
            .fold(self.root().to_path_buf(), |p, s| p.join(s));
        Ok(self.stamps.get(&path).map(|s| s.bytes.to_vec()))
    }
}

/// The files a project's state depends on: the manifest, the attributes
/// that register ryucad with Git, ryupdm's settings, everything under
/// `parts/` two and three levels down, and ryupdm's release records, two
/// levels under `releases/`.
pub fn files(root: &Path) -> Result<Vec<PathBuf>> {
    Ok(listed(root)?.into_iter().map(|(p, _)| p).collect())
}

/// [`files`], each with its metadata. A directory listing says what each
/// entry is (and on Windows gives its size and time) without opening it,
/// so checking a project costs at most one `stat` per file.
fn listed(root: &Path) -> Result<Vec<(PathBuf, fs::Metadata)>> {
    let mut out = Vec::new();
    for name in crate::doc::project::ROOT_FILES {
        let path = root.join(name);
        if let Ok(m) = fs::metadata(&path)
            && m.is_file()
        {
            out.push((path, m));
        }
    }
    for (part, m) in read_dir(&root.join("parts"))? {
        if m.is_dir() {
            for (entry, m) in read_dir(&part)? {
                if m.is_dir() {
                    out.extend(read_dir(&entry)?.into_iter().filter(|(_, m)| m.is_file()));
                } else if m.is_file() {
                    out.push((entry, m));
                }
            }
        }
    }
    let releases = root.join(crate::doc::project::RELEASES);
    for (part, m) in read_dir(&releases)? {
        if m.is_dir() {
            out.extend(read_dir(&part)?.into_iter().filter(|(_, m)| m.is_file()));
        }
    }
    Ok(out)
}

/// Each entry in `dir` with its metadata, following symlinks; empty when
/// `dir` doesn't exist. An entry gone by the time it is looked at is left
/// out.
fn read_dir(dir: &Path) -> Result<Vec<(PathBuf, fs::Metadata)>> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(Error::io(dir, &e)),
    };
    let mut out = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| Error::io(dir, &e))?;
        let path = entry.path();
        let meta = match entry.file_type() {
            Ok(t) if t.is_symlink() => fs::metadata(&path),
            Ok(_) => entry.metadata(),
            Err(e) => Err(e),
        };
        match meta {
            Ok(m) => out.push((path, m)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(Error::io(&path, &e)),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::files::{Feature, Manifest, PartFile, Placement, Primitive, PrimitiveType};
    use crate::doc::project::{MANIFEST, feature_path, part_path, write_json};
    use crate::error::Code;
    use crate::rebuild::rebuild_part;
    use crate::units::{Quantity, Unit};

    fn id(s: &str) -> Id {
        Id::parse(s, "feature").unwrap()
    }

    fn sphere(r: &str) -> serde_json::Value {
        Feature::Primitive(Primitive {
            ty: PrimitiveType::Sphere,
            params: [("radius".to_owned(), Quantity::parse(r, Unit::Mm).unwrap())].into(),
            placement: Placement::identity(Unit::Mm),
            combine: crate::doc::solid::Combine::NEW,
        })
        .to_json()
    }

    fn project() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write_json(&root.join(MANIFEST), &Manifest::new(Unit::Mm).to_json()).unwrap();
        write_json(
            &part_path(root, &id("p")),
            &PartFile {
                features: vec![id("a")],
            }
            .to_json(),
        )
        .unwrap();
        write_json(&feature_path(root, &id("p"), &id("a")), &sphere("1 mm")).unwrap();
        dir
    }

    fn radius(s: &mut Session) -> f64 {
        let p = s.project().part(&id("p")).unwrap();
        p.features[&id("a")]
            .as_ref()
            .unwrap()
            .primitive()
            .unwrap()
            .meters("radius")
    }

    #[test]
    fn sees_external_edits_even_within_the_same_tick() {
        let dir = project();
        let mut s = Session::open(dir.path()).unwrap();
        assert!(!s.refresh().unwrap());
        // Same size, written immediately: only the content hash can tell.
        write_json(
            &feature_path(dir.path(), &id("p"), &id("a")),
            &sphere("2 mm"),
        )
        .unwrap();
        assert!(s.refresh().unwrap());
        assert_eq!(radius(&mut s), 0.002);
        assert!(!s.refresh().unwrap());
    }

    #[test]
    fn says_which_files_changed() {
        let dir = project();
        let mut s = Session::open(dir.path()).unwrap();
        write_json(
            &feature_path(dir.path(), &id("p"), &id("a")),
            &sphere("2 mm"),
        )
        .unwrap();
        write_json(
            &feature_path(dir.path(), &id("p"), &id("b")),
            &sphere("1 mm"),
        )
        .unwrap();
        fs::remove_file(dir.path().join("parts/p/part.json")).unwrap();
        fs::write(dir.path().join("ryupdm.json"), "{}").unwrap();
        fs::write(dir.path().join("parts/p/properties.json"), "{}").unwrap();
        // Release records, two levels down; nothing else under releases/.
        fs::create_dir_all(dir.path().join("releases/p/deeper")).unwrap();
        fs::write(dir.path().join("releases/p/A.json"), "{}").unwrap();
        fs::write(dir.path().join("releases/p/deeper/x.json"), "{}").unwrap();
        fs::write(dir.path().join("releases/top.json"), "{}").unwrap();
        assert_eq!(
            s.refresh_changes().unwrap(),
            [
                ("parts/p/features/a.json".to_owned(), "modified"),
                ("parts/p/features/b.json".to_owned(), "created"),
                ("parts/p/part.json".to_owned(), "deleted"),
                ("parts/p/properties.json".to_owned(), "created"),
                ("releases/p/A.json".to_owned(), "created"),
                ("ryupdm.json".to_owned(), "created"),
            ]
        );
        assert!(s.snapshot().contains_key("releases/p/A.json"));
        // Read in place, the session lists and reads as its snapshot does.
        use crate::doc::source::Source;
        let snap = s.snapshot();
        for dir in ["", "parts", "parts/p", "releases", "releases/p", "nowhere"] {
            assert_eq!(s.list(dir).unwrap(), snap.list(dir).unwrap(), "{dir}");
        }
        assert_eq!(s.read("releases/p/A.json").unwrap(), Some(b"{}".to_vec()));
        assert_eq!(s.read("parts/p/part.json").unwrap(), None);
    }

    #[test]
    fn touching_without_changing_reloads_nothing() {
        let dir = project();
        let mut s = Session::open(dir.path()).unwrap();
        let path = feature_path(dir.path(), &id("p"), &id("a"));
        let text = fs::read(&path).unwrap();
        fs::write(&path, &text).unwrap();
        assert!(!s.refresh().unwrap());
    }

    #[test]
    fn sees_files_added_and_removed() {
        let dir = project();
        let mut s = Session::open(dir.path()).unwrap();
        write_json(
            &part_path(dir.path(), &id("q")),
            &PartFile { features: vec![] }.to_json(),
        )
        .unwrap();
        assert!(s.refresh().unwrap());
        assert!(s.project().parts.contains_key(&id("q")));
        fs::remove_dir_all(dir.path().join("parts/q")).unwrap();
        assert!(s.refresh().unwrap());
        assert!(!s.project().parts.contains_key(&id("q")));
    }

    #[test]
    fn rebuilds_are_cached_by_revision() {
        let dir = project();
        let mut s = Session::open(dir.path()).unwrap();
        let h1 = s.rebuilt(&id("p")).unwrap().hash.clone();
        let ptr1 = s.rebuilt(&id("p")).unwrap() as *const PartResult;
        let ptr2 = s.rebuilt(&id("p")).unwrap() as *const PartResult;
        assert_eq!(ptr1, ptr2);
        write_json(
            &feature_path(dir.path(), &id("p"), &id("a")),
            &sphere("3 mm"),
        )
        .unwrap();
        s.refresh().unwrap();
        assert_ne!(s.rebuilt(&id("p")).unwrap().hash, h1);
    }

    #[test]
    fn only_what_a_change_touches_is_rebuilt() {
        use crate::server::tests::{call, fresh};
        use serde_json::json;
        let (_dir, mut s) = fresh();
        call(&mut s, "part.create", json!({"part": "p"})).unwrap();
        call(
            &mut s,
            "param.set",
            json!({"part": "p", "parameters": {"r": "2 mm", "w": "20 mm"}}),
        )
        .unwrap();
        for f in [
            json!({"type": "box", "id": "b", "params": {"x": "w", "y": 10, "z": 5}}),
            json!({"type": "sphere", "id": "c", "params": {"radius": "r"}}),
            json!({"type": "sketch", "id": "s"}),
        ] {
            let mut f = f;
            f["part"] = json!("p");
            call(&mut s, "feature.add", f).unwrap();
        }
        call(
            &mut s,
            "sketch.edit",
            json!({"part": "p", "sketch": "s", "ops": crate::sketch::tests::rectangle_ops()}),
        )
        .unwrap();
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "extrude", "id": "e", "sketch": "s",
                   "extent": {"length": "w"}}),
        )
        .unwrap();
        let built = |s: &mut Session| -> Vec<String> {
            let full = rebuild_part(s.project().part(&id("p")).unwrap()).hash;
            let r = s.rebuilt(&id("p")).unwrap();
            // Reusing changes nothing.
            assert_eq!(r.hash, full);
            r.built.iter().map(|i| i.to_string()).collect()
        };
        built(&mut s);
        let edit = |s: &mut Session, method: &str, params: serde_json::Value| {
            let mut params = params;
            params["part"] = json!("p");
            call(s, method, params).unwrap();
            built(s)
        };
        assert_eq!(
            edit(
                &mut s,
                "feature.edit",
                json!({"id": "c", "params": {"radius": 3}})
            ),
            ["c"]
        );
        // w is read by the box and the extrude, not the sketch.
        assert_eq!(
            edit(&mut s, "param.set", json!({"parameters": {"w": "25 mm"}})),
            ["b", "e"]
        );
        // The sketch changes, and so does the extrude made from it.
        assert_eq!(
            edit(
                &mut s,
                "sketch.edit",
                json!({"sketch": "s", "ops": [{"op": "set", "id": "width", "value": "30 mm"}]}),
            ),
            ["s", "e"]
        );
        // Nothing changes: nothing is built.
        assert_eq!(
            edit(&mut s, "param.set", json!({"parameters": {"r": "3 mm"}})),
            Vec::<String>::new()
        );
    }

    #[test]
    fn a_change_in_one_part_leaves_the_others() {
        let dir = project();
        let root = dir.path();
        write_json(
            &part_path(root, &id("q")),
            &PartFile {
                features: vec![id("a")],
            }
            .to_json(),
        )
        .unwrap();
        write_json(&feature_path(root, &id("q"), &id("a")), &sphere("1 mm")).unwrap();
        let mut s = Session::open(root).unwrap();
        let p = s.rebuilt(&id("p")).unwrap() as *const PartResult;
        write_json(&feature_path(root, &id("q"), &id("a")), &sphere("4 mm")).unwrap();
        assert!(s.refresh().unwrap());
        assert_eq!(s.rebuilt(&id("p")).unwrap() as *const PartResult, p);
        let fresh = Project::load(root).unwrap();
        assert_eq!(format!("{:?}", s.project()), format!("{fresh:?}"));
        // What the session wrote itself is in its snapshot once it reloads.
        write_json(&feature_path(root, &id("p"), &id("a")), &sphere("5 mm")).unwrap();
        s.reload().unwrap();
        assert_eq!(
            s.snapshot()["parts/p/features/a.json"],
            fs::read(feature_path(root, &id("p"), &id("a"))).unwrap()
        );
        assert_eq!(s.snapshot(), crate::activity::snapshot(root).unwrap());
    }

    #[test]
    fn any_revision_rebuilds_from_its_own_files() {
        use crate::git::tests::git;
        use crate::server::tests::call;
        use serde_json::json;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        git(&root, &["init", "-q", "-b", "main"]);
        crate::server::init(&root, None).unwrap();
        let mut s = Session::open(&root).unwrap();
        call(&mut s, "part.create", json!({"part": "p"})).unwrap();
        let sphere = |s: &mut Session, r: u32| {
            call(
                s,
                "feature.add",
                json!({"part": "p", "type": "sphere", "id": format!("s{r}"), "params": {"radius": r}}),
            )
            .unwrap();
        };
        let mut hashes = Vec::new();
        for r in 1..=6 {
            sphere(&mut s, r);
            git(&root, &["add", "."]);
            git(&root, &["commit", "-q", "-m", &format!("sphere {r}")]);
            hashes.push(
                call(&mut s, "rebuild", json!({"part": "p"})).unwrap()["parts"][0]["hash"].clone(),
            );
        }
        // Uncommitted: one more.
        sphere(&mut s, 7);
        let at = |s: &mut Session, rev: &str| call(s, "rebuild", json!({"part": "p", "rev": rev}));
        let head = at(&mut s, "HEAD~2").unwrap();
        assert_eq!(head["parts"][0]["hash"], hashes[3]);
        let commit = head["commit"].as_str().unwrap().to_owned();
        assert_eq!(commit.len(), 40);
        assert_eq!(at(&mut s, &commit).unwrap()["parts"][0]["hash"], hashes[3]);
        assert_eq!(
            at(&mut s, "working").unwrap(),
            call(&mut s, "rebuild", json!({"part": "p"})).unwrap()
        );
        // A revision starts from the working files' results: HEAD differs
        // from them only by the uncommitted sphere, so nothing is built.
        let (_, h) = s.at(Some("HEAD")).unwrap();
        assert!(h.rebuilt(&id("p")).unwrap().built.is_empty());
        // Inspect and meshes read the revision too; s5 didn't exist then.
        let v = call(&mut s, "inspect", json!({"part": "p", "rev": "HEAD~3"})).unwrap();
        assert_eq!(v["bodies"].as_array().unwrap().len(), 3);
        assert_eq!(
            v["commit"],
            call(&mut s, "rebuild", json!({"rev": "HEAD~3"})).unwrap()["commit"]
        );
        let m = call(
            &mut s,
            "mesh.get",
            json!({"part": "p", "rev": "HEAD~3", "quality": "coarse"}),
        )
        .unwrap();
        assert_eq!(m["bodies"].as_array().unwrap().len(), 3);
        let e = call(
            &mut s,
            "inspect",
            json!({"part": "p", "feature": "s5", "rev": "HEAD~3"}),
        )
        .unwrap_err();
        assert_eq!(
            (e.code, &e.details["commit"]),
            (Code::NotFound, &v["commit"])
        );
        // A few revisions are kept, most recently used first.
        for r in ["HEAD~5", "HEAD~4", "HEAD~3", "HEAD~2", "HEAD~1"] {
            at(&mut s, r).unwrap();
        }
        assert_eq!(s.revisions.len(), REVISIONS);
        // A revision's session never reads the disk.
        fs::remove_file(root.join("parts/p/features/s1.json")).unwrap();
        assert_eq!(at(&mut s, "HEAD~1").unwrap()["parts"][0]["hash"], hashes[4]);
        let e = at(&mut s, "nowhere").unwrap_err();
        assert_eq!(e.code, Code::RevNotFound);
        let (outside, mut t) = crate::server::tests::fresh();
        let e = call(&mut t, "rebuild", json!({"rev": "HEAD"})).unwrap_err();
        assert_eq!(e.code, Code::NotAGitRepo);
        drop(outside);
    }

    #[test]
    fn a_vanished_manifest_is_an_error() {
        let dir = project();
        let mut s = Session::open(dir.path()).unwrap();
        fs::remove_file(dir.path().join(MANIFEST)).unwrap();
        assert!(s.refresh().is_err());
    }
}
