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
    // What the cache's state decides isn't kept: other worktrees' daemons
    // and `--no-daemon` runs fetch into the same machine-wide cache, and
    // this session must see it the next time it asks.
    if !r.as_ref().is_err_and(|e| decided_by_cache(e.code)) {
        session.foreign_pins.insert(key, (fingerprint, r.clone()));
    }
    r
}

/// Faults a fetch into the dependency cache may take away: what wasn't
/// fetched yet, a label not yet official on the branch as fetched, a
/// release branch rewritten under the lock.
fn decided_by_cache(code: Code) -> bool {
    matches!(
        code,
        Code::DependencyNotFetched | Code::DependencyRewritten | Code::LabelNotOfficial
    )
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

/// Fetches made without the daemon's lock, before the request that asked
/// for them ([`prepare`]): by URL, the project folders fetched and what
/// the fetch gave.
pub(crate) type Prefetched =
    std::collections::BTreeMap<String, (Vec<String>, Result<ryupdm::deps::Fetched>)>;

/// The fetches `req` (`deps.add`, `deps.fetch`, `deps.update` unless
/// offline) will make, as work the daemon runs without its lock: a remote
/// that is slow, or stalls until [`ryupdm::deps::http::STALL`], holds up
/// only the request that asked. What it fetched goes into the session,
/// where the request then finds it ([`fetched`]); `None` for any other
/// request, or one that fails before fetching.
pub(crate) fn prepare(
    session: &mut Session,
    req: &crate::proto::Request,
) -> Option<Box<dyn FnOnce() -> Prefetched + Send>> {
    use crate::proto::Request;
    // Nothing an earlier request left is taken for this one's.
    session.prefetched.clear();
    let repos: Vec<(String, Vec<String>)> = match req {
        Request::DepsAdd { url, path, .. } => {
            if crate::doc::deps::url_fault(url).is_some()
                || crate::doc::deps::path_fault(path).is_some()
            {
                return None;
            }
            vec![(url.clone(), vec![path.clone()])]
        }
        Request::DepsFetch { names }
        | Request::DepsUpdate {
            names,
            offline: false,
            ..
        } => repositories(session, names).ok()?.into_iter().collect(),
        _ => return None,
    };
    let cache = session.deps_cache().ok()?;
    Some(Box::new(move || {
        let mut out = Prefetched::new();
        for (url, paths) in repos {
            let asked: Vec<&str> = paths.iter().map(String::as_str).collect();
            let r = cache.fetch(&url, &asked);
            let failed = r.is_err();
            out.insert(url, (paths, r));
            // As the request would: the first that fails ends it.
            if failed {
                break;
            }
        }
        out
    }))
}

/// Fetches `url` (`paths` in it) into `cache`, or takes what [`prepare`]
/// fetched of it already from `pre`.
fn fetched(
    cache: &ryupdm::deps::Cache,
    pre: &mut Prefetched,
    url: &str,
    paths: &[&str],
) -> Result<ryupdm::deps::Fetched> {
    match pre.remove(url) {
        Some((had, r)) if had.iter().map(String::as_str).eq(paths.iter().copied()) => r,
        _ => cache.fetch(url, paths),
    }
}

fn invalid(param: &str, why: &str) -> Error {
    Error::new(Code::DependencyInvalid, why).with("param", param)
}

/// A handle for a dependency from its folder or its repository's name:
/// `cad/fasteners`, `https://github.com/acme/fasteners.git` and a
/// Windows path `C:\cad\fasteners` are `fasteners`.
fn name_from(url: &str, path: &str) -> Option<Id> {
    let stem = match path.rsplit(['/', '\\']).next().filter(|s| !s.is_empty()) {
        Some(s) => s.to_owned(),
        None => {
            let u = url.trim_end_matches(['/', '\\']);
            let u = u.strip_suffix(".git").unwrap_or(u);
            u.rsplit(['/', '\\', ':'])
                .next()
                .unwrap_or_default()
                .to_owned()
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

/// The lock `project` (the working files as a write would leave them)
/// needs, worked out from the dependency cache alone: the labels it
/// places from other projects and everything they place, each the old
/// lock holds keeping its commit. `dependency_not_fetched`,
/// `label_not_found`, `label_not_official` and the rest for a label that
/// can't be locked.
pub(crate) fn relock(session: &mut Session, project: &Project) -> Result<crate::doc::lock::Lock> {
    let start = ryupdm::deps::lock::wanted(project);
    let old = lock_of(session.project()).ok();
    if start.is_empty() {
        return Ok(Default::default());
    }
    let own = project.manifest.project.clone();
    let deps = project.manifest.dependencies.clone();
    let reader = session.deps_reader()?;
    ryupdm::deps::lock::lock(reader, own.as_ref(), &deps, &start, old.as_ref())
}

/// [`relock`] for a write, keeping what the lock holds now: refused only
/// for the labels the write adds or changes. A label the lock already
/// holds, and what it reaches, stays as it is when the cache can't read it
/// ([`ryupdm::deps::lock::lock_keeping`]): moving an instance in a fresh
/// clone needs no fetch, and one dependency not fetched doesn't stop
/// another's part being placed.
pub(crate) fn relock_keeping(
    session: &mut Session,
    project: &Project,
) -> Result<crate::doc::lock::Lock> {
    let start = ryupdm::deps::lock::wanted(project);
    let old = lock_of(session.project()).ok();
    if start.is_empty() {
        return Ok(Default::default());
    }
    let own = project.manifest.project.clone();
    let deps = project.manifest.dependencies.clone();
    let reader = session.deps_reader()?;
    ryupdm::deps::lock::lock_keeping(reader, own.as_ref(), &deps, &start, old.as_ref())
        .map(|(lock, _)| lock)
}

/// After a write: writes `ryucad.lock` again when what the working files
/// place from other projects needs another lock (removes it when they
/// place nothing), and says whether it wrote. A lock that can't be
/// worked out (what it newly needs isn't fetched) is left as it is; a
/// write that needs a new label refuses before this ([`relock_keeping`]).
pub(crate) fn keep_lock(session: &mut Session) -> Result<bool> {
    use crate::doc::lock::LOCK;
    let p = session.project().clone();
    let path = session.root().join(LOCK);
    if p.lock.is_none() && ryupdm::deps::lock::wanted(&p).is_empty() {
        return Ok(false);
    }
    let Ok(lock) = relock_keeping(session, &p) else {
        return Ok(false);
    };
    if lock.is_empty() {
        if p.lock.is_some() {
            crate::doc::project::remove_file(session.root(), &path)?;
            return Ok(true);
        }
        return Ok(false);
    }
    if p.lock.as_ref().and_then(|l| l.as_ref().ok()) == Some(&lock) {
        return Ok(false);
    }
    crate::doc::project::write_json(&path, &lock.to_json())?;
    Ok(true)
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
    let mut pre = std::mem::take(&mut session.prefetched);
    let cache = session.deps_cache()?;
    let fetched = fetched(&cache, &mut pre, url, &[path])?;
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
    let mut pre = std::mem::take(&mut session.prefetched);
    let repos = repositories(session, names)?;
    let cache = session.deps_cache()?;
    let mut out = Vec::new();
    for (url, paths) in &repos {
        let paths: Vec<&str> = paths.iter().map(String::as_str).collect();
        let f = fetched(&cache, &mut pre, url, &paths)?;
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

/// `deps.parts`: what dependency `name` (a name or project ID) has
/// released on its release branch as fetched, offline: each part (or
/// only `part`) with its number, name and kind, and its labels latest
/// first with their states.
pub fn parts(session: &mut Session, name: &str, part: Option<&str>) -> Result<serde_json::Value> {
    let (id, d) = crate::doc::deps::find(&session.project().manifest.dependencies, name)?;
    let (id, d) = (id.clone(), d.clone());
    let only = part.map(|p| Id::parse(p, "part")).transpose()?;
    let (branch, tip, list) =
        ryupdm::deps::parts::parts(session.deps_reader()?, &id, &d.url, &d.path)?;
    let parts: Vec<serde_json::Value> = list
        .into_iter()
        .filter(|p| only.as_ref().is_none_or(|o| *o == p.part))
        .map(|p| {
            let labels: Vec<serde_json::Value> = p
                .labels
                .iter()
                .map(
                    |l| serde_json::json!({"label": l.label, "state": l.state, "commit": l.commit}),
                )
                .collect();
            serde_json::json!({"part": p.part, "of": format!("{}:{}", d.name, p.part),
                               "number": p.number, "name": p.name, "kind": p.kind,
                               "labels": labels})
        })
        .collect();
    if let Some(o) = &only
        && parts.is_empty()
    {
        return Err(Error::new(
            Code::NotFound,
            format!("{} has released no part {o}", d.name),
        )
        .with("dependency", &d.name)
        .with("part", o));
    }
    Ok(serde_json::json!({
        "schema": SCHEMA,
        "dependency": {"name": d.name, "project": id},
        "release_branch": branch,
        "tip": tip,
        "parts": parts,
    }))
}

/// What `deps.update` is asked to move.
pub struct Update<'a> {
    /// The dependencies (names or project IDs) whose instances move: every
    /// one when empty.
    pub names: &'a [String],
    /// Only instances of this part.
    pub part: Option<&'a str>,
    /// To this label, rather than the latest.
    pub to: Option<&'a str>,
    /// Use what was fetched, fetching nothing.
    pub offline: bool,
    /// Answer what it would do, writing nothing.
    pub dry_run: bool,
}

/// One instance `deps.update` moves.
struct Move {
    assembly: Id,
    instance: Id,
    project: ProjectId,
    part: Id,
    from: String,
    to: String,
}

/// `deps.update`: fetches (unless offline), then moves every instance of
/// the named dependencies' parts (or of one part) to its part's latest
/// official label that isn't obsolete (or to `to`), writes the lock and
/// keeps assemblies current, in one logged, undoable change. It answers
/// what moved and what builds now and wouldn't after (`failing`, from a
/// rebuild of the files as they would be), and refuses nothing it can
/// explain: a mate that fails after is reported, not blocked.
pub fn update(
    session: &mut Session,
    ask: Update,
    expected: Option<&str>,
) -> Result<serde_json::Value> {
    use crate::doc::assembly::Rev;
    use serde_json::json;
    crate::config::check_project_revision(session.root(), expected)?;
    let fetched = if ask.offline {
        None
    } else {
        Some(fetch(session, ask.names)?["fetched"].clone())
    };
    let p = session.project().clone();
    let mut chosen = std::collections::BTreeSet::new();
    for n in ask.names {
        chosen.insert(
            crate::doc::deps::find(&p.manifest.dependencies, n)?
                .0
                .clone(),
        );
    }
    let only = ask.part.map(|x| Id::parse(x, "part")).transpose()?;
    let lock = lock_of(&p).unwrap_or_default();
    // Where each project is: the manifest's dependency, else the lock's.
    let place = |id: &ProjectId| -> Option<(String, String)> {
        match p.manifest.dependencies.get(id) {
            Some(d) => Some((d.url.clone(), d.path.clone())),
            None => lock
                .projects
                .get(id)
                .map(|l| (l.url.clone(), l.path.clone())),
        }
    };
    let mut released: std::collections::BTreeMap<
        ProjectId,
        Vec<ryupdm::deps::parts::ReleasedPart>,
    > = Default::default();
    let mut moves = Vec::new();
    for (asm, part) in &p.parts {
        for fid in part.order() {
            let Some(Ok(Feature::Instance(i))) = part.features.get(fid) else {
                continue;
            };
            let Rev::Foreign { project, label } = &i.rev else {
                continue;
            };
            if (!chosen.is_empty() && !chosen.contains(project))
                || only.as_ref().is_some_and(|o| *o != i.part)
            {
                continue;
            }
            if !released.contains_key(project) {
                let Some((url, path)) = place(project) else {
                    continue;
                };
                let (_, _, list) =
                    ryupdm::deps::parts::parts(session.deps_reader()?, project, &url, &path)?;
                released.insert(project.clone(), list);
            }
            let labels = released[project]
                .iter()
                .find(|r| r.part == i.part)
                .map(|r| r.labels.clone())
                .unwrap_or_default();
            let target = match ask.to {
                Some(t) => match labels.iter().find(|l| l.label == t) {
                    Some(l) => l.label.clone(),
                    None => {
                        return Err(Error::new(
                            Code::LabelNotFound,
                            format!(
                                "{} has no official release {t} of {}",
                                name_of(&p, project),
                                i.part
                            ),
                        )
                        .with("project", project)
                        .with("part", &i.part)
                        .with("label", t));
                    }
                },
                None => match labels.iter().find(|l| l.state == "released") {
                    Some(l) => l.label.clone(),
                    None => continue,
                },
            };
            if target != *label {
                moves.push(Move {
                    assembly: asm.clone(),
                    instance: fid.clone(),
                    project: project.clone(),
                    part: i.part.clone(),
                    from: label.clone(),
                    to: target,
                });
            }
        }
    }
    // The files as the moves leave them, and the lock they need.
    let mut after = p.clone();
    for m in &moves {
        let part = after.parts.get_mut(&m.assembly).expect("listed");
        if let Some(Ok(Feature::Instance(i))) = part.features.get_mut(&m.instance) {
            i.rev = Rev::Foreign {
                project: m.project.clone(),
                label: m.to.clone(),
            };
        }
    }
    let new_lock = relock(session, &after)?;
    let mut writes: std::collections::BTreeMap<String, Option<Vec<u8>>> = Default::default();
    for m in &moves {
        let f = &after.parts[&m.assembly].features[&m.instance];
        let at = format!("parts/{}/features/{}.json", m.assembly, m.instance);
        let v = f.as_ref().expect("an instance").to_json();
        writes.insert(at, Some(crate::doc::canonical::to_string(&v).into_bytes()));
    }
    let lock_path = crate::doc::lock::LOCK.to_owned();
    let lock_changed = p.lock.as_ref().and_then(|l| l.as_ref().ok()) != Some(&new_lock)
        && !(p.lock.is_none() && new_lock.is_empty());
    if lock_changed {
        let bytes = (!new_lock.is_empty())
            .then(|| crate::doc::canonical::to_string(&new_lock.to_json()).into_bytes());
        writes.insert(lock_path, bytes);
    }
    let now = crate::import::failing(session)?;
    let mut proposed = session.proposed(&writes)?;
    let later = crate::import::failing(&mut proposed)?;
    let failing: Vec<serde_json::Value> = later
        .into_iter()
        .filter(|(k, _)| !now.contains_key(k))
        .map(|((part, feature), e)| json!({"part": part, "feature": feature, "error": e}))
        .collect();
    let moved: Vec<serde_json::Value> = moves
        .iter()
        .map(|m| {
            json!({"assembly": m.assembly, "instance": m.instance, "part": m.part,
                   "dependency": name_of(&p, &m.project), "project": m.project,
                   "from": m.from, "to": m.to})
        })
        .collect();
    let mut v = json!({"schema": SCHEMA, "dry_run": ask.dry_run, "moved": moved,
                       "failing": failing, "lock_changed": lock_changed});
    if let Some(f) = fetched {
        v["fetched"] = f;
    }
    if ask.dry_run || writes.is_empty() {
        return Ok(v);
    }
    // Assemblies current until now stay so.
    let mut current = Vec::new();
    for m in &moves {
        for a in crate::mate::current(session, &m.assembly)? {
            if !current.contains(&a) {
                current.push(a);
            }
        }
    }
    for (path, bytes) in &writes {
        let at = session.root().join(path);
        match bytes {
            Some(b) => crate::doc::source::write_atomic(&at, b)?,
            None => crate::doc::project::remove_file(session.root(), &at)?,
        }
    }
    session.reload()?;
    let assemblies = crate::mate::keep_current(session, &current)?;
    if !assemblies.is_empty() {
        v["assemblies"] = json!(assemblies);
    }
    v["project_revision"] = json!(crate::config::project_revision(session.root())?);
    Ok(v)
}

/// `deps.outdated`, offline: every instance, of the project's own parts
/// or another project's, at a label older than its part's latest official
/// label that isn't obsolete (in the scheme's order), with that label.
/// Another project's latest is what was fetched; outside a repository the
/// project's own labels are never outdated (nothing is official).
pub fn outdated(session: &mut Session) -> Result<serde_json::Value> {
    use crate::doc::assembly::Rev;
    use serde_json::json;
    let p = session.project().clone();
    let config = ryupdm::config::Config::read(&*session).unwrap_or_default();
    let own = match crate::release::history(session) {
        Ok((_, r)) => ryupdm::deps::parts::latest(&r, config.revisions),
        Err(e) if e.code == Code::NotAGitRepo => Default::default(),
        Err(e) => return Err(e),
    };
    let lock = lock_of(&p).unwrap_or_default();
    // Each dependency's scheme, with each part's latest label in it.
    #[allow(clippy::type_complexity)]
    let mut theirs: std::collections::BTreeMap<
        ProjectId,
        (
            ryupdm::config::Scheme,
            std::collections::BTreeMap<Id, String>,
        ),
    > = Default::default();
    let mut out = Vec::new();
    for (asm, part) in &p.parts {
        for fid in part.order() {
            let Some(Ok(Feature::Instance(i))) = part.features.get(fid) else {
                continue;
            };
            let (latest, scheme, project) = match &i.rev {
                Rev::Working => continue,
                Rev::Label(_) => (own.get(&i.part).cloned(), config.revisions, None),
                Rev::Foreign { project, .. } => {
                    if !theirs.contains_key(project) {
                        let place = match p.manifest.dependencies.get(project) {
                            Some(d) => Some((d.url.clone(), d.path.clone())),
                            None => lock
                                .projects
                                .get(project)
                                .map(|l| (l.url.clone(), l.path.clone())),
                        };
                        // Unknown until fetched: its parts have no latest.
                        let mut found = (config.revisions, Default::default());
                        if let Some((url, path)) = place {
                            let reader = session.deps_reader()?;
                            if let Some(repo) = reader.repo(&url)? {
                                let c =
                                    ryupdm::deps::config_at(&repo, ryupdm::deps::HEAD_REF, &path)?;
                                if let Some(r) = reader.releases(&url, &path, &c.release_branch)? {
                                    found =
                                        (c.revisions, ryupdm::deps::parts::latest(r, c.revisions));
                                }
                            }
                        }
                        theirs.insert(project.clone(), found);
                    }
                    let (scheme, latest) = &theirs[project];
                    (latest.get(&i.part).cloned(), *scheme, Some(project))
                }
            };
            let label = i.rev.label().expect("a label").to_owned();
            let Some(latest) = latest.filter(|l| *l != label) else {
                continue;
            };
            // Older by its project's scheme: a label past the latest isn't
            // outdated.
            let older = ryupdm::label::index(scheme, &label)
                .zip(ryupdm::label::index(scheme, &latest))
                .is_none_or(|(a, b)| a < b);
            if !older {
                continue;
            }
            let mut v = json!({"assembly": asm, "instance": fid, "part": i.part,
                               "label": label, "latest": latest});
            if let Some(project) = project {
                v["project"] = json!(project);
                v["dependency"] = json!(name_of(&p, project));
            }
            out.push(v);
        }
    }
    Ok(json!({"schema": SCHEMA, "outdated": out}))
}

/// `deps.uses`, offline: every other project's `(project, part, label)`
/// the project's assemblies place at `rev` (the working files by default,
/// else a Git revision), at any depth, with each placing assembly, path
/// and count, and the same for every official release of its assemblies,
/// read at their commits ([`ryupdm::where_used::placed_from_others`]):
/// what the review service indexes for organization-wide where-used.
pub fn uses(session: &mut Session, rev: Option<&str>) -> Result<serde_json::Value> {
    let (commit, mut v) = crate::bom::reading(session, rev, |reader, g| {
        Ok(ryupdm::where_used::placed_from_others(reader, &g.config))
    })?;
    v["rev"] = serde_json::json!(rev.unwrap_or("working"));
    if let Some(c) = commit {
        v["commit"] = serde_json::json!(c);
    }
    Ok(v)
}

/// `deps.remove`: takes dependency `name` (a name or project ID) out of
/// the manifest, and the lock with it; `has_dependents` while an instance
/// places its parts or the lock reaches projects through it. One logged,
/// undoable change guarded by the project's revision.
pub fn remove(
    session: &mut Session,
    name: &str,
    expected: Option<&str>,
) -> Result<serde_json::Value> {
    crate::config::check_project_revision(session.root(), expected)?;
    let mut m = session.project().manifest.clone();
    let (id, d) = crate::doc::deps::find(&m.dependencies, name)?;
    let (id, d) = (id.clone(), d.clone());
    let mut placing = Vec::new();
    for (asm, part) in &session.project().parts {
        for fid in part.order() {
            if let Some(Ok(Feature::Instance(i))) = part.features.get(fid)
                && i.rev.project() == Some(&id)
            {
                placing.push(format!("{asm}/{fid}"));
            }
        }
    }
    let through: Vec<ProjectId> = lock_of(session.project())
        .unwrap_or_default()
        .projects
        .iter()
        .filter(|(_, l)| l.via.contains(&id))
        .map(|(p, _)| p.clone())
        .collect();
    if !placing.is_empty() || !through.is_empty() {
        return Err(Error::new(
            Code::HasDependents,
            format!(
                "{} is still placed ({}) or reached through ({} other projects); remove those instances first",
                d.name,
                placing.join(", "),
                through.len()
            ),
        )
        .with("dependency", &d.name)
        .with("instances", &placing)
        .with("projects", &through));
    }
    m.dependencies.remove(&id);
    save_manifest(session, &m)?;
    if keep_lock(session)? {
        session.reload()?;
    }
    Ok(serde_json::json!({
        "schema": SCHEMA,
        "removed": {"name": d.name, "project": id},
        "project_revision": crate::config::project_revision(session.root())?,
    }))
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

    /// Another process (a worktree's daemon, a `--no-daemon` run) fetching
    /// into the machine's cache is seen at the next rebuild: a label that
    /// couldn't be read for want of a fetch isn't remembered as failing.
    #[test]
    fn a_fetch_by_another_process_is_seen_without_a_restart() {
        let (lib_dir, lib) = library();
        let (dir, mut s) = project();
        let cache = ryupdm::deps::Cache::new(dir.path().join("cache"));
        s.deps_cache = Some(cache.clone());
        depend(&mut s, &lib, lib_dir.path());
        place_foreign(dir.path(), "frame", "b", &lib, "p", "A");
        s.refresh().unwrap();
        cache.fetch(&url(lib_dir.path()), &[""]).unwrap();
        write_lock(&mut s);
        std::fs::remove_dir_all(dir.path().join("cache")).unwrap();
        s.deps_fetched();
        let r = s.rebuilt(&id("frame")).unwrap().clone();
        assert_eq!(
            r.features[0].1.as_ref().unwrap_err().details["cause"],
            "dependency_not_fetched"
        );
        // Fetched behind this session's back.
        ryupdm::deps::Cache::new(dir.path().join("cache"))
            .fetch(&url(lib_dir.path()), &[""])
            .unwrap();
        let r = s.rebuilt(&id("frame")).unwrap().clone();
        assert!(r.features[0].1.is_ok(), "{:?}", r.features[0].1);
    }

    /// Puts the project at `root` in a repository of its own (its cache
    /// folder left out), numbers assembly `asm` and its instance of `lib`'s
    /// `p` (as [`place_foreign`] wrote it, by hand), releases it as `A`
    /// and commits.
    pub(crate) fn release_in_git(
        s: &mut Session,
        root: &Path,
        asm: &str,
        lib: &ProjectId,
        message: &str,
    ) {
        git(root, &["init", "-q", "-b", "main"]);
        std::fs::write(root.join(".git/info/exclude"), "cache/\n").unwrap();
        call(
            s,
            "props.set",
            json!({"part": asm, "properties": {"number": "900"}}),
        )
        .unwrap();
        let list = part_path(root, &id(asm));
        let mut f: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&list).unwrap()).unwrap();
        f["items"] = json!({format!("{lib}:p"): 1});
        f["format"] = json!(4);
        write_json(&list, &f).unwrap();
        s.refresh().unwrap();
        call(s, "release.propose", json!({"part": asm})).unwrap();
        git(root, &["add", "."]);
        git(root, &["commit", "-q", "-m", message]);
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
    fn uses_lists_other_projects_parts_placed_now_and_in_releases() {
        let (_lib_dir, dir, lib, mut s) = placing();
        let v = call(&mut s, "deps.uses", json!({})).unwrap();
        assert_eq!((&v["schema"], &v["rev"]), (&json!(1), &json!("working")));
        assert_eq!(v["project"], json!(s.project().manifest.project));
        assert_eq!(
            v["working"],
            json!([{"assembly": "frame", "places": [{"project": lib, "dependency": "lib",
                    "part": "p", "label": "A", "count": 1,
                    "uses": [{"via": ["frame"], "count": 1}]}]}])
        );
        assert_eq!(v["released"], json!([]));
        // Released and committed: its release says the same, at its commit.
        let root = dir.path();
        release_in_git(&mut s, root, "frame", &lib, "frame A");
        let v = call(&mut s, "deps.uses", json!({"rev": "HEAD"})).unwrap();
        let r = &v["released"][0];
        assert_eq!(
            (&r["assembly"], &r["label"]),
            (&json!("frame"), &json!("A"))
        );
        assert_eq!(r["commit"], v["commit"]);
        assert_eq!(r["places"], v["working"][0]["places"]);
    }

    #[test]
    fn a_dependency_is_named_after_its_last_folder() {
        let name = |url, path| super::name_from(url, path).map(|i| i.to_string());
        let want = Some("fasteners".to_owned());
        assert_eq!(name("https://github.com/acme/fasteners.git", ""), want);
        assert_eq!(name("git@github.com:acme/fasteners.git", ""), want);
        assert_eq!(name("git@github.com:fasteners", ""), want);
        assert_eq!(name("/srv/git/fasteners/", ""), want);
        assert_eq!(name(r"C:\Users\me\cad\fasteners", ""), want);
        assert_eq!(name(r"file://C:\Users\me\fasteners.git\", ""), want);
        assert_eq!(name("file:///C:/Users/me/fasteners", ""), want);
        assert_eq!(name("https://example.com/x/lib", "cad/fasteners"), want);
        assert_eq!(name("https://example.com/x/lib", r"cad\fasteners"), want);
    }

    /// The daemon fetches without its lock: `prepare` finds the fetches
    /// and makes them, and the request takes what they gave rather than
    /// fetching again under the lock.
    #[test]
    fn the_daemon_fetches_without_its_lock() {
        let (lib_dir, _lib) = library();
        let (dir, s) = crate::server::tests::fresh();
        let mut slot = Some(s);
        let session = slot.as_mut().unwrap();
        session.deps_cache = Some(ryupdm::deps::Cache::new(dir.path().join("cache")));
        let u = url(lib_dir.path());
        let req: crate::proto::Request =
            serde_json::from_value(json!({"method": "deps.add", "params": {"url": u}})).unwrap();
        let job = crate::server::prepare(&mut slot, &req).expect("a fetch to make");
        let give = job();
        give(slot.as_mut().unwrap());
        assert_eq!(slot.as_ref().unwrap().prefetched.len(), 1);
        // The remote gone, the request still answers from what was fetched.
        let moved = lib_dir.path().with_extension("moved");
        std::fs::rename(lib_dir.path(), &moved).unwrap();
        let s = slot.as_mut().unwrap();
        let a = call(s, "deps.add", json!({"url": u})).unwrap();
        std::fs::rename(&moved, lib_dir.path()).unwrap();
        assert_eq!(a["fetched"]["release_branch"], "main");
        assert!(s.prefetched.is_empty());
        // Requests that fetch nothing prepare nothing.
        let req: crate::proto::Request =
            serde_json::from_value(json!({"method": "deps.list", "params": {}})).unwrap();
        assert!(crate::server::prepare(&mut slot, &req).is_none());
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
        // What it released, by its name.
        let r = call(&mut s, "deps.parts", json!({"name": name})).unwrap();
        assert_eq!(r["parts"][0]["part"], "p");
        assert_eq!(r["parts"][0]["of"], format!("{name}:p"));
        assert_eq!(r["parts"][0]["number"], "100");
        assert_eq!(r["parts"][0]["kind"], "part");
        assert_eq!(r["parts"][0]["labels"][0]["label"], "A");
        assert_eq!(r["parts"][0]["labels"][0]["state"], "released");
        let e = call(&mut s, "deps.parts", json!({"name": name, "part": "q"})).unwrap_err();
        assert_eq!(e.code, Code::NotFound);
        // The cache gone: listed as not fetched, offline.
        std::fs::remove_dir_all(dir.path().join("cache")).unwrap();
        let l = call(&mut s, "deps.list", json!({})).unwrap();
        assert_eq!(l["dependencies"][0]["fetched"], false);
    }

    /// A write refuses only on the labels it adds: a clone with its lock
    /// committed and nothing fetched moves a foreign instance, and adding
    /// another library's part doesn't wait on the first library's fetch.
    #[test]
    fn a_write_judges_only_the_labels_it_adds() {
        use crate::doc::lock::LOCK;
        let (_lib_dir, dir, lib, mut s) = placing();
        let before = std::fs::read(dir.path().join(LOCK)).unwrap();
        std::fs::remove_dir_all(dir.path().join("cache")).unwrap();
        s.deps_fetched();
        // A pose: nothing the lock holds changes.
        call(
            &mut s,
            "feature.edit",
            json!({"part": "frame", "id": "b",
                   "placement": {"origin": ["5 mm", "0 mm", "0 mm"]}}),
        )
        .unwrap();
        assert_eq!(std::fs::read(dir.path().join(LOCK)).unwrap(), before);
        // A second library, fetched; the first still isn't.
        let (other_dir, other) = library();
        let path = s.root().join(MANIFEST);
        let text = std::fs::read_to_string(&path).unwrap();
        let mut m = Manifest::from_json(&crate::doc::canonical::parse(&text).unwrap()).unwrap();
        m.dependencies.insert(
            other.clone(),
            crate::doc::deps::Dependency {
                name: "other".into(),
                url: url(other_dir.path()),
                path: "".into(),
            },
        );
        write_json(&path, &m.to_json()).unwrap();
        s.refresh().unwrap();
        ryupdm::deps::Cache::new(dir.path().join("cache"))
            .fetch(&url(other_dir.path()), &[""])
            .unwrap();
        s.deps_fetched();
        call(
            &mut s,
            "feature.add",
            json!({"part": "frame", "type": "instance", "id": "o", "of": "other:p", "rev": "A"}),
        )
        .unwrap();
        // The lock gains the new label and keeps the first library's.
        let lock = super::lock_of(s.project()).unwrap();
        assert!(lock.label(&other, &id("p"), "A").is_some());
        assert!(lock.label(&lib, &id("p"), "A").is_some());
        // A label the write adds is still judged.
        let e = call(
            &mut s,
            "feature.add",
            json!({"part": "frame", "type": "instance", "id": "c", "of": "lib:p", "rev": "B"}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::DependencyNotFetched);
    }

    #[test]
    fn placing_another_projects_part_writes_the_lock_with_it() {
        use crate::doc::lock::LOCK;
        let (lib_dir, lib) = library();
        let (dir, mut s) = project();
        let cache = ryupdm::deps::Cache::new(dir.path().join("cache"));
        s.deps_cache = Some(cache.clone());
        depend(&mut s, &lib, lib_dir.path());
        let add = |s: &mut Session, rev: &str| {
            call(
                s,
                "feature.add",
                json!({"part": "frame", "type": "instance", "id": "b", "of": "lib:p", "rev": rev}),
            )
        };
        // Not fetched: refused, naming the command, nothing written.
        let e = add(&mut s, "A").unwrap_err();
        assert_eq!(
            (e.code, &e.details["command"]),
            (Code::DependencyNotFetched, &json!("ryucad deps fetch"))
        );
        assert!(!dir.path().join(LOCK).exists());
        call(&mut s, "deps.fetch", json!({})).unwrap();
        // Only a label the dependency released, and only a label.
        let e = add(&mut s, "working").unwrap_err();
        assert_eq!(
            (e.code, &e.details["param"]),
            (Code::InstanceForeignWorking, &json!("rev"))
        );
        let e = add(&mut s, "B").unwrap_err();
        assert_eq!(e.code, Code::LabelNotFound);
        let e = call(
            &mut s,
            "feature.add",
            json!({"part": "frame", "type": "instance", "id": "b", "of": "nope:p", "rev": "A"}),
        )
        .unwrap_err();
        assert_eq!(
            (e.code, &e.details["param"]),
            (Code::NotFound, &json!("of"))
        );
        add(&mut s, "A").unwrap();
        // The lock came with it, and its item number is its own.
        let lock = super::lock_of(s.project()).unwrap();
        assert_eq!(
            lock.label(&lib, &id("p"), "A").unwrap().commit,
            crate::git::repo(lib_dir.path())
                .unwrap()
                .locate("HEAD")
                .unwrap()
                .0
        );
        let list = s
            .project()
            .part(&id("frame"))
            .unwrap()
            .file
            .clone()
            .unwrap();
        assert_eq!(list.items[&crate::doc::id::Id::foreign(&lib, &id("p"))], 1);
        let r = s.rebuilt(&id("frame")).unwrap().clone();
        assert!(r.output(&id("b")).is_some(), "{:?}", r.features);
        // Edited to the working files: refused.
        let e = call(
            &mut s,
            "feature.edit",
            json!({"part": "frame", "id": "b", "rev": "working"}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::InstanceForeignWorking);
        // Deleted, the lock goes with the last of them.
        call(
            &mut s,
            "feature.delete",
            json!({"part": "frame", "id": "b"}),
        )
        .unwrap();
        assert!(!dir.path().join(LOCK).exists());
        assert!(s.project().lock.is_none());
    }

    /// Releases the library's `p` again as its next label, made longer.
    pub(crate) fn release_again(lib_dir: &Path) {
        let mut lib = Session::open(lib_dir).unwrap();
        call(
            &mut lib,
            "feature.edit",
            json!({"part": "p", "id": "b", "params": {"x": 30}}),
        )
        .unwrap();
        call(&mut lib, "release.propose", json!({"part": "p"})).unwrap();
        git(lib_dir, &["add", "."]);
        git(lib_dir, &["commit", "-q", "-m", "p B"]);
    }

    #[test]
    fn an_update_moves_instances_to_the_latest_release() {
        let (lib_dir, dir, lib, mut s) = placing();
        release_again(lib_dir.path());
        // Offline, B isn't known yet: nothing moves.
        let v = call(&mut s, "deps.update", json!({"offline": true})).unwrap();
        assert_eq!(v["moved"], json!([]));
        // Fetched: a dry run says what would move and writes nothing.
        let v = call(&mut s, "deps.update", json!({"dry_run": true})).unwrap();
        assert_eq!(
            v["moved"],
            json!([{"assembly": "frame", "instance": "b", "part": "p", "dependency": "lib",
                    "project": lib, "from": "A", "to": "B"}])
        );
        assert_eq!(v["failing"], json!([]));
        assert_eq!(v["lock_changed"], true);
        let file =
            || std::fs::read_to_string(dir.path().join("parts/frame/features/b.json")).unwrap();
        assert!(file().contains("\"A\""));
        let v = call(&mut s, "deps.update", json!({})).unwrap();
        assert_eq!(v["moved"][0]["to"], "B");
        assert!(file().contains("\"B\""));
        let lock = super::lock_of(s.project()).unwrap();
        assert!(lock.label(&lib, &id("p"), "B").is_some());
        assert!(lock.label(&lib, &id("p"), "A").is_none());
        // B is the longer box.
        let m = s.model(&id("frame"), &Rev::Working).unwrap();
        assert_eq!(
            m.occurrences[0].rev,
            Rev::Foreign {
                project: lib.clone(),
                label: "B".into()
            }
        );
        // Back to A, by name and label.
        let v = call(
            &mut s,
            "deps.update",
            json!({"names": ["lib"], "to": "A", "offline": true}),
        )
        .unwrap();
        assert_eq!(
            (&v["moved"][0]["from"], &v["moved"][0]["to"]),
            (&json!("B"), &json!("A"))
        );
        let e = call(&mut s, "deps.update", json!({"to": "Z", "offline": true})).unwrap_err();
        assert_eq!(e.code, Code::LabelNotFound);
    }

    #[test]
    fn outdated_instances_are_listed_and_a_dependency_removed_once_unplaced() {
        let (lib_dir, dir, lib, mut s) = placing();
        assert_eq!(
            call(&mut s, "deps.outdated", json!({})).unwrap()["outdated"],
            json!([])
        );
        release_again(lib_dir.path());
        // Offline, from what was fetched: nothing newer yet.
        assert_eq!(
            call(&mut s, "deps.outdated", json!({})).unwrap()["outdated"],
            json!([])
        );
        call(&mut s, "deps.fetch", json!({})).unwrap();
        let v = call(&mut s, "deps.outdated", json!({})).unwrap();
        assert_eq!(
            v["outdated"],
            json!([{"assembly": "frame", "instance": "b", "part": "p", "label": "A", "latest": "B",
                    "project": lib, "dependency": "lib"}])
        );
        // Placed: it can't go.
        let e = call(&mut s, "deps.remove", json!({"name": "lib"})).unwrap_err();
        assert_eq!(
            (e.code, &e.details["instances"]),
            (Code::HasDependents, &json!(["frame/b"]))
        );
        call(
            &mut s,
            "feature.delete",
            json!({"part": "frame", "id": "b"}),
        )
        .unwrap();
        let v = call(&mut s, "deps.remove", json!({"name": "lib"})).unwrap();
        assert_eq!(v["removed"]["project"], lib.as_str());
        assert!(s.project().manifest.dependencies.is_empty());
        assert!(!dir.path().join(crate::doc::lock::LOCK).exists());
        let e = call(&mut s, "deps.remove", json!({"name": "lib"})).unwrap_err();
        assert_eq!(e.code, Code::NotFound);
    }

    /// A dependency's labels are older or newer by its own scheme, not
    /// this project's: a library numbering its releases, its `3` obsolete,
    /// placed at `3` and at `1` by a project lettering its own.
    #[test]
    fn a_dependencys_labels_are_ordered_by_its_own_scheme() {
        let (lib_dir, mut l) = crate::release::tests::released_project();
        call(&mut l, "pdm.configure", json!({"revisions": "numeric"})).unwrap();
        call(&mut l, "release.propose", json!({"part": "p"})).unwrap();
        git(lib_dir.path(), &["add", "."]);
        git(lib_dir.path(), &["commit", "-q", "-m", "p 1"]);
        release_again(lib_dir.path());
        let mut l = Session::open(lib_dir.path()).unwrap();
        call(
            &mut l,
            "feature.edit",
            json!({"part": "p", "id": "b", "params": {"x": 40}}),
        )
        .unwrap();
        call(&mut l, "release.propose", json!({"part": "p"})).unwrap();
        git(lib_dir.path(), &["add", "."]);
        git(lib_dir.path(), &["commit", "-q", "-m", "p 3"]);
        call(
            &mut l,
            "release.obsolete",
            json!({"part": "p", "label": "3"}),
        )
        .unwrap();
        git(lib_dir.path(), &["add", "."]);
        git(lib_dir.path(), &["commit", "-q", "-m", "3 is obsolete"]);
        let lib = l.project().manifest.project.clone().unwrap();
        let (dir, mut s) = project();
        let cache = ryupdm::deps::Cache::new(dir.path().join("cache"));
        s.deps_cache = Some(cache.clone());
        depend(&mut s, &lib, lib_dir.path());
        place_foreign(dir.path(), "frame", "new", &lib, "p", "3");
        place_foreign(dir.path(), "frame", "old", &lib, "p", "1");
        s.refresh().unwrap();
        cache.fetch(&url(lib_dir.path()), &[""]).unwrap();
        write_lock(&mut s);
        let v = call(&mut s, "deps.outdated", json!({})).unwrap();
        assert_eq!(
            v["outdated"],
            json!([{"assembly": "frame", "instance": "old", "part": "p", "label": "1", "latest": "2",
                    "project": lib, "dependency": "lib"}])
        );
    }

    #[test]
    fn the_projects_own_labels_are_outdated_too() {
        let (dir, mut s) = crate::assembly::tests::released();
        crate::assembly::tests::place_in(
            dir.path(),
            "frame",
            "old",
            "bolt",
            json!({"label": "A"}),
            [0.0; 3],
        );
        s.refresh().unwrap();
        call(&mut s, "release.propose", json!({"part": "bolt"})).unwrap();
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-q", "-m", "bolt B"]);
        let v = call(&mut s, "deps.outdated", json!({})).unwrap();
        assert_eq!(
            v["outdated"],
            json!([{"assembly": "frame", "instance": "old", "part": "bolt", "label": "A", "latest": "B"}])
        );
    }
}
