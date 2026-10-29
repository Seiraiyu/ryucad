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
}
