//! `index` (design 938): the whole catalogue as JSON lines, for a tool
//! that lists, searches and walks it elsewhere (the hosted service's
//! item index). A header line, one line per item in ID order, then one
//! line of the faults that name no item. It goes to a file so neither
//! side holds 100,000 items as one value.

use crate::Plm;
use crate::error::{Code, Error, Result};
use crate::fault::Fault;
use crate::id::Id;
use crate::load::Catalogue;
use crate::reps::{At, Reps};
use crate::session::Session;
use crate::source::Source;
use ryupdm::history::Releases;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;

pub const KIND: &str = "ryuplm.index";

/// What an index is rendered from.
pub struct Parts<'a> {
    pub cat: &'a Catalogue,
    /// The catalogue's files at the revision indexed.
    pub src: &'a dyn Source,
    pub releases: &'a Releases,
    pub has_repo: bool,
    /// Every finding: the files' own, links, joined BOMs.
    pub faults: &'a [Fault],
    /// The revision as asked (`working` for the working files).
    pub rev: &'a str,
    pub commit: Option<&'a str>,
}

/// How much was written.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Written {
    pub items: usize,
    pub lines: usize,
    pub bytes: usize,
}

fn put(out: &mut dyn Write, v: &Value, n: &mut Written) -> std::io::Result<()> {
    let s = serde_json::to_string(v).expect("JSON values always serialize");
    out.write_all(s.as_bytes())?;
    out.write_all(b"\n")?;
    n.bytes += s.len() + 1;
    Ok(())
}

/// Writes the index of `p` to `out`.
///
/// - Header: `{schema, kind, project, rev, commit, items, classes,
///   schemes}`.
/// - Each item: `id`, the item's fields as its file has them (`number`,
///   `previous_numbers`, `name`, `description`, `class`, `unit`,
///   `make_buy`, `properties`, `aml`, `documents`), `links` (`{product,
///   project, part, resolved, why?}`), `lines` (the joined BOM at depth
///   1 as `bom show` gives a line, plus `child`, the child item as
///   `<catalogue>:<item>` whichever catalogue holds it), `state`,
///   `labels` (its release records as `release list` gives them), and
///   its `errors` and `warnings` counted.
/// - Last: `{faults}`, those naming no item.
pub fn render(
    p: &Parts,
    reps: &mut dyn Reps,
    at: &At,
    out: &mut dyn Write,
) -> std::io::Result<Written> {
    let cat = p.cat;
    let mut n = Written::default();
    let config = ryupdm::config::Config::read_in(p.src, crate::catalogue::NAME).ok();
    let classes: BTreeMap<&String, Value> = cat
        .manifest
        .classes
        .iter()
        .map(|(name, c)| {
            (
                name,
                match &c.properties {
                    Some(list) => json!({"properties": list}),
                    None => json!({}),
                },
            )
        })
        .collect();
    put(
        out,
        &json!({
            "schema": crate::OUTPUT_SCHEMA,
            "kind": KIND,
            "project": cat.manifest.project,
            "rev": p.rev,
            "commit": p.commit,
            "items": cat.items.len(),
            "classes": classes,
            "schemes": config.iter().flat_map(|c| c.schemes.iter()).map(|s| s.to_json()).collect::<Vec<_>>(),
        }),
        &mut n,
    )?;
    // Faults by the item they name: counts, and why a link didn't resolve.
    let mut counts: BTreeMap<&str, (u64, u64)> = BTreeMap::new();
    let mut unresolved: BTreeMap<(&str, &str), &str> = BTreeMap::new();
    let mut loose = Vec::new();
    for f in p.faults {
        let Some(item) = f.error.details["item"].as_str() else {
            loose.push(f.to_json());
            continue;
        };
        let c = counts.entry(item).or_default();
        if f.is_error() {
            c.0 += 1;
        } else {
            c.1 += 1;
        }
        if f.error.code == Code::ItemLinkUnresolved
            && let Some(product) = f.error.details["product"].as_str()
        {
            unresolved
                .entry((item, product))
                .or_insert(f.error.message.as_str());
        }
    }
    let states = ryupdm::state::derive_all_for::<Plm>(p.src, p.releases);
    let mut labels: BTreeMap<Id, Vec<Value>> = BTreeMap::new();
    if let Ok(records) = ryupdm::record::Records::read_in(p.src, crate::catalogue::NAME) {
        for e in &records.entries {
            labels
                .entry(e.part.clone())
                .or_default()
                .push(ryupdm::release::listed(e, p.releases, p.has_repo));
        }
    }
    let linked = crate::links::linked(cat);
    let own = &cat.manifest.project;
    for (id, item) in &cat.items {
        let mut v = item.to_json();
        let o = v.as_object_mut().expect("an item is an object");
        for k in ["format", "kind", "bom", "links", "source", "extra"] {
            o.remove(k);
        }
        o.insert("id".into(), json!(id));
        let links: Vec<Value> = item
            .links
            .iter()
            .map(|l| {
                let mut x = json!({"product": l.product, "project": l.project, "part": l.part});
                match unresolved.get(&(id.as_str(), l.product.as_str())) {
                    Some(why) => {
                        x["resolved"] = json!(false);
                        x["why"] = json!(why);
                    }
                    None => x["resolved"] = json!(true),
                }
                x
            })
            .collect();
        o.insert("links".into(), json!(links));
        let (joined, _) = crate::bom::of(cat, &linked, id, reps, at);
        let lines: Vec<Value> = joined
            .iter()
            .map(|j| {
                let mut x = crate::bom::line_json(cat, j);
                if let Some(r) = &j.item {
                    x["child"] = json!(format!(
                        "{}:{}",
                        r.catalogue.as_ref().unwrap_or(own),
                        r.item
                    ));
                }
                x
            })
            .collect();
        n.lines += lines.len();
        o.insert("lines".into(), json!(lines));
        o.insert(
            "state".into(),
            states.get(id).map_or(Value::Null, |s| s.to_json()),
        );
        o.insert(
            "labels".into(),
            json!(labels.remove(id).unwrap_or_default()),
        );
        let (errors, warnings) = counts.get(id.as_str()).copied().unwrap_or((0, 0));
        o.insert("errors".into(), json!(errors));
        o.insert("warnings".into(), json!(warnings));
        put(out, &v, &mut n)?;
        n.items += 1;
    }
    put(out, &json!({"faults": loose}), &mut n)?;
    Ok(n)
}

/// `index`: writes the index of the catalogue at `rev` (the working files
/// by default) to the file `out`, an absolute path, and answers
/// `{schema, out, project, rev, commit, items, lines, bytes}`. Faults are
/// `check`'s, read at that revision; representations are looked for as
/// `check` does (`within`, `pdm.dependents`).
pub fn write(
    session: &mut Session,
    out: &str,
    rev: Option<&str>,
    within: &[String],
) -> Result<Value> {
    let path = Path::new(out);
    if !path.is_absolute() {
        return Err(Error::new(
            Code::InvalidParam,
            format!("--out {out:?} must be an absolute path"),
        )
        .with("param", "out"));
    }
    let working = session.files();
    let old: BTreeMap<String, Vec<u8>>;
    let (cat, at, src): (Catalogue, At, &dyn Source) = match rev {
        None => (
            session.catalogue().clone(),
            At::Working,
            &*working as &dyn Source,
        ),
        Some(r) => {
            let (repo, _) = crate::release::history(session)?;
            let commit = crate::reps::commit_hex(repo.gix(), r)?;
            old = ryupdm::history::files_for::<Plm>(repo.gix(), &commit, repo.prefix())?;
            (
                Catalogue::load(&old)?,
                At::Commit(commit),
                &old as &dyn Source,
            )
        }
    };
    let (has_repo, releases) = match crate::release::history(session) {
        Ok((_, r)) => (true, r),
        Err(e) if e.code == Code::NotAGitRepo => (false, Releases::default()),
        Err(e) => return Err(e),
    };
    let cx = crate::judge::Context {
        elsewhere: crate::check::elsewhere(within),
        here: Some(session.root().to_path_buf()),
    };
    let mut faults = crate::judge::all(src, &cat, &cx);
    let mut reps = crate::bom::disk(session, within);
    let index = reps.index.clone();
    let others = crate::numbers::others(session.root(), &cx.elsewhere);
    faults.extend(crate::bom::findings(&cat, &index, &mut reps, &others, &at));
    let commit = match &at {
        At::Commit(c) => Some(c.clone()),
        At::Working => None,
    };
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| Error::io(dir, &e))?;
    }
    let file = std::fs::File::create(path).map_err(|e| Error::io(path, &e))?;
    let mut w = std::io::BufWriter::with_capacity(1 << 20, file);
    let n = render(
        &Parts {
            cat: &cat,
            src,
            releases: &releases,
            has_repo,
            faults: &faults,
            rev: rev.unwrap_or("working"),
            commit: commit.as_deref(),
        },
        &mut reps,
        &at,
        &mut w,
    )
    .and_then(|n| w.flush().map(|()| n))
    .map_err(|e| Error::io(path, &e))?;
    Ok(json!({
        "schema": crate::OUTPUT_SCHEMA,
        "out": out,
        "project": cat.manifest.project,
        "rev": rev.unwrap_or("working"),
        "commit": commit,
        "items": n.items,
        "lines": n.lines,
        "bytes": n.bytes,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::Request;
    use crate::server::tests::{ask, catalogue};
    use serde_json::{Value, json};

    #[test]
    fn the_seeded_structure_is_one_line_an_item() {
        let (files, mut reps) = crate::testing::structure("index", 40);
        let cat = Catalogue::load(&files).unwrap();
        let at = At::Working;
        let mut faults = crate::judge::all(&files, &cat, &Default::default());
        faults.extend(crate::bom::joined(&cat, &mut reps, &at));
        let mut out = Vec::new();
        let n = render(
            &Parts {
                cat: &cat,
                src: &files,
                releases: &Default::default(),
                has_repo: false,
                faults: &faults,
                rev: "working",
                commit: None,
            },
            &mut reps,
            &at,
            &mut out,
        )
        .unwrap();
        let text = String::from_utf8(out).unwrap();
        let lines: Vec<Value> = text
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert!(text.ends_with('\n'));
        assert_eq!(lines.len(), cat.items.len() + 2);
        assert_eq!(n.items, cat.items.len());
        let head = &lines[0];
        assert_eq!(
            (
                head["schema"].clone(),
                head["kind"].clone(),
                head["items"].clone(),
                head["commit"].clone()
            ),
            (
                json!(1),
                json!("ryuplm.index"),
                json!(cat.items.len()),
                Value::Null
            )
        );
        assert_eq!(head["project"], json!(cat.manifest.project));
        // Items in ID order, each with what the pages show.
        let ids: Vec<&str> = lines[1..lines.len() - 1]
            .iter()
            .map(|l| l["id"].as_str().unwrap())
            .collect();
        assert_eq!(
            ids,
            cat.items.keys().map(|i| i.as_str()).collect::<Vec<_>>()
        );
        let own = cat.manifest.project.to_string();
        let mut read_lines = 0;
        for l in &lines[1..lines.len() - 1] {
            assert_eq!(l["state"]["state"], "in_work");
            assert_eq!(l["labels"], json!([]));
            assert!(l["errors"].is_u64() && l["warnings"].is_u64());
            for line in l["lines"].as_array().unwrap() {
                read_lines += 1;
                // A child is always named whole: <catalogue>:<item>.
                if let Some(c) = line["child"].as_str() {
                    assert!(c.contains(':'), "{c}");
                }
                if line["from"] == "own" {
                    assert!(
                        line["child"].as_str().unwrap().starts_with(&own)
                            || line["item"].as_str().unwrap().contains(':')
                    );
                }
            }
            for link in l["links"].as_array().unwrap() {
                assert!(link["resolved"].is_boolean());
            }
        }
        assert_eq!(n.lines, read_lines);
        assert!(read_lines > 0);
        // The last line: the faults that name no item.
        assert!(lines.last().unwrap()["faults"].is_array());
        // The same bytes every time.
        let mut again = Vec::new();
        render(
            &Parts {
                cat: &cat,
                src: &files,
                releases: &Default::default(),
                has_repo: false,
                faults: &faults,
                rev: "working",
                commit: None,
            },
            &mut reps,
            &at,
            &mut again,
        )
        .unwrap();
        assert_eq!(text.as_bytes(), again);
    }

    #[test]
    fn index_writes_the_file_it_is_asked_for() {
        let (d, root) = catalogue();
        ask(
            &root,
            Request::ItemCreate {
                fields: serde_json::from_value(json!({"name": "Label"})).unwrap(),
                number: Some("LBL-1".into()),
                next: None,
                expected_revision: None,
            },
        )
        .unwrap();
        let out = d.path().join("out").join("index.jsonl");
        let v = ask(
            &root,
            Request::Index {
                out: out.display().to_string(),
                rev: None,
                within: vec![],
            },
        )
        .unwrap();
        assert_eq!(
            (v["schema"].clone(), v["items"].clone(), v["rev"].clone()),
            (json!(1), json!(1), json!("working"))
        );
        let text = std::fs::read_to_string(&out).unwrap();
        assert_eq!(v["bytes"], json!(text.len()));
        let item: Value = serde_json::from_str(text.lines().nth(1).unwrap()).unwrap();
        assert_eq!(
            (item["number"].clone(), item["name"].clone()),
            (json!("LBL-1"), json!("Label"))
        );
        assert!(
            item.get("source").is_none()
                && item.get("extra").is_none()
                && item.get("bom").is_none()
        );
        // A relative path would be the daemon's folder, not the agent's.
        let e = ask(
            &root,
            Request::Index {
                out: "index.jsonl".into(),
                rev: None,
                within: vec![],
            },
        )
        .unwrap_err();
        assert_eq!(e.code, crate::error::Code::InvalidParam);
        assert_eq!(e.details["param"], "out");
    }
}
