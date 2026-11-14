//! Library labels (design 629): pins, the lock, fetching and updating
//! libraries in other repositories, and what a project places from them.

use crate::error::{Code, Error, Result};
use crate::id::{Id, ProjectId};
use crate::project::{Alias, LOCK, LOCK_KIND, MANIFEST, Manifest};
use crate::session::Session;
use ryu_doc::lock::{Lock, Locked};
use serde_json::{Value, json};
use std::collections::BTreeSet;

/// What ryueda's messages tell an agent to run.
pub const EDA: ryupdm::deps::read::Commands = ryupdm::deps::read::Commands {
    fetch: "ryueda lib fetch",
    update: "ryueda lib update",
    relock: "ryueda lib update --offline",
    lock: LOCK,
};

/// A label the lock must hold: (project, component, label).
pub type Wanted = (ProjectId, Id, String);

/// `ryueda.lock` read: `lock_invalid` when it doesn't (a conflict or a
/// newer format passes through).
pub fn read_lock(bytes: &[u8]) -> Result<Lock> {
    let v = std::str::from_utf8(bytes)
        .ok()
        .and_then(|t| crate::canonical::parse(t).ok())
        .ok_or_else(|| {
            Error::new(Code::LockInvalid, format!("{LOCK} isn't JSON")).with("path", LOCK)
        })?;
    if let Some(e) = ryu_doc::files::conflicted_in(&v, "", crate::pdm::NAME) {
        return Err(e.with("path", LOCK));
    }
    Lock::from_json_as(&v, LOCK_KIND).map_err(|e| match e.code {
        Code::MergeConflict | Code::FormatTooNew => e.with("path", LOCK),
        _ => Error::new(Code::LockInvalid, format!("{LOCK}: {}", e.message))
            .with("path", LOCK)
            .with(
                "pointer",
                e.details.get("pointer").cloned().unwrap_or_default(),
            ),
    })
}

/// The lock's text, `None` when it holds nothing (the file is removed).
pub fn lock_text(lock: &Lock) -> Option<String> {
    (!lock.is_empty()).then(|| crate::canonical::to_string(&lock.to_json_as(LOCK_KIND)))
}

/// Every pin of a library in another repository.
pub fn wanted_labels(m: &Manifest) -> BTreeSet<Wanted> {
    m.libraries
        .values()
        .filter(|a| a.url.is_some())
        .filter_map(|a| Some((a.project.clone()?, a)))
        .flat_map(|(p, a)| {
            a.labels
                .iter()
                .map(move |(c, l)| (p.clone(), c.clone(), l.clone()))
        })
        .collect()
}

/// What the lock lacks, and what it holds that nothing pins.
pub fn judge(m: &Manifest, lock: &Lock) -> (Vec<Wanted>, Vec<Wanted>) {
    let wanted = wanted_labels(m);
    let held: BTreeSet<Wanted> = lock
        .projects
        .iter()
        .flat_map(|(p, l)| {
            l.labels
                .keys()
                .map(move |(c, label)| (p.clone(), c.clone(), label.clone()))
        })
        .collect();
    (
        wanted.difference(&held).cloned().collect(),
        held.difference(&wanted).cloned().collect(),
    )
}

/// The lock `m` needs: each url alias's project with where it is and its
/// release branch (from `old`, else as fetched), each pinned label kept
/// from `old` unless `renew` names its project, else resolved where its
/// release branch as fetched makes it official.
pub fn relock(
    reader: &mut ryupdm::deps::read::Reader,
    m: &Manifest,
    old: Option<&Lock>,
    renew: &BTreeSet<ProjectId>,
) -> Result<Lock> {
    let mut out = Lock::default();
    for (alias, a) in &m.libraries {
        let (Some(url), Some(project)) = (&a.url, &a.project) else {
            continue;
        };
        if a.labels.is_empty() {
            continue;
        }
        let kept = old
            .and_then(|o| o.projects.get(project))
            .filter(|l| l.url == *url && l.path == a.path);
        let branch = match kept {
            Some(l) if !renew.contains(project) => l.release_branch.clone(),
            _ => {
                let repo = reader.repo(url)?.ok_or_else(|| {
                    ryupdm::deps::read::not_fetched_with(
                        project,
                        url,
                        &format!("library {alias}"),
                        &EDA,
                    )
                })?;
                ryupdm::deps::config_at(&repo, ryupdm::deps::HEAD_REF, &a.path)?.release_branch
            }
        };
        let mut locked = Locked {
            url: url.clone(),
            path: a.path.clone(),
            release_branch: branch,
            via: Vec::new(),
            labels: Default::default(),
        };
        for (c, label) in &a.labels {
            let key = (c.clone(), label.clone());
            let l = match kept.and_then(|k| k.labels.get(&key)) {
                Some(l) if !renew.contains(project) => l.clone(),
                _ => ryupdm::deps::read::official_with(reader, project, &locked, c, label, &EDA)?,
            };
            locked.labels.insert(key, l);
        }
        out.projects.insert(project.clone(), locked);
    }
    Ok(out)
}

/// Moves the dependency cache (bare repositories of other repositories'
/// libraries), else `deps` in ryueda's cache.
pub const DEPS_DIR_VAR: &str = "RYUEDA_DEPS_DIR";

/// This machine's dependency cache: ryueda's own, never ryucad's.
pub fn cache() -> Result<ryupdm::deps::Cache> {
    match std::env::var_os(DEPS_DIR_VAR).filter(|v| !v.is_empty()) {
        Some(d) => Ok(ryupdm::deps::Cache::new(std::path::PathBuf::from(d))),
        None => Ok(ryupdm::deps::Cache::new(
            crate::parts::cache_root()?.join("deps"),
        )),
    }
}

/// What a long request fetched without the lock, by URL: the folders
/// asked for and the fetch.
pub(crate) type Prefetched =
    std::collections::BTreeMap<String, (Vec<String>, Result<ryupdm::deps::Fetched>)>;

/// Each url alias's repository (those named, or all), with its folders
/// and the aliases naming it.
fn by_url(
    m: &Manifest,
    names: &[String],
) -> std::collections::BTreeMap<String, (Vec<String>, Vec<String>)> {
    let mut out: std::collections::BTreeMap<String, (Vec<String>, Vec<String>)> =
        Default::default();
    for (alias, a) in &m.libraries {
        let Some(url) = &a.url else { continue };
        if !names.is_empty() && !names.iter().any(|n| n == alias.as_str()) {
            continue;
        }
        let e = out.entry(url.clone()).or_default();
        e.0.push(alias.to_string());
        if !e.1.contains(&a.path) {
            e.1.push(a.path.clone());
        }
    }
    out
}

/// The repositories `req` fetches (url, folders), found under the lock.
fn wanted(session: &Session, req: &crate::proto::Request) -> Option<Vec<(String, Vec<String>)>> {
    use crate::proto::Request;
    let m = &session.project().manifest;
    let repos = |names: &[String]| -> Vec<(String, Vec<String>)> {
        by_url(m, names)
            .into_iter()
            .map(|(u, (_, paths))| (u, paths))
            .collect()
    };
    match req {
        Request::LibAdd {
            url: Some(url),
            path,
            ..
        } => Some(vec![(url.clone(), vec![path.clone()])]),
        Request::LibFetch { aliases } => Some(repos(aliases)),
        Request::LibUpdate {
            aliases,
            offline: false,
            ..
        } => Some(repos(aliases)),
        _ => None,
    }
}

/// A long request's fetches, run by the daemon with its lock released:
/// phase `fetch`, one repository a step, cancelled between them.
pub(crate) fn prepare(
    session: &mut Session,
    req: &crate::proto::Request,
    job: &ryu_session::job::Job,
) -> Option<Box<dyn FnOnce() -> Prefetched + Send>> {
    let repos = wanted(session, req)?;
    let cache = cache().ok()?;
    let job = job.clone();
    Some(Box::new(move || fetch_all(&cache, &repos, &job)))
}

/// Fetches each repository, stopping at a cancel or the first failure.
pub(crate) fn fetch_all(
    cache: &ryupdm::deps::Cache,
    repos: &[(String, Vec<String>)],
    job: &ryu_session::job::Job,
) -> Prefetched {
    let mut out = Prefetched::new();
    let stage = (!repos.is_empty())
        .then(|| job.begin(ryu_session::job::Phase::Fetch, "repositories", repos.len()));
    for (url, paths) in repos {
        if let Some(s) = &stage
            && let Err(e) = s.step(|| url.clone())
        {
            out.insert(url.clone(), (paths.clone(), Err(e)));
            break;
        }
        let refs: Vec<&str> = paths.iter().map(String::as_str).collect();
        let got = cache.fetch(url, &refs);
        let failed = got.is_err();
        out.insert(url.clone(), (paths.clone(), got));
        if failed {
            break;
        }
    }
    out
}

/// `lib.add` with a URL: the repository fetched (by the daemon without its
/// lock, or here), the library project at `path` there read at the
/// remote's `HEAD`, and the alias written with its project ID.
pub fn add_url(
    session: &mut Session,
    alias: &str,
    url: &str,
    path: &str,
    expected: Option<&str>,
) -> Result<Value> {
    let p = session.project();
    let id = Id::parse(alias, "library alias")?;
    if alias == crate::project::SELF {
        return Err(Error::new(
            Code::InvalidParam,
            "self is the project's own library; name the library project otherwise",
        )
        .with("param", "alias"));
    }
    if p.manifest.project_kind == crate::project::ProjectKind::Library {
        return Err(Error::new(
            Code::LibraryNested,
            "a library project can't name libraries of its own",
        ));
    }
    if p.manifest.libraries.contains_key(&id) {
        return Err(Error::new(
            Code::DuplicateId,
            format!("the project already names library {alias}"),
        )
        .with("alias", alias));
    }
    if url.trim().is_empty() {
        return Err(
            Error::new(Code::DependencyInvalid, "a library's URL can't be empty")
                .with("param", "url"),
        );
    }
    if !path.is_empty() && (!crate::project::path_ok(path) || path.split('/').any(|s| s == "..")) {
        return Err(Error::new(
            Code::DependencyInvalid,
            format!("{path:?} isn't folder names joined by /"),
        )
        .with("param", "path"));
    }
    crate::server::check_revision("the project", &p.revision, expected)?;
    let fetched = take_or_fetch(session, url, &[path.to_owned()])?;
    let (manifest, head) = crate::pin::with_reader(&*session.pins, |r| {
        let repo = r.repo(url)?.ok_or_else(|| {
            Error::new(Code::DependencyNotFetched, format!("{url} wasn't fetched"))
        })?;
        let head = repo
            .rev_parse_single(ryupdm::deps::HEAD_REF)
            .map_err(ryupdm::history::git_error)?
            .to_string();
        let bytes =
            ryupdm::history::file_at(&repo, &head, &ryupdm::deps::prefix_of(path), MANIFEST)?;
        Ok((bytes, head))
    })?;
    let missing = |why: String| Error::new(Code::LibraryMissing, why).with("url", url);
    let m = manifest
        .ok_or_else(|| missing(format!("{url} holds no ryueda project at {path:?}")))
        .and_then(|b| {
            Manifest::read(&b).map_err(|e| missing(format!("{url} at {path:?}: {}", e.message)))
        })?;
    if m.project_kind != crate::project::ProjectKind::Library {
        return Err(missing(format!(
            "{url} at {path:?} is a design project, not a library project"
        )));
    }
    let own = &session.project().manifest;
    if m.project == own.project
        || own
            .libraries
            .values()
            .any(|a| a.project.as_ref() == Some(&m.project))
    {
        return Err(Error::new(
            Code::ProjectIdDuplicate,
            format!(
                "project {} is this one or already a library here",
                m.project
            ),
        )
        .with("project", &m.project));
    }
    let mut next = own.clone();
    let mut a = Alias::local(path);
    a.url = Some(url.to_owned());
    a.project = Some(m.project.clone());
    next.libraries.insert(id.clone(), a);
    crate::write::commit(
        session,
        "lib add",
        crate::write::Writes(vec![(MANIFEST.to_owned(), Some(next.to_text()))]),
    )?;
    let branch = fetched
        .branches
        .get(path)
        .map(|(b, t)| json!({"release_branch": b, "tip": t}));
    Ok(json!({
        "library": {"alias": alias, "url": url, "path": path, "project": m.project},
        "fetched": {"url": url, "head": head, "branch": branch},
        "revision": session.project().revision,
    }))
}

/// What the daemon fetched without the lock for `url`, else a fetch now.
fn take_or_fetch(
    session: &mut Session,
    url: &str,
    paths: &[String],
) -> Result<ryupdm::deps::Fetched> {
    if let Some(mut pre) = session.fetched.take()
        && let Some((_, got)) = pre.remove(url)
    {
        return got;
    }
    let refs: Vec<&str> = paths.iter().map(String::as_str).collect();
    crate::pin::with_reader(&*session.pins, |r| r.cache.fetch(url, &refs))
}

/// `lib.fetch`: each url alias's repository (or those named) fetched into
/// the cache; nothing in the project is written. A name that isn't a url
/// alias is `library_missing`.
pub fn fetch(
    session: &mut Session,
    aliases: &[String],
    job: &ryu_session::job::Job,
) -> Result<Value> {
    let m = session.project().manifest.clone();
    for n in aliases {
        let known = Id::parse(n, "library alias")
            .ok()
            .and_then(|i| m.libraries.get(&i))
            .is_some_and(Alias::is_foreign);
        if !known {
            return Err(Error::new(
                Code::LibraryMissing,
                format!("{n} isn't a library in another repository"),
            )
            .with("alias", n));
        }
    }
    fetch_repos(session, by_url(&m, aliases), job)
}

/// Fetches each of `wanted`'s repositories (by url: its aliases and
/// folders): what the daemon fetched without the lock, else fetched now
/// under it (an alias that arrived since the daemon looked). A failure
/// stops there, its error naming the repositories already fetched.
fn fetch_repos(
    session: &mut Session,
    wanted: std::collections::BTreeMap<String, (Vec<String>, Vec<String>)>,
    job: &ryu_session::job::Job,
) -> Result<Value> {
    let mut got = match session.fetched.take() {
        Some(pre) => pre,
        None => {
            let repos: Vec<(String, Vec<String>)> = wanted
                .iter()
                .map(|(u, (_, p))| (u.clone(), p.clone()))
                .collect();
            let cache = crate::pin::with_reader(&*session.pins, |r| Ok(r.cache.clone()))?;
            fetch_all(&cache, &repos, job)
        }
    };
    let mut out = Vec::new();
    for (url, (names, paths)) in wanted {
        let r = match got.remove(&url) {
            Some((_, r)) => r,
            None => {
                let refs: Vec<&str> = paths.iter().map(String::as_str).collect();
                crate::pin::with_reader(&*session.pins, |r| r.cache.fetch(&url, &refs))
            }
        };
        let f = match r {
            Ok(f) => f,
            Err(e) => {
                // What was fetched may resolve pins now, failure or not.
                session.reload_pins()?;
                return Err(e.with("fetched", &out));
            }
        };
        out.push(json!({
            "url": url,
            "aliases": names,
            "head": f.head,
            "branches": f.branches.iter().map(|(path, (b, tip))| json!({
                "path": path,
                "release_branch": b,
                "tip": tip,
                "before": f.before.get(b),
                "changed": f.before.get(b) != tip.as_ref(),
            })).collect::<Vec<_>>(),
        }));
    }
    // What a fetch brought may resolve pins now.
    session.reload_pins()?;
    Ok(json!({"fetched": out}))
}

/// What `lib update` was asked for.
pub struct Update<'a> {
    pub aliases: &'a [String],
    pub to: Option<&'a str>,
    pub offline: bool,
    pub dry_run: bool,
}

/// `lib.update`: each pin of the named aliases (all by default) moved to
/// `to` or its component's latest official label that isn't obsolete;
/// the lock written again, renewing those aliases' projects' labels (what
/// makes a rewritten history this command's decision); what moved, what
/// the lock moved, and the ERC errors the move would bring. A component
/// with no current label stays where it is, listed in `skipped`; a local
/// label `to` names is resolved first, so no pin is written to fail.
pub fn update(
    session: &mut Session,
    ask: Update,
    expected: Option<&str>,
    job: &ryu_session::job::Job,
) -> Result<Value> {
    crate::server::check_revision("the project", &session.project().revision, expected)?;
    let m = session.project().manifest.clone();
    for n in ask.aliases {
        let known = Id::parse(n, "library alias")
            .ok()
            .is_some_and(|i| m.libraries.contains_key(&i));
        if !known {
            return Err(Error::new(
                Code::LibraryMissing,
                format!("the project names no library {n}"),
            )
            .with("alias", n));
        }
    }
    // Only the url aliases are fetched: a local one named beside them
    // isn't `fetch`'s.
    let foreign = by_url(&m, ask.aliases);
    let fetched = if ask.offline || foreign.is_empty() {
        session.fetched = None;
        None
    } else {
        Some(fetch_repos(session, foreign, job)?)
    };
    let root = session.root().to_path_buf();
    let old = session.lock();
    let chosen =
        |alias: &Id| ask.aliases.is_empty() || ask.aliases.iter().any(|n| n == alias.as_str());
    let mut next = m.clone();
    let mut moved = Vec::new();
    let mut skipped = Vec::new();
    for (alias, a) in &m.libraries {
        if !chosen(alias) {
            continue;
        }
        for (c, from) in &a.labels {
            let to = match ask.to {
                Some(l) => l.to_owned(),
                None => match latest(session, alias, a, c) {
                    Ok(l) => l,
                    Err(e) if e.code == Code::LabelNotFound => {
                        skipped.push(json!({"alias": alias, "component": c, "label": from,
                                            "code": e.code, "reason": e.message}));
                        continue;
                    }
                    Err(e) => return Err(e),
                },
            };
            if ask.to.is_some() && a.url.is_none() && &to != from {
                session.pins.pin(&root, alias, a, c, &to, old.as_ref())?;
            }
            if &to != from {
                next.libraries
                    .get_mut(alias)
                    .expect("named")
                    .labels
                    .insert(c.clone(), to.clone());
                moved.push(json!({"alias": alias, "component": c, "from": from, "to": to}));
            }
        }
    }
    let renew: BTreeSet<ProjectId> = m
        .libraries
        .iter()
        .filter(|(alias, a)| chosen(alias) && a.url.is_some())
        .filter_map(|(_, a)| a.project.clone())
        .collect();
    let lock = if wanted_labels(&next).is_empty() {
        Lock::default()
    } else {
        crate::pin::with_reader(&*session.pins, |r| relock(r, &next, old.as_ref(), &renew))?
    };
    let relocked: Vec<Value> = lock
        .projects
        .iter()
        .flat_map(|(p, l)| {
            let before = old.as_ref().and_then(|o| o.projects.get(p));
            l.labels.iter().filter_map(move |((c, label), now)| {
                let was = before?.labels.get(&(c.clone(), label.clone()))?;
                (was.commit != now.commit).then(|| {
                    json!({"project": p, "component": c, "label": label, "from": was.commit, "to": now.commit})
                })
            })
        })
        .collect();
    let text = lock_text(&lock);
    let before = session
        .files()
        .get(LOCK)
        .map(|b| String::from_utf8_lossy(b).into_owned());
    let lock_changed = text != before;
    let mut writes = Vec::new();
    if next != m {
        writes.push((MANIFEST.to_owned(), Some(next.to_text())));
    }
    if lock_changed {
        writes.push((LOCK.to_owned(), text));
    }
    let failing = if writes.is_empty() {
        Vec::new()
    } else {
        new_erc_errors(session, &writes)?
    };
    let mut v = json!({
        "dry_run": ask.dry_run,
        "moved": moved,
        "skipped": skipped,
        "relocked": relocked,
        "failing": failing,
        "lock_changed": lock_changed,
    });
    if let Some(f) = fetched {
        v["fetched"] = f["fetched"].clone();
    }
    if ask.dry_run || writes.is_empty() {
        return Ok(v);
    }
    crate::write::commit(session, "lib update", crate::write::Writes(writes))?;
    v["revision"] = json!(session.project().revision);
    Ok(v)
}

/// ERC's errors of every design after `writes` that it doesn't have now,
/// by code, design and references.
fn new_erc_errors(session: &Session, writes: &[(String, Option<String>)]) -> Result<Vec<Value>> {
    let errors = |p: &crate::project::Project| -> Vec<Value> {
        p.conn
            .designs
            .keys()
            .flat_map(|d| {
                crate::erc::check_design(p, d)
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|f| f.severity == crate::fault::Severity::Error && f.waived.is_none())
                    .map(|f| json!({"design": f.design, "code": f.code, "refs": f.refs, "message": f.message}))
            })
            .collect()
    };
    let now = errors(session.project());
    let after = crate::write::proposed(session, &crate::write::Writes(writes.to_vec()))?;
    Ok(errors(&after)
        .into_iter()
        .filter(|e| !now.contains(e))
        .collect())
}

/// What each design of project `p` places from other projects: by
/// (alias, component, label), the populated occurrences (`count`) and
/// every occurrence (`uses`), with the library's project. ryupdm's `deps
/// uses` shape, a design where ryucad has an assembly.
fn placed(p: &crate::project::Project) -> Vec<Value> {
    let mut out = Vec::new();
    for d in p.conn.designs.keys() {
        let Ok(n) = crate::netlist::flatten(p, d) else {
            continue;
        };
        let mut by: std::collections::BTreeMap<(String, String, String), (u64, u64)> =
            Default::default();
        for o in &n.occurrences {
            let Some(r) = crate::erc::of_ref(&o.of) else {
                continue;
            };
            let Some(alias) = &r.library else { continue };
            let label = p
                .manifest
                .libraries
                .get(alias)
                .and_then(|a| a.labels.get(&r.id))
                .cloned()
                .unwrap_or_else(|| "working".into());
            let e = by
                .entry((alias.to_string(), r.id.to_string(), label))
                .or_default();
            e.1 += 1;
            if !crate::erc::occurrence_dnp(p, o) {
                e.0 += 1;
            }
        }
        if by.is_empty() {
            continue;
        }
        let places: Vec<Value> = by
            .into_iter()
            .map(|((alias, c, label), (count, uses))| {
                let project = Id::parse(&alias, "library alias")
                    .ok()
                    .and_then(|a| p.manifest.libraries.get(&a))
                    .and_then(|a| a.project.clone());
                json!({"project": project, "alias": alias, "component": c, "label": label,
                       "count": count, "uses": uses})
            })
            .collect();
        out.push(json!({"design": d, "places": places}));
    }
    out
}

/// `lib.uses`: what this project places from others at `rev` (working by
/// default), and in each official release of its designs, read at its
/// commit.
pub fn uses(session: &mut Session, rev: Option<&str>) -> Result<Value> {
    let rev = rev.filter(|r| *r != "working");
    let (commit, working) = match rev {
        Some(r) => {
            let (c, p) = session.at(r)?;
            (Some(c), placed(&p))
        }
        None => (None, placed(session.project())),
    };
    let releases = crate::pdm::releases_or_none(session);
    let mut released = Vec::new();
    for (path, o) in &releases.official {
        let Some((part, label, ryupdm::record::Kind::Release)) = ryupdm::history::record_at(path)
        else {
            continue;
        };
        let Ok((_, then)) = session.at(&o.commit) else {
            continue;
        };
        if !then.conn.designs.contains_key(&part) {
            continue;
        }
        if let Some(r) = placed(&then)
            .into_iter()
            .find(|r| r["design"] == part.as_str())
        {
            released.push(json!({"design": part, "label": label, "commit": o.commit,
                                 "places": r["places"]}));
        }
    }
    Ok(json!({
        "rev": rev.unwrap_or("working"),
        "commit": commit,
        "working": working,
        "released": released,
    }))
}

/// `<alias>/<component>`.
pub(crate) fn target(t: &str) -> Result<(Id, Id)> {
    let (a, c) = t.split_once('/').ok_or_else(|| {
        Error::new(
            Code::Usage,
            format!("{t}: name a component as <alias>/<component>"),
        )
        .with("param", "components")
    })?;
    Ok((Id::parse(a, "library alias")?, Id::parse(c, "component")?))
}

/// `library_foreign_working` for a component of a library in another
/// repository: only its released labels reach this project.
pub(crate) fn working_refused(alias: &Id, a: &Alias, c: &Id) -> Result<()> {
    match &a.url {
        Some(url) => Err(Error::new(
            Code::LibraryForeignWorking,
            format!(
                "{alias}/{c} is in another repository ({url}): only its released labels can be placed"
            ),
        )
        .with("alias", alias)
        .with("component", c)),
        None => Ok(()),
    }
}

/// What `lib pin` was asked for.
pub enum Choice<'a> {
    Label(&'a str),
    Latest,
    Working,
}

/// `lib.pin`: each target's label in the manifest (and the lock, for
/// another repository's), checked now so no pin is written to fail; one
/// logged change.
pub fn pin(
    session: &mut Session,
    targets: &[String],
    choice: Choice,
    expected: Option<&str>,
) -> Result<Value> {
    let p = session.project();
    crate::server::check_revision("the project", &p.revision, expected)?;
    if p.manifest.project_kind == crate::project::ProjectKind::Library {
        return Err(Error::new(
            Code::LibraryNested,
            "a library project names no libraries, so it pins nothing",
        ));
    }
    let root = session.root().to_path_buf();
    let mut m = p.manifest.clone();
    let mut pinned = Vec::new();
    for t in targets {
        let (alias, c) = target(t)?;
        let a = m.libraries.get(&alias).cloned().ok_or_else(|| {
            Error::new(
                Code::LibraryMissing,
                format!("the project names no library {alias}"),
            )
            .with("alias", &alias)
        })?;
        let label = match &choice {
            Choice::Working => {
                working_refused(&alias, &a, &c)?;
                None
            }
            Choice::Label(l) => Some((*l).to_owned()),
            Choice::Latest => Some(latest(session, &alias, &a, &c)?),
        };
        let entry = m.libraries.get_mut(&alias).expect("named");
        let from = match &label {
            Some(l) => {
                let lock = relocked(session, &m, &alias, &c, l)?;
                let got = session.pins.pin(&root, &alias, &a, &c, l, lock.as_ref())?;
                let entry = m.libraries.get_mut(&alias).expect("named");
                entry.project = Some(got.project.clone());
                entry.labels.insert(c.clone(), l.clone())
            }
            None => entry.labels.remove(&c),
        };
        pinned.push(json!({
            "alias": alias,
            "component": c,
            "from": from.unwrap_or_else(|| "working".into()),
            "to": label.unwrap_or_else(|| "working".into()),
        }));
    }
    let mut writes = vec![(MANIFEST.to_owned(), Some(m.to_text()))];
    writes.extend(relock_writes(session, &m)?);
    crate::write::commit(session, "lib pin", crate::write::Writes(writes))?;
    Ok(json!({"pinned": pinned, "revision": session.project().revision}))
}

/// The lock a pin of `alias/c` at `label` would read through: the
/// session's, with that label locked for a library in another
/// repository.
fn relocked(
    session: &mut Session,
    m: &Manifest,
    alias: &Id,
    c: &Id,
    label: &str,
) -> Result<Option<Lock>> {
    let old = session.lock();
    if m.libraries.get(alias).is_none_or(|a| a.url.is_none()) {
        return Ok(old);
    }
    let mut next = m.clone();
    next.libraries
        .get_mut(alias)
        .expect("named")
        .labels
        .insert(c.clone(), label.to_owned());
    crate::pin::with_reader(&*session.pins, |r| {
        relock(r, &next, old.as_ref(), &Default::default())
    })
    .map(Some)
}

/// The lock's write for manifest `m`, nothing when it doesn't change.
pub(crate) fn relock_writes(
    session: &mut Session,
    m: &Manifest,
) -> Result<Vec<(String, Option<String>)>> {
    let old = session.lock();
    let before = session
        .files()
        .get(LOCK)
        .map(|b| String::from_utf8_lossy(b).into_owned());
    if wanted_labels(m).is_empty() && before.is_none() {
        return Ok(Vec::new());
    }
    let lock = if wanted_labels(m).is_empty() {
        Lock::default()
    } else {
        crate::pin::with_reader(&*session.pins, |r| {
            relock(r, m, old.as_ref(), &Default::default())
        })?
    };
    let text = lock_text(&lock);
    Ok(if text == before {
        Vec::new()
    } else {
        vec![(LOCK.to_owned(), text)]
    })
}

/// `c`'s latest official label that isn't obsolete in `alias`'s library:
/// `label_not_found` when it has none.
pub(crate) fn latest(session: &mut Session, alias: &Id, a: &Alias, c: &Id) -> Result<String> {
    let (config, releases) = session.pins.releases_of(session.root(), a)?;
    ryupdm::deps::parts::latest(&releases, config.revisions)
        .remove(c)
        .ok_or_else(|| {
            Error::new(
                Code::LabelNotFound,
                format!("{alias}/{c} has no official release that isn't obsolete"),
            )
            .with("alias", alias)
            .with("component", c)
        })
}

/// One finding of `check`'s `deps` section.
fn finding(code: Code, severity: &str, message: String, details: Value) -> Value {
    json!({"code": code, "severity": severity, "source": "deps", "message": message, "details": details})
}

/// `check`'s `deps` findings for the working files: the lock's
/// (`lock_invalid`, `dependency_unlocked`, `lock_unused`), then each pin
/// that doesn't resolve (an error, naming what to run), each obsolete one
/// (`component_obsolete`), each older than its component's latest
/// (`component_outdated`), in alias and component order.
pub fn findings(session: &mut Session) -> Result<Vec<Value>> {
    let mut out = Vec::new();
    let m = session.project().manifest.clone();
    match session.files().get(LOCK).map(|b| read_lock(b)) {
        Some(Err(e)) => out.push(finding(
            e.code,
            "error",
            e.message.clone(),
            json!({"path": LOCK}),
        )),
        lock => {
            let lock = lock.and_then(Result::ok).unwrap_or_default();
            let (unlocked, unused) = judge(&m, &lock);
            for (p, c, l) in unlocked {
                out.push(finding(
                    Code::DependencyUnlocked,
                    "error",
                    format!(
                        "{c}@{l} of {p} isn't in {LOCK}: write it again (`{}`)",
                        EDA.relock
                    ),
                    json!({"project": p, "component": c, "label": l}),
                ));
            }
            for (p, c, l) in unused {
                out.push(finding(
                    Code::LockUnused,
                    "warning",
                    format!("{LOCK} holds {c}@{l} of {p}, which nothing pins"),
                    json!({"project": p, "component": c, "label": l}),
                ));
            }
        }
    }
    let pinned: Vec<_> = session
        .project()
        .pinned
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    for ((alias, c), got) in pinned {
        match got {
            Err(e) => out.push(finding(
                e.code,
                "error",
                format!("{alias}/{c}: {}", e.message),
                json!({"alias": alias, "component": c, "error": e}),
            )),
            Ok(p) if p.obsolete => out.push(finding(
                Code::ComponentObsolete,
                "warning",
                format!(
                    "{alias}/{c} is pinned at {}, which is obsolete: pin a current label (`ryueda lib update`)",
                    p.label
                ),
                json!({"alias": alias, "component": c, "label": p.label}),
            )),
            Ok(p) => {
                let alias_id = Id::parse(&alias, "library alias")?;
                let Some(a) = session.project().manifest.libraries.get(&alias_id).cloned() else {
                    continue;
                };
                if let Ok(l) = latest(session, &alias_id, &a, &c)
                    && l != p.label
                {
                    out.push(finding(
                        Code::ComponentOutdated,
                        "warning",
                        format!("{alias}/{c} is pinned at {}; {l} is its latest", p.label),
                        json!({"alias": alias, "component": c, "label": p.label, "latest": l}),
                    ));
                }
            }
        }
    }
    Ok(out)
}

/// `lib.outdated`: each pin with a newer official label.
pub fn outdated(session: &mut Session) -> Result<Value> {
    let list: Vec<Value> = findings(session)?
        .into_iter()
        .filter(|f| f["code"] == "component_outdated")
        .map(|f| f["details"].clone())
        .collect();
    Ok(json!({"outdated": list}))
}

/// `lib.parts`: what `alias`'s library has released, by component, each
/// with its number and name at its latest label (the name from the
/// component's value when its properties don't set one) and its labels
/// latest first (`released`, `superseded`, `obsolete`).
pub fn parts(session: &mut Session, alias: &str) -> Result<Value> {
    let alias_id = Id::parse(alias, "library alias")?;
    let a = session
        .project()
        .manifest
        .libraries
        .get(&alias_id)
        .cloned()
        .ok_or_else(|| {
            Error::new(
                Code::LibraryMissing,
                format!("the project names no library {alias}"),
            )
            .with("alias", alias)
        })?;
    let root = session.root().to_path_buf();
    let (config, releases) = session.pins.releases_of(&root, &a)?;
    let mut list = Vec::new();
    for (c, labels) in ryupdm::deps::parts::released(&releases, config.revisions) {
        let first = &labels[0];
        let read = |rel: String| session.pins.file_at(&root, &a, &first.commit, &rel);
        let values = read(format!("{}/{c}.json", crate::pdm::PROPERTIES))?
            .and_then(|b| String::from_utf8(b).ok())
            .and_then(|t| crate::canonical::parse(&t).ok())
            .and_then(|v| ryupdm::props::Properties::from_json_in(&v, crate::pdm::NAME).ok())
            .map(|p| p.values)
            .unwrap_or_default();
        let rel = format!(
            "{}/{}/{c}.json",
            crate::library::DIR,
            crate::library::COMPONENTS
        );
        let comp = read(rel.clone())?.and_then(|b| {
            crate::doc::read::<crate::libcomp::LibComp>(
                &b,
                &rel,
                crate::libcomp::KIND,
                crate::libcomp::FORMAT,
            )
            .ok()
        });
        let name = values
            .get("name")
            .cloned()
            .or_else(|| comp.map(|c| Value::from(c.value)));
        list.push(json!({
            "component": c,
            "number": values.get("number"),
            "name": name,
            "labels": labels
                .iter()
                .map(|l| json!({"label": l.label, "state": l.state, "commit": l.commit}))
                .collect::<Vec<_>>(),
        }));
    }
    Ok(json!({
        "alias": alias,
        "project": a.project,
        "release_branch": config.release_branch,
        "parts": list,
    }))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn id(s: &str) -> Id {
        Id::parse(s, "id").unwrap()
    }

    #[test]
    fn targets_are_alias_slash_component_and_a_foreign_one_has_no_working() {
        assert_eq!(
            target("passives/r_4k7").unwrap(),
            (id("passives"), id("r_4k7"))
        );
        assert_eq!(target("r_4k7").unwrap_err().code, Code::Usage);
        let mut a = crate::project::Alias::local("");
        a.url = Some("file:///x".into());
        assert_eq!(
            working_refused(&id("passives"), &a, &id("r_4k7"))
                .unwrap_err()
                .code,
            Code::LibraryForeignWorking
        );
        assert!(
            working_refused(
                &id("passives"),
                &crate::project::Alias::local("../p"),
                &id("r_4k7")
            )
            .is_ok()
        );
    }

    #[test]
    fn a_pin_is_written_checked_and_undone() {
        let (_d, board, fake) = crate::project::tests::pinned_board();
        let mut s = Session::open_with_pins(&board, Box::new(fake)).unwrap();
        let v = pin(&mut s, &["passives/c_100n".into()], Choice::Latest, None).unwrap();
        assert_eq!(v["pinned"][0]["from"], "B");
        assert_eq!(v["pinned"][0]["to"], "B");
        let e = pin(
            &mut s,
            &["passives/r_100k".into()],
            Choice::Label("Z"),
            None,
        )
        .unwrap_err();
        assert_eq!(e.code, Code::LabelNotFound);
        // r1 places r_100k: at working, ../passives isn't there, so the
        // write is refused for the fault it would add.
        let e = pin(&mut s, &["passives/r_100k".into()], Choice::Working, None).unwrap_err();
        assert_eq!(e.code, Code::LibraryMissing, "{e:?}");
        // c_100n isn't placed: back to working.
        let v = pin(&mut s, &["passives/c_100n".into()], Choice::Working, None).unwrap();
        assert_eq!(
            (
                v["pinned"][0]["from"].as_str(),
                v["pinned"][0]["to"].as_str()
            ),
            (Some("B"), Some("working"))
        );
        let labels = &s.project().manifest.libraries[&id("passives")].labels;
        assert!(!labels.contains_key(&id("c_100n")));
        assert_eq!(labels[&id("r_100k")], "A");
        let e = pin(&mut s, &["nope/r_100k".into()], Choice::Latest, None).unwrap_err();
        assert_eq!(e.code, Code::LibraryMissing);
    }

    #[test]
    fn an_older_pin_is_outdated_and_an_obsolete_one_said() {
        let (_d, board, mut fake) = crate::project::tests::pinned_board();
        let mut s = Session::open_with_pins(&board, Box::new(fake.clone())).unwrap();
        assert_eq!(findings(&mut s).unwrap(), Vec::<Value>::new());
        fake.release("r_100k", "B");
        fake.obsolete("c_100n", "B");
        let mut s = Session::open_with_pins(&board, Box::new(fake)).unwrap();
        let f = findings(&mut s).unwrap();
        let codes: Vec<&str> = f.iter().map(|x| x["code"].as_str().unwrap()).collect();
        assert_eq!(codes, ["component_obsolete", "component_outdated"], "{f:?}");
        assert!(
            f.iter()
                .all(|x| x["severity"] == "warning" && x["source"] == "deps")
        );
        let v = outdated(&mut s).unwrap();
        assert_eq!(v["outdated"][0]["latest"], "B");
        assert_eq!(v["outdated"][0]["component"], "r_100k");
    }

    #[test]
    fn a_librarys_parts_list_their_labels_latest_first() {
        let (_d, board, mut fake) = crate::project::tests::pinned_board();
        fake.release("r_100k", "B");
        fake.obsolete("c_100n", "B");
        let mut s = Session::open_with_pins(&board, Box::new(fake)).unwrap();
        let v = parts(&mut s, "passives").unwrap();
        let states = |c: &str| -> Vec<(String, String)> {
            let r = v["parts"]
                .as_array()
                .unwrap()
                .iter()
                .find(|p| p["component"] == c)
                .unwrap();
            assert!(r["name"].is_string(), "{r}");
            r["labels"]
                .as_array()
                .unwrap()
                .iter()
                .map(|l| {
                    (
                        l["label"].as_str().unwrap().to_owned(),
                        l["state"].as_str().unwrap().to_owned(),
                    )
                })
                .collect()
        };
        let pair = |a: &str, b: &str| (a.to_owned(), b.to_owned());
        assert_eq!(
            states("r_100k"),
            [pair("B", "released"), pair("A", "superseded")]
        );
        assert_eq!(
            states("c_100n"),
            [pair("B", "obsolete"), pair("A", "released")]
        );
        assert_eq!(
            parts(&mut s, "nope").unwrap_err().code,
            Code::LibraryMissing
        );
    }

    #[test]
    fn the_cache_is_ryuedas_own() {
        let d = tempfile::tempdir().unwrap();
        // SAFETY: only this test sets RYUEDA_DEPS_DIR in this process.
        unsafe { std::env::set_var(DEPS_DIR_VAR, d.path()) };
        assert_eq!(cache().unwrap().dir, d.path());
        unsafe { std::env::remove_var(DEPS_DIR_VAR) };
        assert!(cache().unwrap().dir.ends_with("deps"));
    }

    #[test]
    fn a_fetch_of_nothing_is_nothing_and_a_bad_url_stops_it() {
        let d = tempfile::tempdir().unwrap();
        let c = ryupdm::deps::Cache::new(d.path());
        let job = ryu_session::job::Job::default();
        assert!(fetch_all(&c, &[], &job).is_empty());
        let bad = format!("file://{}", d.path().join("none").display());
        let got = fetch_all(
            &c,
            &[
                (bad.clone(), vec!["".into()]),
                ("file:///never".into(), vec![]),
            ],
            &job,
        );
        assert_eq!(got.len(), 1);
        assert!(got[&bad].1.is_err());
    }

    #[test]
    fn the_lock_holds_exactly_the_foreign_pins() {
        use crate::units::Unit;
        let p1 = ProjectId::parse(&"1".repeat(32)).unwrap();
        let mut m = Manifest::new(
            ProjectId::parse(&"9".repeat(32)).unwrap(),
            Unit::Mm,
            crate::project::ProjectKind::Design,
        );
        let mut a = Alias::local("lib");
        a.url = Some("file:///l".into());
        a.project = Some(p1.clone());
        a.labels.insert(id("r"), "A".into());
        m.libraries.insert(id("passives"), a);
        let mut local = Alias::local("../own");
        local.project = Some(ProjectId::parse(&"2".repeat(32)).unwrap());
        local.labels.insert(id("s"), "B".into());
        m.libraries.insert(id("own"), local);
        assert_eq!(
            wanted_labels(&m),
            BTreeSet::from([(p1.clone(), id("r"), "A".to_owned())])
        );
        let mut lock = Lock::default();
        let (unlocked, unused) = judge(&m, &lock);
        assert_eq!((unlocked.len(), unused.len()), (1, 0));
        lock.projects.entry(p1.clone()).or_default().labels.insert(
            (id("x"), "A".into()),
            ryu_doc::lock::LockedLabel {
                commit: "a".repeat(40),
                content: "c".repeat(64),
            },
        );
        let (_, unused) = judge(&m, &lock);
        assert_eq!(unused, [(p1, id("x"), "A".to_owned())]);
        assert_eq!(
            read_lock(lock_text(&lock).unwrap().as_bytes()).unwrap(),
            lock
        );
        assert_eq!(lock_text(&Lock::default()), None);
        assert_eq!(read_lock(b"nope").unwrap_err().code, Code::LockInvalid);
        let e =
            read_lock(b"{\"format\": 1, \"kind\": \"ryucad.lock\", \"projects\": {}}").unwrap_err();
        assert_eq!(e.code, Code::LockInvalid);
    }

    /// A design project (the breakout) in `t`'s folder, its session
    /// reading the test's dependency cache, and `lib` of `t`'s repository
    /// added as `passives`.
    pub(crate) fn board_with_foreign() -> (Session, (tempfile::TempDir, String, ProjectId, String))
    {
        use crate::project::ProjectKind;
        use crate::project::tests::{design_in, library_in, manifest};
        let t = crate::pin::tests::foreign_library();
        let board = t.0.path().join("board");
        manifest(&board, ProjectKind::Design, &[]);
        library_in(&board);
        design_in(&board);
        let pins = crate::pin::Pins::with_cache(ryupdm::deps::Cache::new(t.0.path().join("cache")));
        let mut s = Session::open_with_pins(&board, Box::new(pins)).unwrap();
        add_url(&mut s, "passives", &t.1, "lib", None).unwrap();
        (s, t)
    }

    /// [`board_with_foreign`] with `passives/r_100k` pinned at `label`.
    pub(crate) fn board_pinned_foreign(
        label: &str,
    ) -> (Session, (tempfile::TempDir, String, ProjectId, String)) {
        let (mut s, t) = board_with_foreign();
        pin(
            &mut s,
            &["passives/r_100k".into()],
            Choice::Label(label),
            None,
        )
        .unwrap();
        (s, t)
    }

    #[test]
    fn a_library_in_another_repository_is_added_by_url() {
        let (mut s, t) = board_with_foreign();
        let a = &s.project().manifest.libraries[&id("passives")];
        assert_eq!(
            (a.url.as_deref(), a.path.as_str(), a.project.as_ref()),
            (Some(t.1.as_str()), "lib", Some(&t.2))
        );
        // No working library, and no fault for it.
        assert_eq!(
            s.project().library("passives").unwrap_err().code,
            Code::LibraryForeignWorking
        );
        let e = add_url(&mut s, "passives", &t.1, "lib", None).unwrap_err();
        assert_eq!(e.code, Code::DuplicateId);
        let e = add_url(&mut s, "again", &t.1, "lib", None).unwrap_err();
        assert_eq!(e.code, Code::ProjectIdDuplicate);
        let e = add_url(&mut s, "other", &t.1, "nope", None).unwrap_err();
        assert_eq!(e.code, Code::LibraryMissing);
        let e = add_url(&mut s, "other", &t.1, "../x", None).unwrap_err();
        assert_eq!(e.code, Code::DependencyInvalid);
        // Pinned: the lock holds it; at working, refused.
        let e = pin(&mut s, &["passives/r_100k".into()], Choice::Working, None).unwrap_err();
        assert_eq!(e.code, Code::LibraryForeignWorking);
        let v = pin(&mut s, &["passives/r_100k".into()], Choice::Latest, None).unwrap();
        assert_eq!(v["pinned"][0]["to"], "A");
        let lock = s.lock().expect("a lock written");
        assert_eq!(lock.label(&t.2, &id("r_100k"), "A").unwrap().commit, t.3);
        assert!(s.project().pinned[&("passives".into(), id("r_100k"))].is_ok());
    }

    #[test]
    fn a_fetch_names_each_repository_and_writes_nothing() {
        let (mut s, t) = board_with_foreign();
        let before = s.project().revision.clone();
        let job = ryu_session::job::Job::default();
        let v = fetch(&mut s, &[], &job).unwrap();
        assert_eq!(v["fetched"][0]["url"], t.1);
        assert_eq!(v["fetched"][0]["aliases"], json!(["passives"]));
        assert_eq!(v["fetched"][0]["branches"][0]["changed"], false);
        assert_eq!(s.project().revision, before);
        let e = fetch(&mut s, &["nope".into()], &job).unwrap_err();
        assert_eq!(e.code, Code::LibraryMissing);
        // The daemon fetched nothing for it (an alias that arrived since):
        // fetched now regardless.
        s.fetched = Some(Prefetched::new());
        let v = fetch(&mut s, &[], &job).unwrap();
        assert_eq!(v["fetched"][0]["url"], t.1, "{v}");
    }

    #[test]
    fn an_update_moves_pins_to_the_latest_and_relocks() {
        let (mut s, t) = board_pinned_foreign("A");
        crate::pin::tests::release_again(&t, "r_100k", "B");
        let job = ryu_session::job::Job::default();
        let ask = |dry_run, offline| Update {
            aliases: &[],
            to: None,
            offline,
            dry_run,
        };
        // Offline, before a fetch: B isn't known yet.
        let v = update(&mut s, ask(true, true), None, &job).unwrap();
        assert_eq!(v["moved"], json!([]));
        let dry = update(&mut s, ask(true, false), None, &job).unwrap();
        assert_eq!(
            dry["moved"],
            json!([{"alias": "passives", "component": "r_100k", "from": "A", "to": "B"}])
        );
        assert_eq!(
            s.project().manifest.libraries[&id("passives")].labels[&id("r_100k")],
            "A"
        );
        let v = update(&mut s, ask(false, true), None, &job).unwrap();
        assert_eq!(v["lock_changed"], true, "{v}");
        assert_eq!(
            s.project().manifest.libraries[&id("passives")].labels[&id("r_100k")],
            "B"
        );
        let lock = s.lock().unwrap();
        assert!(lock.label(&t.2, &id("r_100k"), "B").is_some());
        assert!(lock.label(&t.2, &id("r_100k"), "A").is_none());
        assert!(s.project().pinned[&("passives".into(), id("r_100k"))].is_ok());
    }

    #[test]
    fn an_update_names_a_local_library_beside_one_in_another_repository() {
        let (mut s, t) = board_pinned_foreign("A");
        crate::pin::tests::release_again(&t, "r_100k", "B");
        let mut m = s.project().manifest.clone();
        m.libraries.insert(id("own"), Alias::local("../own"));
        crate::source::write_atomic(&s.root().join(MANIFEST), m.to_text()).unwrap();
        s.reload().unwrap();
        let job = ryu_session::job::Job::default();
        let names = ["own".to_owned(), "passives".to_owned()];
        let ask = Update {
            aliases: &names,
            to: None,
            offline: false,
            dry_run: false,
        };
        let v = update(&mut s, ask, None, &job).unwrap();
        assert_eq!(
            v["moved"],
            json!([{"alias": "passives", "component": "r_100k", "from": "A", "to": "B"}])
        );
        assert_eq!(v["fetched"][0]["aliases"], json!(["passives"]), "{v}");
        // fetch itself still refuses a local alias.
        let e = fetch(&mut s, &names, &job).unwrap_err();
        assert_eq!(e.code, Code::LibraryMissing);
    }

    #[test]
    fn an_update_leaves_a_component_with_no_current_label_and_says_so() {
        let (_d, board, mut fake) = crate::project::tests::pinned_board();
        fake.release("r_100k", "B");
        fake.obsolete("c_100n", "A");
        fake.obsolete("c_100n", "B");
        let mut s = Session::open_with_pins(&board, Box::new(fake)).unwrap();
        let job = ryu_session::job::Job::default();
        let v = update(
            &mut s,
            Update {
                aliases: &[],
                to: None,
                offline: true,
                dry_run: false,
            },
            None,
            &job,
        )
        .unwrap();
        assert_eq!(
            v["moved"],
            json!([{"alias": "passives", "component": "r_100k", "from": "A", "to": "B"}])
        );
        assert_eq!(v["skipped"][0]["component"], "c_100n", "{v}");
        assert_eq!(v["skipped"][0]["label"], "B", "{v}");
        assert_eq!(v["skipped"][0]["code"], "label_not_found", "{v}");
        let labels = &s.project().manifest.libraries[&id("passives")].labels;
        assert_eq!(
            (
                labels[&id("r_100k")].as_str(),
                labels[&id("c_100n")].as_str()
            ),
            ("B", "B")
        );
    }

    #[test]
    fn an_update_to_a_local_label_that_doesnt_resolve_writes_nothing() {
        let (_d, board, fake) = crate::project::tests::pinned_board();
        let mut s = Session::open_with_pins(&board, Box::new(fake)).unwrap();
        let before = s.project().revision.clone();
        let job = ryu_session::job::Job::default();
        let ask = |to| Update {
            aliases: &[],
            to: Some(to),
            offline: true,
            dry_run: false,
        };
        let e = update(&mut s, ask("Z"), None, &job).unwrap_err();
        assert_eq!(e.code, Code::LabelNotFound, "{e:?}");
        assert_eq!(s.project().revision, before);
        // Every pin resolves at A: c_100n moves, r_100k is there already.
        let v = update(&mut s, ask("A"), None, &job).unwrap();
        assert_eq!(
            v["moved"],
            json!([{"alias": "passives", "component": "c_100n", "from": "B", "to": "A"}])
        );
    }

    #[test]
    fn the_lock_is_judged_in_check_and_repaired_by_doctor() {
        let (mut s, _t) = board_pinned_foreign("A");
        assert_eq!(findings(&mut s).unwrap(), Vec::<Value>::new());
        // Pin c_100n at A by hand, leaving the lock as it was.
        let mut m = s.project().manifest.clone();
        m.libraries
            .get_mut(&id("passives"))
            .unwrap()
            .labels
            .insert(id("c_100n"), "A".into());
        crate::source::write_atomic(&s.root().join(MANIFEST), m.to_text()).unwrap();
        s.reload().unwrap();
        let codes: Vec<String> = findings(&mut s)
            .unwrap()
            .iter()
            .map(|f| f["code"].as_str().unwrap().to_owned())
            .collect();
        assert!(
            codes.contains(&"dependency_unlocked".to_owned()),
            "{codes:?}"
        );
        // c_100n was never released: the repair can't lock it.
        let d = crate::doctor::diagnose_with(s.root(), s.project(), &*s.pins);
        let i = d
            .iter()
            .find(|i| i.code == Code::DependencyUnlocked)
            .expect("found");
        assert!(i.repairs.is_empty(), "{:?}", i.repairs);
        assert!(i.message.contains("ryueda lib fetch"), "{}", i.message);
        // The lock removed instead: doctor writes it again.
        let mut m = s.project().manifest.clone();
        m.libraries
            .get_mut(&id("passives"))
            .unwrap()
            .labels
            .remove(&id("c_100n"));
        crate::source::write_atomic(&s.root().join(MANIFEST), m.to_text()).unwrap();
        let lock = std::fs::read(s.root().join(LOCK)).unwrap();
        std::fs::remove_file(s.root().join(LOCK)).unwrap();
        s.reload().unwrap();
        let d = crate::doctor::diagnose_with(s.root(), s.project(), &*s.pins);
        let i = d
            .iter()
            .find(|i| i.code == Code::DependencyUnlocked)
            .expect("found");
        assert!(
            i.repairs[0].id.starts_with("deps_lock:write:"),
            "{:?}",
            i.repairs
        );
        assert_eq!(
            i.repairs[0].changes[0].content.as_deref(),
            Some(lock.as_slice())
        );
    }

    #[test]
    fn uses_lists_what_each_design_places_from_others() {
        let (_d, board, fake) = crate::project::tests::pinned_board();
        let mut s = Session::open_with_pins(&board, Box::new(fake)).unwrap();
        let v = uses(&mut s, None).unwrap();
        let places = &v["working"][0]["places"];
        assert_eq!(v["working"][0]["design"], "d", "{v}");
        assert_eq!(places[0]["alias"], "passives", "{v}");
        assert_eq!(places[0]["component"], "r_100k");
        assert_eq!(places[0]["label"], "A");
        assert_eq!(
            places[0]["project"],
            crate::project::tests::pid(2).to_string()
        );
        assert!(places[0]["count"].as_u64().unwrap() >= 1);
        assert_eq!(v["released"], json!([]));
        assert_eq!(v["rev"], "working");
    }
}
