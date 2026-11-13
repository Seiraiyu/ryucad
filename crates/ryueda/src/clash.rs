//! Semantic clashes in a merge (design 518, phase 7): what the merged
//! project holds that neither side did, though every file merged. Two
//! branches that each took the smallest free designator, named a new net
//! alike, or connected to a net the other removed leave a project neither
//! side had; each such clash is a `merge.status` item with its repairs
//! planned (the changes each would write), and `merge.resolve` applies
//! one. What else the merge broke is `merge_faults`, blamed on a side;
//! what a side broke already is `already_failing`, which doesn't block.
//!
//! Nothing here reads a side unless the merged project holds an error or
//! an ERC finding: a clean merge costs one judgement of the merged project.

use crate::conn;
use crate::design::{Part, part_path};
use crate::erc::Finding;
use crate::error::{Code, Error, Result};
use crate::fault::{Fault, Severity};
use crate::id::Id;
use crate::project::Project;
use crate::validate::{Sides, item, json_of};
use ryu_git::MergeState;
use ryu_git::value::Kind;
use ryu_session::fresh::Files;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// A project judged as `check` judges it: its error faults (but its
/// layouts', which `merge status` lists by block), and each design's ERC
/// findings, severities and waivers applied, waived ones left out, with
/// the netlist they were found in.
pub(crate) struct Judged {
    pub faults: Vec<Fault>,
    pub erc: Vec<Finding>,
    pub nets: BTreeMap<Id, crate::netlist::Netlist>,
}

/// Whether `f` is a layout's: those are the layout items' business.
fn layout_fault(f: &Fault) -> bool {
    f.error.code == Code::MergeConflict || detail(f, "path").ends_with("/layout.json")
}

pub(crate) fn judge(p: &Project) -> Judged {
    let faults = p
        .faults()
        .into_iter()
        .filter(|f| f.is_error() && !layout_fault(f))
        .collect();
    let mut erc = Vec::new();
    let mut nets = BTreeMap::new();
    for d in p.conn.designs.keys() {
        let Ok(n) = crate::netlist::flatten(p, d) else {
            continue;
        };
        let found = crate::erc::config(p).apply(crate::erc::check_netlist(p, &n));
        erc.extend(found.into_iter().filter(|f| f.waived.is_none()));
        nets.insert(d.clone(), n);
    }
    Judged { faults, erc, nets }
}

/// A fault's detail as text (`path`, `pointer`), empty when it has none.
pub(crate) fn detail(f: &Fault, k: &str) -> String {
    f.error
        .details
        .get(k)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

/// What makes a fault the same on every side: its code, file and place,
/// as `write::added_errors` compares them.
pub(crate) fn key(f: &Fault) -> String {
    format!(
        "{}\u{0}{}\u{0}{}",
        json!(f.error.code),
        detail(f, "path"),
        detail(f, "pointer")
    )
}

/// Whether `side` holds finding `f`: one of its code in its design that
/// shares a reference with it. A finding's ID hashes its references, so
/// one that grows in a merge (a net's pins) gets a new ID but is still
/// the side's.
pub(crate) fn erc_on(side: &[Finding], f: &Finding) -> bool {
    side.iter().any(|s| {
        s.code == f.code && s.design == f.design && s.refs.iter().any(|r| f.refs.contains(r))
    })
}

/// A side of the merge as a project, read from Git's objects with its
/// aliases as that commit has them.
fn side(root: &Path, commit: &str) -> Result<Project> {
    let r = crate::git::revision(root, commit)?;
    Project::load_from(root, &r.files, &r.aliases)
}

/// The merge's sides, loaded.
pub(crate) struct Three<'a> {
    pub ours: &'a Project,
    pub theirs: &'a Project,
    pub base: Option<&'a Project>,
}

/// What `merge.status` adds for the merged project.
#[derive(Default)]
pub(crate) struct Found {
    /// Clashes: they block the commit and hold the layout merge back.
    pub clashes: Vec<Value>,
    /// `merge_faults`: they block the commit, not the layout merge.
    pub faults: Vec<Value>,
    /// Errors the merged project shares with a side: listed, not blocking.
    pub already: Vec<Value>,
    /// ERC warnings new in the merge: listed, not blocking.
    pub warnings: Vec<Value>,
}

/// A component of `p` by block and ID, if it reads.
pub(crate) fn component<'a>(p: &'a Project, b: &Id, c: &Id) -> Option<&'a conn::Component> {
    p.conn.blocks.get(b)?.components.get(c)?.item.as_ref().ok()
}

/// The sides that brought component `(b, c)`'s designator into the
/// merge: each whose designator for it differs from the base's (or the
/// base has none) and is the merge's.
fn brought(three: &Three, p: &Project, b: &Id, c: &Id) -> Vec<&'static str> {
    let refdes = |q: &Project| component(q, b, c).and_then(|x| x.refdes.clone());
    let now = refdes(p);
    let was = three.base.and_then(refdes);
    let mut out = Vec::new();
    for (name, q) in [("ours", three.ours), ("theirs", three.theirs)] {
        let it = refdes(q);
        if it.is_some() && it != was && it == now {
            out.push(name);
        }
    }
    out
}

/// Which side caused an item, from whether each brought part of it.
fn cause(ours: bool, theirs: bool) -> &'static str {
    match (ours, theirs) {
        (true, false) => "ours",
        (false, true) => "theirs",
        _ => "both",
    }
}

/// What `annotate --reset unannotated` gives a clash's unlocked
/// components once they are placeholders (`<prefix>?`): their new
/// designators; `None` when annotate can't (two components locked alike
/// anywhere in the project, a component with no prefix).
fn annotated(
    root: &Path,
    full: &Files,
    p: &Project,
    design: &Id,
    comps: &BTreeSet<(Id, Id)>,
) -> Result<Option<Vec<Value>>> {
    let mut files = full.clone();
    let mut mine = BTreeSet::new();
    for (b, c) in comps {
        let Some(x) = component(p, b, c).filter(|x| !x.refdes_locked) else {
            continue;
        };
        let Some(prefix) = crate::annotate::prefix_of(p, x) else {
            return Ok(None);
        };
        edit(
            &mut files,
            &part_path(b, Part::Component, c),
            "/refdes",
            Some(json!(format!("{prefix}?"))),
        )?;
        mine.insert(format!("{b}/{c}"));
    }
    let q = crate::validate::project_over(root, &files)?;
    let Ok(plan) = crate::annotate::plan(
        &q,
        Some(design.as_str()),
        crate::annotate::Order::Down,
        crate::annotate::Reset::Unannotated,
    ) else {
        return Ok(None);
    };
    Ok(Some(
        plan.changes
            .iter()
            .filter(|c| mine.contains(&c.component))
            .map(|c| json!({"component": c.component, "refdes": c.to}))
            .collect(),
    ))
}

/// Sets `pointer` of the JSON file `path` in `files` to `x` (`None`
/// removes it), the file canonical at the lowest format that holds it
/// (`merge::settle`'s rule).
pub(crate) fn edit(files: &mut Files, path: &str, pointer: &str, x: Option<Value>) -> Result<()> {
    let v = files.get(path).and_then(|b| json_of(b)).ok_or_else(|| {
        Error::new(Code::ConflictNotFound, format!("{path} isn't readable now")).with("path", path)
    })?;
    let out = crate::merge::settle(&v, pointer, Kind::Value, x);
    files.insert(
        path.to_owned(),
        crate::canonical::to_string(&out).into_bytes().into(),
    );
    Ok(())
}

/// The `refdes_clash` (or `annotate_conflict`) items: each designator
/// `refdes_duplicate` finds in the merge and on neither side.
fn refdes(
    root: &Path,
    full: &Files,
    p: &Project,
    merged: &Judged,
    new_erc: &[&Finding],
    three: &Three,
) -> Result<Vec<Value>> {
    let mut out = Vec::new();
    // Every designator taken: the components' and the occurrences'.
    let mut taken: BTreeSet<String> = p
        .conn
        .blocks
        .values()
        .flat_map(|b| b.components.values())
        .filter_map(|c| c.item.as_ref().ok()?.refdes.clone())
        .collect();
    for n in merged.nets.values() {
        taken.extend(n.occurrences.iter().filter_map(|o| o.refdes.clone()));
    }
    // Each prefix and rest counts from 1, past what is taken, as annotate.
    let mut next: BTreeMap<(String, String), u64> = BTreeMap::new();
    for f in new_erc.iter().filter(|f| f.code == Code::RefdesDuplicate) {
        let r = f.details["refdes"].as_str().unwrap_or_default().to_owned();
        let Some(n) = merged.nets.get(&f.design) else {
            continue;
        };
        // The clashing components, each once (one placed twice is one).
        let comps: BTreeSet<(Id, Id)> = n
            .occurrences
            .iter()
            .filter(|o| f.refs.contains(&o.reference))
            .map(|o| (o.block.clone(), o.component.clone()))
            .collect();
        let refs: Vec<String> = comps.iter().map(|(b, c)| format!("{b}/{c}")).collect();
        let files: Vec<String> = comps
            .iter()
            .map(|(b, c)| part_path(b, Part::Component, c))
            .collect();
        let (mut by_ours, mut by_theirs) = (false, false);
        let (mut locked, mut theirs, mut listed) = (Vec::new(), Vec::new(), Vec::new());
        for (b, c) in &comps {
            let x = component(p, b, c);
            let lock = x.is_some_and(|x| x.refdes_locked);
            let sides = brought(three, p, b, c);
            by_ours |= sides.contains(&"ours");
            by_theirs |= sides.contains(&"theirs");
            if lock {
                locked.push(format!("{b}/{c}"));
            } else if sides.contains(&"theirs") {
                theirs.push((b.clone(), c.clone()));
            }
            listed.push(json!({"component": format!("{b}/{c}"), "locked": lock, "brought_by": sides}));
        }
        let id = format!("refdes:{}/{r}", f.design);
        let what = json!({"design": f.design, "refs": refs});
        if locked.len() > 1 {
            let mut x = item(
                id,
                "annotate_conflict",
                files,
                what,
                cause(by_ours, by_theirs),
                format!(
                    "{} are all locked as {r}: unlock all but one (`ryueda component edit <component> --unlock`), then run merge status again",
                    locked.join(", ")
                ),
            );
            x["refdes"] = json!(r);
            x["components"] = json!(listed);
            out.push(x);
            continue;
        }
        let mut repairs = Vec::new();
        let mut changes = Vec::new();
        for (b, c) in &theirs {
            let Some(cur) = component(p, b, c).and_then(|x| x.refdes.clone()) else {
                continue;
            };
            let Some((prefix, _, rest)) = conn::numbered(&cur) else {
                continue;
            };
            let n = next.entry((prefix.to_owned(), rest.to_owned())).or_insert(1);
            let Some(to) = crate::annotate::next_free(prefix, rest, n, &taken) else {
                continue;
            };
            taken.insert(to.clone());
            changes.push(json!({"component": format!("{b}/{c}"), "refdes": to}));
        }
        if !changes.is_empty() {
            repairs.push(json!({"id": "renumber", "changes": changes, "summary":
                "give the components whose designator came from theirs the next free designators; ours' stand"}));
        }
        if let Some(changes) = annotated(root, full, p, &f.design, &comps)? {
            repairs.push(json!({"id": "annotate", "changes": changes, "summary":
                "make the clashing components placeholders and annotate them in canvas order"}));
        }
        let mut x = item(
            id,
            "refdes_clash",
            files,
            what,
            cause(by_ours, by_theirs),
            format!(
                "{} occurrences share {r} in the merge, and neither side had them share it",
                f.refs.len()
            ),
        );
        x["refdes"] = json!(r);
        x["components"] = json!(listed);
        x["repairs"] = json!(repairs);
        out.push(x);
    }
    Ok(out)
}

/// The semantic items of the merge `state` for the merged project `p`
/// over the session's files `full`.
pub(crate) fn find(root: &Path, state: &MergeState, p: &Project, full: &Files) -> Result<Found> {
    let merged = judge(p);
    if merged.faults.is_empty() && merged.erc.is_empty() {
        return Ok(Found::default());
    }
    let (ours, theirs) = rayon::join(|| side(root, &state.ours), || side(root, &state.theirs));
    let (ours, theirs) = (ours?, theirs?);
    let base = state.base.as_deref().map(|c| side(root, c)).transpose()?;
    let (jo, jt) = rayon::join(|| judge(&ours), || judge(&theirs));
    let files = crate::validate::sides(root, state)?;
    let three = Three {
        ours: &ours,
        theirs: &theirs,
        base: base.as_ref(),
    };
    let on_a_side = |f: &Fault| jo.faults.iter().chain(&jt.faults).any(|g| key(g) == key(f));
    let new: Vec<&Fault> = merged.faults.iter().filter(|f| !on_a_side(f)).collect();
    let new_erc: Vec<&Finding> = merged
        .erc
        .iter()
        .filter(|f| !erc_on(&jo.erc, f) && !erc_on(&jt.erc, f))
        .collect();
    let mut found = Found::default();
    // Faults an item explains, which `merge_faults` doesn't list again.
    let mut covered: BTreeSet<String> = BTreeSet::new();
    found
        .clashes
        .extend(refdes(root, full, p, &merged, &new_erc, &three)?);
    //CLASH-CALLS
    let _ = (&new, &files, &mut covered);
    found.already = merged
        .faults
        .iter()
        .filter(|f| on_a_side(f))
        .map(Fault::to_json)
        .chain(
            merged
                .erc
                .iter()
                .filter(|f| f.severity == Severity::Error)
                .filter(|f| erc_on(&jo.erc, f) || erc_on(&jt.erc, f))
                .map(|f| json!(f)),
        )
        .collect();
    found.warnings = new_erc
        .iter()
        .filter(|f| f.severity == Severity::Warning)
        .map(|f| json!(f))
        .collect();
    Ok(found)
}

/// Applies repair `id` of status item `x` to `files`: the changes the
/// status planned for it. `sides` are the merge's files.
pub(crate) fn repair(
    root: &Path,
    files: &mut Files,
    x: &Value,
    id: &str,
    sides: &Sides,
) -> Result<()> {
    let planned = x["repairs"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|r| r["id"] == id)
        .cloned()
        .ok_or_else(|| {
            Error::new(
                Code::RepairConflict,
                format!(
                    "no repair {id} for {} now: run merge status again",
                    x["id"].as_str().unwrap_or_default()
                ),
            )
            .with("repair", id)
        })?;
    let _ = (root, sides);
    match (x["kind"].as_str().unwrap_or_default(), id) {
        ("refdes_clash", "renumber" | "annotate") => {
            for c in planned["changes"].as_array().into_iter().flatten() {
                let (b, comp) = c["component"]
                    .as_str()
                    .unwrap_or_default()
                    .split_once('/')
                    .unwrap_or_default();
                let path = part_path(
                    &Id::parse(b, "block")?,
                    Part::Component,
                    &Id::parse(comp, "component")?,
                );
                edit(files, &path, "/refdes", Some(c["refdes"].clone()))?;
            }
        }
        //REPAIR-ARMS
        (kind, _) => {
            return Err(
                Error::new(Code::RepairConflict, format!("{kind} has no repair {id}"))
                    .with("repair", id),
            );
        }
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::project::ProjectKind;
    use crate::project::tests::{design_in, library_in, manifest};
    use crate::session::Session;
    use std::collections::BTreeMap;

    /// Writes each `(path, Some(json))` and deletes each `(path, None)`.
    pub(crate) fn put(r: &Path, files: &[(&str, Option<Value>)]) {
        for (path, v) in files {
            let at = r.join(path);
            match v {
                Some(v) => crate::source::write_atomic(&at, crate::canonical::to_string(v)).unwrap(),
                None => std::fs::remove_file(&at).unwrap(),
            }
        }
    }

    /// The test design (block `top`: net `sda`, R1 on it, instances
    /// `ch_a` and `ch_b` of block `s`) committed in a repository, then
    /// `theirs` committed on branch `other`, `ours` on `main`, and the
    /// merge begun with `--no-commit`. With no driver installed Git
    /// merges as lines, so each test changes any file on one side only.
    pub(crate) fn merging(r: &Path, ours: &[(&str, Option<Value>)], theirs: &[(&str, Option<Value>)]) {
        let git = |a: &[&str]| crate::git::tests::git(r, a);
        git(&["init", "-q"]);
        manifest(r, ProjectKind::Design, &[]);
        library_in(r);
        design_in(r);
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "base"]);
        git(&["checkout", "-q", "-b", "other"]);
        put(r, theirs);
        git(&["add", "-A"]);
        git(&["commit", "-q", "-m", "theirs"]);
        git(&["checkout", "-q", "main"]);
        put(r, ours);
        git(&["add", "-A"]);
        git(&["commit", "-q", "-m", "ours"]);
        git(&["merge", "--no-commit", "-q", "other"]);
    }

    /// A component of the test library's `r_100k` with `extra` fields.
    pub(crate) fn part(extra: Value) -> Option<Value> {
        let mut v = json!({"kind": "ryueda.component", "format": 1, "of": {"id": "r_100k"}});
        for (k, x) in extra.as_object().unwrap() {
            v[k] = x.clone();
        }
        let format = crate::conn::format_of(crate::conn::COMPONENT_KIND, &v);
        v["format"] = json!(format);
        Some(v)
    }

    /// The status's items of one kind.
    pub(crate) fn of_kind(v: &Value, kind: &str) -> Vec<Value> {
        v["conflicts"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|c| c["kind"] == kind)
            .cloned()
            .collect()
    }

    #[test]
    fn a_merge_that_breaks_nothing_judges_no_side() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        // Each side adds a part on SDA, with designators of their own.
        merging(
            r,
            &[("blocks/top/components/r2.json", part(json!({"refdes": "R2", "pins": {"1": "sda", "2": {"rail": "gnd"}}})))],
            &[("blocks/top/components/r3.json", part(json!({"refdes": "R3", "pins": {"1": "sda", "2": {"rail": "gnd"}}})))],
        );
        let mut s = Session::open(r).unwrap();
        let v = crate::validate::status(&mut s).unwrap();
        assert_eq!(v["clean"], true, "{v}");
        assert_eq!((v["already_failing"].clone(), v["warnings"].clone()), (json!([]), json!([])));
        // The merged project alone: nothing to compare.
        let p = s.project();
        let j = judge(p);
        assert!(j.faults.is_empty(), "{:?}", j.faults);
    }

    #[test]
    fn findings_match_a_sides_by_a_shared_reference() {
        let f = |id: &str, refs: &[&str]| crate::erc::Finding {
            id: id.to_owned(),
            code: Code::NetSinglePin,
            severity: Severity::Warning,
            design: Id::parse("d", "design").unwrap(),
            refs: refs.iter().map(|s| s.to_string()).collect(),
            message: String::new(),
            details: Value::Null,
            waived: None,
        };
        let side = [f("a", &["d:r1/1", "d:sda"])];
        assert!(erc_on(&side, &f("b", &["d:sda", "d:r2/1"])), "grown, still the side's");
        assert!(!erc_on(&side, &f("c", &["d:r3/1"])));
    }

    #[test]
    fn two_branches_taking_one_designator_clash_and_renumber_theirs() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        let on_sda = |d: &str| part(json!({"refdes": d, "pins": {"1": "sda", "2": {"rail": "gnd"}}}));
        merging(
            r,
            &[("blocks/top/components/r2.json", on_sda("R5"))],
            &[("blocks/top/components/r3.json", on_sda("R5"))],
        );
        let mut s = Session::open(r).unwrap();
        let v = crate::validate::status(&mut s).unwrap();
        let c = of_kind(&v, "refdes_clash");
        assert_eq!(c.len(), 1, "{v}");
        assert_eq!(c[0]["id"], "refdes:d/R5");
        assert_eq!(c[0]["cause"], "both");
        assert_eq!(c[0]["concerns"]["refs"], json!(["top/r2", "top/r3"]));
        // renumber first: theirs' R5 takes the first free R (R1 and R5
        // are taken, the channels' U1_A and U1_B are another prefix).
        assert_eq!(c[0]["repairs"][0]["id"], "renumber");
        assert_eq!(
            c[0]["repairs"][0]["changes"],
            json!([{"component": "top/r3", "refdes": "R2"}])
        );
        assert_eq!(c[0]["repairs"][1]["id"], "annotate");
        let none = BTreeMap::new();
        let mut fix = crate::proto::Resolution {
            conflict: "refdes:d/R5".to_owned(),
            choice: "repair".to_owned(),
            value: None,
            repair: Some("renumber".to_owned()),
        };
        let out = crate::validate::resolve(&mut s, std::slice::from_ref(&fix), &none).unwrap();
        assert_eq!(out["status"]["clean"], true, "{out}");
        let r3: Value = serde_json::from_slice(&std::fs::read(r.join("blocks/top/components/r3.json")).unwrap()).unwrap();
        assert_eq!(r3["refdes"], "R2");
        // Asked again, there is no such clash.
        fix.repair = Some("annotate".to_owned());
        let e = crate::validate::resolve(&mut s, &[fix], &none).unwrap_err();
        assert_eq!(e.code, Code::ConflictNotFound);
    }

    #[test]
    fn two_locked_alike_are_an_annotate_conflict() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        let locked = |d: &str| part(json!({"refdes": d, "refdes_locked": true, "pins": {"1": "sda", "2": {"rail": "gnd"}}}));
        merging(
            r,
            &[("blocks/top/components/r2.json", locked("R5"))],
            &[("blocks/top/components/r3.json", locked("R5"))],
        );
        let mut s = Session::open(r).unwrap();
        let v = crate::validate::status(&mut s).unwrap();
        let c = of_kind(&v, "annotate_conflict");
        assert_eq!(c.len(), 1, "{v}");
        assert_eq!((c[0]["id"].clone(), c[0]["repairs"].clone()), (json!("refdes:d/R5"), json!([])));
    }
}
