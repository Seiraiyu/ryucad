//! Where-used over other checkouts on disk: the ryucad projects in given
//! folders (or the `pdm.dependents` preference's) whose `ryucad.lock`
//! names this project, each asked where a part of this project (or of a
//! project they share) is used, in their working files and in their own
//! official releases, read from their own repositories. Offline: their
//! foreign instances are read from this machine's dependency cache, as
//! this project's are. A folder that can't be read is a problem, never a
//! failure.

use crate::doc::assembly::Rev;
use crate::doc::id::ProjectId;
use crate::doc::project::{MANIFEST, Project};
use crate::doc::source::Folder;
use crate::error::{Code, Error, Result};
use crate::session::Session;
use ryupdm::config::Config;
use ryupdm::history::Releases;
use ryupdm::structure::{Cache, Reader, Repo};
use ryupdm::where_used::{Query, where_used};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// The preference listing the folders a where-used searches when it is
/// given none.
pub const PREFERENCE: &str = "pdm.dependents";

/// Folders whose names say they hold no project: Git's, build output,
/// installed packages.
const SKIPPED: [&str; 2] = ["target", "node_modules"];

/// The folders a where-used also searches: `within` when it names any,
/// else the `pdm.dependents` preference unless `skip`; each relative one
/// taken from `root`.
pub(crate) fn folders(root: &Path, within: &[String], skip: bool) -> Vec<PathBuf> {
    let listed: Vec<String> = if !within.is_empty() {
        within.to_vec()
    } else if skip {
        Vec::new()
    } else {
        crate::config::user(PREFERENCE)
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|p| p.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default()
    };
    listed.iter().map(|f| root.join(f)).collect()
}

/// The ryucad projects at or under `folder`: the folder itself when it
/// holds a manifest, else every folder below holding one (never looking
/// inside a project, a hidden folder, `target` or `node_modules`), sorted.
pub(crate) fn projects_under(folder: &Path) -> std::io::Result<Vec<PathBuf>> {
    if folder.join(MANIFEST).is_file() {
        return Ok(vec![folder.to_path_buf()]);
    }
    let mut out = Vec::new();
    let mut entries: Vec<PathBuf> = std::fs::read_dir(folder)?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter(|e| {
            let name = e.file_name();
            let name = name.to_string_lossy();
            !name.starts_with('.') && !SKIPPED.contains(&name.as_ref())
        })
        .map(|e| e.path())
        .collect();
    entries.sort();
    for dir in entries {
        // A folder below that can't be read holds nothing to search.
        out.extend(projects_under(&dir).unwrap_or_default());
    }
    Ok(out)
}

/// `q` as the project `theirs` (another project, depending on `own`)
/// sees it: this project's part is another project's there, and a part
/// of `theirs` is its own.
fn seen_by(q: &Query, own: &ProjectId, theirs: Option<&ProjectId>) -> Query {
    let project = match &q.project {
        None => Some(own.clone()),
        Some(p) if Some(p) == theirs => None,
        Some(p) => Some(p.clone()),
    };
    let rev = q.rev.as_ref().map(|r| match (r, &project) {
        (Rev::Label(l) | Rev::Foreign { label: l, .. }, Some(p)) => Rev::Foreign {
            project: p.clone(),
            label: l.clone(),
        },
        (Rev::Label(l) | Rev::Foreign { label: l, .. }, None) => Rev::Label(l.clone()),
        (Rev::Working, _) => Rev::Working,
    });
    Query {
        part: q.part.clone(),
        project,
        rev,
        direct: q.direct,
    }
}

/// What other projects' assemblies answer: their rows, each with the
/// project, folder and name it came from, and what couldn't be read.
#[derive(Default)]
pub(crate) struct Found {
    pub working: Vec<Value>,
    pub released: Vec<Value>,
    pub problems: Vec<Value>,
    /// The project folders searched that depend on this project.
    pub searched: Vec<String>,
}

/// Searches the projects in `folders` whose lock names `own` (the
/// session's project's ID) for where `q` (asked of the session's project)
/// is used. The session's own folder is left out.
pub(crate) fn search(
    session: &mut Session,
    own: &ProjectId,
    q: &Query,
    folders: &[PathBuf],
) -> Result<Found> {
    let mut found = Found::default();
    let here = std::fs::canonicalize(session.root()).ok();
    let mut deps = session.take_deps_reader()?;
    for folder in folders {
        let projects = match projects_under(folder) {
            Ok(p) => p,
            Err(e) => {
                found.problems.push(json!(
                    Error::new(
                        Code::NotFound,
                        format!("{} can't be read: {e}", folder.display())
                    )
                    .with("folder", folder.display().to_string())
                ));
                continue;
            }
        };
        for dir in projects {
            if std::fs::canonicalize(&dir).ok() == here {
                continue;
            }
            match one(&mut deps, own, q, &dir) {
                Ok(Some(v)) => {
                    let shown = dir.display().to_string();
                    let name = dir
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    let tag = |row: &Value| {
                        let mut row = row.clone();
                        row["project"] = v["project_id"].clone();
                        row["folder"] = json!(shown);
                        row["name"] = json!(name);
                        row
                    };
                    for (from, into) in [
                        ("working", &mut found.working),
                        ("released", &mut found.released),
                    ] {
                        into.extend(v[from].as_array().into_iter().flatten().map(tag));
                    }
                    for p in v["problems"].as_array().into_iter().flatten() {
                        let mut p = p.clone();
                        p["details"]["folder"] = json!(shown);
                        found.problems.push(p);
                    }
                    found.searched.push(shown);
                }
                Ok(None) => {}
                Err(e) => found
                    .problems
                    .push(json!(e.with("folder", dir.display().to_string()))),
            }
        }
    }
    session.put_deps_reader(deps);
    Ok(found)
}

/// Where `q` is used in the project at `dir`, if its lock names `own`
/// (`None` otherwise): `where_used`'s answer with `project_id`, its ID.
fn one(
    deps: &mut ryupdm::deps::read::Reader,
    own: &ProjectId,
    q: &Query,
    dir: &Path,
) -> Result<Option<Value>> {
    let src = Folder(dir);
    let project = Project::load_from(dir, &src)?;
    let lock = match &project.lock {
        Some(Ok(l)) if l.projects.contains_key(own) => l.clone(),
        Some(Err(e)) => return Err(e.clone()),
        _ => return Ok(None),
    };
    let config = Config::read(&src).unwrap_or_default();
    let repo = crate::git::Repo::open(dir)?;
    let releases = match &repo {
        Some(r) => Some(Releases::read(r.gix(), r.prefix(), &config)?),
        None => None,
    };
    let theirs = project.manifest.project.clone();
    let mut through = ryupdm::deps::read::Through {
        reader: deps,
        lock,
        deps: &project.manifest.dependencies,
    };
    let mut cache = Cache::new();
    let mut reader = Reader::new(
        &src,
        &project,
        repo.as_ref().map(|r| Repo {
            git: r.gix(),
            prefix: r.prefix(),
        }),
        releases.as_ref(),
        &mut cache,
    )
    .with_others(&mut through);
    let mut v = where_used(&mut reader, &seen_by(q, own, theirs.as_ref()), &config);
    v["project_id"] = json!(theirs);
    Ok(Some(v))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assembly::tests::id;

    #[test]
    fn projects_are_found_in_a_folder_or_under_it() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for p in ["a", "repo/cad/b", "repo/.git/c", "repo/target/d", "a/inner"] {
            std::fs::create_dir_all(root.join(p)).unwrap();
            std::fs::write(root.join(p).join(MANIFEST), "{}").unwrap();
        }
        let found = projects_under(root).unwrap();
        assert_eq!(found, [root.join("a"), root.join("repo/cad/b")]);
        assert_eq!(projects_under(&root.join("a")).unwrap(), [root.join("a")]);
        assert!(projects_under(&root.join("nowhere")).is_err());
    }

    #[test]
    fn a_query_is_seen_from_the_other_project() {
        let (own, lib, theirs) = (
            ProjectId::from_bytes([1; 16]),
            ProjectId::from_bytes([2; 16]),
            ProjectId::from_bytes([3; 16]),
        );
        let ask = |project: Option<&ProjectId>, rev: Option<Rev>| Query {
            part: id("p"),
            project: project.cloned(),
            rev,
            direct: false,
        };
        let a = |p: &ProjectId| Rev::Foreign {
            project: p.clone(),
            label: "A".into(),
        };
        // This project's part, at its label: another project's there.
        let q = seen_by(
            &ask(None, Some(Rev::Label("A".into()))),
            &own,
            Some(&theirs),
        );
        assert_eq!((q.project, q.rev), (Some(own.clone()), Some(a(&own))));
        // A library both use stays the library's.
        let q = seen_by(&ask(Some(&lib), Some(a(&lib))), &own, Some(&theirs));
        assert_eq!((q.project, q.rev), (Some(lib.clone()), Some(a(&lib))));
        // Their own part is their own.
        let q = seen_by(&ask(Some(&theirs), Some(a(&theirs))), &own, Some(&theirs));
        assert_eq!((q.project, q.rev), (None, Some(Rev::Label("A".into()))));
    }
}
