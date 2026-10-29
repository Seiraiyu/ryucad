//! Other projects' parts: what a foreign instance places, read from the
//! machine's dependency cache at the commit the lock names. Only `deps
//! add`, `deps fetch` and `deps update` reach the network (`ryupdm::deps`
//! fetches); everything here reads what was fetched.

use crate::assembly::{Pin, Pinned};
use crate::doc::files::Feature;
use crate::doc::id::{Id, ProjectId};
use crate::doc::project::Project;
use crate::error::{Code, Error, Result};
use crate::session::Session;

/// Moves the dependency cache, a folder of bare repositories.
pub const DEPS_DIR_VAR: &str = "RYUCAD_DEPS_DIR";

/// The machine's dependency cache: `RYUCAD_DEPS_DIR`, else `deps` in
/// ryucad's cache folder (beside the derived-data entries, which trimming
/// never reaches).
pub fn cache() -> Result<ryupdm::deps::Cache> {
    match std::env::var_os(DEPS_DIR_VAR).filter(|v| !v.is_empty()) {
        Some(d) => Ok(ryupdm::deps::Cache::new(d)),
        None => Ok(ryupdm::deps::Cache::new(
            crate::cache::cache_dir()?.join("deps"),
        )),
    }
}

/// A dependency's name in `project`'s manifest, else its project ID (a
/// project the lock reaches through another).
pub(crate) fn name_of(project: &Project, id: &ProjectId) -> String {
    project
        .manifest
        .dependencies
        .get(id)
        .map_or_else(|| id.to_string(), |d| d.name.clone())
}

/// The properties of `part` of `project` at its release `label`, as its
/// own files there have them; `None` when it can't be read.
pub(crate) fn properties(
    session: &mut Session,
    project: &ProjectId,
    part: &Id,
    label: &str,
) -> Option<ryupdm::props::Properties> {
    let lock = lock_of(session.project()).ok()?;
    let locked = lock.projects.get(project)?.clone();
    let read = session
        .deps_reader()
        .ok()?
        .label(project, &locked, part, label)
        .ok()?;
    ryupdm::props::read(&read.files, part).ok()
}

/// The lock as the working files have it: empty without one, its error
/// when it can't be read.
pub(crate) fn lock_of(project: &Project) -> Result<crate::doc::lock::Lock> {
    match &project.lock {
        None => Ok(Default::default()),
        Some(Ok(l)) => Ok(l.clone()),
        Some(Err(e)) => Err(e.clone()),
    }
}

/// `part` of the other project `project` as its release `label` has it:
/// its files at the commit the lock names, read from the dependency cache
/// and checked there ([`ryupdm::deps::read::Reader::label`]), its own
/// labels read as `project`'s releases. A project neither the manifest
/// nor the lock names is `dependency_missing`; one the lock lacks,
/// `dependency_unlocked`.
pub(crate) fn pinned(
    session: &mut Session,
    project: &ProjectId,
    part: &Id,
    label: &str,
) -> Result<Pinned> {
    let p = session.project();
    let own = p.manifest.project.clone();
    let lock = lock_of(p)?;
    let Some(locked) = lock.projects.get(project).cloned() else {
        let code = if p.manifest.dependencies.contains_key(project) {
            Code::DependencyUnlocked
        } else {
            Code::DependencyMissing
        };
        let why = match code {
            Code::DependencyUnlocked => {
                "which ryucad.lock doesn't hold: write it again (`ryucad deps update --offline`)"
            }
            _ => "which isn't a dependency",
        };
        return Err(Error::new(
            code,
            format!("{part}@{label} is of project {project}, {why}"),
        )
        .with("project", project)
        .with("part", part)
        .with("label", label));
    };
    let fingerprint = format!(
        "{} {} {} {:?}",
        locked.url,
        locked.path,
        locked.release_branch,
        locked.labels.get(&(part.clone(), label.to_owned()))
    );
    let key = (project.clone(), part.clone(), label.to_owned());
    if let Some((f, r)) = session.foreign_pins.get(&key)
        && *f == fingerprint
    {
        return r.clone();
    }
    let r = read(session, project, own.as_ref(), &locked, part, label);
    session.foreign_pins.insert(key, (fingerprint, r.clone()));
    r
}

/// [`pinned`] from the dependency cache.
fn read(
    session: &mut Session,
    project: &ProjectId,
    own: Option<&ProjectId>,
    locked: &crate::doc::lock::Locked,
    part: &Id,
    label: &str,
) -> Result<Pinned> {
    let read = session.deps_reader()?.label(project, locked, part, label)?;
    let mut theirs = Project::load_from(session.root(), &read.files)?;
    let mut p = theirs.parts.remove(part).ok_or_else(|| {
        Error::new(
            Code::NotFound,
            format!("{part} isn't in project {project} at {}", read.commit),
        )
        .with("project", project)
        .with("part", part)
        .with("commit", &read.commit)
    })?;
    for f in p.features.values_mut() {
        if let Ok(Feature::Instance(i)) = f {
            i.rev = i.rev.seen_from(project, own);
        }
    }
    Ok(Pinned {
        part: p,
        pin: Pin {
            commit: Some(read.commit),
            obsolete: read.obsolete,
        },
    })
}

/// The schema of `deps` outputs (design 65's rule: a new field is not a
/// change).
pub const SCHEMA: u64 = 1;

fn invalid(param: &str, why: &str) -> Error {
    Error::new(Code::DependencyInvalid, why).with("param", param)
}

/// A handle for a dependency from its folder or its repository's name:
/// `cad/fasteners` and `https://github.com/acme/fasteners.git` are
/// `fasteners`.
fn name_from(url: &str, path: &str) -> Option<Id> {
    let stem = match path.rsplit('/').next().filter(|s| !s.is_empty()) {
        Some(s) => s.to_owned(),
        None => {
            let u = url.trim_end_matches('/');
            let u = u.strip_suffix(".git").unwrap_or(u);
            u.rsplit(['/', ':']).next().unwrap_or_default().to_owned()
        }
    };
    crate::import::id_from(&stem)
}

/// Writes `m` as the manifest, and reads the project again.
fn save_manifest(session: &mut Session, m: &crate::doc::files::Manifest) -> Result<()> {
    let path = session.root().join(crate::doc::project::MANIFEST);
    crate::doc::project::write_json(&path, &m.to_json())?;
    session.reload()
}

/// `deps.add`: fetches `url`, reads the project's ID in its folder `path`
/// there and lists it as a dependency named `name` (else after its folder
/// or repository). One logged, undoable change guarded by the project's
/// revision.
pub fn add(
    session: &mut Session,
    url: &str,
    path: &str,
    name: Option<&str>,
    expected: Option<&str>,
) -> Result<serde_json::Value> {
    use crate::doc::deps::{Dependency, path_fault, url_fault};
    crate::config::check_project_revision(session.root(), expected)?;
    if let Some(why) = url_fault(url) {
        return Err(invalid("url", why).with("url", url));
    }
    if let Some(why) = path_fault(path) {
        return Err(invalid("path", why).with("path", path));
    }
    let mut m = session.project().manifest.clone();
    let key = ryupdm::deps::normalize(url);
    if let Some((id, d)) = m
        .dependencies
        .iter()
        .find(|(_, d)| ryupdm::deps::normalize(&d.url) == key && d.path == path)
    {
        return Err(Error::new(
            Code::DuplicateId,
            format!("{url} ({path:?}) is already the dependency {}", d.name),
        )
        .with("dependency", &d.name)
        .with("project", id));
    }
    let cache = session.deps_cache()?;
    let fetched = cache.fetch(url, &[path])?;
    session.deps_fetched();
    let repo = cache.open(url)?.expect("just fetched");
    let found = ryupdm::deps::manifest_at(&repo, ryupdm::deps::HEAD_REF, path)?;
    let Some(id) = found.and_then(|m| m.project) else {
        return Err(Error::new(
            Code::DependencyProjectMismatch,
            format!(
                "{url} holds no ryucad project with an ID at {path:?} (a project gets its ID at its first PDM write)"
            ),
        )
        .with("url", url)
        .with("path", path));
    };
    if m.project.as_ref() == Some(&id) {
        return Err(Error::new(
            Code::ProjectIdDuplicate,
            format!(
                "{url} holds this project ({id}): a copy needs an ID of its own (`ryucad pdm new-id`)"
            ),
        )
        .with("project", &id)
        .with("url", url));
    }
    if let Some(d) = m.dependencies.get(&id) {
        return Err(Error::new(
            Code::ProjectIdDuplicate,
            format!(
                "{url} holds project {id}, already the dependency {} at {}",
                d.name, d.url
            ),
        )
        .with("project", &id)
        .with("dependency", &d.name)
        .with("urls", [&d.url, &url.to_owned()]));
    }
    let taken = |n: &str| m.dependencies.values().any(|d| d.name == n);
    let name = match name {
        Some(n) => {
            let n = Id::parse(n, "dependency").map_err(|e| e.with("param", "name"))?;
            if taken(n.as_str()) {
                return Err(Error::new(
                    Code::DuplicateId,
                    format!("a dependency is already named {n}"),
                )
                .with("param", "name"));
            }
            n
        }
        None => {
            let base = name_from(url, path)
                .unwrap_or_else(|| Id::parse("dependency", "dependency").expect("an ID"));
            crate::import::free(&base, taken)
        }
    };
    m.dependencies.insert(
        id.clone(),
        Dependency {
            name: name.to_string(),
            url: url.to_owned(),
            path: path.to_owned(),
        },
    );
    save_manifest(session, &m)?;
    let (branch, tip) = fetched.branches.get(path).cloned().unwrap_or_default();
    Ok(serde_json::json!({
        "schema": SCHEMA,
        "dependency": {"name": name, "project": id, "url": url, "path": path},
        "fetched": {"url": url, "head": fetched.head, "release_branch": branch, "tip": tip},
        "project_revision": crate::config::project_revision(session.root())?,
    }))
}

/// Every repository the manifest and the lock name, with the project
/// folders in it: what `deps fetch` fetches, narrowed to the named
/// dependencies (by name or ID) and the projects the lock reaches through
/// them.
fn repositories(
    session: &Session,
    names: &[String],
) -> Result<std::collections::BTreeMap<String, Vec<String>>> {
    let p = session.project();
    let mut wanted = std::collections::BTreeSet::new();
    for n in names {
        let (id, _) = crate::doc::deps::find(&p.manifest.dependencies, n)?;
        wanted.insert(id.clone());
    }
    let lock = lock_of(p).unwrap_or_default();
    let picked = |id: &ProjectId, via: &[ProjectId]| {
        wanted.is_empty() || wanted.contains(id) || via.iter().any(|v| wanted.contains(v))
    };
    let mut out: std::collections::BTreeMap<String, Vec<String>> = Default::default();
    let mut add = |url: &str, path: &str| {
        let paths = out.entry(url.to_owned()).or_default();
        if !paths.iter().any(|x| x == path) {
            paths.push(path.to_owned());
        }
    };
    for (id, d) in &p.manifest.dependencies {
        if picked(id, &[]) {
            add(&d.url, &d.path);
        }
    }
    for (id, l) in &lock.projects {
        if picked(id, &l.via) {
            add(&l.url, &l.path);
        }
    }
    Ok(out)
}

/// `deps.fetch`: fetches every dependency the manifest and the lock name
/// (or the named ones, and what the lock reaches through them), writing
/// nothing in the project. `dependency_fetch_failed` (exit 4) for the
/// first that can't be read.
pub fn fetch(session: &mut Session, names: &[String]) -> Result<serde_json::Value> {
    let repos = repositories(session, names)?;
    let cache = session.deps_cache()?;
    let mut out = Vec::new();
    for (url, paths) in &repos {
        let paths: Vec<&str> = paths.iter().map(String::as_str).collect();
        let f = cache.fetch(url, &paths)?;
        let branches: Vec<serde_json::Value> = f
            .branches
            .iter()
            .map(|(path, (branch, tip))| {
                serde_json::json!({"path": path, "release_branch": branch, "tip": tip,
                                   "before": f.before.get(branch),
                                   "changed": f.before.get(branch) != tip.as_ref()})
            })
            .collect();
        out.push(serde_json::json!({"url": url, "head": f.head, "branches": branches}));
    }
    session.deps_fetched();
    Ok(serde_json::json!({"schema": SCHEMA, "fetched": out}))
}

/// `deps.list`: each dependency and each project the lock reaches through
/// one, offline: its name, ID, URL, path, release branch, whether it is
/// fetched and the fetched tip, its faults, and each label the lock holds
/// with the instances that place it.
pub fn list(session: &mut Session) -> Result<serde_json::Value> {
    use serde_json::json;
    let p = session.project().clone();
    let lock = lock_of(&p);
    let locked = lock
        .as_ref()
        .map(|l| l.projects.clone())
        .unwrap_or_default();
    let faults = crate::doc::deps::faults(p.manifest.project.as_ref(), &p.manifest.dependencies);
    // Each foreign instance, by project, part and label.
    let mut uses: std::collections::BTreeMap<(ProjectId, Id, String), Vec<serde_json::Value>> =
        Default::default();
    for (asm, part) in &p.parts {
        for fid in part.order() {
            if let Some(Ok(Feature::Instance(i))) = part.features.get(fid)
                && let crate::doc::assembly::Rev::Foreign { project, label } = &i.rev
            {
                uses.entry((project.clone(), i.part.clone(), label.clone()))
                    .or_default()
                    .push(json!({"assembly": asm, "instance": fid}));
            }
        }
    }
    let mut ids: Vec<ProjectId> = p.manifest.dependencies.keys().cloned().collect();
    ids.extend(
        locked
            .keys()
            .filter(|k| !p.manifest.dependencies.contains_key(*k))
            .cloned(),
    );
    let cache = session.deps_cache()?;
    let mut out = Vec::new();
    for id in ids {
        let dep = p.manifest.dependencies.get(&id);
        let l = locked.get(&id);
        let (url, path) = match (dep, l) {
            (Some(d), _) => (d.url.clone(), d.path.clone()),
            (None, Some(l)) => (l.url.clone(), l.path.clone()),
            (None, None) => unreachable!("listed from one or the other"),
        };
        let repo = cache.open(&url)?;
        let branch = match (l, &repo) {
            (Some(l), _) => Some(l.release_branch.clone()),
            (None, Some(r)) => {
                Some(ryupdm::deps::config_at(r, ryupdm::deps::HEAD_REF, &path)?.release_branch)
            }
            (None, None) => None,
        };
        let tip = match (&repo, &branch) {
            (Some(r), Some(b)) => ryupdm::deps::tip(r, b)?,
            _ => None,
        };
        let labels: Vec<serde_json::Value> = l
            .map(|l| {
                l.labels
                    .iter()
                    .map(|((part, label), x)| {
                        let placed = uses
                            .get(&(id.clone(), part.clone(), label.clone()))
                            .cloned()
                            .unwrap_or_default();
                        json!({"part": part, "label": label, "commit": x.commit,
                               "content": x.content, "instances": placed})
                    })
                    .collect()
            })
            .unwrap_or_default();
        let mine: Vec<&Error> = faults
            .iter()
            .filter(|e| e.details["project"] == id.as_str())
            .collect();
        out.push(json!({
            "name": dep.map(|d| d.name.clone()),
            "project": id,
            "url": url,
            "path": path,
            "direct": dep.is_some(),
            "via": l.map(|l| l.via.clone()).unwrap_or_default(),
            "release_branch": branch,
            "fetched": repo.is_some(),
            "tip": tip,
            "labels": labels,
            "faults": mine,
        }));
    }
    let mut v = json!({"schema": SCHEMA, "dependencies": out,
                       "project_revision": crate::config::project_revision(session.root())?});
    if let Err(e) = lock {
        v["lock_fault"] = json!(e);
    }
    Ok(v)
}

#[cfg(test)]
pub(crate) mod tests {
    use crate::assembly::tests::{id, project};
    use crate::doc::assembly::Rev;
    use crate::doc::files::{Manifest, PartFile};
    use crate::doc::id::ProjectId;
    use crate::doc::project::{MANIFEST, feature_path, part_path, write_json};
    use crate::error::Code;
    use crate::git::tests::git;
    use crate::server::tests::call;
    use crate::session::Session;
    use serde_json::json;
    use std::path::Path;

    /// A library: a project in a repository whose part `p` (a box) is
    /// released as `A`, official on `main`.
    pub(crate) fn library() -> (tempfile::TempDir, ProjectId) {
        let (dir, mut s) = crate::release::tests::released_project();
        call(&mut s, "release.propose", json!({"part": "p"})).unwrap();
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-q", "-m", "p A"]);
        let id = s.project().manifest.project.clone().unwrap();
        (dir, id)
    }

    pub(crate) fn url(dir: &Path) -> String {
        format!("file://{}", dir.display())
    }

    /// Lists `lib` (at `dir`) as the dependency `lib` of `s`'s project.
    pub(crate) fn depend(s: &mut Session, lib: &ProjectId, dir: &Path) {
        let path = s.root().join(MANIFEST);
        let text = std::fs::read_to_string(&path).unwrap();
        let mut m = Manifest::from_json(&crate::doc::canonical::parse(&text).unwrap()).unwrap();
        m.dependencies.insert(
            lib.clone(),
            crate::doc::deps::Dependency {
                name: "lib".into(),
                url: url(dir),
                path: "".into(),
            },
        );
        write_json(&path, &m.to_json()).unwrap();
        s.refresh().unwrap();
    }

    /// Writes the lock the working files reach, from `s`'s cache.
    pub(crate) fn write_lock(s: &mut Session) {
        let p = s.project().clone();
        let start = ryupdm::deps::lock::wanted(&p);
        let reader = s.deps_reader().unwrap();
        let lock = ryupdm::deps::lock::lock(
            reader,
            p.manifest.project.as_ref(),
            &p.manifest.dependencies,
            &start,
            None,
        )
        .unwrap();
        write_json(&s.root().join(crate::doc::lock::LOCK), &lock.to_json()).unwrap();
        s.refresh().unwrap();
    }

    /// Writes instance `inst` of `project`'s `part` at `label` into
    /// assembly `asm`, listed last, as a file would hold it.
    pub(crate) fn place_foreign(
        root: &Path,
        asm: &str,
        inst: &str,
        project: &ProjectId,
        part: &str,
        label: &str,
    ) {
        let v = json!({"format": 6, "kind": "ryucad.feature", "type": "instance",
                       "project": project, "part": part, "rev": {"label": label},
                       "pose": {"origin_m": [0, 0, 0], "z": [0, 0, 1], "x": [1, 0, 0]}});
        write_json(&feature_path(root, &id(asm), &id(inst)), &v).unwrap();
        let p = part_path(root, &id(asm));
        let text = std::fs::read_to_string(&p).unwrap();
        let mut list = PartFile::from_json(&crate::doc::canonical::parse(&text).unwrap()).unwrap();
        list.features.push(id(inst));
        write_json(&p, &list.to_json()).unwrap();
    }

    #[test]
    fn a_foreign_instance_needs_a_dependency_then_a_lock() {
        let (dir, mut s) = project();
        let lib = ProjectId::from_bytes([7; 16]);
        place_foreign(dir.path(), "frame", "b", &lib, "m6-bolt", "B");
        s.refresh().unwrap();
        let r = s.rebuilt(&id("frame")).unwrap().clone();
        // An instance fails through what it places, as for a local label.
        let e = r.features[0].1.as_ref().unwrap_err();
        assert_eq!(
            (e.code, &e.details["cause"]),
            (Code::DependencyFailed, &json!("dependency_missing"))
        );
        // Listed as a dependency, the lock must hold it.
        let text = std::fs::read_to_string(dir.path().join(MANIFEST)).unwrap();
        let mut m = Manifest::from_json(&crate::doc::canonical::parse(&text).unwrap()).unwrap();
        m.dependencies.insert(
            lib.clone(),
            crate::doc::deps::Dependency {
                name: "fasteners".into(),
                url: "file:///nowhere".into(),
                path: "".into(),
            },
        );
        write_json(&dir.path().join(MANIFEST), &m.to_json()).unwrap();
        s.refresh().unwrap();
        let r = s.rebuilt(&id("frame")).unwrap().clone();
        let e = r.features[0].1.as_ref().unwrap_err();
        assert_eq!(e.details["cause"], "dependency_unlocked");
    }

    #[test]
    fn a_foreign_instance_builds_the_libraries_release() {
        let (lib_dir, lib) = library();
        let (dir, mut s) = project();
        let cache = ryupdm::deps::Cache::new(dir.path().join("cache"));
        s.deps_cache = Some(cache.clone());
        depend(&mut s, &lib, lib_dir.path());
        place_foreign(dir.path(), "frame", "b", &lib, "p", "A");
        s.refresh().unwrap();
        // Locked, but not fetched: the instance says what to run.
        cache.fetch(&url(lib_dir.path()), &[""]).unwrap();
        write_lock(&mut s);
        std::fs::remove_dir_all(dir.path().join("cache")).unwrap();
        s.deps_fetched();
        let r = s.rebuilt(&id("frame")).unwrap().clone();
        assert_eq!(
            r.features[0].1.as_ref().unwrap_err().details["cause"],
            "dependency_not_fetched"
        );
        cache.fetch(&url(lib_dir.path()), &[""]).unwrap();
        s.deps_fetched();
        let r = s.rebuilt(&id("frame")).unwrap().clone();
        let placed = r.output(&id("b")).unwrap().instance().unwrap();
        let commit = git_head(lib_dir.path());
        assert_eq!(
            placed.model.pin.as_ref().unwrap().commit.as_deref(),
            Some(commit.as_str())
        );
        let rev = Rev::Foreign {
            project: lib.clone(),
            label: "A".into(),
        };
        assert_eq!(placed.model.occurrences.len(), 1);
        // The library's own build of its release, bit for bit.
        let mut theirs = Session::open(lib_dir.path()).unwrap();
        let hash = theirs
            .rebuilt(&id("p"))
            .unwrap()
            .output(&id("b"))
            .unwrap()
            .hash();
        let m = s.model(&id("frame"), &Rev::Working).unwrap();
        assert_eq!(m.occurrences[0].rev, rev);
        assert_eq!(m.occurrences[0].solids[0].built.hash, hash);
    }

    fn git_head(dir: &Path) -> String {
        crate::git::repo(dir).unwrap().locate("HEAD").unwrap().0
    }

    /// [`project`] depending on [`library`], its frame placing the
    /// library's `p` at `A` as `b`, fetched and locked.
    pub(crate) fn placing() -> (tempfile::TempDir, tempfile::TempDir, ProjectId, Session) {
        let (lib_dir, lib) = library();
        let (dir, mut s) = project();
        let cache = ryupdm::deps::Cache::new(dir.path().join("cache"));
        s.deps_cache = Some(cache.clone());
        depend(&mut s, &lib, lib_dir.path());
        place_foreign(dir.path(), "frame", "b", &lib, "p", "A");
        s.refresh().unwrap();
        cache.fetch(&url(lib_dir.path()), &[""]).unwrap();
        write_lock(&mut s);
        (lib_dir, dir, lib, s)
    }

    #[test]
    fn a_foreign_part_exports_mates_and_releases_as_a_local_one() {
        let (_lib_dir, dir, _lib, mut s) = placing();
        // Exported, it is a product of its own, named by its PDM number,
        // at its label.
        let (a, _) = crate::export::tests::exported(
            &mut s,
            json!({"part": "frame", "time_stamp": "2026-10-29T00:00:00"}),
        )
        .unwrap();
        let p = a["products"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["part"] == "p")
            .unwrap()
            .clone();
        assert_eq!(
            (&p["product"], &p["revision"]),
            (&json!("100"), &json!("A"))
        );
        // A mate holds it to a local instance.
        call(
            &mut s,
            "feature.add",
            json!({"part": "frame", "type": "instance", "id": "pl", "of": "plate", "fixed": true,
                   "placement": {"origin": ["10 mm", "0 mm", "0 mm"]}}),
        )
        .unwrap();
        call(
            &mut s,
            "feature.add",
            json!({"part": "frame", "type": "mate", "id": "m", "form": "fastened",
                   "a": "frame/pl", "b": "frame/b"}),
        )
        .unwrap();
        let r = s.rebuilt(&id("frame")).unwrap().clone();
        let placed = r.output(&id("b")).unwrap().instance().unwrap();
        let x = crate::assembly::rigid_pose(&placed.pose).origin_m[0];
        assert!((x - 0.01).abs() < 1e-12, "{x}");
        // A release of the frame takes it as an official release.
        git(dir.path(), &["init", "-q", "-b", "main"]);
        let d = call(
            &mut s,
            "release.propose",
            json!({"part": "frame", "dry_run": true}),
        )
        .unwrap();
        let of = |inst: &str| {
            d["problems"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|p| {
                    p["code"] == "instance_not_released" && p["details"]["instance"] == inst
                })
                .count()
        };
        // The local plate at working isn't released; the library's is.
        assert_eq!((of("pl"), of("b")), (1, 0), "{d}");
        // Not fetched, it can't be judged official.
        std::fs::remove_dir_all(dir.path().join("cache")).unwrap();
        s.deps_fetched();
        let d = call(
            &mut s,
            "release.propose",
            json!({"part": "frame", "dry_run": true}),
        )
        .unwrap();
        let b = d["problems"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["details"]["instance"] == "b")
            .cloned()
            .unwrap_or_default();
        assert_eq!(b["code"], "dependency_not_fetched", "{d}");
    }

    #[test]
    fn dependencies_are_added_fetched_and_listed() {
        let (lib_dir, lib) = library();
        let (dir, mut s) = crate::server::tests::fresh();
        s.deps_cache = Some(ryupdm::deps::Cache::new(dir.path().join("cache")));
        let u = url(lib_dir.path());
        let a = call(&mut s, "deps.add", json!({"url": u})).unwrap();
        // Named after the repository's folder, made an ID.
        let name = a["dependency"]["name"].as_str().unwrap().to_owned();
        assert_eq!(
            Some(name.clone()),
            lib_dir
                .path()
                .file_name()
                .and_then(|n| crate::import::id_from(n.to_str().unwrap()))
                .map(|i| i.to_string())
        );
        assert_eq!(a["dependency"]["project"], lib.as_str());
        assert_eq!(a["fetched"]["release_branch"], "main");
        let m = &s.project().manifest;
        assert_eq!(m.dependencies[&lib].url, u);
        assert_eq!(m.to_json()["format"], 3);
        // Again: refused, the same repository and folder.
        let e = call(&mut s, "deps.add", json!({"url": format!("{u}/")})).unwrap_err();
        assert_eq!(e.code, Code::DuplicateId);
        // A folder with no project.
        let e = call(&mut s, "deps.add", json!({"url": u, "path": "parts"})).unwrap_err();
        assert_eq!(e.code, Code::DependencyProjectMismatch);
        // A URL that is no URL.
        let e = call(&mut s, "deps.add", json!({"url": "fasteners"})).unwrap_err();
        assert_eq!(
            (e.code, &e.details["param"]),
            (Code::DependencyInvalid, &json!("url"))
        );
        // Listed, fetched, nothing locked yet.
        let l = call(&mut s, "deps.list", json!({})).unwrap();
        let d = &l["dependencies"][0];
        assert_eq!(
            (&d["name"], &d["fetched"], &d["direct"]),
            (&json!(name), &json!(true), &json!(true))
        );
        assert_eq!(
            d["tip"],
            crate::git::repo(lib_dir.path())
                .unwrap()
                .locate("HEAD")
                .unwrap()
                .0
        );
        assert_eq!(d["labels"], json!([]));
        // Fetched again: nothing changed.
        let f = call(&mut s, "deps.fetch", json!({})).unwrap();
        assert_eq!(f["fetched"][0]["branches"][0]["changed"], false);
        let e = call(&mut s, "deps.fetch", json!({"names": ["nope"]})).unwrap_err();
        assert_eq!(e.code, Code::NotFound);
        // The cache gone: listed as not fetched, offline.
        std::fs::remove_dir_all(dir.path().join("cache")).unwrap();
        let l = call(&mut s, "deps.list", json!({})).unwrap();
        assert_eq!(l["dependencies"][0]["fetched"], false);
    }
}
