//! Where a part is used: every assembly in the files a reader reads as
//! they are whose structure reaches it, and every official release of an
//! assembly that does, read at the commit that made it official. Each
//! use says the path of parts down to it, the revision placed and how
//! many.

use crate::bom::{Row, tree};
use crate::config::Config;
use crate::label;
use crate::record::{Kind, parse_name};
use crate::structure::Reader;
use ryucad_doc::doc::assembly::Rev;
use ryucad_doc::doc::files::PartKind;
use ryucad_doc::doc::id::Id;
use ryucad_doc::error::Error;
use serde_json::{Value, json};

/// What a where-used looks for: `part` at any revision (`rev` `None`), or
/// only at one; `direct` keeps only assemblies that place it themselves.
pub struct Query {
    pub part: Id,
    pub rev: Option<Rev>,
    pub direct: bool,
}

/// The uses of the query's part under `root`: `{via, rev, count}`, `via`
/// the parts from `root` down to the one placing it, `count` how many
/// one `root` holds.
fn uses(root: &Row, q: &Query) -> Vec<Value> {
    let mut out = Vec::new();
    fn walk(row: &Row, via: &mut Vec<Id>, times: u64, q: &Query, out: &mut Vec<Value>) {
        via.push(row.part.clone());
        for c in &row.children {
            let n = times * c.count;
            if c.part == q.part {
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
            **id != q.part && p.file.as_ref().is_ok_and(|f| f.kind == PartKind::Assembly)
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
    // Official releases, by part and label.
    let mut official: Vec<(Id, String)> = reader
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
    official.sort_by_key(|(p, l)| (p.clone(), place(l), l.clone()));
    let mut released = Vec::new();
    for (a, l) in &official {
        if *a == q.part {
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
}
