//! A design's BOM (design 629): its flattened netlist's occurrences
//! grouped into purchasable lines, by library component, the label it is
//! placed at and the occurrence's value and footprint; DNP occurrences
//! listed, never counted. ryupdm's `lines` writes their columns, CSV and
//! diff.

use crate::id::Id;
use crate::netlist::Occurrence;
use crate::project::{Project, SELF};
use ryupdm::lines::Line;
use serde_json::Value;
use std::collections::BTreeMap;

/// ryueda's columns after ryupdm's core ones, in order.
pub const COLUMNS: [&str; 8] = [
    "refdes",
    "dnp",
    "value",
    "footprint",
    "manufacturer",
    "mpn",
    "alternates",
    "offers",
];

/// The columns `bom diff` compares besides revision and quantity: what
/// the board is, not where it is bought.
pub const COMPARED: [&str; 7] = [
    "refdes",
    "dnp",
    "value",
    "footprint",
    "manufacturer",
    "mpn",
    "alternates",
];

/// How a designator sorts: its letters, its number, then its text
/// (`R2` before `R10`, `U1` before `U1A`).
pub(crate) fn designator_key(d: &str) -> (String, u64, String) {
    let letters: String = d.chars().take_while(|c| c.is_ascii_alphabetic()).collect();
    let digits: String = d[letters.len()..]
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    (letters, digits.parse().unwrap_or(u64::MAX), d.to_owned())
}

/// What a line's `part` is: `self/<id>` for the design's own library,
/// `<project ID>:<id>` where the alias names its project, else
/// `<alias>/<id>`; and the project and the label it is placed at.
fn part_of(
    p: &Project,
    of: &crate::libcomp::LibRef,
) -> (String, Option<crate::id::ProjectId>, String) {
    match &of.library {
        None => (format!("{SELF}/{}", of.id), None, "working".to_owned()),
        Some(alias) => {
            let a = p.manifest.libraries.get(alias);
            let label = a
                .and_then(|a| a.labels.get(&of.id))
                .cloned()
                .unwrap_or_else(|| "working".to_owned());
            match a.and_then(|a| a.project.clone()) {
                Some(pid) => (format!("{pid}:{}", of.id), Some(pid), label),
                None => (format!("{alias}/{}", of.id), None, label),
            }
        }
    }
}

/// The text after `lib/<alias>/footprint/` of a footprint reference.
fn footprint_id(f: &str) -> &str {
    f.rsplit('/').next().unwrap_or(f)
}

/// The lines of `occs` (a design's occurrences, or one block
/// occurrence's), in part then key order, each counting its populated
/// occurrences times `build`. Properties are the component's defaults
/// (`name` its value, `description`, `make_buy` `buy`): [`with_properties`]
/// lays a part's PDM properties over them.
pub fn lines_of(p: &Project, occs: &[&Occurrence], build: u64) -> Vec<Line> {
    struct Acc {
        line: Line,
        refdes: Vec<String>,
        dnp: Vec<String>,
    }
    let mut by: BTreeMap<String, Acc> = BTreeMap::new();
    for o in occs {
        let Some(of) = crate::erc::of_ref(&o.of) else {
            continue;
        };
        let (part, project, revision) = part_of(p, &of);
        let value = o.value.clone().unwrap_or_default();
        let footprint = o
            .footprint
            .as_deref()
            .map(footprint_id)
            .unwrap_or_default()
            .to_owned();
        let key = format!("{part}\0{value}\0{footprint}");
        let comp = crate::erc::placed_libcomp(p, &o.of).and_then(|l| l.item.as_ref().ok());
        let acc = by.entry(key.clone()).or_insert_with(|| {
            let mut properties = BTreeMap::new();
            let mut defaulted = Vec::new();
            let mut columns = BTreeMap::new();
            if let Some(c) = comp {
                if !c.value.is_empty() {
                    properties.insert("name".to_owned(), Value::from(c.value.clone()));
                    defaulted.push("name".to_owned());
                }
                if !c.description.is_empty() {
                    properties.insert("description".to_owned(), Value::from(c.description.clone()));
                    defaulted.push("description".to_owned());
                }
                columns.insert("manufacturer".to_owned(), c.manufacturer.clone());
                columns.insert("mpn".to_owned(), c.mpn.clone());
                columns.insert(
                    "alternates".to_owned(),
                    c.alternates
                        .iter()
                        .map(|a| format!("{}:{}", a.manufacturer, a.mpn))
                        .collect::<Vec<_>>()
                        .join(";"),
                );
                columns.insert(
                    "offers".to_owned(),
                    c.offers
                        .iter()
                        .map(|f| format!("{}={}", f.supplier, f.sku))
                        .collect::<Vec<_>>()
                        .join(";"),
                );
            }
            properties.insert("make_buy".to_owned(), Value::from("buy"));
            defaulted.push("make_buy".to_owned());
            columns.insert("value".to_owned(), value.clone());
            columns.insert("footprint".to_owned(), footprint.clone());
            Acc {
                line: Line {
                    key,
                    part,
                    project,
                    revision,
                    quantity: 0,
                    properties,
                    defaulted,
                    columns,
                },
                refdes: Vec::new(),
                dnp: Vec::new(),
            }
        });
        let name = o.refdes.clone().unwrap_or_else(|| o.reference.clone());
        if crate::erc::occurrence_dnp(p, o) {
            acc.dnp.push(name);
        } else {
            acc.refdes.push(name);
            acc.line.quantity += build;
        }
    }
    by.into_values()
        .map(|mut a| {
            a.refdes.sort_by_cached_key(|d| designator_key(d));
            a.dnp.sort_by_cached_key(|d| designator_key(d));
            a.line
                .columns
                .insert("refdes".to_owned(), a.refdes.join(","));
            a.line.columns.insert("dnp".to_owned(), a.dnp.join(","));
            a.line
        })
        .collect()
}

/// Lays the PDM properties file `bytes` (a part's `properties/<c>.json`)
/// over `line`'s defaults: what it sets wins and is no longer
/// defaulted. A file that doesn't read changes nothing (its fault is
/// `check`'s in its own project).
pub(crate) fn overlay(line: &mut Line, bytes: Option<&[u8]>) {
    let Some(values) = bytes
        .and_then(|b| std::str::from_utf8(b).ok())
        .and_then(|t| crate::canonical::parse(t).ok())
        .and_then(|v| ryupdm::props::Properties::from_json_in(&v, super::NAME).ok())
        .map(|p| p.values)
    else {
        return;
    };
    for (k, v) in values {
        line.defaulted.retain(|d| *d != k);
        line.properties.insert(k, v);
    }
}

/// Lays each line's part's PDM properties over its defaults: another
/// project's component read where it is pinned (`Resolve::file_at` at
/// the pin's commit), a sibling's at `working` from its folder (the
/// working files) or at `commit` (a revision read); `self` lines have
/// none (question 852).
pub fn with_properties(
    session: &crate::session::Session,
    p: &Project,
    commit: Option<&str>,
    lines: &mut [Line],
) {
    let root = session.root();
    for l in lines.iter_mut() {
        let Some((alias, comp)) = alias_and_component(p, l) else {
            continue;
        };
        let Some(a) = p.manifest.libraries.get(&alias) else {
            continue;
        };
        let rel = format!("{}/{comp}.json", super::PROPERTIES);
        let bytes = match p.pinned.get(&(alias.to_string(), comp.clone())) {
            Some(Ok(pin)) => session
                .pins
                .file_at(root, a, &pin.commit, &rel)
                .ok()
                .flatten(),
            _ if a.url.is_some() => None,
            _ => match commit {
                Some(c) => session.pins.file_at(root, a, c, &rel).ok().flatten(),
                None => std::fs::read(root.join(&a.path).join(&rel)).ok(),
            },
        };
        overlay(l, bytes.as_deref());
    }
}

/// The alias and component a line of another project names, from the
/// first occurrence the line's key came from: its part is
/// `<project>:<id>` or `<alias>/<id>`.
fn alias_and_component(p: &Project, l: &Line) -> Option<(Id, Id)> {
    let (head, id) = l.part.split_once([':', '/'])?;
    if head == SELF {
        return None;
    }
    let id = Id::parse(id, "component").ok()?;
    let alias = p
        .manifest
        .libraries
        .iter()
        .find(|(name, a)| {
            name.as_str() == head || a.project.as_ref().is_some_and(|pid| pid.as_str() == head)
        })
        .map(|(name, _)| name.clone())?;
    Some((alias, id))
}

/// The commit `rev` reads `design` at: `None` for the working files (no
/// `rev`, or `working`); a Git revision as `Session::at` reads it; and
/// `label:L`, the commit where `releases/<design>/<L>.json` became
/// official, `label_not_official` while only the working files hold the
/// record, `label_not_found` otherwise.
pub fn commit_of(
    session: &mut crate::session::Session,
    design: &Id,
    rev: Option<&str>,
) -> crate::error::Result<Option<String>> {
    use crate::error::{Code, Error};
    let Some(rev) = rev.filter(|r| *r != "working") else {
        return Ok(None);
    };
    let Some(label) = rev.strip_prefix("label:") else {
        return Ok(Some(rev.to_owned()));
    };
    let path = ryupdm::record::path(design, label, ryupdm::record::Kind::Release);
    let (_, releases) = super::history(session)?;
    if let Some(o) = releases.official.get(&path) {
        return Ok(Some(o.commit.clone()));
    }
    let code = if session.files().contains_key(&path) {
        Code::LabelNotOfficial
    } else {
        Code::LabelNotFound
    };
    Err(
        Error::new(code, format!("{design} has no official release {label}"))
            .with("part", design)
            .with("label", label),
    )
}

/// The design `asked` names, or the project's only one.
pub fn design_id(p: &Project, asked: Option<&str>) -> crate::error::Result<Id> {
    use crate::error::{Code, Error};
    match asked {
        Some(d) => Id::parse(d, "design"),
        None => {
            let mut ids = p.conn.designs.keys();
            match (ids.next(), ids.next()) {
                (Some(d), None) => Ok(d.clone()),
                _ => Err(Error::new(
                    Code::Usage,
                    "name the design: the project has none or several",
                )
                .with("param", "design")),
            }
        }
    }
}

/// How `bom.get` gives a BOM.
#[derive(Clone, Copy)]
pub struct Ask<'a> {
    pub design: Option<&'a str>,
    /// A Git revision or `label:L`; working by default.
    pub rev: Option<&'a str>,
    /// `json` (the default) or `csv`.
    pub format: Option<&'a str>,
    /// Times `parts.json`'s build quantity.
    pub build: bool,
    /// By block occurrence rather than flat.
    pub by_block: bool,
}

/// `bom_unidentified` for each populated line with no maker part number
/// (design 629), once per line.
pub fn unidentified(design: &Id, lines: &[Line]) -> Vec<Value> {
    lines
        .iter()
        .filter(|l| l.quantity > 0 && l.columns.get("mpn").is_none_or(|m| m.trim().is_empty()))
        .map(|l| {
            serde_json::json!({
                "code": crate::error::Code::BomUnidentified,
                "severity": "warning",
                "source": "bom",
                "design": design,
                "part": l.part,
                "refdes": l.columns.get("refdes"),
                "message": format!(
                    "{} ({}) has no maker part number: a BOM line nobody can buy",
                    l.part,
                    l.columns.get("refdes").map(String::as_str).unwrap_or_default()
                ),
            })
        })
        .collect()
}

/// What `rev` reads `design` at: the commit ([`commit_of`]), the project
/// there (`None` for the working files, which the session holds) and the
/// PDM settings there.
type At = (
    Option<String>,
    Option<std::sync::Arc<Project>>,
    ryupdm::config::Config,
);

fn read_at(
    session: &mut crate::session::Session,
    design: &Id,
    rev: Option<&str>,
) -> crate::error::Result<At> {
    let commit = commit_of(session, design, rev)?;
    match &commit {
        None => {
            let config = ryupdm::config::Config::read_in(&*session.files(), super::NAME)?;
            Ok((None, None, config))
        }
        Some(c) => {
            let (_, p) = session.at(c)?;
            let (_, files) = session.files_at(c)?;
            let config = ryupdm::config::Config::read_in(&files, super::NAME)?;
            Ok((commit, Some(p), config))
        }
    }
}

/// `bom.get`: `{schema, design, rev, commit?, columns, lines | blocks,
/// findings, build_quantity?, csv?}`.
pub fn get(session: &mut crate::session::Session, ask: &Ask) -> crate::error::Result<Value> {
    use crate::error::{Code, Error};
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
    let design = design_id(session.project(), ask.design)?;
    let (commit, then, config) = read_at(session, &design, ask.rev)?;
    let session: &crate::session::Session = session;
    let p: &Project = then.as_deref().unwrap_or(session.project());
    let n = crate::netlist::flatten(p, &design)?;
    let build = if ask.build {
        crate::sourcing::config(p).build_quantity()
    } else {
        1
    };
    let all: Vec<&Occurrence> = n.occurrences.iter().collect();
    let mut lines = lines_of(p, &all, build);
    with_properties(session, p, commit.as_deref(), &mut lines);
    let mut v = serde_json::json!({
        "schema": ryupdm::lines::SCHEMA,
        "design": design,
        "rev": ask.rev.unwrap_or("working"),
        "columns": ryupdm::lines::columns(&config, &COLUMNS),
        "findings": unidentified(&design, &lines),
    });
    if ask.build {
        v["build_quantity"] = serde_json::json!(build);
    }
    if let Some(c) = &commit {
        v["commit"] = serde_json::json!(c);
    }
    if ask.by_block {
        let groups = by_block(session, p, commit.as_deref(), &design, &n, build);
        v["blocks"] = groups
            .iter()
            .map(|(b, ls)| serde_json::json!({"block": b, "lines": ls.iter().map(ryupdm::lines::line_json).collect::<Vec<_>>()}))
            .collect();
        if csv {
            v["csv"] = serde_json::json!(ryupdm::lines::csv_grouped(
                &groups, "block", &config, &COLUMNS
            ));
        }
    } else {
        v["lines"] = lines.iter().map(ryupdm::lines::line_json).collect();
        if csv {
            v["csv"] = serde_json::json!(ryupdm::lines::csv(&lines, &config, &COLUMNS));
        }
    }
    Ok(v)
}

/// The block occurrence an occurrence sits in: its reference up to its
/// last `/` (`<design>:<instance>/…`), or `<design>:` for the root
/// block's own components.
fn block_of(design: &Id, o: &Occurrence) -> String {
    match o.reference.rsplit_once('/') {
        Some((head, _)) => head.to_owned(),
        None => format!("{design}:"),
    }
}

/// `n`'s lines by block occurrence (the indented form), occurrences in
/// reference order, each group's lines as [`lines_of`] gives them.
fn by_block(
    session: &crate::session::Session,
    p: &Project,
    commit: Option<&str>,
    design: &Id,
    n: &crate::netlist::Netlist,
    build: u64,
) -> Vec<(String, Vec<Line>)> {
    let mut groups: BTreeMap<String, Vec<&Occurrence>> = BTreeMap::new();
    for o in &n.occurrences {
        groups.entry(block_of(design, o)).or_default().push(o);
    }
    groups
        .into_iter()
        .map(|(b, occs)| {
            let mut lines = lines_of(p, &occs, build);
            with_properties(session, p, commit, &mut lines);
            (b, lines)
        })
        .collect()
}

/// A design's flat lines at `rev` (working by default), with their
/// properties, the settings there, and the commit read; `None` lines
/// when the design isn't there.
fn side(
    session: &mut crate::session::Session,
    design: &Id,
    rev: Option<&str>,
) -> crate::error::Result<(Option<Vec<Line>>, ryupdm::config::Config, Option<String>)> {
    let (commit, then, config) = read_at(session, design, rev)?;
    let session: &crate::session::Session = session;
    let p: &Project = then.as_deref().unwrap_or(session.project());
    if !p.conn.designs.contains_key(design) {
        return Ok((None, config, commit));
    }
    let n = crate::netlist::flatten(p, design)?;
    let all: Vec<&Occurrence> = n.occurrences.iter().collect();
    let mut lines = lines_of(p, &all, 1);
    with_properties(session, p, commit.as_deref(), &mut lines);
    Ok((Some(lines), config, commit))
}

/// `bom.diff`: what changed in `design`'s BOM from `from` (HEAD by
/// default) to `to` (working by default), properties judged revising by
/// the `to` side's settings: ryupdm's line diff, with `design`, `from`,
/// `to` and the commits read.
pub fn diff(
    session: &mut crate::session::Session,
    design: Option<&str>,
    from: Option<&str>,
    to: Option<&str>,
) -> crate::error::Result<Value> {
    let design = design_id(session.project(), design)?;
    let (from, to) = (from.unwrap_or("HEAD"), to.unwrap_or("working"));
    let (a, _, ca) = side(session, &design, Some(from))?;
    let (b, config, cb) = side(session, &design, Some(to))?;
    let mut v = ryupdm::lines::diff(
        a.as_deref().unwrap_or_default(),
        b.as_deref().unwrap_or_default(),
        &config,
        &COMPARED,
    );
    v["schema"] = serde_json::json!(ryupdm::lines::SCHEMA);
    v["design"] = serde_json::json!(design);
    v["from"] = serde_json::json!(from);
    v["to"] = serde_json::json!(to);
    if let Some(c) = ca {
        v["from_commit"] = serde_json::json!(c);
    }
    if let Some(c) = cb {
        v["to_commit"] = serde_json::json!(c);
    }
    Ok(v)
}

/// `check`'s `bom` findings for the working files: each design's
/// `bom_unidentified`, in design order; none in a project without
/// `ryupdm.json` (a project without PDM checks as it always has).
pub fn findings(session: &crate::session::Session) -> Vec<Value> {
    if !session.files().contains_key(super::PDM_CONFIG) {
        return Vec::new();
    }
    let p = session.project();
    let mut out = Vec::new();
    for d in p.conn.designs.keys() {
        let Ok(n) = crate::netlist::flatten(p, d) else {
            continue;
        };
        let all: Vec<&Occurrence> = n.occurrences.iter().collect();
        out.extend(unidentified(d, &lines_of(p, &all, 1)));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::netlist::tests::project;

    #[test]
    fn a_designs_lines_count_its_populated_occurrences_by_component_value_and_footprint() {
        let (_d, p) = project(&[]);
        let design = p.conn.designs.keys().next().unwrap().clone();
        let n = crate::netlist::flatten(&p, &design).unwrap();
        let occs: Vec<&crate::netlist::Occurrence> = n.occurrences.iter().collect();
        let lines = lines_of(&p, &occs, 1);
        let populated = n
            .occurrences
            .iter()
            .filter(|o| !crate::erc::occurrence_dnp(&p, o))
            .count() as u64;
        assert_eq!(lines.iter().map(|l| l.quantity).sum::<u64>(), populated);
        let mut keys: Vec<&str> = lines.iter().map(|l| l.key.as_str()).collect();
        let before = keys.len();
        keys.dedup();
        assert_eq!(keys.len(), before, "one line per key");
        for l in &lines {
            assert!(l.part.starts_with("self/"), "{}", l.part);
            assert_eq!(l.revision, "working");
            assert_eq!(l.properties["make_buy"], "buy");
            let refdes = &l.columns["refdes"];
            assert!(!refdes.is_empty());
        }
        assert_eq!(lines_of(&p, &occs, 10)[0].quantity, lines[0].quantity * 10);
    }

    #[test]
    fn designators_sort_by_letters_then_number() {
        let mut v = vec!["R10", "C1", "R2", "R1", "U1A", "R", "U1"];
        v.sort_by_cached_key(|a| designator_key(a));
        assert_eq!(v, ["C1", "R1", "R2", "R10", "R", "U1", "U1A"]);
    }

    #[test]
    fn a_parts_properties_lie_over_the_components_defaults() {
        let mut l = ryupdm::lines::Line {
            key: "k".into(),
            part: "p:c".into(),
            project: None,
            revision: "A".into(),
            quantity: 1,
            properties: BTreeMap::from([
                ("name".to_owned(), Value::from("10k")),
                ("make_buy".to_owned(), Value::from("buy")),
            ]),
            defaulted: vec!["name".into(), "make_buy".into()],
            columns: BTreeMap::new(),
        };
        let file =
            br#"{"format":1,"kind":"ryupdm.properties","name":"R 10k 1%","number":"100-0001"}"#;
        overlay(&mut l, Some(file));
        assert_eq!(l.properties["name"], "R 10k 1%");
        assert_eq!(l.properties["number"], "100-0001");
        assert_eq!(l.defaulted, ["make_buy"]);
        overlay(&mut l, Some(b"{"));
        assert_eq!(
            l.properties["name"], "R 10k 1%",
            "a file that doesn't read changes nothing"
        );
    }

    #[test]
    fn a_label_resolves_through_the_release_branch_or_says_why_not() {
        let (_d, mut s) = crate::pdm::cmd::tests::session(crate::project::ProjectKind::Design);
        let design = crate::pdm::tests::id("d");
        let e = commit_of(&mut s, &design, Some("label:A")).unwrap_err();
        assert!(
            matches!(
                e.code,
                crate::error::Code::LabelNotFound | crate::error::Code::NotAGitRepo
            ),
            "{e:?}"
        );
        assert_eq!(commit_of(&mut s, &design, None).unwrap(), None);
        assert_eq!(commit_of(&mut s, &design, Some("working")).unwrap(), None);
    }

    #[test]
    fn the_answer_holds_columns_lines_findings_and_csv_on_request() {
        let (_d, mut s) = crate::pdm::cmd::tests::session(crate::project::ProjectKind::Design);
        let ask = Ask {
            design: None,
            rev: None,
            format: Some("csv"),
            build: false,
            by_block: false,
        };
        let v = get(&mut s, &ask).unwrap();
        assert_eq!(v["schema"], 1);
        assert_eq!(v["rev"], "working");
        assert_eq!(v["columns"][8], "refdes");
        assert!(!v["lines"].as_array().unwrap().is_empty(), "{v}");
        assert!(v["csv"].as_str().unwrap().starts_with("number,name,"));
        for f in v["findings"].as_array().unwrap() {
            assert_eq!(
                (f["code"].as_str(), f["source"].as_str()),
                (Some("bom_unidentified"), Some("bom"))
            );
        }
        let bad = Ask {
            format: Some("xml"),
            ..ask
        };
        assert_eq!(
            get(&mut s, &bad).unwrap_err().code,
            crate::error::Code::InvalidParam
        );
    }

    #[test]
    fn by_block_groups_lines_by_block_occurrence_and_sums_to_the_flat_bom() {
        let (_d, mut s) = crate::pdm::cmd::tests::session(crate::project::ProjectKind::Design);
        let flat = get(
            &mut s,
            &Ask {
                design: None,
                rev: None,
                format: None,
                build: false,
                by_block: false,
            },
        )
        .unwrap();
        let tree = get(
            &mut s,
            &Ask {
                design: None,
                rev: None,
                format: None,
                build: false,
                by_block: true,
            },
        )
        .unwrap();
        let sum = |ls: &Value| {
            ls.as_array()
                .unwrap()
                .iter()
                .map(|l| l["quantity"].as_u64().unwrap())
                .sum::<u64>()
        };
        let blocks = tree["blocks"].as_array().unwrap();
        assert!(!blocks.is_empty());
        assert!(
            blocks
                .iter()
                .all(|b| b["block"].as_str().unwrap().contains(':'))
        );
        assert_eq!(
            blocks.iter().map(|b| sum(&b["lines"])).sum::<u64>(),
            sum(&flat["lines"])
        );
    }

    #[test]
    fn a_diff_of_the_working_files_with_themselves_is_empty() {
        let (_d, mut s) = crate::pdm::cmd::tests::session(crate::project::ProjectKind::Design);
        let v = diff(&mut s, None, Some("working"), Some("working")).unwrap();
        assert_eq!(
            v["summary"],
            serde_json::json!({"added": 0, "removed": 0, "changed": 0})
        );
        assert_eq!(
            (v["from"].as_str(), v["to"].as_str()),
            (Some("working"), Some("working"))
        );
    }

    #[test]
    fn checks_bom_section_appears_only_with_pdm() {
        let (_d, mut s) = crate::pdm::cmd::tests::session(crate::project::ProjectKind::Design);
        assert!(findings(&s).is_empty(), "no ryupdm.json, no bom findings");
        // A component without a maker part number, so there is one.
        let at = s.root().join("library/components/r_100k.json");
        let mut v: Value = serde_json::from_slice(&std::fs::read(&at).unwrap()).unwrap();
        v.as_object_mut().unwrap().remove("mpn");
        crate::source::write_atomic(&at, crate::canonical::to_string(&v)).unwrap();
        s.reload().unwrap();
        assert!(findings(&s).is_empty(), "still no ryupdm.json");
        crate::pdm::cmd::init(&mut s).unwrap();
        let with = findings(&s);
        assert!(!with.is_empty());
        let ask = Ask {
            design: None,
            rev: None,
            format: None,
            build: false,
            by_block: false,
        };
        let lines = get(&mut s, &ask).unwrap();
        assert_eq!(with.len(), lines["findings"].as_array().unwrap().len());
    }
}
