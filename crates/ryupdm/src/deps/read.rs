//! Reading another project's release from the dependency cache, offline:
//! the label's files at the commit the lock names, checked against the
//! record there and the fetched release branch, so a label resolves to
//! exactly what the lock says or not at all.

use super::{Cache, prefix_of};
use crate::config::Config;
use crate::history::{Releases, git_error};
use crate::record::{Kind, Records};
use ryucad_doc::doc::canonical;
use ryucad_doc::doc::deps::Dependencies;
use ryucad_doc::doc::files::Manifest;
use ryucad_doc::doc::id::{Id, ProjectId};
use ryucad_doc::doc::lock::{Lock, Locked, LockedLabel};
use ryucad_doc::doc::project::{IMPORTS, MANIFEST};
use ryucad_doc::error::{Code, Error, Result};
use std::collections::BTreeMap;

/// A label of another project, read.
#[derive(Clone, Debug, PartialEq)]
pub struct Label {
    /// The part's files and its project's root files at `commit`, by path
    /// relative to the project, with the imported files its features name:
    /// what reading the part needs, as [`crate::history::part_files`]
    /// gives a local label.
    pub files: BTreeMap<String, Vec<u8>>,
    pub commit: String,
    /// Its record's content hash.
    pub content: String,
    /// An obsolete record names it on the fetched release branch.
    pub obsolete: bool,
}

/// Reads labels from a [`Cache`], each release branch's history read once
/// per tip. It holds no repository open (gix's aren't `Send` without its
/// `parallel` feature, and a daemon's session moves between threads), so
/// it may be kept anywhere.
pub struct Reader {
    pub cache: Cache,
    releases: BTreeMap<(String, String, String, ProjectId), Releases>,
}

/// `dependency_not_fetched` for `project` at `url`, saying what to run.
pub fn not_fetched(project: &ProjectId, url: &str, what: &str) -> Error {
    Error::new(
        Code::DependencyNotFetched,
        format!("{what} isn't in this machine's dependency cache: run `ryucad deps fetch`"),
    )
    .with("project", project)
    .with("url", url)
    .with("command", "ryucad deps fetch")
}

impl Reader {
    pub fn new(cache: Cache) -> Reader {
        Reader {
            cache,
            releases: BTreeMap::new(),
        }
    }

    /// `url`'s repository in the cache, `None` before its first fetch.
    pub fn repo(&self, url: &str) -> Result<Option<gix::Repository>> {
        self.cache.open(url)
    }

    /// The release branch `branch` of `project` at `path` in `url`'s
    /// repository as fetched, its official records only those naming
    /// `project`: a fork's records inherited from its upstream (naming the
    /// ID it had) are the upstream's history, never its own releases to
    /// depend on. `None` when the cache lacks the branch.
    pub fn releases(
        &mut self,
        project: &ProjectId,
        url: &str,
        path: &str,
        branch: &str,
    ) -> Result<Option<&Releases>> {
        let Some(repo) = self.repo(url)? else {
            return Ok(None);
        };
        let Some(tip) = super::tip(&repo, branch)? else {
            return Ok(None);
        };
        let key = (
            url.to_owned(),
            path.to_owned(),
            branch.to_owned(),
            project.clone(),
        );
        let current = self
            .releases
            .get(&key)
            .and_then(|r| r.branch.as_ref())
            .is_some_and(|b| b.tip == tip);
        if !current {
            let config = Config {
                release_branch: branch.to_owned(),
                ..Config::default()
            };
            let mut read = Releases::read(&repo, &prefix_of(path), &config)?;
            read.official
                .retain(|path, o| naming(&repo, path, o.blob).is_none_or(|p| p == *project));
            self.releases.insert(key.clone(), read);
        }
        Ok(self.releases.get(&key))
    }

    /// `part`'s release `label` of `project`, which the lock holds as
    /// `locked`: its files at the locked commit, checked. Without the
    /// cache's repository or the commit, `dependency_not_fetched`; a
    /// label the lock doesn't hold, `dependency_unlocked`; no record
    /// there, `label_not_found`; a manifest or record naming another
    /// project, `dependency_project_mismatch`; a content hash other than
    /// the lock's, or a release branch that no longer makes the record
    /// official at that commit, `dependency_rewritten`.
    pub fn label(
        &mut self,
        project: &ProjectId,
        locked: &Locked,
        part: &Id,
        label: &str,
    ) -> Result<Label> {
        let what = format!("{part}@{label} of {project}");
        let Some(l) = locked
            .labels
            .get(&(part.clone(), label.to_owned()))
            .cloned()
        else {
            return Err(Error::new(
                Code::DependencyUnlocked,
                format!("{what} isn't in ryucad.lock: write the lock again (`ryucad deps update --offline`)"),
            )
            .with("project", project)
            .with("part", part)
            .with("label", label));
        };
        let url = locked.url.clone();
        let prefix = prefix_of(&locked.path);
        let files = {
            let Some(repo) = self.repo(&url)? else {
                return Err(not_fetched(project, &url, &what));
            };
            let Ok(commit) = gix::ObjectId::from_hex(l.commit.as_bytes()) else {
                return Err(not_fetched(project, &url, &what));
            };
            if !repo.has_object(commit) {
                return Err(not_fetched(project, &url, &what));
            }
            label_files(&repo, &l, &prefix, part, label)?
        };
        check_project(project, &files, &url, &locked.path)?;
        let records = Records::read(&files)?;
        let path = crate::record::path(part, label, Kind::Release);
        let found = crate::structure::lookup_in(&records, part, label)?;
        if let Some(Ok(r)) = records.at(&path).map(|e| &e.record)
            && let Some(rel) = r.release()
            && rel.project != *project
        {
            return Err(Error::new(
                Code::DependencyProjectMismatch,
                format!("{what}'s record names project {}", rel.project),
            )
            .with("project", project)
            .with("found", &rel.project));
        }
        let rewritten = |why: String| {
            Error::new(
                Code::DependencyRewritten,
                format!(
                    "{what}: {why}; decide, then move to what it has now (`ryucad deps update`)"
                ),
            )
            .with("project", project)
            .with("part", part)
            .with("label", label)
            .with("url", &url)
            .with("locked", &l.commit)
        };
        if found.content != l.content {
            return Err(rewritten(format!(
                "its record names content {} where the lock has {}",
                found.content, l.content
            )));
        }
        let Some(releases) = self.releases(project, &url, &locked.path, &locked.release_branch)?
        else {
            return Err(not_fetched(project, &url, &what));
        };
        match releases.official.get(&path) {
            Some(o) if o.commit == l.commit => {}
            Some(o) => {
                return Err(rewritten(format!(
                    "its release branch {} makes it official at {}, not {}",
                    locked.release_branch, o.commit, l.commit
                )));
            }
            None => {
                return Err(rewritten(format!(
                    "its release branch {} no longer holds its record",
                    locked.release_branch
                )));
            }
        }
        let obsolete =
            releases
                .official
                .contains_key(&crate::record::path(part, label, Kind::Obsolete));
        Ok(Label {
            files,
            commit: l.commit,
            content: l.content,
            obsolete,
        })
    }
}

/// The project the record at `path` (its blob `blob`) names, if it reads.
fn naming(repo: &gix::Repository, path: &str, blob: gix::ObjectId) -> Option<ProjectId> {
    let (_, kind) = crate::record::parse_name(path.rsplit('/').next()?)?;
    let data = repo.find_object(blob).ok()?.data.clone();
    let v = canonical::parse(std::str::from_utf8(&data).ok()?).ok()?;
    crate::record::Record::from_json(&v, kind)
        .ok()
        .map(|r| r.project().clone())
}

/// Other projects' releases as a project's lock has them, read from the
/// dependency cache: what a [`crate::structure::Reader`] follows foreign
/// instances through. A project the lock lacks is `dependency_unlocked`
/// when `deps` (the reading project's manifest) names it, else
/// `dependency_missing`. Locks read with releases at their commits add
/// what this one lacks ([`crate::structure::Others::learn`]).
pub struct Through<'a> {
    pub reader: &'a mut Reader,
    pub lock: Lock,
    pub deps: &'a Dependencies,
    /// A project read from a repository on disk rather than the cache: a
    /// project searching the projects that depend on it reads their
    /// instances of its own parts from its own history.
    pub local: Option<Local<'a>>,
}

/// A project's own repository, holding its locked labels' commits.
#[derive(Clone, Copy)]
pub struct Local<'a> {
    pub project: &'a ProjectId,
    pub repo: &'a gix::Repository,
    /// Its folder in the repository, ending in `/` unless it is the root.
    pub prefix: &'a str,
}

impl crate::structure::Others for Through<'_> {
    fn label(&mut self, project: &ProjectId, part: &Id, label: &str) -> Result<Label> {
        let Some(locked) = self.lock.projects.get(project) else {
            let (code, why) = if self.deps.contains_key(project) {
                (
                    Code::DependencyUnlocked,
                    "which ryucad.lock doesn't hold: write it again (`ryucad deps update --offline`)",
                )
            } else {
                (Code::DependencyMissing, "which isn't a dependency")
            };
            return Err(Error::new(
                code,
                format!("{part}@{label} is of project {project}, {why}"),
            )
            .with("project", project)
            .with("part", part)
            .with("label", label));
        };
        if let Some(l) = self.local.filter(|l| l.project == project)
            && let Some(ll) = locked.labels.get(&(part.clone(), label.to_owned()))
            && gix::ObjectId::from_hex(ll.commit.as_bytes()).is_ok_and(|c| l.repo.has_object(c))
        {
            let files = label_files(l.repo, ll, l.prefix, part, label)?;
            let found = crate::structure::lookup(&files, part, label)?;
            return Ok(Label {
                files,
                commit: ll.commit.clone(),
                content: found.content,
                obsolete: found.obsolete,
            });
        }
        self.reader.label(project, locked, part, label)
    }

    fn learn(&mut self, lock: &Lock) {
        for (id, l) in &lock.projects {
            let into = self
                .lock
                .projects
                .entry(id.clone())
                .or_insert_with(|| l.clone());
            if into.url == l.url && into.path == l.path {
                for (k, v) in &l.labels {
                    into.labels.entry(k.clone()).or_insert_with(|| v.clone());
                }
            }
        }
    }
}

/// The files reading `part` at `l.commit` needs: its own, the project's
/// root files, the imported files its import features name, and its
/// records.
fn label_files(
    repo: &gix::Repository,
    l: &LockedLabel,
    prefix: &str,
    part: &Id,
    label: &str,
) -> Result<BTreeMap<String, Vec<u8>>> {
    let mut files = crate::history::part_files(repo, &l.commit, prefix, part)?;
    let commit = repo
        .find_commit(gix::ObjectId::from_hex(l.commit.as_bytes()).map_err(git_error)?)
        .map_err(git_error)?;
    let tree = commit.tree_id().map_err(git_error)?.detach();
    let mut wanted: Vec<String> = files
        .iter()
        .filter(|(path, _)| path.starts_with("parts/"))
        .filter_map(|(_, bytes)| serde_json::from_slice::<serde_json::Value>(bytes).ok())
        .filter(|v| v["type"] == "import")
        .filter_map(|v| v["file"]["path"].as_str().map(str::to_owned))
        .filter(|p| p.starts_with(&format!("{IMPORTS}/")))
        .collect();
    for kind in [Kind::Release, Kind::Obsolete] {
        wanted.push(crate::record::path(part, label, kind));
    }
    for path in wanted {
        if let Some(id) = crate::history::entry(repo, tree, prefix, &path)? {
            let data = repo.find_object(id).map_err(git_error)?.data.clone();
            files.insert(path, data);
        }
    }
    Ok(files)
}

/// `dependency_project_mismatch` unless the manifest in `files` names
/// `project`.
pub fn check_project(
    project: &ProjectId,
    files: &BTreeMap<String, Vec<u8>>,
    url: &str,
    path: &str,
) -> Result<()> {
    let found = files
        .get(MANIFEST)
        .and_then(|b| std::str::from_utf8(b).ok())
        .and_then(|t| canonical::parse(t).ok())
        .and_then(|v| Manifest::from_json(&v).ok())
        .and_then(|m| m.project);
    if found.as_ref() == Some(project) {
        return Ok(());
    }
    let what = match &found {
        Some(p) => format!("project {p}"),
        None => "no project with an ID".to_owned(),
    };
    Err(Error::new(
        Code::DependencyProjectMismatch,
        format!("{url} holds {what} at {path:?}, not {project}"),
    )
    .with("project", project)
    .with("url", url)
    .with("path", path)
    .with("found", found))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::super::tests::url;
    use super::*;
    use crate::history::tests::git;
    use crate::state::tests::{PROJECT, add, obsolete, project, release_now};
    use std::path::Path;

    /// Writes `files` into the repository at `dir` under `path` and
    /// commits them; returns the commit.
    pub(crate) fn commit(dir: &Path, path: &str, files: &BTreeMap<String, Vec<u8>>) -> String {
        for (p, bytes) in files {
            let at = dir.join(path).join(p);
            std::fs::create_dir_all(at.parent().unwrap()).unwrap();
            std::fs::write(at, bytes).unwrap();
        }
        git(dir, &["add", "-A"]);
        git(dir, &["commit", "-q", "--allow-empty", "-m", "c"]);
        git(dir, &["rev-parse", "HEAD"]).trim().to_owned()
    }

    /// A library repository at `dir` whose project (at `path`) has part `p`
    /// released as `A` on `main`; returns the commit making it official.
    pub(crate) fn library(dir: &Path, path: &str) -> String {
        git(dir, &["init", "-q"]);
        let mut f = project();
        commit(dir, path, &f);
        let a = release_now(&f, "A", None);
        add(&mut f, &a);
        commit(dir, path, &f)
    }

    pub(crate) fn locked(dir: &Path, path: &str, commit: &str) -> Locked {
        let f = project();
        let content = crate::content::hash(&f, &crate::state::tests::id("p")).unwrap();
        let mut labels = BTreeMap::new();
        labels.insert(
            (crate::state::tests::id("p"), "A".to_owned()),
            LockedLabel {
                commit: commit.to_owned(),
                content,
            },
        );
        Locked {
            url: url(dir),
            path: path.to_owned(),
            release_branch: "main".into(),
            via: vec![],
            labels,
        }
    }

    fn pid() -> ProjectId {
        ProjectId::parse(PROJECT).unwrap()
    }

    #[test]
    fn a_locked_label_reads_its_files_at_its_commit() {
        let up = tempfile::tempdir().unwrap();
        let at = library(up.path(), "lib");
        let cached = tempfile::tempdir().unwrap();
        let cache = Cache::new(cached.path());
        let lock = locked(up.path(), "lib", &at);
        let p = crate::state::tests::id("p");
        let mut r = Reader::new(cache.clone());
        // Nothing fetched yet.
        let e = r.label(&pid(), &lock, &p, "A").unwrap_err();
        assert_eq!(
            (e.code, &e.details["command"]),
            (
                Code::DependencyNotFetched,
                &serde_json::json!("ryucad deps fetch")
            )
        );
        cache.fetch(&lock.url, &["lib"]).unwrap();
        let mut r = Reader::new(cache.clone());
        let l = r.label(&pid(), &lock, &p, "A").unwrap();
        assert_eq!(l.commit, at);
        assert!(!l.obsolete);
        assert!(l.files.contains_key("parts/p/features/a.json"));
        assert!(l.files.contains_key("releases/p/A.json"));
        assert!(l.files.contains_key(MANIFEST));
        // Later work on the part changes nothing the label reads.
        let mut f = project();
        f.insert("parts/p/features/a.json".into(), b"{\"x\": 2}\n".to_vec());
        commit(up.path(), "lib", &f);
        // An obsolete record on the branch says so; the label still reads.
        let mut f = BTreeMap::new();
        add(&mut f, &obsolete("A"));
        commit(up.path(), "lib", &f);
        cache.fetch(&lock.url, &["lib"]).unwrap();
        let mut r = Reader::new(cache.clone());
        let l = r.label(&pid(), &lock, &p, "A").unwrap();
        assert!(l.obsolete);
        assert_eq!(
            l.files["parts/p/features/a.json"],
            project()["parts/p/features/a.json"]
        );
        // A label the lock doesn't hold.
        let e = r.label(&pid(), &lock, &p, "B").unwrap_err();
        assert_eq!(e.code, Code::DependencyUnlocked);
    }

    #[test]
    fn a_structure_reader_follows_the_lock_into_the_cache() {
        use crate::structure::{Cache as Nodes, Reader as Nodes_};
        use ryucad_doc::doc::deps::Dependency;
        let up = tempfile::tempdir().unwrap();
        let at = library(up.path(), "");
        let cached = tempfile::tempdir().unwrap();
        let cache = Cache::new(cached.path());
        let lock = Lock {
            projects: BTreeMap::from([(pid(), locked(up.path(), "", &at))]),
        };
        cache.fetch(&url(up.path()), &[""]).unwrap();
        // A project of its own, whose ID isn't the library's.
        let mut files = project();
        files.insert(
            MANIFEST.into(),
            canonical::to_string(&serde_json::json!({"default_unit": "mm", "format": 2,
                "kind": "ryucad.project", "project": ProjectId::from_bytes([9; 16])}))
            .into_bytes(),
        );
        let me = ryucad_doc::doc::project::Project::load_from(Path::new(""), &files).unwrap();
        let mut reader = Reader::new(cache.clone());
        let deps = Dependencies::new();
        let mut through = Through {
            reader: &mut reader,
            lock: lock.clone(),
            deps: &deps,
            local: None,
        };
        let mut nodes = Nodes::new();
        let mut r = Nodes_::new(&files, &me, None, None, &mut nodes).with_others(&mut through);
        let rev = |project: ProjectId| ryucad_doc::doc::assembly::Rev::Foreign {
            project,
            label: "A".into(),
        };
        let p = crate::state::tests::id("p");
        let other = ProjectId::from_bytes([4; 16]);
        let n = r.node(&p, &rev(other.clone())).map(|n| n.commit.clone());
        assert_eq!(n.unwrap_err().code, Code::DependencyMissing);
        drop(r);
        // The manifest names it, the lock doesn't.
        let mut named = Dependencies::new();
        named.insert(
            other.clone(),
            Dependency {
                name: "x".into(),
                url: "file:///x".into(),
                path: "".into(),
            },
        );
        through.deps = &named;
        let mut r = Nodes_::new(&files, &me, None, None, &mut nodes).with_others(&mut through);
        let e = r.node(&p, &rev(other)).unwrap_err();
        assert_eq!(e.code, Code::DependencyUnlocked);
        drop(r);
        // Locked: read from the cache at the locked commit. A lock read
        // with a release brings in what this one lacks, never changing
        // what it holds.
        through.lock = Lock::default();
        let mut r = Nodes_::new(&files, &me, None, None, &mut nodes).with_others(&mut through);
        assert_eq!(
            r.node(&p, &rev(pid())).unwrap_err().code,
            Code::DependencyMissing
        );
        drop(r);
        let mut wrong = lock.clone();
        wrong.projects.get_mut(&pid()).unwrap().url = "file:///elsewhere".into();
        through.lock = wrong.clone();
        crate::structure::Others::learn(&mut through, &lock);
        assert_eq!(through.lock, wrong);
        through.lock = Lock::default();
        crate::structure::Others::learn(&mut through, &lock);
        let mut r = Nodes_::new(&files, &me, None, None, &mut nodes).with_others(&mut through);
        let n = r.node(&p, &rev(pid())).unwrap();
        assert_eq!(
            (n.commit.as_deref(), n.project.as_ref()),
            (Some(at.as_str()), Some(&pid()))
        );
        assert_eq!(n.text("number"), Some("100"));
    }

    #[test]
    fn a_label_that_isnt_what_the_lock_says_is_refused() {
        let up = tempfile::tempdir().unwrap();
        let at = library(up.path(), "");
        let cached = tempfile::tempdir().unwrap();
        let cache = Cache::new(cached.path());
        let lock = locked(up.path(), "", &at);
        cache.fetch(&lock.url, &[""]).unwrap();
        let p = crate::state::tests::id("p");
        // Another project's ID.
        let other = ProjectId::from_bytes([3; 16]);
        let e = Reader::new(cache.clone())
            .label(&other, &lock, &p, "A")
            .unwrap_err();
        assert_eq!(e.code, Code::DependencyProjectMismatch);
        // A content hash the record doesn't name.
        let mut wrong = lock.clone();
        wrong
            .labels
            .values_mut()
            .for_each(|l| l.content = "0".repeat(64));
        let e = Reader::new(cache.clone())
            .label(&pid(), &wrong, &p, "A")
            .unwrap_err();
        assert_eq!(e.code, Code::DependencyRewritten);
        // The branch rewritten: the release made again on a new history.
        git(up.path(), &["checkout", "-q", "--orphan", "new"]);
        git(up.path(), &["rm", "-rq", "--cached", "."]);
        let mut f = project();
        let a = release_now(&f, "A", None);
        add(&mut f, &a);
        let again = commit(up.path(), "", &f);
        git(up.path(), &["branch", "-f", "main", "new"]);
        git(up.path(), &["checkout", "-q", "main"]);
        cache.fetch(&lock.url, &[""]).unwrap();
        let e = Reader::new(cache.clone())
            .label(&pid(), &lock, &p, "A")
            .unwrap_err();
        assert_eq!(e.code, Code::DependencyRewritten);
        assert!(e.message.contains(&again), "{e}");
    }
}
