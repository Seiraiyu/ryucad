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

use crate::erc::Finding;
use crate::error::{Code, Result};
use crate::fault::{Fault, Severity};
use crate::id::Id;
use crate::project::Project;
use ryu_git::MergeState;
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
    //CLASH-CALLS
    let _ = (&new, &new_erc, &three, &files, &mut covered, full);
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
}
