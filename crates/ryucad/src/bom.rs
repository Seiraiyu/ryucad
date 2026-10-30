//! Product structure through the protocol: `bom.get`, an assembly's
//! indented or flat BOM (JSON or CSV) at the working files, a Git
//! revision or a release label; `bom.diff`, what changed in it between two
//! revisions; and `bom.where_used`, the assemblies that use a part now and
//! in every official release. ryupdm reads them from the files alone
//! ([`ryupdm::bom`], [`ryupdm::where_used`]); nothing is rebuilt.
//! `bom.renumber` changes an assembly's item numbers
//! ([`ryupdm::items`]).
//!
//! Foreign instances are followed into their projects at the commits
//! `ryucad.lock` names, read from the machine's dependency cache, offline
//! ([`ryupdm::deps::read::Through`]); what can't be read there (not
//! fetched, not locked) is the row's problem, never the BOM's failure.

use crate::doc::assembly::Rev;
use crate::doc::files::PartKind;
use crate::doc::id::{Id, ProjectId};
use crate::doc::project::Project;
use crate::error::{Code, Error, Result};
use crate::server::part_id;
use crate::session::Session;
use ryupdm::bom::{self, Row};
use ryupdm::config::Config;
use ryupdm::structure::{Reader, Repo};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// What a reader reads besides the files: the commit, the project, the
/// settings and each working part's state.
struct Ground {
    commit: Option<String>,
    config: Config,
    states: BTreeMap<Id, Value>,
}

/// Runs `f` with a reader over the project at `rev` (`working` or none
/// for the working files, else a Git revision), labels resolved against
/// the release branch as it is (none outside a repository).
fn reading<T>(
    session: &mut Session,
    rev: Option<&str>,
    f: impl FnOnce(&mut Reader, &Ground) -> Result<T>,
) -> Result<(Option<String>, T)> {
    let (commit, s) = session.at(rev)?;
    let history = match crate::release::history(s) {
        Ok(h) => Some(h),
        Err(e) if e.code == Code::NotAGitRepo => None,
        Err(e) => return Err(e),
    };
    let mut cache = std::mem::take(&mut s.structure);
    let mut deps = s.take_deps_reader()?;
    let lock = crate::deps::lock_of(s.project()).unwrap_or_default();
    let out = {
        // The files as the session last read them, uncopied.
        let s: &Session = s;
        let releases = history.as_ref().map(|(_, r)| r);
        let ground = Ground {
            commit: commit.clone(),
            config: Config::read(s).unwrap_or_default(),
            states: ryupdm::state::derive_all(s, &releases.cloned().unwrap_or_default())
                .into_iter()
                .map(|(p, st)| (p, st.to_json()))
                .collect(),
        };
        let repo = history.as_ref().map(|(r, _)| Repo {
            git: r.gix(),
            prefix: r.prefix(),
        });
        let mut through = ryupdm::deps::read::Through {
            reader: &mut deps,
            lock,
            deps: &s.project().manifest.dependencies,
        };
        let mut reader =
            Reader::new(s, s.project(), repo, releases, &mut cache).with_others(&mut through);
        f(&mut reader, &ground)
    };
    s.put_deps_reader(deps);
    s.structure = cache;
    Ok((commit, out?))
}

/// The part a BOM is of: the one asked for, else the project's one root
/// assembly (one no assembly places); several or none is `invalid_param`
/// naming them.
fn root(project: &Project, part: Option<&str>) -> Result<Id> {
    if let Some(p) = part {
        let id = part_id(p)?;
        project.part(&id)?;
        return Ok(id);
    }
    let is_assembly = |id: &Id| {
        project.parts[id]
            .file
            .as_ref()
            .is_ok_and(|f| f.kind == PartKind::Assembly)
    };
    let placed: Vec<&Id> = project
        .parts
        .values()
        .flat_map(|p| p.features.values())
        .filter_map(|f| match f {
            Ok(crate::doc::files::Feature::Instance(i)) => Some(&i.part),
            _ => None,
        })
        .collect();
    let roots: Vec<&Id> = project
        .parts
        .keys()
        .filter(|id| is_assembly(id) && !placed.contains(id))
        .collect();
    match roots.as_slice() {
        [one] => Ok((*one).clone()),
        _ => Err(Error::new(
            Code::InvalidParam,
            "the project has no one root assembly: name the part whose BOM you want",
        )
        .with("param", "part")
        .with("roots", roots)),
    }
}

/// `label:<L>` as a release label; anything else is a revision of the
/// project's files.
fn label_of(rev: Option<&str>) -> Option<Rev> {
    rev.and_then(|r| r.strip_prefix("label:"))
        .map(|l| Rev::Label(l.to_owned()))
}

/// How `bom.get` gives a BOM.
pub struct Ask<'a> {
    pub part: Option<&'a str>,
    pub rev: Option<&'a str>,
    pub flat: bool,
    pub depth: Option<usize>,
    /// `json` (the default) or `csv`.
    pub format: Option<&'a str>,
}

/// `bom.get`: `{schema, root, rev, commit?, tree | flat, csv?, columns,
/// problems, findings}`, `columns` the CSV's (its declared properties in
/// the settings' order), whatever the format.
pub fn get(session: &mut Session, ask: &Ask) -> Result<Value> {
    let csv = match ask.format.unwrap_or("json") {
        "json" => false,
        "csv" => true,
        other => {
            return Err(Error::new(
                Code::InvalidParam,
                format!("format is json or csv, not {other:?}"),
            )
            .with("param", "format"));
        }
    };
    let label = label_of(ask.rev);
    let files_rev = if label.is_some() { None } else { ask.rev };
    let (_, v) = reading(session, files_rev, |reader, g| {
        let part = root(reader.project, ask.part)?;
        let rev = label.clone().unwrap_or(Rev::Working);
        let whole = bom::tree(reader, &part, &rev, None)?;
        let mut v = json!({
            "schema": bom::SCHEMA,
            "root": part,
            "rev": ask.rev.unwrap_or("working"),
            "problems": bom::problems(&whole),
            "findings": bom::findings(&whole),
            "columns": bom::columns(&g.config, bom::has_foreign(&whole)),
        });
        if ask.flat {
            let rows = bom::flat(&whole, &g.config);
            v["flat"] = rows.iter().map(|f| bom::flat_json(f, &g.states)).collect();
            if csv {
                v["csv"] = json!(bom::csv_flat(&rows, &g.config));
            }
        } else {
            let shown = match ask.depth {
                None => whole,
                Some(d) => bom::tree(reader, &part, &rev, Some(d))?,
            };
            v["tree"] = bom::row_json(&shown, &g.states);
            if csv {
                v["csv"] = json!(bom::csv_tree(&shown, &g.config));
            }
        }
        if let Some(c) = &g.commit {
            v["commit"] = json!(c);
        }
        Ok(v)
    })?;
    Ok(v)
}

/// The whole BOM of `part` at `rev` (or `None` when the part isn't
/// there), with the commit read and the settings there.
fn side(
    session: &mut Session,
    part: &Id,
    rev: &str,
) -> Result<(Option<String>, Option<Row>, Config)> {
    let label = label_of(Some(rev));
    let files_rev = if label.is_some() { None } else { Some(rev) };
    let (commit, (row, config)) = reading(session, files_rev, |reader, g| {
        let rev = label.clone().unwrap_or(Rev::Working);
        let row = match bom::tree(reader, part, &rev, None) {
            Ok(r) => Some(r),
            Err(e) if e.code == Code::NotFound => None,
            Err(e) => return Err(e),
        };
        Ok((row, g.config.clone()))
    })?;
    Ok((commit, row, config))
}

/// `bom.diff`: the BOM of `part` (else the working files' one root
/// assembly) from `from` (`HEAD` by default) to `to` (`working`).
pub fn diff(
    session: &mut Session,
    part: Option<&str>,
    from: Option<&str>,
    to: Option<&str>,
) -> Result<Value> {
    let (from, to) = (from.unwrap_or("HEAD"), to.unwrap_or("working"));
    let part = match part {
        Some(p) => part_id(p)?,
        None => root(session.project(), None)?,
    };
    let (ca, a, _) = side(session, &part, from)?;
    let (cb, b, config) = side(session, &part, to)?;
    if a.is_none() && b.is_none() {
        return Err(Error::new(
            Code::NotFound,
            format!("{part} is in neither {from} nor {to}"),
        )
        .with("part", &part));
    }
    let mut v = bom::diff(a.as_ref(), b.as_ref(), &config);
    v["schema"] = json!(bom::SCHEMA);
    v["root"] = json!(part);
    v["from"] = json!({"rev": from, "commit": ca});
    v["to"] = json!({"rev": to, "commit": cb});
    Ok(v)
}

/// The part `of` names: this project's (`bolt`), or another project's as
/// `<dependency>:<part>`, the dependency by its name or project ID (a
/// project the lock reaches through a dependency by its ID).
pub(crate) fn of_part(project: &Project, of: &str) -> Result<(Option<ProjectId>, Id)> {
    let Some((dep, part)) = of.split_once(':') else {
        return Ok((None, part_id(of)?));
    };
    let id = match crate::doc::deps::find(&project.manifest.dependencies, dep) {
        Ok((id, _)) => id.clone(),
        Err(e) => match ProjectId::parse(dep) {
            Ok(id) => id,
            Err(_) => return Err(e.with("param", "part")),
        },
    };
    Ok((Some(id), part_id(part)?))
}

/// What `bom.where_used` is asked.
pub struct WhereUsed<'a> {
    /// A part, or another project's as `<dependency>:<part>`.
    pub part: &'a str,
    pub label: Option<&'a str>,
    pub working: bool,
    pub direct: bool,
    /// Other projects' folders on disk to search too; none for the
    /// `pdm.dependents` preference's.
    pub within: &'a [String],
    /// Search no other folders, whatever the preference says.
    pub no_dependents: bool,
}

/// `bom.where_used`: where `part` is used, at any revision, or only at
/// release `label`, or only `working`; in this project's assemblies, then
/// in those of the projects on disk that depend on it
/// ([`crate::dependents`]), each row of theirs with its `project`,
/// `folder` and `name`.
pub fn where_used(session: &mut Session, ask: &WhereUsed) -> Result<Value> {
    let rev = match (ask.label, ask.working) {
        (Some(_), true) => {
            return Err(Error::new(
                Code::InvalidParam,
                "ask for a label or for working, not both",
            )
            .with("param", "label"));
        }
        (Some(l), false) => Some(Rev::Label(crate::doc::assembly::label_text(l)?)),
        (None, true) => Some(Rev::Working),
        (None, false) => None,
    };
    let (project, part) = of_part(session.project(), ask.part)?;
    let rev = match (rev, &project) {
        (Some(Rev::Label(l)), Some(p)) => Some(Rev::Foreign {
            project: p.clone(),
            label: l,
        }),
        (r, _) => r,
    };
    let q = ryupdm::where_used::Query {
        part,
        project,
        rev,
        direct: ask.direct,
    };
    let (_, mut v) = reading(session, None, |reader, g| {
        Ok(ryupdm::where_used::where_used(reader, &q, &g.config))
    })?;
    let folders = crate::dependents::folders(session.root(), ask.within, ask.no_dependents);
    if folders.is_empty() {
        return Ok(v);
    }
    let Some(own) = session.project().manifest.project.clone() else {
        v["problems"].as_array_mut().expect("a list").push(json!(Error::new(
            Code::ProjectIdMissing,
            "this project has no ID yet, so no other project can depend on it: nothing else was searched",
        )));
        return Ok(v);
    };
    let found = crate::dependents::search(session, &own, &q, &folders)?;
    for (k, rows) in [
        ("working", found.working),
        ("released", found.released),
        ("problems", found.problems),
    ] {
        v[k].as_array_mut().expect("a list").extend(rows);
    }
    v["dependents"] = json!(found.searched);
    Ok(v)
}

/// `bom.renumber`: sets the item numbers of parts assembly `part` places
/// (`items`, part ID to number), or with `compact` numbers them 1 to n in
/// their current order; one change, guarded by the assembly's revision.
pub fn renumber(
    session: &mut Session,
    part: &str,
    items: &serde_json::Map<String, Value>,
    compact: bool,
    expected: Option<&str>,
) -> Result<Value> {
    let id = part_id(part)?;
    let p = session.project().part(&id)?.clone();
    crate::server::check_revision(&p, expected)?;
    let file = p.file.clone()?;
    if file.kind != PartKind::Assembly {
        return Err(Error::new(
            Code::WrongPartKind,
            format!("{id} is a part: only an assembly numbers what it places"),
        )
        .with("part", &id));
    }
    let placed = ryupdm::items::placed(&p);
    let new = match (compact, items.is_empty()) {
        (true, true) => ryupdm::items::compacted(&file.items, &placed),
        (false, false) => {
            let sets = items
                .iter()
                .map(|(k, v)| {
                    let n = v.as_u64().ok_or_else(|| {
                        Error::new(
                            Code::InvalidParam,
                            format!("item for {k}: expected a whole number, got {v}"),
                        )
                        .with("param", "items")
                        .with("part", k)
                    })?;
                    Ok((part_id(k)?, n))
                })
                .collect::<Result<Vec<_>>>()?;
            ryupdm::items::renumbered(&file.items, &placed, &sets)?
        }
        _ => {
            return Err(Error::new(
                Code::Usage,
                "give PART=N for each number to set, or --compact alone",
            ));
        }
    };
    crate::doc::project::write_json(
        &crate::doc::project::part_path(session.root(), &id),
        &crate::doc::files::PartFile { items: new, ..file }.to_json(),
    )?;
    session.reload()?;
    let p = session.project().part(&id)?;
    let items = p.file.as_ref().map(|f| json!(f.items)).unwrap_or_default();
    Ok(json!({"part": id, "revision": p.revision, "items": items}))
}

/// What makes a BOM of the working files unsound, as `check` findings
/// (`source: bom`): each part's BOM read, each finding once, in part
/// order.
pub(crate) fn findings(session: &mut Session) -> Result<Vec<Value>> {
    let (_, out) = reading(session, None, |reader, _| {
        let mut out: Vec<Value> = Vec::new();
        let parts: Vec<Id> = reader.project.parts.keys().cloned().collect();
        for part in parts {
            let Ok(row) = bom::tree(reader, &part, &Rev::Working, None) else {
                continue;
            };
            for f in bom::findings(&row) {
                if !out.contains(&f) {
                    out.push(f);
                }
            }
        }
        Ok(out)
    })?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use crate::git::tests::git;
    use crate::release::tests::released_project;
    use crate::server::tests::call;
    use crate::session::Session;
    use serde_json::{Value, json};

    /// `p` released as A and committed; assembly `asm` places two of
    /// `p@A` and one bought `kit` (two tubes counted in metres).
    fn assembly() -> (tempfile::TempDir, Session) {
        let (dir, mut s) = released_project();
        let root = dir.path();
        call(&mut s, "release.propose", json!({"part": "p"})).unwrap();
        git(root, &["add", "."]);
        git(root, &["commit", "-q", "-m", "release p A"]);
        for (part, kind) in [("asm", "assembly"), ("kit", "assembly"), ("tube", "part")] {
            call(&mut s, "part.create", json!({"part": part, "kind": kind})).unwrap();
        }
        let add = |s: &mut Session, asm: &str, id: &str, of: &str, rev: Value| {
            call(
                s,
                "feature.add",
                json!({"part": asm, "type": "instance", "id": id, "of": of, "rev": rev}),
            )
            .unwrap();
        };
        add(&mut s, "asm", "p1", "p", json!("A"));
        add(&mut s, "asm", "p2", "p", json!("A"));
        add(&mut s, "asm", "k", "kit", json!("working"));
        add(&mut s, "kit", "t1", "tube", json!("working"));
        add(&mut s, "kit", "t2", "tube", json!("working"));
        call(
            &mut s,
            "props.set",
            json!({"part": "tube", "properties": {"uom": "m", "amount": "250 mm"}}),
        )
        .unwrap();
        call(
            &mut s,
            "props.set",
            json!({"part": "kit", "properties": {"make_buy": "buy", "name": "Tube kit"}}),
        )
        .unwrap();
        git(root, &["add", "."]);
        git(root, &["commit", "-q", "-m", "asm"]);
        (dir, s)
    }

    #[test]
    fn renumbering_writes_the_assemblys_items_against_its_revision() {
        let (_dir, mut s) = assembly();
        let items = |s: &Session| {
            let p = s
                .project()
                .part(&crate::doc::id::Id::parse("asm", "part").unwrap())
                .unwrap();
            json!(p.file.as_ref().unwrap().items)
        };
        assert_eq!(items(&s), json!({"p": 1, "kit": 2}));
        let rev = call(&mut s, "part.list", json!({})).unwrap()["parts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["part"] == "asm")
            .unwrap()["revision"]
            .clone();
        let v = call(
            &mut s,
            "bom.renumber",
            json!({"part": "asm", "items": {"p": 2, "kit": 1}, "expected_revision": rev}),
        )
        .unwrap();
        assert_eq!(v["items"], json!({"p": 2, "kit": 1}));
        // The revision it was asked against has moved on.
        let e = call(
            &mut s,
            "bom.renumber",
            json!({"part": "asm", "items": {"p": 7}, "expected_revision": rev}),
        )
        .unwrap_err();
        assert_eq!(e.code, crate::error::Code::Conflict);
        let tree = call(&mut s, "bom.get", json!({"part": "asm"})).unwrap();
        assert_eq!(tree["tree"]["children"][0]["item"], 2);
        let v = call(
            &mut s,
            "bom.renumber",
            json!({"part": "asm", "compact": true}),
        )
        .unwrap();
        assert_eq!(v["items"], json!({"kit": 1, "p": 2}));
        let e = call(&mut s, "bom.renumber", json!({"part": "asm"})).unwrap_err();
        assert_eq!(e.code, crate::error::Code::Usage);
    }

    #[test]
    fn a_bom_is_read_from_the_files_at_any_revision() {
        let (dir, mut s) = assembly();
        let v = call(&mut s, "bom.get", json!({})).unwrap();
        assert_eq!((&v["schema"], &v["root"]), (&json!(1), &json!("asm")));
        let kids = v["tree"]["children"].as_array().unwrap();
        assert_eq!(kids[0]["quantity"], json!({"count": 2, "uom": "each"}));
        assert_eq!(kids[0]["rev"], json!({"label": "A"}));
        assert_eq!(kids[0]["properties"]["number"], "100");
        assert!(kids[0]["commit"].is_string());
        assert_eq!(kids[1]["children"][0]["quantity"]["text"], "0.5 m");
        assert_eq!(kids[1]["children"][0]["bought"], true);
        let flat = call(&mut s, "bom.get", json!({"flat": true})).unwrap();
        let rows: Vec<(&Value, &Value)> = flat["flat"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| (&r["part"], &r["quantity"]["count"]))
            .collect();
        assert_eq!(rows, [(&json!("p"), &json!(2)), (&json!("kit"), &json!(1))]);
        // CSV, and the working files moved on from HEAD.
        call(
            &mut s,
            "feature.add",
            json!({"part": "asm", "type": "instance", "id": "p3", "of": "p",
                   "rev": "A"}),
        )
        .unwrap();
        let csv = call(&mut s, "bom.get", json!({"flat": true, "format": "csv"})).unwrap();
        assert!(
            csv["csv"]
                .as_str()
                .unwrap()
                .contains("\n,1,100,,p,A,3,each,make,,\n")
        );
        let head = call(&mut s, "bom.get", json!({"rev": "HEAD"})).unwrap();
        assert_eq!(head["tree"]["children"][0]["quantity"]["count"], 2);
        assert_eq!(head["commit"].as_str().unwrap().len(), 40);
        let at = call(&mut s, "bom.get", json!({"part": "p", "rev": "label:A"})).unwrap();
        assert_eq!(at["tree"]["rev"], json!({"label": "A"}));
        // Which root: several assemblies nothing places is a question.
        call(
            &mut s,
            "part.create",
            json!({"part": "other", "kind": "assembly"}),
        )
        .unwrap();
        let e = call(&mut s, "bom.get", json!({})).unwrap_err();
        assert_eq!(e.code, crate::error::Code::InvalidParam);
        assert_eq!(e.details["roots"], json!(["asm", "other"]));
        let e = call(&mut s, "bom.get", json!({"part": "asm", "format": "xml"})).unwrap_err();
        assert_eq!(e.details["param"], "format");
        drop(dir);
    }

    #[test]
    fn a_bom_names_the_csvs_columns_declared_properties_last() {
        let (_dir, mut s) = assembly();
        let v = call(&mut s, "bom.get", json!({"part": "asm"})).unwrap();
        let core = [
            "level",
            "item",
            "number",
            "name",
            "part",
            "revision",
            "quantity",
            "uom",
            "make_buy",
            "material",
            "description",
        ];
        assert_eq!(v["columns"], json!(core));
        call(
            &mut s,
            "pdm.configure",
            json!({"properties": {"finish": {"type": "string"}, "coating": {"type": "string"}}}),
        )
        .unwrap();
        let v = call(
            &mut s,
            "bom.get",
            json!({"part": "asm", "flat": true, "format": "csv"}),
        )
        .unwrap();
        let header = v["csv"]
            .as_str()
            .unwrap()
            .lines()
            .next()
            .unwrap()
            .to_owned();
        let columns: Vec<&str> = v["columns"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c.as_str().unwrap())
            .collect();
        assert_eq!(header, columns.join(","));
        assert_eq!(columns.len(), core.len() + 2);
    }

    #[test]
    fn a_diff_says_what_the_working_files_changed() {
        let (_dir, mut s) = assembly();
        call(
            &mut s,
            "feature.add",
            json!({"part": "asm", "type": "instance", "id": "p3", "of": "p",
                   "rev": "A"}),
        )
        .unwrap();
        let v = call(&mut s, "bom.diff", json!({})).unwrap();
        assert_eq!(
            v["summary"],
            json!({"added": 0, "removed": 0, "changed": 1})
        );
        assert_eq!(v["flat"]["changed"][0]["quantity"]["to"]["count"], 3);
        assert_eq!(v["from"]["rev"], "HEAD");
        assert!(v["from"]["commit"].is_string());
        assert_eq!(v["to"], json!({"rev": "working", "commit": null}));
        let e = call(&mut s, "bom.diff", json!({"part": "nope"})).unwrap_err();
        assert_eq!(e.code, crate::error::Code::NotFound);
    }

    #[test]
    fn where_used_finds_working_and_released_assemblies() {
        let (dir, mut s) = assembly();
        let root = dir.path();
        let v = call(&mut s, "bom.where_used", json!({"part": "p"})).unwrap();
        assert_eq!(v["working"][0]["assembly"], "asm");
        assert_eq!(v["working"][0]["count"], 2);
        assert_eq!(v["released"], json!([]));
        // asm released and official (its kit taken out: a release holds
        // only releases): found there too, at its commit.
        call(&mut s, "feature.delete", json!({"part": "asm", "id": "k"})).unwrap();
        call(
            &mut s,
            "props.set",
            json!({"part": "asm", "properties": {"number": "900"}}),
        )
        .unwrap();
        call(&mut s, "release.propose", json!({"part": "asm"})).unwrap();
        git(root, &["add", "."]);
        git(root, &["commit", "-q", "-m", "release asm A"]);
        let v = call(&mut s, "bom.where_used", json!({"part": "p", "label": "A"})).unwrap();
        assert_eq!(v["released"][0]["label"], "A");
        assert_eq!(v["released"][0]["state"], "released");
        let v = call(
            &mut s,
            "bom.where_used",
            json!({"part": "p", "working": true}),
        )
        .unwrap();
        assert_eq!((&v["working"], &v["released"]), (&json!([]), &json!([])));
        let e = call(
            &mut s,
            "bom.where_used",
            json!({"part": "p", "working": true, "label": "A"}),
        )
        .unwrap_err();
        assert_eq!(e.code, crate::error::Code::InvalidParam);
    }

    #[test]
    fn check_reports_what_makes_a_bom_unsound() {
        let (dir, mut s) = assembly();
        let found = |s: &mut Session, part: Option<&str>| -> Vec<(String, String)> {
            let params = part.map_or(json!({}), |p| json!({"part": p}));
            let v = call(s, "check", params).unwrap();
            v["findings"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|f| f["source"] == "bom")
                .map(|f| {
                    (
                        f["code"].as_str().unwrap().to_owned(),
                        f["part"].as_str().unwrap().to_owned(),
                    )
                })
                .collect()
        };
        assert!(found(&mut s, None).is_empty());
        // The tube loses its amount; the assembly places p at A and as it
        // is.
        call(
            &mut s,
            "props.set",
            json!({"part": "tube", "properties": {"amount": null}}),
        )
        .unwrap();
        call(
            &mut s,
            "feature.add",
            json!({"part": "asm", "type": "instance", "id": "p9", "of": "p"}),
        )
        .unwrap();
        assert_eq!(
            found(&mut s, None),
            [
                ("bom_mixed_revisions".to_owned(), "asm".to_owned()),
                ("bom_amount_missing".to_owned(), "tube".to_owned()),
            ]
        );
        assert_eq!(
            found(&mut s, Some("tube")),
            [("bom_amount_missing".to_owned(), "tube".to_owned())]
        );
        // A hand edit gives the kit p's number: an error, reported once,
        // as the BOM's (doctor's copy is left out).
        let path = dir.path().join("parts/asm/part.json");
        let mut list: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        list["items"]["kit"] = json!(1);
        crate::doc::project::write_json(&path, &list).unwrap();
        let e = call(&mut s, "check", json!({})).unwrap_err();
        assert_eq!(e.code, crate::error::Code::CheckFailed);
        let items: Vec<(&Value, &Value, &Value)> = e.details["findings"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|f| f["code"].as_str().unwrap().starts_with("bom_item"))
            .map(|f| (&f["code"], &f["source"], &f["parts"]))
            .collect();
        assert_eq!(
            items,
            [(
                &json!("bom_item_duplicate"),
                &json!("bom"),
                &json!(["p", "kit"])
            )]
        );
    }

    #[test]
    fn a_bom_follows_another_projects_part_through_the_lock() {
        let (_lib_dir, dir, lib, mut s) = crate::deps::tests::placing();
        let v = call(&mut s, "bom.get", json!({"part": "frame"})).unwrap();
        let b = v["tree"]["children"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["project"] == json!(lib))
            .unwrap();
        assert_eq!(
            (&b["part"], &b["dependency"], &b["rev"]),
            (
                &json!("p"),
                &json!("lib"),
                &json!({"label": "A", "project": lib})
            )
        );
        assert_eq!(b["properties"]["number"], "100");
        assert!(b["commit"].is_string());
        assert_eq!(v["columns"].as_array().unwrap().last().unwrap(), "project");
        let csv = call(
            &mut s,
            "bom.get",
            json!({"part": "frame", "flat": true, "format": "csv"}),
        )
        .unwrap();
        assert!(
            csv["csv"]
                .as_str()
                .unwrap()
                .contains(",p,A,1,each,make,,,lib\n"),
            "{}",
            csv["csv"]
        );
        // Not fetched here: the row says so, and the BOM still answers.
        std::fs::remove_dir_all(dir.path().join("cache")).unwrap();
        s.deps_fetched();
        let v = call(&mut s, "bom.get", json!({"part": "frame"})).unwrap();
        assert_eq!(v["problems"][0]["code"], "dependency_not_fetched");
    }

    /// A release placing another project's part reads it through the lock
    /// committed with it, whatever the working files' lock holds now.
    #[test]
    fn a_release_reads_other_projects_parts_through_its_own_lock() {
        let (_lib_dir, dir, lib, mut s) = crate::deps::tests::placing();
        let root = dir.path();
        git(root, &["init", "-q", "-b", "main"]);
        std::fs::write(root.join(".git/info/exclude"), "cache/\n").unwrap();
        call(
            &mut s,
            "props.set",
            json!({"part": "frame", "properties": {"number": "900"}}),
        )
        .unwrap();
        // Numbered by hand, as the instance was written.
        let list = crate::doc::project::part_path(root, &crate::assembly::tests::id("frame"));
        let mut v: Value = serde_json::from_str(&std::fs::read_to_string(&list).unwrap()).unwrap();
        v["items"] = json!({format!("{lib}:p"): 1});
        v["format"] = json!(4);
        crate::doc::project::write_json(&list, &v).unwrap();
        s.refresh().unwrap();
        call(&mut s, "release.propose", json!({"part": "frame"})).unwrap();
        git(root, &["add", "."]);
        git(root, &["commit", "-q", "-m", "frame A"]);
        // The frame's library part taken out: the lock goes with it.
        call(
            &mut s,
            "feature.delete",
            json!({"part": "frame", "id": "b"}),
        )
        .unwrap();
        assert!(!root.join(crate::doc::lock::LOCK).exists());
        let v = call(
            &mut s,
            "bom.get",
            json!({"part": "frame", "rev": "label:A"}),
        )
        .unwrap();
        assert_eq!(v["problems"], json!([]), "{v}");
        assert_eq!(v["tree"]["children"][0]["project"], json!(lib));
        // As the commit has it too.
        let v = call(&mut s, "bom.get", json!({"part": "frame", "rev": "HEAD"})).unwrap();
        assert_eq!(v["tree"]["children"][0]["project"], json!(lib));
    }

    /// A product depending on [`crate::deps::tests::library`], in a
    /// repository of its own, its assembly `asm` placing the library's `p`
    /// at `A` and released as `A`, committed.
    fn product(lib_dir: &std::path::Path, lib: &crate::doc::id::ProjectId) -> tempfile::TempDir {
        use crate::deps::tests::{depend, place_foreign, write_lock};
        let (dir, mut s) = crate::assembly::tests::project();
        let root = dir.path();
        s.deps_cache = Some(ryupdm::deps::Cache::new(root.join("cache")));
        call(
            &mut s,
            "part.create",
            json!({"part": "asm", "kind": "assembly"}),
        )
        .unwrap();
        depend(&mut s, lib, lib_dir);
        place_foreign(root, "asm", "b", lib, "p", "A");
        s.refresh().unwrap();
        s.deps_cache
            .clone()
            .unwrap()
            .fetch(&crate::deps::tests::url(lib_dir), &[""])
            .unwrap();
        write_lock(&mut s);
        git(root, &["init", "-q", "-b", "main"]);
        std::fs::write(root.join(".git/info/exclude"), "cache/\n").unwrap();
        call(
            &mut s,
            "props.set",
            json!({"part": "asm", "properties": {"number": "900"}}),
        )
        .unwrap();
        let list = crate::doc::project::part_path(root, &crate::assembly::tests::id("asm"));
        let mut v: Value = serde_json::from_str(&std::fs::read_to_string(&list).unwrap()).unwrap();
        v["items"] = json!({format!("{lib}:p"): 1});
        v["format"] = json!(4);
        crate::doc::project::write_json(&list, &v).unwrap();
        s.refresh().unwrap();
        call(&mut s, "release.propose", json!({"part": "asm"})).unwrap();
        git(root, &["add", "."]);
        git(root, &["commit", "-q", "-m", "asm A"]);
        dir
    }

    #[test]
    fn where_used_searches_the_checkouts_that_depend_on_the_project() {
        let (lib_dir, lib) = crate::deps::tests::library();
        let (one, two) = (product(lib_dir.path(), &lib), product(lib_dir.path(), &lib));
        let mut s = Session::open(lib_dir.path()).unwrap();
        // Fetched by the products into their own caches; the library's
        // session reads its own.
        let cache = ryupdm::deps::Cache::new(lib_dir.path().join("cache"));
        cache
            .fetch(&crate::deps::tests::url(lib_dir.path()), &[""])
            .unwrap();
        s.deps_cache = Some(cache);
        let parent = one.path().parent().unwrap().to_path_buf();
        let within = vec![
            one.path().display().to_string(),
            two.path().display().to_string(),
            parent.join("nowhere").display().to_string(),
        ];
        let v = call(&mut s, "bom.where_used", json!({"part": "p", "in": within})).unwrap();
        let working: Vec<(&Value, &Value)> = v["working"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| (&w["assembly"], &w["folder"]))
            .collect();
        let shown = |d: &tempfile::TempDir| json!(d.path().display().to_string());
        assert_eq!(
            working,
            [(&json!("asm"), &shown(&one)), (&json!("asm"), &shown(&two))],
            "{v}"
        );
        let released = &v["released"][0];
        assert_eq!(
            (
                &released["assembly"],
                &released["label"],
                &released["state"]
            ),
            (&json!("asm"), &json!("A"), &json!("released"))
        );
        assert!(released["project"].is_string());
        assert_eq!(v["dependents"].as_array().unwrap().len(), 2);
        // The folder that isn't there is a problem, not a failure.
        assert_eq!(v["problems"].as_array().unwrap().len(), 1, "{v}");
        // At a label nobody places, nothing; with no folders, as before.
        let v = call(
            &mut s,
            "bom.where_used",
            json!({"part": "p", "label": "B", "in": [one.path()]}),
        )
        .unwrap();
        assert_eq!(v["working"], json!([]));
        // The preference's folders, when none are given.
        crate::config::tests::with_config(|_| {
            crate::config::set_user(
                crate::dependents::PREFERENCE,
                json!([one.path().display().to_string()]),
            )
            .unwrap();
            let v = call(&mut s, "bom.where_used", json!({"part": "p"})).unwrap();
            assert_eq!(v["working"].as_array().unwrap().len(), 1, "{v}");
            let v = call(
                &mut s,
                "bom.where_used",
                json!({"part": "p", "no_dependents": true}),
            )
            .unwrap();
            assert!(v.get("dependents").is_none());
        });
    }

    #[test]
    fn where_used_finds_another_projects_part() {
        let (_lib_dir, _dir, lib, mut s) = crate::deps::tests::placing();
        let v = call(&mut s, "bom.where_used", json!({"part": "lib:p"})).unwrap();
        assert_eq!(
            (&v["project"], &v["dependency"]),
            (&json!(lib), &json!("lib"))
        );
        assert_eq!(v["working"][0]["assembly"], "frame");
        assert_eq!(
            v["working"][0]["uses"][0]["rev"],
            json!({"label": "A", "project": lib})
        );
        // By its ID, and at its label.
        let by_id = format!("{lib}:p");
        let v = call(
            &mut s,
            "bom.where_used",
            json!({"part": by_id, "label": "A"}),
        )
        .unwrap();
        assert_eq!(v["working"][0]["count"], 1);
        let v = call(
            &mut s,
            "bom.where_used",
            json!({"part": "lib:p", "label": "B"}),
        )
        .unwrap();
        assert_eq!(v["working"], json!([]));
        // This project's own p is another part.
        let v = call(&mut s, "bom.where_used", json!({"part": "p"})).unwrap();
        assert!(
            v["working"]
                .as_array()
                .unwrap()
                .iter()
                .all(|w| w["uses"][0]["rev"].get("project").is_none())
        );
        let e = call(&mut s, "bom.where_used", json!({"part": "nope:p"})).unwrap_err();
        assert_eq!(e.details["param"], "part");
    }

    #[test]
    fn a_part_with_no_features_is_placed_counted_and_draws_nothing() {
        let (_dir, mut s) = assembly();
        let before = call(&mut s, "mesh.get", json!({"part": "asm"})).unwrap();
        // One box body, placed twice; the kit's tubes have no features.
        assert_eq!(before["bodies"].as_array().unwrap().len(), 1);
        call(&mut s, "part.create", json!({"part": "glue"})).unwrap();
        call(
            &mut s,
            "props.set",
            json!({"part": "glue", "properties":
                   {"make_buy": "buy", "uom": "l", "amount": "5 ml"}}),
        )
        .unwrap();
        call(
            &mut s,
            "feature.add",
            json!({"part": "asm", "type": "instance", "id": "g", "of": "glue"}),
        )
        .unwrap();
        let r = call(&mut s, "rebuild", json!({"part": "asm"})).unwrap();
        let features = r["parts"][0]["features"].as_array().unwrap();
        assert!(features.iter().all(|f| f["ok"] == true), "{r}");
        let after = call(&mut s, "mesh.get", json!({"part": "asm"})).unwrap();
        assert_eq!(after["bodies"], before["bodies"]);
        let glue = &after["occurrences"][4];
        assert_eq!(
            (&glue["part"], &glue["solids"]),
            (&json!("glue"), &json!([]))
        );
        let v = call(&mut s, "bom.get", json!({"flat": true})).unwrap();
        let glue = v["flat"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["part"] == "glue")
            .unwrap();
        assert_eq!(
            glue["quantity"],
            json!({"count": 1, "uom": "l", "value": 0.005, "text": "0.005 l"})
        );
    }
}
