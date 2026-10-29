//! Working out `ryucad.lock`: every other project's label the working
//! files reach, at any depth, resolved from the dependency cache alone.
//! Labels the old lock holds at the same place keep their commits, so a
//! write that changes one instance moves nothing else; a label new to
//! the lock takes the commit its release branch, as fetched, made it
//! official at.

use super::read::{Reader, not_fetched};
use super::{HEAD_REF, config_at};
use crate::record::Kind;
use crate::structure::occurrences;
use ryucad_doc::doc::assembly::Rev;
use ryucad_doc::doc::deps::Dependencies;
use ryucad_doc::doc::id::{Id, ProjectId};
use ryucad_doc::doc::lock::{Lock, Locked, LockedLabel};
use ryucad_doc::doc::project::Project;
use ryucad_doc::error::{Code, Error, Result};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::Path;

/// One label of another project to lock: its project, part and label.
pub type Wanted = (ProjectId, Id, String);

/// The labels of other projects the parts of `project` place directly,
/// in order: what its lock starts from.
pub fn wanted(project: &Project) -> BTreeSet<Wanted> {
    let mut out = BTreeSet::new();
    for p in project.parts.values() {
        for o in occurrences(p).0 {
            if let Rev::Foreign { project, label } = o.rev {
                out.insert((project, o.part, label));
            }
        }
    }
    out
}

/// Where a project's repository is and how it was reached.
#[derive(Clone)]
struct Place {
    url: String,
    path: String,
    via: Vec<ProjectId>,
}

/// The lock for the labels `start` names, reached from a project whose
/// own ID is `own` and whose manifest lists `deps`: each label and every
/// label of another project it places, at any depth. `old` is the lock as
/// it is; each label it holds where the project still is keeps its
/// commit. Fails on the first label that can't be locked from what was
/// fetched: `dependency_missing` (a project nothing names), `dependency_not_fetched`, `label_not_found`, `label_not_official`, or
/// a label's own fault reading it.
pub fn lock(
    reader: &mut Reader,
    own: Option<&ProjectId>,
    deps: &Dependencies,
    start: &BTreeSet<Wanted>,
    old: Option<&Lock>,
) -> Result<Lock> {
    let mut places: BTreeMap<ProjectId, Place> = deps
        .iter()
        .map(|(id, d)| {
            (
                id.clone(),
                Place {
                    url: d.url.clone(),
                    path: d.path.clone(),
                    via: Vec::new(),
                },
            )
        })
        .collect();
    let mut out = Lock::default();
    let mut queue: VecDeque<(Wanted, Vec<ProjectId>)> =
        start.iter().map(|w| (w.clone(), Vec::new())).collect();
    let mut seen = BTreeSet::new();
    while let Some(((project, part, label), via)) = queue.pop_front() {
        if !seen.insert((project.clone(), part.clone(), label.clone())) {
            continue;
        }
        let Some(place) = places.get(&project).cloned() else {
            let by = via
                .last()
                .map_or("this project".to_owned(), |p| p.to_string());
            return Err(Error::new(
                Code::DependencyMissing,
                format!(
                    "{part}@{label} is of project {project}, which neither this project's dependencies nor {by}'s name"
                ),
            )
            .with("project", &project)
            .with("part", &part)
            .with("label", &label)
            .with("via", &via));
        };
        if !out.projects.contains_key(&project) {
            let what = format!("project {project}");
            let Some(repo) = reader.repo(&place.url)? else {
                return Err(not_fetched(&project, &place.url, &what));
            };
            let config = config_at(repo, HEAD_REF, &place.path)?;
            out.projects.insert(
                project.clone(),
                Locked {
                    url: place.url.clone(),
                    path: place.path.clone(),
                    release_branch: config.release_branch,
                    via: place.via.clone(),
                    labels: BTreeMap::new(),
                },
            );
        }
        let key = (part.clone(), label.clone());
        let kept = old
            .and_then(|l| l.projects.get(&project))
            .filter(|l| l.url == place.url && l.path == place.path)
            .and_then(|l| l.labels.get(&key))
            .cloned();
        let locked = match kept {
            Some(l) => l,
            None => official(reader, &project, &out.projects[&project], &part, &label)?,
        };
        out.projects
            .get_mut(&project)
            .expect("just put")
            .labels
            .insert(key, locked);
        // What the label places, read from its files as the lock has it.
        let read = reader.label(&project, &out.projects[&project], &part, &label)?;
        let theirs = Project::load_from(Path::new(""), &read.files)?;
        let mut chain = via.clone();
        chain.push(project.clone());
        for (id, d) in &theirs.manifest.dependencies {
            places.entry(id.clone()).or_insert_with(|| Place {
                url: d.url.clone(),
                path: d.path.clone(),
                via: chain.clone(),
            });
        }
        if let Some(p) = theirs.parts.get(&part) {
            for o in occurrences(p).0 {
                let next = match o.rev.seen_from(&project, own) {
                    Rev::Foreign { project, label } => (project, o.part, label),
                    // Its own working files aren't released (the rebuild
                    // says so), and the root's labels are its own.
                    _ => continue,
                };
                queue.push_back((next, chain.clone()));
            }
        }
    }
    Ok(out)
}

/// Where `project`'s release `label` of `part` became official on its
/// release branch as fetched, with its record's content hash there.
fn official(
    reader: &mut Reader,
    project: &ProjectId,
    locked: &Locked,
    part: &Id,
    label: &str,
) -> Result<LockedLabel> {
    let what = format!("{part}@{label} of {project}");
    let path = crate::record::path(part, label, Kind::Release);
    let Some(releases) = reader.releases(&locked.url, &locked.path, &locked.release_branch)? else {
        return Err(not_fetched(project, &locked.url, &what));
    };
    let Some(o) = releases.official.get(&path) else {
        let code = if releases.modified.contains_key(&path) {
            Code::LabelNotOfficial
        } else {
            Code::LabelNotFound
        };
        return Err(Error::new(
            code,
            format!(
                "{what} isn't an official release on {}'s {} as fetched",
                locked.url, locked.release_branch
            ),
        )
        .with("project", project)
        .with("part", part)
        .with("label", label));
    };
    let (commit, blob) = (o.commit.clone(), o.blob);
    let repo = reader.repo(&locked.url)?.expect("its releases were read");
    let blob = repo
        .find_object(blob)
        .map_err(crate::history::git_error)?
        .data
        .clone();
    let mut files = BTreeMap::new();
    files.insert(path, blob);
    let found = crate::structure::lookup(&files, part, label)?;
    Ok(LockedLabel {
        commit,
        content: found.content,
    })
}

#[cfg(test)]
mod tests {
    use super::super::Cache;
    use super::super::read::tests::{commit, library};
    use super::super::tests::url;
    use super::*;
    use crate::history::tests::git;
    use crate::state::tests::{PROJECT, add, id, project};
    use ryucad_doc::doc::canonical;
    use ryucad_doc::doc::deps::Dependency;
    use serde_json::json;

    fn lib() -> ProjectId {
        ProjectId::parse(PROJECT).unwrap()
    }

    fn dep(name: &str, url: &str) -> Dependency {
        Dependency {
            name: name.into(),
            url: url.into(),
            path: "".into(),
        }
    }

    /// A repository at `dir` whose project (ID `[2; 16]`, depending on the
    /// library at `lib_url`) has assembly `asm` placing the library's `p`
    /// at `A`, released as `A`.
    fn subassembly(dir: &Path, lib_url: &str) -> ProjectId {
        let me = ProjectId::from_bytes([2; 16]);
        git(dir, &["init", "-q"]);
        let mut f = BTreeMap::new();
        let mut put = |p: &str, v: serde_json::Value| {
            f.insert(p.to_owned(), canonical::to_string(&v).into_bytes());
        };
        put(
            "ryucad.json",
            json!({"default_unit": "mm", "format": 3, "kind": "ryucad.project", "project": me,
                   "dependencies": {lib().as_str(): {"name": "lib", "path": "", "url": lib_url}}}),
        );
        put(
            "parts/asm/part.json",
            json!({"features": ["i"], "format": 2, "kind": "ryucad.part", "part_kind": "assembly"}),
        );
        put(
            "parts/asm/features/i.json",
            json!({"format": 6, "kind": "ryucad.feature", "type": "instance", "project": lib(),
                   "part": "p", "rev": {"label": "A"},
                   "pose": {"origin_m": [0, 0, 0], "z": [0, 0, 1], "x": [1, 0, 0]}}),
        );
        commit(dir, "", &f);
        let r = crate::record::Record::Release(crate::record::Release {
            project: me.clone(),
            part: id("asm"),
            label: "A".into(),
            content: crate::content::hash(&f, &id("asm")).unwrap(),
            properties: BTreeMap::new(),
            results: BTreeMap::new(),
            built_with: crate::record::BuiltWith {
                ryucad: "0".into(),
                ryukei: "0".into(),
                commit: "c".into(),
            },
            released_by: crate::state::tests::by(),
            supersedes: None,
            note: None,
        });
        add(&mut f, &r);
        commit(dir, "", &f);
        me
    }

    #[test]
    fn a_lock_reaches_through_a_dependencys_dependencies() {
        let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let at = library(a.path(), "");
        let sub = subassembly(b.path(), &url(a.path()));
        let cache = Cache::new(tempfile::tempdir().unwrap().keep());
        let mut deps = Dependencies::new();
        deps.insert(sub.clone(), dep("sub", &url(b.path())));
        let start = BTreeSet::from([(sub.clone(), id("asm"), "A".to_owned())]);
        let me = ProjectId::from_bytes([9; 16]);
        // Nothing fetched: nothing can be locked.
        let e = lock(&mut Reader::new(&cache), Some(&me), &deps, &start, None).unwrap_err();
        assert_eq!(e.code, Code::DependencyNotFetched);
        cache.fetch(&url(b.path()), &[""]).unwrap();
        // The subassembly read, its library not yet.
        let e = lock(&mut Reader::new(&cache), Some(&me), &deps, &start, None).unwrap_err();
        assert_eq!(
            (e.code, &e.details["project"]),
            (Code::DependencyNotFetched, &json!(lib()))
        );
        cache.fetch(&url(a.path()), &[""]).unwrap();
        let l = lock(&mut Reader::new(&cache), Some(&me), &deps, &start, None).unwrap();
        assert_eq!(l.projects.len(), 2);
        let sub_at = git(b.path(), &["rev-parse", "HEAD"]).trim().to_owned();
        assert_eq!(l.label(&sub, &id("asm"), "A").unwrap().commit, sub_at);
        assert_eq!(l.label(&lib(), &id("p"), "A").unwrap().commit, at);
        assert_eq!(l.projects[&lib()].via, std::slice::from_ref(&sub));
        assert_eq!(l.projects[&lib()].url, url(a.path()));
        assert_eq!(l.projects[&sub].release_branch, "main");
        // The old lock's labels are kept where the project is: as they
        // are, and read as they are (here one made wrong is refused).
        let again = lock(&mut Reader::new(&cache), Some(&me), &deps, &start, Some(&l)).unwrap();
        assert_eq!(again, l);
        let mut old = l.clone();
        old.projects
            .get_mut(&lib())
            .unwrap()
            .labels
            .values_mut()
            .for_each(|x| x.content = "0".repeat(64));
        let e = lock(
            &mut Reader::new(&cache),
            Some(&me),
            &deps,
            &start,
            Some(&old),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::DependencyRewritten);
        // A label the branch never released.
        let start = BTreeSet::from([(sub.clone(), id("asm"), "B".to_owned())]);
        let e = lock(&mut Reader::new(&cache), Some(&me), &deps, &start, None).unwrap_err();
        assert_eq!(e.code, Code::LabelNotFound);
        // A project nothing names.
        let start = BTreeSet::from([(ProjectId::from_bytes([5; 16]), id("x"), "A".to_owned())]);
        let e = lock(&mut Reader::new(&cache), Some(&me), &deps, &start, None).unwrap_err();
        assert_eq!(e.code, Code::DependencyMissing);
    }

    #[test]
    fn the_roots_url_for_a_transitive_project_wins() {
        let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        library(a.path(), "");
        let sub = subassembly(b.path(), "file:///nowhere");
        let cache = Cache::new(tempfile::tempdir().unwrap().keep());
        cache.fetch(&url(a.path()), &[""]).unwrap();
        cache.fetch(&url(b.path()), &[""]).unwrap();
        let mut deps = Dependencies::new();
        deps.insert(sub.clone(), dep("sub", &url(b.path())));
        deps.insert(lib(), dep("mirror", &url(a.path())));
        let start = BTreeSet::from([(sub, id("asm"), "A".to_owned())]);
        let l = lock(&mut Reader::new(&cache), None, &deps, &start, None).unwrap();
        assert_eq!(l.projects[&lib()].url, url(a.path()));
        assert!(l.projects[&lib()].via.is_empty());
        // Wanted labels come from the parts' instances.
        let mut files = project();
        files.insert(
            "parts/asm/part.json".into(),
            canonical::to_string(&json!({"features": ["i"], "format": 2, "kind": "ryucad.part", "part_kind": "assembly"})).into_bytes(),
        );
        files.insert(
            "parts/asm/features/i.json".into(),
            canonical::to_string(
                &json!({"format": 6, "kind": "ryucad.feature", "type": "instance",
                "project": lib(), "part": "p", "rev": {"label": "A"},
                "pose": {"origin_m": [0, 0, 0], "z": [0, 0, 1], "x": [1, 0, 0]}}),
            )
            .into_bytes(),
        );
        let p = Project::load_from(Path::new(""), &files).unwrap();
        assert_eq!(
            wanted(&p),
            BTreeSet::from([(lib(), id("p"), "A".to_owned())])
        );
    }
}
