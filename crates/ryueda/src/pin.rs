//! Pinned library components (design 629, "Library labels"). A design
//! project's manifest pins another project's components at release labels
//! (`libraries.<alias>.labels`). Each is read at its label's official
//! commit (in this repository) or locked commit (in another) with its
//! symbol, footprints and their models as they were there, in a library
//! of its own, so two components pinned at different labels may bring
//! different versions of one footprint.

use crate::error::{Code, Error, Result};
use crate::footprint::{self, Footprint, Model};
use crate::id::{Id, ProjectId};
use crate::libcomp::{self, LibComp};
use crate::library::{self, Library};
use crate::project::Alias;
use crate::source::join;
use ryu_doc::lock::Lock;
use ryupdm::history::Releases;
use ryupdm::record::Kind;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

/// A pinned component, read.
#[derive(Clone, Debug)]
pub struct Pinned {
    pub alias: Id,
    pub component: Id,
    pub label: String,
    /// The library project the label is a release of.
    pub project: ProjectId,
    /// Where its record is official (or locked).
    pub commit: String,
    /// The record's content hash.
    pub content: String,
    /// An obsolete record names the label.
    pub obsolete: bool,
    /// The component, its symbol, footprints, their models and the
    /// library's standards as `commit` has them, base `@<alias>/<label>`.
    pub library: Library,
}

/// Resolves a project's pins. `lock` is the project's `ryueda.lock`, when
/// it has one that reads.
pub trait Resolve: Sync {
    fn pin(
        &self,
        root: &Path,
        alias: &Id,
        a: &Alias,
        component: &Id,
        label: &str,
        lock: Option<&Lock>,
    ) -> Result<Arc<Pinned>>;

    /// Alias `a`'s library's PDM settings and its release branch, as read
    /// (for the project at `root`).
    fn releases_of(
        &self,
        root: &Path,
        a: &Alias,
    ) -> Result<(ryupdm::config::Config, Arc<Releases>)> {
        let _ = (root, a);
        Err(Error::new(
            Code::NotAGitRepo,
            "a library's releases are read only in a Git repository",
        ))
    }

    /// The file `rel` of alias `a`'s library project at `commit`.
    fn file_at(&self, root: &Path, a: &Alias, commit: &str, rel: &str) -> Result<Option<Vec<u8>>> {
        let _ = (root, a, commit, rel);
        Ok(None)
    }

    /// Lends `f` the reader of this machine's dependency cache.
    fn with_reader_dyn(
        &self,
        f: &mut dyn FnMut(&mut ryupdm::deps::read::Reader) -> Result<()>,
    ) -> Result<()> {
        let _ = f;
        Err(Error::new(
            Code::DependencyNotFetched,
            "no dependency cache: run `ryueda lib fetch`",
        ))
    }
}

/// Lends `f` the reader of `pins`' dependency cache.
pub fn with_reader<T>(
    pins: &dyn Resolve,
    f: impl FnOnce(&mut ryupdm::deps::read::Reader) -> Result<T>,
) -> Result<T> {
    let mut f = Some(f);
    let mut out = None;
    pins.with_reader_dyn(&mut |r| {
        let f = f.take().expect("lent once");
        out = Some(f(r)?);
        Ok(())
    })?;
    Ok(out.expect("lent"))
}

/// Resolves no pin: for projects read outside a repository, and tests.
pub struct NoPins;

impl Resolve for NoPins {
    fn pin(
        &self,
        _: &Path,
        alias: &Id,
        _: &Alias,
        component: &Id,
        label: &str,
        _: Option<&Lock>,
    ) -> Result<Arc<Pinned>> {
        Err(Error::new(
            Code::NotAGitRepo,
            format!("{alias}/{component}@{label}: labels resolve only in a Git repository"),
        )
        .with("alias", alias)
        .with("component", component)
        .with("label", label))
    }
}

/// The session's resolver: each library's release branch read once per
/// refs key, each pinned component read once per commit.
#[derive(Default)]
pub struct Pins(Mutex<Inner>);

/// A read component, by (repository: `""` for this one, else its URL;
/// prefix; component; commit; alias; label).
type ReadKey = (String, String, Id, String, Id, String);

#[derive(Default)]
struct Inner {
    /// By the library's prefix in this repository: the refs key and the
    /// release branch read at it.
    releases: BTreeMap<String, (String, Arc<Releases>)>,
    read: BTreeMap<ReadKey, Arc<Pinned>>,
    /// The dependency cache this resolver reads, when not this machine's
    /// (`crate::libdeps::cache`): tests'.
    cache: Option<ryupdm::deps::Cache>,
    /// Other repositories, through the dependency cache: opened on first
    /// use.
    deps: Option<ryupdm::deps::read::Reader>,
}

impl Pins {
    /// A resolver reading other repositories from `cache`.
    pub fn with_cache(cache: ryupdm::deps::Cache) -> Pins {
        Pins(Mutex::new(Inner {
            cache: Some(cache),
            ..Inner::default()
        }))
    }
}

/// The work-tree path, ending in `/`, of the folder `rel` relative to the
/// project at `prefix`; `library_missing` when it leaves the repository.
pub(crate) fn within(prefix: &str, rel: &str) -> Result<String> {
    let mut parts: Vec<&str> = prefix.split('/').filter(|s| !s.is_empty()).collect();
    for s in rel.split('/') {
        match s {
            "" | "." => {}
            ".." => {
                if parts.pop().is_none() {
                    return Err(Error::new(
                        Code::LibraryMissing,
                        format!("{rel} leaves the repository"),
                    ));
                }
            }
            s => parts.push(s),
        }
    }
    Ok(if parts.is_empty() {
        String::new()
    } else {
        format!("{}/", parts.join("/"))
    })
}

/// `label_not_found`, or `label_not_official` when the record is on the
/// branch's history but not official.
fn label_error(
    releases: &Releases,
    record: &str,
    alias: &Id,
    component: &Id,
    label: &str,
) -> Error {
    let code = if releases.modified.contains_key(record) {
        Code::LabelNotOfficial
    } else {
        Code::LabelNotFound
    };
    Error::new(
        code,
        format!("{alias}/{component}@{label} isn't an official release of the library"),
    )
    .with("alias", alias)
    .with("component", component)
    .with("label", label)
}

/// The library holding `component` and the objects it names, read from
/// `files` (a library project's files by path, as a commit has them).
#[cfg(test)]
pub(crate) fn from_files(
    files: &BTreeMap<String, Vec<u8>>,
    alias: &Id,
    label: &str,
    component: &Id,
) -> Result<Library> {
    let base = format!("@{alias}/{label}");
    let at = |rel: &str| files.get(rel).cloned();
    from_reader(&at, alias, &base, component)
}

/// [`from_files`] over any reader of files at the label's commit.
fn from_reader(
    at: &dyn Fn(&str) -> Option<Vec<u8>>,
    alias: &Id,
    base: &str,
    component: &Id,
) -> Result<Library> {
    let rel = format!("{}/{}/{component}.json", library::DIR, library::COMPONENTS);
    let bytes = at(&rel).ok_or_else(|| {
        Error::new(
            Code::NotFound,
            format!("{alias}/{component} isn't in its library at the label"),
        )
        .with("alias", alias)
        .with("component", component)
    })?;
    let c: LibComp = crate::doc::read(&bytes, &rel, libcomp::KIND, libcomp::FORMAT)?;
    let mut out: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    out.insert(join(base, &rel), bytes);
    let mut wanted = vec![
        format!("{}/{}/{}.json", library::DIR, library::SYMBOLS, c.symbol.id),
        format!("{}/{}", library::DIR, crate::standards::FILE),
    ];
    let fps: Vec<String> = c
        .footprints
        .iter()
        .map(|u| {
            format!(
                "{}/{}/{}.json",
                library::DIR,
                library::FOOTPRINTS,
                u.footprint.id
            )
        })
        .collect();
    wanted.extend(fps.iter().cloned());
    for rel in &wanted {
        if let Some(b) = at(rel) {
            out.insert(join(base, rel), b);
        }
    }
    let mut models = Vec::new();
    for rel in &fps {
        let Some(b) = out.get(&join(base, rel)) else {
            continue;
        };
        let Ok(f) = crate::doc::read::<Footprint>(b, rel, footprint::KIND, footprint::FORMAT)
        else {
            continue;
        };
        for m in &f.models {
            let Model::Step(s) = &m.model;
            models.push(s.path.clone());
        }
    }
    for path in models {
        if !path.contains("..")
            && let Some(b) = at(&path)
        {
            out.insert(join(base, &path), b);
        }
    }
    let mut lib = library::load(&out, alias.as_str(), base)?;
    library::judge_own(&mut lib);
    Ok(lib)
}

impl Resolve for Pins {
    fn pin(
        &self,
        root: &Path,
        alias: &Id,
        a: &Alias,
        component: &Id,
        label: &str,
        lock: Option<&Lock>,
    ) -> Result<Arc<Pinned>> {
        let mut inner = self.0.lock().unwrap_or_else(|p| p.into_inner());
        match &a.url {
            None => inner.local(root, alias, a, component, label),
            Some(_) => inner.foreign(alias, a, component, label, lock),
        }
    }

    fn releases_of(
        &self,
        root: &Path,
        a: &Alias,
    ) -> Result<(ryupdm::config::Config, Arc<Releases>)> {
        let mut inner = self.0.lock().unwrap_or_else(|p| p.into_inner());
        match &a.url {
            None => inner
                .local_releases(root, a)
                .map(|(_, _, config, releases)| (config, releases)),
            Some(_) => inner.foreign_releases(a),
        }
    }

    fn with_reader_dyn(
        &self,
        f: &mut dyn FnMut(&mut ryupdm::deps::read::Reader) -> Result<()>,
    ) -> Result<()> {
        let mut inner = self.0.lock().unwrap_or_else(|p| p.into_inner());
        f(inner.reader()?)
    }

    fn file_at(&self, root: &Path, a: &Alias, commit: &str, rel: &str) -> Result<Option<Vec<u8>>> {
        let mut inner = self.0.lock().unwrap_or_else(|p| p.into_inner());
        match &a.url {
            None => {
                let repo: crate::git::Repo = ryu_git::repo::repo(root)?;
                let prefix = within(repo.prefix(), &a.path)?;
                ryupdm::history::file_at(repo.gix(), commit, &prefix, rel)
            }
            Some(_) => inner.foreign_file_at(a, commit, rel),
        }
    }
}

impl Inner {
    fn local_releases(
        &mut self,
        root: &Path,
        a: &Alias,
    ) -> Result<(
        crate::git::Repo,
        String,
        ryupdm::config::Config,
        Arc<Releases>,
    )> {
        let repo: crate::git::Repo = ryu_git::repo::repo(root)?;
        let prefix = within(repo.prefix(), &a.path)?;
        let lib_root = crate::session::path_of(root, &a.path);
        let config =
            ryupdm::config::Config::read_in(&crate::source::Folder(&lib_root), crate::pdm::NAME)?;
        let key = ryupdm::release::refs_key(repo.gix(), &config)?;
        let releases = match self.releases.get(&prefix) {
            Some((k, r)) if *k == key => r.clone(),
            _ => {
                let r = Arc::new(Releases::read(repo.gix(), &prefix, &config)?);
                self.releases.insert(prefix.clone(), (key, r.clone()));
                r
            }
        };
        Ok((repo, prefix, config, releases))
    }

    fn local(
        &mut self,
        root: &Path,
        alias: &Id,
        a: &Alias,
        component: &Id,
        label: &str,
    ) -> Result<Arc<Pinned>> {
        let (repo, prefix, _, releases) = self.local_releases(root, a)?;
        let rec = ryupdm::record::path(component, label, Kind::Release);
        let Some(o) = releases.official.get(&rec) else {
            return Err(label_error(&releases, &rec, alias, component, label));
        };
        let obsolete =
            releases
                .official
                .contains_key(&ryupdm::record::path(component, label, Kind::Obsolete));
        let data = repo
            .gix()
            .find_object(o.blob)
            .map_err(ryupdm::history::git_error)?
            .data
            .clone();
        let v = crate::canonical::parse(std::str::from_utf8(&data).unwrap_or_default())?;
        let r = ryupdm::record::Record::from_json_in(&v, Kind::Release, crate::pdm::NAME)?;
        if let Some(want) = &a.project
            && r.project() != want
        {
            return Err(Error::new(
                Code::DependencyProjectMismatch,
                format!(
                    "{alias}/{component}@{label}'s record names project {}, not {want}",
                    r.project()
                ),
            )
            .with("alias", alias)
            .with("project", want)
            .with("found", r.project()));
        }
        let content = r.release().map(|x| x.content.clone()).unwrap_or_default();
        let key = (
            String::new(),
            prefix.clone(),
            component.clone(),
            o.commit.clone(),
            alias.clone(),
            label.to_owned(),
        );
        if let Some(p) = self.read.get(&key)
            && p.obsolete == obsolete
        {
            return Ok(p.clone());
        }
        let commit = o.commit.clone();
        let gix = repo.gix();
        let at = |rel: &str| {
            ryupdm::history::file_at(gix, &commit, &prefix, rel)
                .ok()
                .flatten()
        };
        let library = from_reader(&at, alias, &format!("@{alias}/{label}"), component)?;
        let p = Arc::new(Pinned {
            alias: alias.clone(),
            component: component.clone(),
            label: label.to_owned(),
            project: r.project().clone(),
            commit,
            content,
            obsolete,
            library,
        });
        self.read.insert(key, p.clone());
        Ok(p)
    }

    fn reader(&mut self) -> Result<&mut ryupdm::deps::read::Reader> {
        if self.deps.is_none() {
            let cache = match &self.cache {
                Some(c) => c.clone(),
                None => crate::libdeps::cache()?,
            };
            self.deps = Some(ryupdm::deps::read::Reader::new(cache));
        }
        Ok(self.deps.as_mut().expect("just set"))
    }

    /// A url alias's URL and project.
    fn place(a: &Alias) -> Result<(String, ProjectId)> {
        match (&a.url, &a.project) {
            (Some(u), Some(p)) => Ok((u.clone(), p.clone())),
            _ => Err(Error::new(
                Code::InvalidDocument,
                "a library in another repository names its url and project",
            )),
        }
    }

    fn foreign_releases(&mut self, a: &Alias) -> Result<(ryupdm::config::Config, Arc<Releases>)> {
        let (url, project) = Inner::place(a)?;
        let reader = self.reader()?;
        let not_fetched = || {
            ryupdm::deps::read::not_fetched_with(
                &project,
                &url,
                &format!("project {project}"),
                &crate::libdeps::EDA,
            )
        };
        let repo = reader.repo(&url)?.ok_or_else(not_fetched)?;
        let config = ryupdm::deps::config_at(&repo, ryupdm::deps::HEAD_REF, &a.path)?;
        let releases = reader
            .releases(&project, &url, &a.path, &config.release_branch)?
            .cloned()
            .ok_or_else(not_fetched)?;
        Ok((config, Arc::new(releases)))
    }

    fn foreign_file_at(&mut self, a: &Alias, commit: &str, rel: &str) -> Result<Option<Vec<u8>>> {
        let (url, _) = Inner::place(a)?;
        let Some(repo) = self.reader()?.repo(&url)? else {
            return Ok(None);
        };
        ryupdm::history::file_at(&repo, commit, &ryupdm::deps::prefix_of(&a.path), rel)
    }

    fn foreign(
        &mut self,
        alias: &Id,
        a: &Alias,
        component: &Id,
        label: &str,
        lock: Option<&Lock>,
    ) -> Result<Arc<Pinned>> {
        let (url, project) = Inner::place(a)?;
        let empty = ryu_doc::lock::Locked {
            url: url.clone(),
            path: a.path.clone(),
            ..Default::default()
        };
        let locked = lock
            .and_then(|l| l.projects.get(&project))
            .unwrap_or(&empty);
        let v = self.reader()?.verify_for::<crate::Eda>(
            &project,
            locked,
            component,
            label,
            &crate::libdeps::EDA,
        )?;
        let prefix = ryupdm::deps::prefix_of(&a.path);
        let key = (
            url.clone(),
            prefix.clone(),
            component.clone(),
            v.commit.clone(),
            alias.clone(),
            label.to_owned(),
        );
        if let Some(p) = self.read.get(&key)
            && p.obsolete == v.obsolete
        {
            return Ok(p.clone());
        }
        let repo = self.reader()?.repo(&url)?.ok_or_else(|| {
            ryupdm::deps::read::not_fetched_with(
                &project,
                &url,
                alias.as_str(),
                &crate::libdeps::EDA,
            )
        })?;
        let at = |rel: &str| {
            ryupdm::history::file_at(&repo, &v.commit, &prefix, rel)
                .ok()
                .flatten()
        };
        let library = from_reader(&at, alias, &format!("@{alias}/{label}"), component)?;
        let p = Arc::new(Pinned {
            alias: alias.clone(),
            component: component.clone(),
            label: label.to_owned(),
            project,
            commit: v.commit,
            content: v.content,
            obsolete: v.obsolete,
            library,
        });
        self.read.insert(key, p.clone());
        Ok(p)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn a_folder_relative_to_the_project_is_found_in_the_work_tree() {
        assert_eq!(within("board/", "../lib").unwrap(), "lib/");
        assert_eq!(within("a/b/", "../../c/d").unwrap(), "c/d/");
        assert_eq!(within("", "lib").unwrap(), "lib/");
        assert_eq!(within("", "../x").unwrap_err().code, Code::LibraryMissing);
    }

    #[test]
    fn nothing_resolves_without_a_repository() {
        let a = crate::project::Alias::local("../lib");
        let e = NoPins
            .pin(
                Path::new("/x"),
                &id("passives"),
                &a,
                &id("r_4k7"),
                "A",
                None,
            )
            .unwrap_err();
        assert_eq!(e.code, Code::NotAGitRepo);
    }

    #[test]
    fn a_pinned_library_holds_the_component_and_its_objects_only() {
        let files = crate::library::tests::files(crate::library::tests::STEP);
        let lib = from_files(&files, &id("passives"), "B", &id("r_100k")).unwrap();
        assert_eq!(lib.components.keys().collect::<Vec<_>>(), [&id("r_100k")]);
        assert_eq!(lib.base, "@passives/B");
        let c = lib.components[&id("r_100k")].item.as_ref().unwrap();
        assert!(lib.symbols.contains_key(&c.symbol.id));
        for u in &c.footprints {
            assert!(lib.footprints.contains_key(&u.footprint.id));
        }
        assert_eq!(lib.models.len(), 1, "{:?}", lib.models);
        assert!(
            lib.components[&id("r_100k")]
                .faults
                .iter()
                .all(|f| !f.is_error()),
            "{:?}",
            lib.components[&id("r_100k")].faults
        );
        assert!(lib.faults.is_empty(), "{:?}", lib.faults);
        let e = from_files(&files, &id("passives"), "B", &id("c_1u")).unwrap_err();
        assert_eq!(e.code, Code::NotFound);
    }

    /// A release (or obsolete) record of library project 2's `c` at
    /// `label`, as ryueda writes it.
    pub(crate) fn record(c: &str, label: &str, kind: Kind) -> String {
        use ryupdm::record::{BuiltWith, Obsolete, Record, Release, ReleasedBy};
        let project = crate::project::tests::pid(2);
        let part = id(c);
        let by = ReleasedBy {
            name: None,
            email: None,
            participant: "agent".into(),
        };
        let r = match kind {
            Kind::Release => Record::Release(Release {
                project,
                part,
                label: label.into(),
                content: format!("{:0>64}", label.to_ascii_lowercase()),
                properties: BTreeMap::new(),
                results: BTreeMap::new(),
                built_with: BuiltWith {
                    ryucad: "0".into(),
                    ryukei: "0".into(),
                    commit: "c".into(),
                    product: None,
                },
                released_by: by,
                supersedes: None,
                note: None,
            }),
            Kind::Obsolete => Record::Obsolete(Obsolete {
                project,
                part,
                label: label.into(),
                released_by: by,
                note: None,
            }),
        };
        crate::canonical::to_string(&r.to_json())
    }

    /// Writes `c`'s record of `kind` at `label` into the library project
    /// at `top/lib` and commits it on `main`.
    pub(crate) fn commit_record(top: &Path, lib: &str, c: &str, label: &str, kind: Kind) {
        use crate::git::tests::git;
        let path = ryupdm::record::path(&id(c), label, kind);
        let at = crate::session::path_of(&top.join(lib), &path);
        std::fs::create_dir_all(at.parent().unwrap()).unwrap();
        std::fs::write(&at, record(c, label, kind)).unwrap();
        git(top, &["add", "-A"]);
        git(top, &["commit", "-q", "-m", &format!("{c} {label}")]);
    }

    /// `c`'s `label` made obsolete on the library's release branch.
    pub(crate) fn obsolete(top: &Path, lib: &str, c: &str, label: &str) {
        commit_record(top, lib, c, label, Kind::Obsolete);
    }

    /// A repository holding `lib/` (library project 2, the test library,
    /// `r_100k` released at A on `main`) and `board/` (the breakout,
    /// naming `lib` as `passives`, its `r1` placing `passives/r_100k`
    /// pinned at A).
    pub(crate) fn released_library_and_board() -> (tempfile::TempDir, std::path::PathBuf) {
        use crate::git::tests::git;
        use crate::project::tests::{design_in, library_in, manifest, pid};
        use crate::project::{MANIFEST, Manifest, ProjectKind};
        let d = tempfile::tempdir().unwrap();
        let top = d.path().to_path_buf();
        git(&top, &["init", "-q", "-b", "main"]);
        let (lib, board) = (top.join("lib"), top.join("board"));
        std::fs::create_dir_all(&lib).unwrap();
        let m = Manifest::new(pid(2), crate::units::Unit::Mm, ProjectKind::Library);
        std::fs::write(lib.join(MANIFEST), m.to_text()).unwrap();
        library_in(&lib);
        manifest(&board, ProjectKind::Design, &[]);
        library_in(&board);
        design_in(&board);
        let mut m = Manifest::read(&std::fs::read(board.join(MANIFEST)).unwrap()).unwrap();
        let mut a = Alias::local("../lib");
        a.project = Some(pid(2));
        a.labels.insert(id("r_100k"), "A".into());
        m.libraries.insert(id("passives"), a);
        std::fs::write(board.join(MANIFEST), m.to_text()).unwrap();
        let r1 = board.join("blocks/top/components/r1.json");
        let mut v: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&r1).unwrap()).unwrap();
        v["of"] = serde_json::json!({"library": "passives", "id": "r_100k"});
        std::fs::write(&r1, crate::canonical::to_string(&v)).unwrap();
        commit_record(&top, "lib", "r_100k", "A", Kind::Release);
        (d, board)
    }

    #[test]
    fn a_pin_in_the_repository_reads_its_label_official_commit() {
        let (d, board) = released_library_and_board();
        let pins = Pins::default();
        let m = crate::project::Manifest::read(
            &std::fs::read(board.join(crate::project::MANIFEST)).unwrap(),
        )
        .unwrap();
        let a = &m.libraries[&id("passives")];
        let p = pins
            .pin(&board, &id("passives"), a, &id("r_100k"), "A", None)
            .unwrap();
        assert_eq!(p.project, crate::project::tests::pid(2));
        assert!(!p.obsolete);
        assert!(p.library.components.contains_key(&id("r_100k")));
        let e = pins
            .pin(&board, &id("passives"), a, &id("r_100k"), "B", None)
            .unwrap_err();
        assert_eq!(e.code, Code::LabelNotFound);
        // Another project named: refused.
        let mut other = a.clone();
        other.project = Some(crate::project::tests::pid(3));
        let e = pins
            .pin(&board, &id("passives"), &other, &id("r_100k"), "A", None)
            .unwrap_err();
        assert_eq!(e.code, Code::DependencyProjectMismatch);
        let (config, releases) = pins.releases_of(&board, a).unwrap();
        assert_eq!(config.release_branch, "main");
        assert_eq!(
            ryupdm::deps::parts::latest(&releases, config.revisions)[&id("r_100k")],
            "A"
        );
        obsolete(d.path(), "lib", "r_100k", "A");
        let p = pins
            .pin(&board, &id("passives"), a, &id("r_100k"), "A", None)
            .unwrap();
        assert!(p.obsolete);
    }

    /// A library repository of its own: `lib/` (library project 2, the
    /// test library with `c_100n` too, `r_100k` released at A on `main`).
    /// (its folder, its URL, the project, the release's commit).
    pub(crate) fn foreign_library() -> (tempfile::TempDir, String, ProjectId, String) {
        use crate::git::tests::git;
        use crate::project::{MANIFEST, Manifest, ProjectKind};
        let d = tempfile::tempdir().unwrap();
        let top = d.path().join("libs");
        std::fs::create_dir_all(top.join("lib")).unwrap();
        git(&top, &["init", "-q", "-b", "main"]);
        let m = Manifest::new(
            crate::project::tests::pid(2),
            crate::units::Unit::Mm,
            ProjectKind::Library,
        );
        std::fs::write(top.join("lib").join(MANIFEST), m.to_text()).unwrap();
        for (p, b) in crate::project::tests::pinned_files(None) {
            let at = crate::session::path_of(&top.join("lib"), &p);
            std::fs::create_dir_all(at.parent().unwrap()).unwrap();
            std::fs::write(at, b).unwrap();
        }
        commit_record(&top, "lib", "r_100k", "A", Kind::Release);
        let commit = head(&top);
        let url = format!("file://{}", top.display());
        (d, url, crate::project::tests::pid(2), commit)
    }

    /// The commit `HEAD` names in `dir`.
    pub(crate) fn head(dir: &Path) -> String {
        let out = crate::git::tests::git_command()
            .args(["rev-parse", "HEAD"])
            .current_dir(dir)
            .output()
            .unwrap();
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    }

    /// The foreign library releases `c` at `label` (its footprint edited
    /// first, so the release differs).
    pub(crate) fn release_again(
        t: &(tempfile::TempDir, String, ProjectId, String),
        c: &str,
        label: &str,
    ) {
        let top = t.0.path().join("libs");
        let fp = top.join("lib/library/footprints/r_0402_1005metric.json");
        let mut v: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&fp).unwrap()).unwrap();
        v["description"] = format!("at {label}").into();
        std::fs::write(&fp, crate::canonical::to_string(&v)).unwrap();
        commit_record(&top, "lib", c, label, Kind::Release);
    }

    #[test]
    fn a_foreign_pin_reads_through_the_lock() {
        let t = foreign_library();
        let pins = Pins::with_cache(ryupdm::deps::Cache::new(t.0.path().join("cache")));
        let mut a = Alias::local("lib");
        a.url = Some(t.1.clone());
        a.project = Some(t.2.clone());
        a.labels.insert(id("r_100k"), "A".into());
        let mut m = crate::project::Manifest::new(
            ProjectId::parse(&"9".repeat(32)).unwrap(),
            crate::units::Unit::Mm,
            crate::project::ProjectKind::Design,
        );
        m.libraries.insert(id("passives"), a.clone());
        // Not fetched: dependency_not_fetched.
        let lock = |pins: &Pins| {
            with_reader(pins, |r| {
                crate::libdeps::relock(r, &m, None, &Default::default())
            })
        };
        assert_eq!(lock(&pins).unwrap_err().code, Code::DependencyNotFetched);
        with_reader(&pins, |r| r.cache.fetch(&t.1, &["lib"]).map(drop)).unwrap();
        // No lock: dependency_unlocked.
        let e = pins
            .pin(
                Path::new("/"),
                &id("passives"),
                &a,
                &id("r_100k"),
                "A",
                None,
            )
            .unwrap_err();
        assert_eq!(e.code, Code::DependencyUnlocked);
        let lock = lock(&pins).unwrap();
        let p = pins
            .pin(
                Path::new("/"),
                &id("passives"),
                &a,
                &id("r_100k"),
                "A",
                Some(&lock),
            )
            .unwrap();
        assert_eq!(p.commit, t.3);
        assert!(p.library.components.contains_key(&id("r_100k")));
        assert_eq!(p.library.base, "@passives/A");
        // Its releases and files, from the cache.
        let (config, releases) = pins.releases_of(Path::new("/"), &a).unwrap();
        assert_eq!(config.release_branch, "main");
        assert!(releases.official.contains_key("releases/r_100k/A.json"));
        let f = pins
            .file_at(Path::new("/"), &a, &t.3, "library/components/r_100k.json")
            .unwrap();
        assert!(f.is_some());
    }

    pub(crate) fn id(s: &str) -> Id {
        Id::parse(s, "id").unwrap()
    }
}
