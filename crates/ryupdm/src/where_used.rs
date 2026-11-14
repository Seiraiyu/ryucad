//! Where a part is used: every assembly in the files a reader reads as
//! they are whose structure reaches it, and every official release of an
//! assembly that does, read at the commit that made it official. Each
//! use says the path of parts down to it, the revision placed and how
//! many. The part may be another project's: a reader that follows other
//! projects' releases finds it wherever their assemblies place it too,
//! and a path through another project's assembly names it
//! `<project ID>:<part>`.

use crate::bom::{Row, tree};
use crate::config::Config;
use crate::label;
use crate::record::{Kind, parse_name};
use crate::structure::Reader;
use ryu_doc::error::Error;
use ryu_doc::id::{Id, ProjectId};
use ryucad_doc::doc::assembly::Rev;
use ryucad_doc::doc::files::{Feature, PartKind};
use ryucad_doc::doc::project::Part;
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// What a where-used looks for: `part` (of the other project `project`,
/// or the reading project's own) at any revision (`rev` `None`), or only
/// at one; `direct` keeps only assemblies that place it themselves.
pub struct Query {
    pub part: Id,
    pub project: Option<ProjectId>,
    pub rev: Option<Rev>,
    pub direct: bool,
}

/// The uses of the query's part under `root`: `{via, rev, count}`, `via`
/// the parts from `root` down to the one placing it, `count` how many
/// one `root` holds.
fn uses(root: &Row, q: &Query) -> Vec<Value> {
    let mut out = Vec::new();
    fn walk(row: &Row, via: &mut Vec<Id>, times: u64, q: &Query, out: &mut Vec<Value>) {
        via.push(row.key());
        for c in &row.children {
            let n = times * c.count;
            if q.is(c) {
                if q.rev.as_ref().is_none_or(|r| *r == c.rev) {
                    out.push(json!({"via": via, "rev": c.rev.to_json(), "count": n}));
                }
            } else if !q.direct {
                walk(c, via, n, q, out);
            }
        }
        via.pop();
    }
    walk(root, &mut Vec::new(), 1, q, &mut out);
    out
}

impl Query {
    /// Whether `row` is of the part asked about, at any revision.
    pub fn is(&self, row: &Row) -> bool {
        row.part == self.part && row.project() == self.project.as_ref()
    }

    /// Whether an insert of `part` at `rev` (design 521) inserts the part
    /// asked about, at the revision asked about if one is.
    fn is_insert_of(&self, part: &Id, rev: &Rev) -> bool {
        *part == self.part
            && rev.project() == self.project.as_ref()
            && self.rev.as_ref().is_none_or(|r| r == rev)
    }

    /// Whether the reading project's own part `part` is the one asked
    /// about.
    fn is_own(&self, part: &Id) -> bool {
        self.project.is_none() && *part == self.part
    }
}

/// The official releases on the reader's release branch (none outside a
/// repository), by part and label in `config`'s order.
fn official(reader: &Reader, config: &Config) -> Vec<(Id, String)> {
    let mut out: Vec<(Id, String)> = reader
        .releases
        .map(|r| {
            r.official
                .keys()
                .filter_map(|path| {
                    let mut seg = path.split('/').skip(1);
                    let part = Id::parse(seg.next()?, "part").ok()?;
                    let (label, kind) = parse_name(seg.next()?)?;
                    (kind == Kind::Release).then_some((part, label))
                })
                .collect()
        })
        .unwrap_or_default();
    let place = |l: &str| label::index(config.revisions, l);
    out.sort_by_key(|(p, l)| (p.clone(), place(l), l.clone()));
    out
}

/// Each insert of `p` the query asks about (design 521), by feature, in
/// feature order: `(feature, rev)`. An insert is geometry, never a
/// component, so BOMs don't count it: it is used where it is inserted.
fn inserts_of(p: &Part, q: &Query) -> Vec<(Id, Rev)> {
    let mut out: Vec<(Id, Rev)> = p
        .features
        .iter()
        .filter_map(|(fid, f)| match f {
            Ok(Feature::Insert(i)) if q.is_insert_of(&i.part, &i.rev) => {
                Some((fid.clone(), i.rev.clone()))
            }
            _ => None,
        })
        .collect();
    out.sort();
    out
}

/// Each insert of `p` of another project's part at a label: `(feature,
/// project, part, label)`, in feature order.
fn foreign_inserts(p: &Part) -> Vec<(Id, ProjectId, Id, String)> {
    let mut out: Vec<_> = p
        .features
        .iter()
        .filter_map(|(fid, f)| match f {
            Ok(Feature::Insert(i)) => match &i.rev {
                Rev::Foreign { project, label } => {
                    Some((fid.clone(), project.clone(), i.part.clone(), label.clone()))
                }
                _ => None,
            },
            _ => None,
        })
        .collect();
    out.sort();
    out
}

/// What the BOM under `root` places from other projects, at any depth:
/// each `(project, part, label)` with every path to it (`via` the parts
/// from `root` down to the one placing it, `<project ID>:<part>` for
/// another project's) and how many `root` holds.
fn foreign(root: &Row) -> BTreeMap<(ProjectId, Id, String), Vec<Value>> {
    let mut out: BTreeMap<_, Vec<Value>> = BTreeMap::new();
    fn walk(
        row: &Row,
        via: &mut Vec<Id>,
        times: u64,
        out: &mut BTreeMap<(ProjectId, Id, String), Vec<Value>>,
    ) {
        via.push(row.key());
        for c in &row.children {
            let n = times * c.count;
            if let Rev::Foreign { project, label } = &c.rev {
                out.entry((project.clone(), c.part.clone(), label.clone()))
                    .or_default()
                    .push(json!({"via": via, "count": n}));
            }
            walk(c, via, n, out);
        }
        via.pop();
    }
    walk(root, &mut Vec::new(), 1, &mut out);
    out
}

/// [`foreign`] for one assembly as a list: `{project, dependency, part,
/// label, count, uses}` in project, part and label order.
fn foreign_list(reader: &Reader, root: &Row) -> Vec<Value> {
    foreign(root)
        .into_iter()
        .map(|((project, part, label), uses)| {
            json!({"project": project, "dependency": reader.name(&project).unwrap_or(project.as_str()),
                   "part": part, "label": label, "count": total(&uses), "uses": uses})
        })
        .collect()
}

/// What this project places from other projects, offline, as `deps uses`
/// answers it (schema [`crate::bom::SCHEMA`]): `working`, each assembly of
/// the reader's files as they are with what it places from other
/// projects (`{assembly, places}`, `places` [`foreign_list`]'s); `released`,
/// each official release of an assembly that places any, with its label
/// and commit; `inserts`, each part inserting another project's part
/// (`{kind: insert, part, feature, project, dependency, of, label}`, and
/// `release`, `commit` for one in a release; left out when none);
/// `problems`, what couldn't be read. The service indexes it.
pub fn placed_from_others(reader: &mut Reader, config: &Config) -> Value {
    let mut working = Vec::new();
    let mut problems: Vec<Error> = Vec::new();
    let assemblies: Vec<Id> = reader
        .project
        .parts
        .iter()
        .filter(|(_, p)| p.file.as_ref().is_ok_and(|f| f.kind == PartKind::Assembly))
        .map(|(id, _)| id.clone())
        .collect();
    for a in assemblies {
        match tree(reader, &a, &Rev::Working, None) {
            Ok(root) => {
                problems.extend(crate::bom::problems(&root));
                let places = foreign_list(reader, &root);
                if !places.is_empty() {
                    working.push(json!({"assembly": a, "places": places}));
                }
            }
            Err(e) => problems.push(e),
        }
    }
    // Each part that inserts another project's part (design 521).
    let insert_row =
        |reader: &Reader, pid: &Id, (fid, project, part, label): (Id, ProjectId, Id, String)| {
            json!({"kind": "insert", "part": pid, "feature": fid, "project": project,
               "dependency": reader.name(&project).unwrap_or(project.as_str()),
               "of": part, "label": label})
        };
    let parts: Vec<(Id, Part)> = reader
        .project
        .parts
        .iter()
        .map(|(id, p)| (id.clone(), p.clone()))
        .collect();
    // Beside the assemblies' rows, not among them: the service indexes
    // those by assembly.
    let mut inserts = Vec::new();
    for (pid, p) in &parts {
        for i in foreign_inserts(p) {
            inserts.push(insert_row(reader, pid, i));
        }
    }
    let mut released = Vec::new();
    for (a, l) in official(reader, config) {
        let rev = Rev::Label(l.clone());
        let root = match reader.node(&a, &rev) {
            Ok(n) if n.kind() != Some(PartKind::Assembly) => {
                let (part, commit) = (n.part.clone(), n.commit.clone());
                for i in foreign_inserts(&part) {
                    let mut row = insert_row(reader, &a, i);
                    row["release"] = json!(l);
                    row["commit"] = json!(commit);
                    inserts.push(row);
                }
                continue;
            }
            Ok(_) => tree(reader, &a, &rev, None),
            Err(e) => Err(e),
        };
        let root = match root {
            Ok(r) => r,
            Err(e) => {
                problems.push(e);
                continue;
            }
        };
        problems.extend(crate::bom::problems(&root));
        let places = foreign_list(reader, &root);
        if !places.is_empty() {
            let commit = root.node.as_ref().ok().and_then(|n| n.commit.clone());
            released.push(json!({"assembly": a, "label": l, "commit": commit, "places": places}));
        }
    }
    problems.dedup();
    let mut v = json!({
        "schema": crate::bom::SCHEMA,
        "project": reader.own(),
        "working": working,
        "released": released,
        "problems": problems,
    });
    // Each part inserting another project's part (design 521), working and
    // in its official releases (`release`, `commit`).
    if !inserts.is_empty() {
        v["inserts"] = json!(inserts);
    }
    if reader.releases.and_then(|r| r.branch.as_ref()).is_none() {
        v["released_note"] =
            json!("no release branch was read, so no released assembly was searched");
    }
    v
}

fn total(uses: &[Value]) -> u64 {
    uses.iter().filter_map(|u| u["count"].as_u64()).sum()
}

/// Where the query's part is used: `working`, each assembly the reader's
/// project has as it is; `released`, each official release of an
/// assembly (the reader's release branch; none outside a repository),
/// with its label, its state (`released`, `superseded` by a later
/// official one, `obsolete`) and the commit read; `problems`, releases
/// that couldn't be read. `config` orders labels.
pub fn where_used(reader: &mut Reader, q: &Query, config: &Config) -> Value {
    let mut working = Vec::new();
    let mut problems: Vec<Error> = Vec::new();
    let assemblies: Vec<Id> = reader
        .project
        .parts
        .iter()
        .filter(|(id, p)| {
            !q.is_own(id) && p.file.as_ref().is_ok_and(|f| f.kind == PartKind::Assembly)
        })
        .map(|(id, _)| id.clone())
        .collect();
    for a in assemblies {
        match tree(reader, &a, &Rev::Working, None) {
            Ok(root) => {
                let u = uses(&root, q);
                if !u.is_empty() {
                    working.push(json!({"assembly": a, "count": total(&u), "uses": u}));
                }
            }
            Err(e) => problems.push(e),
        }
    }
    // Each working part that inserts it (design 521), read from the files
    // alone as placements are, by part then feature.
    for (pid, p) in &reader.project.parts {
        for (fid, rev) in inserts_of(p, q) {
            working.push(json!({"kind": "insert", "part": pid, "feature": fid,
                                "rev": rev.to_json()}));
        }
    }
    let official = official(reader, config);
    let place = |l: &str| label::index(config.revisions, l);
    let mut released = Vec::new();
    for (a, l) in &official {
        if q.is_own(a) {
            continue;
        }
        let rev = Rev::Label(l.clone());
        let node = match reader.node(a, &rev) {
            Ok(n) => n,
            Err(e) => {
                problems.push(e);
                continue;
            }
        };
        let later = official
            .iter()
            .any(|(p, other)| p == a && place(other) > place(l));
        let state = if node.obsolete {
            "obsolete"
        } else if later {
            "superseded"
        } else {
            "released"
        };
        // A release of any part that inserts it, at the commit that made
        // it official.
        for (fid, r) in inserts_of(&node.part, q) {
            released.push(
                json!({"kind": "insert", "part": a, "label": l, "state": state,
                                 "commit": node.commit, "feature": fid, "rev": r.to_json()}),
            );
        }
        if node.kind() != Some(PartKind::Assembly) {
            continue;
        }
        let root = match tree(reader, a, &rev, None) {
            Ok(r) => r,
            Err(e) => {
                problems.push(e);
                continue;
            }
        };
        let u = uses(&root, q);
        if u.is_empty() {
            continue;
        }
        released.push(json!({"assembly": a, "label": l, "state": state,
                             "commit": node.commit, "count": total(&u), "uses": u}));
    }
    let mut v = json!({
        "schema": crate::bom::SCHEMA,
        "part": q.part,
        "working": working,
        "released": released,
        "problems": problems,
    });
    if let Some(r) = &q.rev {
        v["rev"] = r.to_json();
    }
    if q.direct {
        v["direct"] = json!(true);
    }
    if let Some(p) = &q.project {
        v["project"] = json!(p);
        v["dependency"] = json!(reader.name(p).unwrap_or(p.as_str()));
    }
    if reader.releases.and_then(|r| r.branch.as_ref()).is_none() {
        v["released_note"] =
            json!("no release branch was read, so no released assembly was searched");
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bom::tests::frame;
    use crate::state::tests::id;
    use crate::structure::Cache;
    use ryucad_doc::doc::project::Project;
    use std::path::Path;

    fn ask(part: &str, rev: Option<Rev>, direct: bool) -> Value {
        let f = frame();
        let project = Project::load_from(Path::new(""), &f).unwrap();
        let mut cache = Cache::new();
        let mut r = Reader::new(&f, &project, None, None, &mut cache);
        let q = Query {
            part: id(part),
            project: None,
            rev,
            direct,
        };
        where_used(&mut r, &q, &Config::default())
    }

    #[test]
    fn every_assembly_that_reaches_a_part_says_how() {
        let v = ask("bolt", None, false);
        let working = v["working"].as_array().unwrap();
        let names: Vec<&Value> = working.iter().map(|w| &w["assembly"]).collect();
        assert_eq!(names, [&json!("frame"), &json!("motor"), &json!("sub")]);
        // frame: four through its two subs, one inside its bought motor.
        assert_eq!(working[0]["count"], 5);
        assert_eq!(
            working[0]["uses"][0],
            json!({"via": ["frame", "sub"], "rev": "working", "count": 4})
        );
        assert_eq!(v["released"], json!([]));
        assert!(v["released_note"].is_string());
        // Directly: only sub and motor place bolts themselves.
        let v = ask("bolt", None, true);
        assert_eq!(v["working"].as_array().unwrap().len(), 2);
        // At a label nobody places: nothing.
        let v = ask("bolt", Some(Rev::Label("A".into())), false);
        assert_eq!(v["working"], json!([]));
        assert_eq!(v["rev"], json!({"label": "A"}));
    }

    #[test]
    fn another_projects_part_is_found_through_its_assemblies() {
        let (f, mut others) = crate::bom::tests::across();
        let lib = ProjectId::from_bytes([1; 16]);
        let project = Project::load_from(Path::new(""), &f).unwrap();
        let mut cache = Cache::new();
        let mut r = Reader::new(&f, &project, None, None, &mut cache).with_others(&mut others);
        let q = Query {
            part: id("bolt"),
            project: Some(lib.clone()),
            rev: None,
            direct: false,
        };
        let v = where_used(&mut r, &q, &Config::default());
        assert_eq!(
            (&v["project"], &v["dependency"]),
            (&json!(lib), &json!("lib"))
        );
        let kit = Id::foreign(&lib, &id("kit"));
        assert_eq!(
            v["working"],
            json!([{"assembly": "asm", "count": 2, "uses": [
                {"via": ["asm", kit], "rev": {"label": "A", "project": lib}, "count": 2}]}])
        );
        // This project's own plate, placed by the library's kit.
        let q = Query {
            part: id("plate"),
            project: None,
            rev: None,
            direct: false,
        };
        let v = where_used(&mut r, &q, &Config::default());
        assert_eq!(v["working"][0]["uses"][0]["via"], json!(["asm", kit]));
        // A part of this project's with another project's part's name: none.
        let q = Query {
            part: id("bolt"),
            project: None,
            rev: None,
            direct: false,
        };
        assert_eq!(
            where_used(&mut r, &q, &Config::default())["working"],
            json!([])
        );
    }

    #[test]
    fn uses_lists_what_each_assembly_places_from_other_projects() {
        let (f, mut others) = crate::bom::tests::across();
        let lib = ProjectId::from_bytes([1; 16]);
        let project = Project::load_from(Path::new(""), &f).unwrap();
        let mut cache = Cache::new();
        let mut r = Reader::new(&f, &project, None, None, &mut cache).with_others(&mut others);
        let v = placed_from_others(&mut r, &Config::default());
        let kit = Id::foreign(&lib, &id("kit"));
        assert_eq!(v["project"], json!(ProjectId::from_bytes([9; 16])));
        assert_eq!(
            v["working"],
            json!([{"assembly": "asm", "places": [
                {"project": lib, "dependency": "lib", "part": "bolt", "label": "A", "count": 2,
                 "uses": [{"via": ["asm", kit], "count": 2}]},
                {"project": lib, "dependency": "lib", "part": "kit", "label": "A", "count": 1,
                 "uses": [{"via": ["asm"], "count": 1}]}]}])
        );
        assert_eq!((&v["released"], &v["problems"]), (&json!([]), &json!([])));
        assert!(v["released_note"].is_string());
        // Within one project, nothing.
        let f = frame();
        let project = Project::load_from(Path::new(""), &f).unwrap();
        let mut r = Reader::new(&f, &project, None, None, &mut cache);
        assert_eq!(
            placed_from_others(&mut r, &Config::default())["working"],
            json!([])
        );
    }

    #[test]
    fn a_part_inserted_is_used_where_it_is_inserted() {
        let mut f = frame();
        let put = |f: &mut std::collections::BTreeMap<String, Vec<u8>>, p: &str, v: Value| {
            f.insert(p.to_owned(), ryu_doc::canonical::to_string(&v).into_bytes());
        };
        for (part, rev) in [
            ("enclosure", json!("working")),
            ("case", json!({"label": "A"})),
        ] {
            put(
                &mut f,
                &format!("parts/{part}/part.json"),
                json!({"features": ["board"], "format": 1, "kind": "ryucad.part"}),
            );
            put(
                &mut f,
                &format!("parts/{part}/features/board.json"),
                json!({"format": 21, "kind": "ryucad.feature", "type": "insert",
                       "part": "bolt", "rev": rev}),
            );
        }
        let project = Project::load_from(Path::new(""), &f).unwrap();
        let mut cache = Cache::new();
        let mut r = Reader::new(&f, &project, None, None, &mut cache);
        let q = Query {
            part: id("bolt"),
            project: None,
            rev: None,
            direct: false,
        };
        let v = where_used(&mut r, &q, &Config::default());
        let w = v["working"].as_array().unwrap();
        let inserts: Vec<&Value> = w.iter().filter(|r| r["kind"] == "insert").collect();
        assert_eq!(inserts.len(), 2, "{v}");
        assert_eq!(
            *inserts[0],
            json!({"kind": "insert", "part": "case", "feature": "board", "rev": {"label": "A"}})
        );
        assert_eq!(inserts[1]["part"], "enclosure");
        assert!(w.iter().any(|r| r["assembly"] == "frame"));
        // At working only: the enclosure's.
        let q = Query {
            part: id("bolt"),
            project: None,
            rev: Some(Rev::Working),
            direct: false,
        };
        let v = where_used(&mut r, &q, &Config::default());
        let parts: Vec<&Value> = v["working"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| r["kind"] == "insert")
            .map(|r| &r["part"])
            .collect();
        assert_eq!(parts, [&json!("enclosure")]);
    }
}
