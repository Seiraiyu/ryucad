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
}

impl Pins {
    /// A same-repository alias's library: the repository, its prefix
    /// there, its PDM settings and its release branch as read (memoised
    /// by refs key).
    pub(crate) fn local_releases(
        &self,
        root: &Path,
        a: &Alias,
    ) -> Result<(
        crate::git::Repo,
        String,
        ryupdm::config::Config,
        Arc<Releases>,
    )> {
        let mut inner = self.0.lock().unwrap_or_else(|p| p.into_inner());
        inner.local_releases(root, a)
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

    fn foreign(
        &mut self,
        alias: &Id,
        _: &Alias,
        component: &Id,
        label: &str,
        _: Option<&Lock>,
    ) -> Result<Arc<Pinned>> {
        Err(Error::new(
            Code::DependencyNotFetched,
            format!("{alias}/{component}@{label}: run `ryueda lib fetch`"),
        )
        .with("alias", alias)
        .with("component", component)
        .with("label", label))
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

    pub(crate) fn id(s: &str) -> Id {
        Id::parse(s, "id").unwrap()
    }
}
