//! Where a library project's component is used (design 629): by
//! designs of this repository's ryueda projects, of other checkouts on
//! disk, and in their official releases, each row a design with the
//! label it places and how many populated occurrences.

use crate::id::{Id, ProjectId};
use crate::project::Project;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// The library asked about.
pub struct Library {
    pub project: ProjectId,
    /// Its folder, canonical, for aliases that name it by path.
    pub dir: Option<PathBuf>,
    pub component: Id,
}

/// Which labels a row may place.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Filter {
    Any,
    Label(String),
    Working,
}

impl Filter {
    pub fn takes(&self, label: &str) -> bool {
        match self {
            Filter::Any => true,
            Filter::Label(l) => l == label,
            Filter::Working => label == "working",
        }
    }
}

/// The aliases of `p` (at `root`) that name `lib`: by its project ID,
/// else, with none, by resolving to its folder.
fn aliases(p: &Project, root: &Path, lib: &Library) -> Vec<Id> {
    let resolved = crate::project::resolve(root, &p.manifest);
    p.manifest
        .libraries
        .iter()
        .filter(|(name, a)| match &a.project {
            Some(pid) => *pid == lib.project,
            None => lib.dir.as_ref().is_some_and(|d| {
                resolved
                    .get(*name)
                    .and_then(|r| r.as_ref().ok())
                    .and_then(|path| std::fs::canonicalize(root.join(path)).ok())
                    .is_some_and(|c| c == *d)
            }),
        })
        .map(|(name, _)| name.clone())
        .collect()
}

/// Each design of `p` placing `lib`'s component at a label `filter`
/// takes: `{design, label, count, uses}` (`count` the populated
/// occurrences, `uses` all), in design then label order.
pub fn placing(p: &Project, root: &Path, lib: &Library, filter: &Filter) -> Vec<Value> {
    let names = aliases(p, root, lib);
    if names.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for d in p.conn.designs.keys() {
        let Ok(n) = crate::netlist::flatten(p, d) else {
            continue;
        };
        let mut by: std::collections::BTreeMap<String, (u64, u64)> = Default::default();
        for o in &n.occurrences {
            let Some(r) = crate::erc::of_ref(&o.of) else {
                continue;
            };
            let Some(alias) = &r.library else { continue };
            if r.id != lib.component || !names.contains(alias) {
                continue;
            }
            let label = p
                .manifest
                .libraries
                .get(alias)
                .and_then(|a| a.labels.get(&r.id))
                .cloned()
                .unwrap_or_else(|| "working".into());
            if !filter.takes(&label) {
                continue;
            }
            let e = by.entry(label).or_default();
            e.1 += 1;
            if !crate::erc::occurrence_dnp(p, o) {
                e.0 += 1;
            }
        }
        for (label, (count, uses)) in by {
            out.push(json!({"design": d, "label": label, "count": count, "uses": uses}));
        }
    }
    out
}

/// The ryueda projects at or under `top`, `own` (this project) left out.
pub(crate) fn others_under(top: &Path, own: &Path) -> Vec<PathBuf> {
    let own = std::fs::canonicalize(own).ok();
    ryupdm::dependents::projects_under(top, crate::project::MANIFEST)
        .unwrap_or_default()
        .into_iter()
        .filter(|d| std::fs::canonicalize(d).ok() != own)
        .collect()
}

/// A row for each design of the project at `dir` placing `lib`'s
/// component as its working files are: `{project, folder, design, label,
/// count, uses}`, `folder` as `shown`; a project that doesn't load is a
/// problem.
pub(crate) fn working_rows(
    dir: &Path,
    shown: &str,
    lib: &Library,
    filter: &Filter,
    problems: &mut Vec<Value>,
) -> Vec<Value> {
    match Project::load(dir) {
        Ok(p) => placing(&p, dir, lib, filter)
            .into_iter()
            .map(|mut r| {
                r["project"] = json!(p.manifest.project);
                r["folder"] = json!(shown);
                r
            })
            .collect(),
        Err(e) => {
            problems.push(json!(e.with("folder", shown)));
            Vec::new()
        }
    }
}

/// A row for each official release of a design of the project at `dir`
/// that placed `lib`'s component, read at the commit that made it
/// official: `{project, folder, design, release, state, commit, label,
/// count, uses}`, `release` the design's label and `state` its state
/// (`released`, `superseded`, `obsolete`), `label` the component's.
/// None outside a repository; a release that can't be read is a problem.
pub(crate) fn released_rows(
    dir: &Path,
    shown: &str,
    lib: &Library,
    filter: &Filter,
    problems: &mut Vec<Value>,
) -> Vec<Value> {
    let Ok(mut s) = crate::session::Session::open(dir) else {
        return Vec::new();
    };
    let Ok((_, releases)) = crate::pdm::history(&mut s) else {
        return Vec::new();
    };
    let config =
        ryupdm::config::Config::read_in(&s.snapshot(), crate::pdm::NAME).unwrap_or_default();
    let project = s.project().manifest.project.clone();
    let mut out = Vec::new();
    for (design, labels) in ryupdm::deps::parts::released(&releases, config.revisions) {
        for r in labels {
            let then = match s.at(&r.commit) {
                Ok((_, p)) => p,
                Err(e) => {
                    problems.push(json!(e.with("folder", shown).with("commit", &r.commit)));
                    continue;
                }
            };
            if !then.conn.designs.contains_key(&design) {
                continue;
            }
            for mut row in placing(&then, dir, lib, filter)
                .into_iter()
                .filter(|x| x["design"] == design.as_str())
            {
                row["project"] = json!(project);
                row["folder"] = json!(shown);
                row["release"] = json!(r.label);
                row["state"] = json!(r.state);
                row["commit"] = json!(r.commit);
                out.push(row);
            }
        }
    }
    out
}

/// The preference listing the folders where-used searches when given none.
pub const PREFERENCE: &str = "pdm.dependents";

/// The folders where-used also searches: `within` when it names any,
/// else the preference unless `skip`; each relative one taken from `root`,
/// and each canonical where it exists, as `checkout_dirs` compares them.
pub fn folders(root: &Path, within: &[String], skip: bool) -> Vec<PathBuf> {
    let listed: Vec<String> = if !within.is_empty() {
        within.to_vec()
    } else if skip {
        Vec::new()
    } else {
        ryu_session::prefs::Preferences::<crate::Eda>::load()
            .ok()
            .and_then(|p| p.values.get(PREFERENCE).cloned())
            .and_then(|v| v.as_array().cloned())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default()
    };
    // Canonical where they exist (#1077): one that doesn't keeps its
    // joined path, for checkout_dirs to report.
    listed
        .iter()
        .map(|f| {
            let p = root.join(f);
            std::fs::canonicalize(&p)
                .map(crate::source::plain)
                .unwrap_or(p)
        })
        .collect()
}

/// The projects under `folders`, each once by its canonical folder
/// (#928), in the order first found, none of `skip` (canonical already);
/// and a problem for each folder that can't be read.
fn checkout_dirs(folders: &[PathBuf], skip: &[PathBuf]) -> (Vec<PathBuf>, Vec<Value>) {
    let (mut dirs, mut problems, mut seen) = (Vec::new(), Vec::new(), Vec::new());
    for f in folders {
        match ryupdm::dependents::projects_under(f, crate::project::MANIFEST) {
            Ok(found) => {
                for d in found {
                    let Ok(c) = std::fs::canonicalize(&d) else {
                        continue;
                    };
                    if !skip.contains(&c) && !seen.contains(&c) {
                        seen.push(c);
                        dirs.push(d);
                    }
                }
            }
            Err(e) => problems.push(json!(
                crate::error::Error::new(
                    crate::error::Code::NotFound,
                    format!("{} can't be read: {e}", f.display()),
                )
                .with("folder", f.display().to_string())
            )),
        }
    }
    (dirs, problems)
}

/// The projects in `folders` (none of `skip`, this repository's, already
/// read) with their rows, working and released, side by side on the
/// current rayon pool and gathered in folder order, so the answer doesn't
/// depend on the thread count: `(working, released, problems, searched)`.
pub(crate) fn checkouts(
    folders: &[PathBuf],
    skip: &[PathBuf],
    lib: &Library,
    filter: &Filter,
) -> (Vec<Value>, Vec<Value>, Vec<Value>, Vec<String>) {
    use rayon::prelude::*;
    let (mut working, mut released, mut problems, mut searched) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let skip: Vec<PathBuf> = skip
        .iter()
        .filter_map(|d| std::fs::canonicalize(d).ok())
        .collect();
    let (dirs, found_problems) = checkout_dirs(folders, &skip);
    problems.extend(found_problems);
    let answers: Vec<(Vec<Value>, Vec<Value>, Vec<Value>)> = dirs
        .par_iter()
        .map(|dir| {
            let shown = dir.display().to_string();
            let mut problems = Vec::new();
            let w = working_rows(dir, &shown, lib, filter, &mut problems);
            let r = released_rows(dir, &shown, lib, filter, &mut problems);
            (w, r, problems)
        })
        .collect();
    for (dir, (w, r, p)) in dirs.iter().zip(answers) {
        if !w.is_empty() || !r.is_empty() {
            searched.push(dir.display().to_string());
        }
        working.extend(w);
        released.extend(r);
        problems.extend(p);
    }
    (working, released, problems, searched)
}

/// How `bom.where_used` asks.
pub struct Ask<'a> {
    pub part: &'a str,
    pub filter: Filter,
    /// Echoed: every placement in a design is direct.
    pub direct: bool,
    pub within: &'a [String],
    pub no_dependents: bool,
}

/// `bom.where_used`, asked in a library project: `{schema, part,
/// project, working, released, problems, dependents?, label?, working_only?,
/// direct?}`, rows from this repository's ryueda projects, then other
/// checkouts'.
pub fn answer(session: &mut crate::session::Session, ask: &Ask) -> crate::error::Result<Value> {
    use crate::error::{Code, Error};
    let p = session.project();
    if p.manifest.project_kind != crate::project::ProjectKind::Library {
        return Err(Error::new(
            Code::InvalidParam,
            "where-used asks about a library project's component: run it in the library project",
        )
        .with("param", "part"));
    }
    let component = Id::parse(ask.part, "component")?;
    if !ryupdm::product::part_ids::<crate::Eda>(&session.snapshot())?.contains(&component) {
        return Err(Error::new(
            Code::NotFound,
            format!("the library has no component {component}"),
        )
        .with("part", &component));
    }
    let root = session.root().to_path_buf();
    let lib = Library {
        project: session.project().manifest.project.clone(),
        dir: std::fs::canonicalize(&root).ok(),
        component: component.clone(),
    };
    let (mut working, mut released, mut problems) = (Vec::new(), Vec::new(), Vec::new());
    let mut seen = vec![root.clone()];
    let repo: Option<crate::git::Repo> = ryu_git::repo::repo(&root).ok();
    let top = repo.and_then(|r| r.gix().workdir().map(Path::to_path_buf));
    if let Some(top) = top {
        for dir in others_under(&top, &root) {
            let shown = dir
                .strip_prefix(&top)
                .map(|r| r.to_string_lossy().replace('\\', "/"))
                .unwrap_or_else(|_| dir.display().to_string());
            working.extend(working_rows(&dir, &shown, &lib, &ask.filter, &mut problems));
            released.extend(released_rows(
                &dir,
                &shown,
                &lib,
                &ask.filter,
                &mut problems,
            ));
            seen.push(dir);
        }
    }
    let folders = folders(&root, ask.within, ask.no_dependents);
    let (w, r, p, searched) = checkouts(&folders, &seen, &lib, &ask.filter);
    working.extend(w);
    released.extend(r);
    problems.extend(p);
    let mut v = json!({
        "schema": ryupdm::lines::SCHEMA,
        "part": component,
        "project": lib.project,
        "working": working,
        "released": released,
        "problems": problems,
    });
    if !folders.is_empty() {
        v["dependents"] = json!(searched);
    }
    match &ask.filter {
        Filter::Label(l) => v["label"] = json!(l),
        Filter::Working => v["working_only"] = json!(true),
        Filter::Any => {}
    }
    if ask.direct {
        v["direct"] = json!(true);
    }
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_checkout_named_twice_is_searched_once() {
        let d = tempfile::tempdir().unwrap();
        let board = d.path().join("other/board");
        std::fs::create_dir_all(&board).unwrap();
        std::fs::write(board.join(crate::project::MANIFEST), "{}").unwrap();
        let folders = [
            d.path().join("other"),
            board.clone(),
            d.path().join("other/../other/board"),
        ];
        let (dirs, problems) = checkout_dirs(&folders, &[]);
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(dirs.len(), 1, "{dirs:?}");
    }

    #[test]
    fn a_design_places_a_component_of_the_library_its_alias_names() {
        let (_d, s) = crate::pdm::cmd::tests::session(crate::project::ProjectKind::Design);
        let p = s.project();
        let lib = Library {
            project: crate::project::tests::pid(9),
            dir: None,
            component: crate::pdm::tests::id("r_100k"),
        };
        assert!(
            placing(p, s.root(), &lib, &Filter::Any).is_empty(),
            "the breakout places its own"
        );
    }

    #[test]
    fn filters_pick_labels() {
        assert!(Filter::Any.takes("A") && Filter::Any.takes("working"));
        assert!(Filter::Label("A".into()).takes("A") && !Filter::Label("A".into()).takes("B"));
        assert!(Filter::Working.takes("working") && !Filter::Working.takes("A"));
    }
    #[test]
    fn the_repositorys_projects_are_found_by_manifest_leaving_this_one_out() {
        let d = tempfile::tempdir().unwrap();
        for f in [
            "lib/ryueda.json",
            "a/ryueda.json",
            "b/c/ryueda.json",
            "target/x/ryueda.json",
        ] {
            let p = d.path().join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, "{}").unwrap();
        }
        let found = others_under(d.path(), &d.path().join("lib"));
        let names: Vec<_> = found
            .iter()
            .map(|p| p.strip_prefix(d.path()).unwrap().to_owned())
            .collect();
        assert_eq!(names, [PathBuf::from("a"), PathBuf::from("b/c")]);
    }
    #[test]
    fn a_project_outside_a_repository_has_no_released_rows() {
        let (d, _) = crate::pdm::cmd::tests::session(crate::project::ProjectKind::Design);
        let dir = d.path().join("p");
        let lib = Library {
            project: crate::project::tests::pid(9),
            dir: None,
            component: crate::pdm::tests::id("r"),
        };
        let mut problems = Vec::new();
        assert!(released_rows(&dir, "board", &lib, &Filter::Any, &mut problems).is_empty());
    }
    #[test]
    fn pdm_dependents_is_a_list_of_folders() {
        use ryu_session::prefs::Schema;
        assert!(crate::Eda::accepts(
            "pdm.dependents",
            &serde_json::json!(["../boards"])
        ));
        assert!(!crate::Eda::accepts(
            "pdm.dependents",
            &serde_json::json!("../boards")
        ));
        assert_eq!(
            crate::Eda::default_of("pdm.dependents"),
            Some(serde_json::json!([]))
        );
    }

    #[test]
    fn folders_come_from_in_else_the_preference_unless_skipped() {
        let root = std::path::Path::new("/w/lib");
        assert_eq!(
            crate::pdm::where_used::folders(root, &["../a".into()], false),
            [root.join("../a")]
        );
        assert!(crate::pdm::where_used::folders(root, &[], true).is_empty());
    }

    /// #1077 (Windows): a dependents folder from the preference is canonical,
    /// as `--in`'s are.
    #[test]
    fn preference_folders_are_canonical() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir(d.path().join("a")).unwrap();
        assert_eq!(
            folders(d.path(), &["a/../a".into()], false),
            [crate::source::plain(
                std::fs::canonicalize(d.path().join("a")).unwrap()
            )]
        );
    }
    #[test]
    fn where_used_is_a_library_projects_question() {
        let (_d, mut s) = crate::pdm::cmd::tests::session(crate::project::ProjectKind::Design);
        let e = answer(
            &mut s,
            &Ask {
                part: "r_100k",
                filter: Filter::Any,
                direct: false,
                within: &[],
                no_dependents: true,
            },
        )
        .unwrap_err();
        assert_eq!(e.code, crate::error::Code::InvalidParam);
        let (_d, mut l) = crate::pdm::cmd::tests::session(crate::project::ProjectKind::Library);
        let e = answer(
            &mut l,
            &Ask {
                part: "nope",
                filter: Filter::Any,
                direct: false,
                within: &[],
                no_dependents: true,
            },
        )
        .unwrap_err();
        assert_eq!(e.code, crate::error::Code::NotFound);
        let v = answer(
            &mut l,
            &Ask {
                part: "r_100k",
                filter: Filter::Any,
                direct: true,
                within: &[],
                no_dependents: true,
            },
        )
        .unwrap();
        assert_eq!(
            (v["schema"].as_u64(), v["direct"].as_bool()),
            (Some(1), Some(true))
        );
        assert!(v["working"].as_array().unwrap().is_empty());
    }
}
