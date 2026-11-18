//! Where-used (design 734): every item whose own lines, or whose
//! representations' lines, name an item; in this catalogue, in other
//! catalogues' checkouts (`--in`, the `pdm.dependents` preference, the
//! work tree) and in their official releases.

use crate::error::Result;
use crate::id::{Id, ProjectId};
use crate::item::Item;
use crate::links::Key;
use crate::load::Catalogue;
use crate::reps::{At, Reps};
use crate::session::Session;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The item asked about: its catalogue, ID and representations.
pub struct Target {
    pub catalogue: ProjectId,
    pub item: Id,
    pub links: Vec<Key>,
}

/// Item `id` of `cat` as a target.
pub fn target(cat: &Catalogue, id: &Id) -> Target {
    Target {
        catalogue: cat.manifest.project.clone(),
        item: id.clone(),
        links: cat.items[id]
            .links
            .iter()
            .map(|l| (l.product.clone(), l.project.clone(), l.part.clone()))
            .collect(),
    }
}

/// The rows of `items` (of the catalogue `own`) naming `t`: by an own
/// line (`{item, number, via: "own", line, quantity}`) or by a line of
/// one of their representations read at `at` (`{item, number, via:
/// <product>, key, quantity, rev}`). Representations are read only when
/// `t` has any; one that can't be read is a problem, not a failure.
pub fn rows<'a>(
    own: &ProjectId,
    items: impl Iterator<Item = (&'a Id, &'a Item)>,
    t: &Target,
    reps: &mut dyn Reps,
    at: &At,
    problems: &mut Vec<Value>,
) -> Vec<Value> {
    let mut out = Vec::new();
    for (id, item) in items {
        for l in &item.bom {
            let p = l.item.catalogue.as_ref().unwrap_or(own);
            if *p == t.catalogue && l.item.item == t.item {
                out.push(json!({"item": id, "number": item.number, "via": "own",
                    "line": l.line, "quantity": l.quantity}));
            }
        }
        if t.links.is_empty() {
            continue;
        }
        for link in &item.links {
            match reps.lines(link, at) {
                Err(e) => problems.push(json!(e.with("item", id))),
                Ok(read) => {
                    for rl in &read.lines {
                        let names_t = rl.project.as_ref().is_some_and(|p| {
                            t.links.iter().any(|(pr, pj, pt)| {
                                *pr == link.product && pj == p && *pt == rl.part
                            })
                        });
                        if names_t {
                            out.push(json!({"item": id, "number": item.number, "via": link.product,
                                "key": rl.key, "quantity": rl.quantity.to_string(), "rev": read.rev}));
                        }
                    }
                }
            }
        }
    }
    out
}

/// [`rows`] over every item of `cat`.
pub fn rows_in(
    cat: &Catalogue,
    t: &Target,
    reps: &mut dyn Reps,
    at: &At,
    problems: &mut Vec<Value>,
) -> Vec<Value> {
    rows(
        &cat.manifest.project,
        cat.items.iter(),
        t,
        reps,
        at,
        problems,
    )
}

/// The rows of each official release of `s`'s items naming `t`, read at
/// the commit that made it official: [`rows`] plus `{release, state,
/// commit}`. None outside a repository; a commit that can't be read is a
/// problem.
fn released_rows(
    s: &mut Session,
    t: &Target,
    reps: &mut dyn Reps,
    problems: &mut Vec<Value>,
    shown: &str,
) -> Vec<Value> {
    let Ok((repo, releases)) = crate::release::history(s) else {
        return Vec::new();
    };
    let config =
        ryupdm::config::Config::read_in(&s.snapshot(), crate::catalogue::NAME).unwrap_or_default();
    let mut at_commit: BTreeMap<String, Option<Catalogue>> = BTreeMap::new();
    let mut out = Vec::new();
    for (part, labels) in ryupdm::deps::parts::released(&releases, config.revisions) {
        for r in labels {
            if !at_commit.contains_key(&r.commit) {
                let read =
                    ryupdm::history::files_for::<crate::Plm>(repo.gix(), &r.commit, repo.prefix())
                        .and_then(|f| Catalogue::load(&f));
                let c = match read {
                    Ok(c) => Some(c),
                    Err(e) => {
                        problems.push(json!(e.with("folder", shown).with("commit", &r.commit)));
                        None
                    }
                };
                at_commit.insert(r.commit.clone(), c);
            }
            let Some(Some(cat)) = at_commit.get(&r.commit) else {
                continue;
            };
            let at = At::Commit(r.commit.clone());
            let found = rows(
                &cat.manifest.project,
                cat.items.get_key_value(&part).into_iter(),
                t,
                reps,
                &at,
                problems,
            );
            for mut row in found {
                row["release"] = json!(r.label);
                row["state"] = json!(r.state);
                row["commit"] = json!(r.commit);
                out.push(row);
            }
        }
    }
    out
}

/// The folders where-used also searches: `within` when it names any
/// (relative ones from `root`), else the `pdm.dependents` preference
/// unless `skip`.
pub fn folders(root: &Path, within: &[String], skip: bool) -> Vec<PathBuf> {
    if !within.is_empty() {
        return within.iter().map(|f| root.join(f)).collect();
    }
    if skip {
        return Vec::new();
    }
    crate::check::elsewhere(&[])
}

/// How `bom.where_used` asks.
pub struct Ask<'a> {
    pub item: &'a str,
    pub within: &'a [String],
    pub no_dependents: bool,
    /// Working files only: no releases.
    pub working: bool,
}

/// `bom.where_used`: `{schema, item, number, catalogue, working,
/// released, problems, searched}`, rows from this catalogue then each
/// other catalogue found, each row with its `catalogue` and `folder`.
pub fn answer(session: &mut Session, ask: &Ask) -> Result<Value> {
    let cat = session.catalogue().clone();
    let id = cat.find(ask.item)?;
    let t = target(&cat, &id);
    let folders = folders(session.root(), ask.within, ask.no_dependents);
    let tag = |rows: Vec<Value>, project: &ProjectId, folder: &str| -> Vec<Value> {
        rows.into_iter()
            .map(|mut r| {
                r["catalogue"] = json!(project);
                r["folder"] = json!(folder);
                r
            })
            .collect()
    };
    let mut problems = Vec::new();
    let here = session.root().display().to_string();
    let mut reps = crate::reps::Disk::new(
        crate::reps::Index::scan(&crate::reps::tops(session.root(), &folders)),
        session.root(),
    );
    let mut working = tag(
        rows_in(&cat, &t, &mut reps, &At::Working, &mut problems),
        &cat.manifest.project,
        &here,
    );
    let mut released = Vec::new();
    if !ask.working {
        let r = released_rows(session, &t, &mut reps, &mut problems, &here);
        released.extend(tag(r, &cat.manifest.project, &here));
    }
    let others = crate::numbers::others(session.root(), &folders);
    let mut searched = Vec::new();
    for dir in &others {
        let shown = dir.display().to_string();
        let mut s = match Session::open(dir) {
            Ok(s) => s,
            Err(e) => {
                problems.push(json!(e.with("folder", &shown)));
                continue;
            }
        };
        let c = s.catalogue().clone();
        if c.manifest.project == t.catalogue {
            continue; // another checkout of this catalogue
        }
        searched.push(shown.clone());
        let mut r = crate::reps::Disk::new(
            crate::reps::Index::scan(&crate::reps::tops(dir, &folders)),
            dir,
        );
        working.extend(tag(
            rows_in(&c, &t, &mut r, &At::Working, &mut problems),
            &c.manifest.project,
            &shown,
        ));
        if !ask.working {
            let rr = released_rows(&mut s, &t, &mut r, &mut problems, &shown);
            released.extend(tag(rr, &c.manifest.project, &shown));
        }
    }
    Ok(json!({
        "schema": crate::bom::SCHEMA,
        "item": id,
        "number": cat.items[&id].number,
        "catalogue": t.catalogue,
        "working": working,
        "released": released,
        "problems": problems,
        "searched": searched,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bom::tests::{id, pcba};

    #[test]
    fn an_item_is_used_by_own_lines_and_by_representations() {
        let (f, mut reps) = pcba();
        let cat = Catalogue::load(&f).unwrap();
        let screw = target(&cat, &id("0000000000002"));
        let mut problems = Vec::new();
        let rows = rows_in(&cat, &screw, &mut reps, &At::Working, &mut problems);
        let got: Vec<(String, String, String)> = rows
            .iter()
            .map(|r| {
                (
                    r["item"].as_str().unwrap().to_owned(),
                    r["via"].as_str().unwrap().to_owned(),
                    r["quantity"].as_str().unwrap().to_owned(),
                )
            })
            .collect();
        assert_eq!(
            got,
            [
                ("0000000000001".into(), "own".into(), "4".into()),
                ("0000000000001".into(), "ryucad".into(), "4".into()),
            ]
        );
        assert_eq!(problems, Vec::<Value>::new());
        // The glue has no links: only own lines are searched.
        let glue = target(&cat, &id("0000000000004"));
        assert_eq!(
            rows_in(&cat, &glue, &mut reps, &At::Working, &mut problems).len(),
            1
        );
        // Another catalogue names this one's items by project and ID.
        let mut other = cat.clone();
        other.manifest.project = crate::id::ProjectId::from_bytes([8; 16]);
        for it in other.items.values_mut() {
            for l in &mut it.bom {
                l.item.catalogue = Some(cat.manifest.project.clone());
            }
        }
        let rows = rows_in(&other, &screw, &mut reps, &At::Working, &mut problems);
        assert_eq!(rows.iter().filter(|r| r["via"] == "own").count(), 1);
    }
}
