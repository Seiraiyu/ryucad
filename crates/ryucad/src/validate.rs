//! Merge validation: what a merge in progress leaves to settle, and
//! settling it. `merge.status` lists every conflict: those in the files'
//! conflict records, the paths Git left unmerged (a file deleted on one
//! side and changed on the other, one merged as lines), a feature Git took
//! to be renamed and merged into another, and, from rebuilding both sides
//! and the merge, what the merge broke that built on either side and the
//! sketches it left stale; and the PDM faults the merge has that neither
//! side had (`pdm_broke`: duplicate numbers, values the settings refuse,
//! proposals made stale or out of sequence), and item numbers each side
//! gave another part of one assembly (`item_collision`). Each conflict has a stable
//! ID, the files and the parts of the model it concerns, the side that
//! caused it, its choices, and its planned repairs.
//!
//! ryucad never touches the index: once a file is settled, `git add` and
//! the commit are the agent's or the human's.

use crate::doc::id::Id;
use crate::error::{Code, Error, Result};
use crate::git::{MergeState, Repo, Unmerged};
use crate::merge::{Conflict, Kind};
use crate::rebuild::Output;
use crate::session::Session;
use ryu_session::job::Phase;
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// Every kind of conflict `merge.status` reports. The strings are stable.
pub const KINDS: [&str; 14] = [
    "value",
    "delete_edit",
    "order",
    "id_collision",
    "modify_delete",
    "merge_unparsed",
    "merge_broke",
    "sketch_stale",
    "assembly_stale",
    "import_misplaced",
    "rename_merged",
    "pdm_broke",
    "item_collision",
    "dependency_unlocked",
];

/// The error for a request that needs a merge when there is none.
pub fn not_merging(root: &std::path::Path) -> Error {
    Error::new(
        Code::NotMerging,
        format!(
            "Git is in no merge, cherry-pick, revert or rebase step at {}",
            root.display()
        ),
    )
    .with("path", root)
}

/// The repository and the merge it is in, or why there is none.
fn merging(root: &std::path::Path) -> Result<(Repo, MergeState)> {
    let repo = crate::git::repo(root)?;
    let state = repo.merge_state()?.ok_or_else(|| not_merging(root))?;
    Ok((repo, state))
}

/// What a project file is about: its part, and its feature when it is one.
fn about(path: &str) -> (Option<&str>, Option<&str>) {
    let seg: Vec<&str> = path.split('/').collect();
    match seg.as_slice() {
        ["parts", part, "features", file] => (Some(*part), file.strip_suffix(".json")),
        ["parts", part, ..] => (Some(*part), None),
        _ => (None, None),
    }
}

/// The model a conflict concerns, in the agent's terms: its part, feature,
/// and the sketch entities, parameter or listed feature its pointer names.
fn concerns(path: &str, pointer: &str) -> Value {
    let (part, feature) = about(path);
    let mut v = json!({});
    if let Some(p) = part {
        v["part"] = json!(p);
    }
    if let Some(f) = feature {
        v["feature"] = json!(f);
    }
    let seg: Vec<String> = pointer
        .split('/')
        .skip(1)
        .map(crate::merge::unescape)
        .collect();
    match (path.rsplit('/').next(), seg.as_slice()) {
        (Some("part.json"), [f, id, ..]) if f == "features" => v["feature"] = json!(id),
        (Some("parameters.json"), [p, name, ..]) if p == "parameters" => {
            v["parameter"] = json!(name);
        }
        (_, [g, id, ..]) if feature.is_some() && (g == "entities" || g == "constraints") => {
            v["entities"] = json!([id]);
        }
        _ => {}
    }
    v
}

/// The message for a conflict in a file's record.
fn record_message(path: &str, pointer: &str, c: &Conflict) -> String {
    let at = if pointer.is_empty() {
        path.to_owned()
    } else {
        format!("{path} at {pointer}")
    };
    match c.kind {
        Kind::Value => format!("{at}: changed differently on each side"),
        Kind::DeleteEdit => {
            format!("{at}: removed on one side, and changed or used on the other")
        }
        Kind::Order => format!("{at}: each side puts this feature somewhere else"),
        Kind::IdCollision => format!("{at}: each side added something different under this ID"),
    }
}

/// A conflict as `merge.status` reports it.
fn conflict(
    id: String,
    kind: &str,
    files: Vec<String>,
    concerns: Value,
    cause: &str,
    message: String,
) -> Value {
    json!({
        "id": id, "kind": kind, "files": files, "concerns": concerns, "cause": cause,
        "message": message, "choices": [], "repairs": [],
    })
}

/// A file's JSON, if it is JSON.
fn json_of(bytes: &[u8]) -> Option<Value> {
    serde_json::from_str(std::str::from_utf8(bytes).ok()?).ok()
}

/// A side's version of a file, as a conflict shows it: its JSON, or its
/// text when it isn't JSON; `None` when the side has no file.
fn shown(bytes: Option<&[u8]>) -> Option<Value> {
    let b = bytes?;
    Some(json_of(b).unwrap_or_else(|| Value::String(String::from_utf8_lossy(b).into_owned())))
}

/// The conflicts in the working files' records.
fn records(working: &crate::activity::Files) -> Vec<Value> {
    let mut out = Vec::new();
    for (path, bytes) in working {
        let Some(v) = json_of(bytes) else { continue };
        for (pointer, c) in crate::merge::record(&v) {
            let mut x = conflict(
                format!("{path}#{pointer}"),
                c.kind.as_str(),
                vec![path.clone()],
                concerns(path, &pointer),
                "both",
                record_message(path, &pointer, &c),
            );
            x["pointer"] = json!(pointer);
            for (k, side) in [("base", &c.base), ("ours", &c.ours), ("theirs", &c.theirs)] {
                if let Some(s) = side {
                    x[k] = s.clone();
                }
            }
            x["choices"] = json!(["ours", "theirs", "value"]);
            out.push(x);
        }
    }
    out
}

/// What Git left unmerged that the records don't say: files deleted on one
/// side and changed on the other, and files it merged as lines. Those it
/// left unmerged that now read cleanly are settled, waiting for `git add`.
fn unmerged(
    working: &crate::activity::Files,
    paths: &BTreeMap<String, Unmerged>,
) -> (Vec<Value>, Vec<String>) {
    let (mut conflicts, mut unstaged) = (Vec::new(), Vec::new());
    for (path, u) in paths {
        let (part, feature) = about(path);
        let mut concerns = json!({});
        if let Some(p) = part {
            concerns["part"] = json!(p);
        }
        if let Some(f) = feature {
            concerns["feature"] = json!(f);
        }
        let file = working.get(path).map(|b| b.to_vec());
        let mut x = if u.ours.is_none() || u.theirs.is_none() {
            // Deleted in the working files: settled, waiting for `git rm`.
            // Kept looks the same as Git left it, so it stays a conflict
            // until it is staged.
            if file.is_none() {
                unstaged.push(path.clone());
                continue;
            }
            let by = if u.ours.is_none() { "ours" } else { "theirs" };
            let mut x = conflict(
                format!("{path}#"),
                "modify_delete",
                vec![path.clone()],
                concerns,
                "both",
                format!(
                    "{path} was deleted by {by} and changed by the other side (once settled, `git add` or `git rm` it: kept, it looks as Git left it)"
                ),
            );
            x["deleted_by"] = json!(by);
            x
        } else {
            match file.as_deref().map(json_of) {
                Some(Some(v)) if v.get(crate::doc::project::CONFLICTS).is_some() => continue,
                Some(Some(_)) => {
                    unstaged.push(path.clone());
                    continue;
                }
                // An imported file is binary to Git: it merges neither by
                // value nor as lines. Settled, it is either side's bytes.
                _ if crate::doc::import::is_import_path(path) => {
                    let hash = |b: &Option<Vec<u8>>| {
                        b.as_ref()
                            .map(|b| json!(blake3::hash(b).to_hex().to_string()))
                    };
                    // Theirs taken is settled; ours looks as Git left it,
                    // so it stays a conflict until it is staged.
                    if file.is_some() && file == u.theirs && u.theirs != u.ours {
                        unstaged.push(path.clone());
                        continue;
                    }
                    let mut x = conflict(
                        format!("{path}#"),
                        "merge_unparsed",
                        vec![path.clone()],
                        concerns,
                        "both",
                        format!(
                            "{path} is an imported STEP file, which Git doesn't merge: take one side's bytes, and import the other version again (`import --update`) if it needs both; the import features' hashes must match the bytes taken"
                        ),
                    );
                    for (k, side) in [("base", &u.base), ("ours", &u.ours), ("theirs", &u.theirs)] {
                        if let Some(h) = hash(side) {
                            x[k] = json!({"blake3": h});
                        }
                    }
                    x["choices"] = json!(["ours", "theirs"]);
                    conflicts.push(x);
                    continue;
                }
                _ => conflict(
                    format!("{path}#"),
                    "merge_unparsed",
                    vec![path.clone()],
                    concerns,
                    "both",
                    format!(
                        "{path} couldn't be merged by value (a version isn't a ryucad file this build reads), so Git merged it as lines"
                    ),
                ),
            }
        };
        for (k, side) in [("base", &u.base), ("ours", &u.ours), ("theirs", &u.theirs)] {
            if let Some(s) = shown(side.as_deref()) {
                x[k] = s;
            }
        }
        x["choices"] = json!(["ours", "theirs"]);
        conflicts.push(x);
    }
    (conflicts, unstaged)
}

/// The features a part's `part.json` lists in `files`, if it reads.
fn listed(files: &crate::activity::Files, part: &str) -> Option<Vec<String>> {
    let v = json_of(files.get(&format!("parts/{part}/part.json"))?)?;
    serde_json::from_value(v["features"].clone()).ok()
}

/// Feature files deleted on one side and changed on the other that were
/// kept and staged (`git add`, as Git suggests), so no longer unmerged,
/// while the merged list took the deletion (issue #90): the change the
/// keeping side made would be lost. Each is still a `modify_delete` until
/// its part lists it or the file goes; settling it on the keeping side
/// lists it where that side does.
pub(crate) fn kept_unlisted(
    working: &crate::activity::Files,
    paths: &BTreeMap<String, Unmerged>,
    [ours, theirs, base]: [&crate::activity::Files; 3],
) -> Vec<Value> {
    let mut out = Vec::new();
    for path in working.keys() {
        let (Some(part), Some(feature)) = about(path) else {
            continue;
        };
        if paths.contains_key(path) || !base.contains_key(path) {
            continue;
        }
        let Some(list) = listed(working, part) else {
            continue;
        };
        if list.iter().any(|f| f == feature) {
            continue;
        }
        let (by, kept) = match (ours.contains_key(path), theirs.contains_key(path)) {
            (false, true) => ("ours", theirs),
            (true, false) => ("theirs", ours),
            _ => continue,
        };
        if !listed(kept, part).is_some_and(|l| l.iter().any(|f| f == feature)) {
            continue;
        }
        let mut x = conflict(
            format!("{path}#"),
            "modify_delete",
            vec![path.clone(), format!("parts/{part}/part.json")],
            json!({"part": part, "feature": feature}),
            "both",
            format!(
                "{path} was deleted by {by} and changed by the other side; it was kept, but parts/{part}/part.json doesn't list it, so the model leaves it out (settle it to list it where the side that changed it does, or to delete it)"
            ),
        );
        x["deleted_by"] = json!(by);
        for (k, side) in [("base", base), ("ours", ours), ("theirs", theirs)] {
            if let Some(s) = shown(side.get(path).map(|b| &b[..])) {
                x[k] = s;
            }
        }
        x["choices"] = json!(["ours", "theirs"]);
        out.push(x);
    }
    out
}

/// [`kept_unlisted`] for the merge the session's repository is in (none
/// outside one): for `check`, which rebuilds no sides.
pub fn kept_unlisted_now(session: &Session) -> Result<Vec<Value>> {
    let Some(repo) = Repo::open(session.root())? else {
        return Ok(Vec::new());
    };
    let Some(state) = repo.merge_state()? else {
        return Ok(Vec::new());
    };
    let files = |c: Option<&String>| -> Result<crate::activity::Files> {
        c.map_or(Ok(Default::default()), |c| repo.files(c))
    };
    let (ours, theirs) = (files(Some(&state.ours))?, files(Some(&state.theirs))?);
    let base = files(state.base.as_ref())?;
    Ok(kept_unlisted(
        &session.snapshot(),
        &repo.unmerged()?,
        [&ours, &theirs, &base],
    ))
}

/// One side of a merge, rebuilt: its files, and each part's features by
/// ID with what they built and what they read.
struct Side {
    files: crate::activity::Files,
    parts: BTreeMap<Id, Built>,
    /// The fixed imported instances not where their files place them, by
    /// assembly and instance ([`crate::import::misplaced`]).
    misplaced: std::collections::BTreeSet<(Id, Id)>,
    /// Its lock lacks labels its files reach, or holds some they don't.
    out_of_step: bool,
}

/// Whether `s`'s lock is out of step with what its files reach, judged
/// from the dependency cache ([`crate::deps::judged`]).
fn out_of_step(s: &mut Session) -> bool {
    crate::deps::judged(s).is_ok_and(|j| !j.unlocked.is_empty() || !j.unused.is_empty())
}

/// A merged lock out of step with what the merged files reach (a label one
/// side placed that the other side's lock won, or labels nothing places
/// any more), where at least one side's lock was in step with its own
/// files: `dependency_unlocked`, blamed on both, with the repair
/// `deps_lock` that writes the lock again from what was fetched.
fn unlocked(
    merged: &crate::activity::Files,
    at: &mut Session,
    ours: &Side,
    theirs: &Side,
) -> Vec<Value> {
    use crate::doc::lock::LOCK;
    let Ok(j) = crate::deps::judged(at) else {
        return Vec::new();
    };
    if (j.unlocked.is_empty() && j.unused.is_empty()) || (ours.out_of_step && theirs.out_of_step) {
        return Vec::new();
    }
    let text = |l: &[ryupdm::deps::lock::Wanted]| -> Vec<String> {
        l.iter()
            .map(|(p, part, label)| format!("{p}:{part}@{label}"))
            .collect()
    };
    let files = vec![LOCK.to_owned()];
    let cid = format!("{LOCK}#dependency_unlocked");
    let mut c = conflict(
        cid.clone(),
        "dependency_unlocked",
        files.clone(),
        json!({"unlocked": text(&j.unlocked), "unused": text(&j.unused)}),
        "both",
        format!(
            "the merged {LOCK} doesn't hold what the merged assemblies place ({} missing, {} placed by nothing)",
            j.unlocked.len(),
            j.unused.len()
        ),
    );
    let p = at.project().clone();
    if let Ok(lock) = crate::deps::relock(at, &p) {
        let content = (!lock.is_empty()).then(|| crate::doctor::canonical_bytes(&lock.to_json()));
        let repair = crate::doctor::Repair {
            id: format!(
                "{cid}:deps_lock@{}",
                crate::doctor::basis_of(merged, &files)
            ),
            summary: format!("write {LOCK} again from what was fetched"),
            changes: vec![crate::doctor::Change {
                path: LOCK.to_owned(),
                content,
            }],
            params: Vec::new(),
        };
        c["repairs"] = json!([repair.to_json()]);
    }
    vec![c]
}

/// [`crate::import::misplaced`] in `s`'s project, by assembly and instance.
fn misplaced_in(s: &Session) -> std::collections::BTreeSet<(Id, Id)> {
    crate::import::misplaced(s.project())
        .into_iter()
        .map(|m| (m.assembly, m.instance))
        .collect()
}

/// A part on one side: each listed feature's result, and the features
/// (a solid as the feature's view of it) and parameters each one reads.
#[derive(Default)]
struct Built {
    results: BTreeMap<Id, Result<Output>>,
    inputs: BTreeMap<Id, Vec<Id>>,
    uses: BTreeMap<Id, std::collections::BTreeSet<String>>,
    /// What each instance places: a part, at a revision.
    places: BTreeMap<Id, crate::assembly::Node>,
    /// An assembly's solve: whether it was stale, and if so (and its mates
    /// hold) the instances with their solved poses.
    stale: Option<(bool, Vec<(Id, crate::doc::files::Feature)>)>,
}

/// A part as a session has it, rebuilt.
fn built(s: &mut Session) -> Result<BTreeMap<Id, Built>> {
    let ids: Vec<Id> = s.project().parts.keys().cloned().collect();
    let mut out = BTreeMap::new();
    for id in ids {
        let p = s.project().part(&id)?.clone();
        let mut b = Built {
            uses: p.uses.clone(),
            ..Default::default()
        };
        let solids = crate::solid::Solids::of(&p);
        for (fid, f) in &p.features {
            if let Ok(crate::doc::files::Feature::Instance(i)) = f {
                b.places
                    .insert(fid.clone(), (i.part.clone(), i.rev.clone()));
            }
            if let Ok(f) = f {
                b.inputs.insert(
                    fid.clone(),
                    crate::graph::reads(&solids, fid, f)
                        .into_iter()
                        .cloned()
                        .collect(),
                );
            }
        }
        // A part whose list can't be read builds nothing.
        if p.file.is_ok() {
            let r = s.rebuilt(&id)?.clone();
            b.results = r.features.iter().cloned().collect();
            if let Some(solve) = r.solve.clone() {
                let writes = if solve.stale() && solve.report.converged {
                    crate::mate::Moved {
                        result: r,
                        held: false,
                        moves: Vec::new(),
                    }
                    .writes(&p)
                } else {
                    Vec::new()
                };
                b.stale = Some((solve.stale(), writes));
            }
        }
        out.insert(id, b);
    }
    Ok(out)
}

/// One side of the merge, from its commit (nothing, for a missing base).
fn side(session: &mut Session, commit: Option<&str>) -> Result<Side> {
    let Some(c) = commit else {
        return Ok(Side {
            files: Default::default(),
            parts: BTreeMap::new(),
            misplaced: Default::default(),
            out_of_step: false,
        });
    };
    match session.at(Some(c)) {
        Ok((_, s)) => Ok(Side {
            files: s.snapshot(),
            parts: built(s)?,
            misplaced: misplaced_in(s),
            out_of_step: out_of_step(s),
        }),
        // A commit with no project here has nothing of it.
        Err(e) if e.code == Code::NotFound => Ok(Side {
            files: Default::default(),
            parts: BTreeMap::new(),
            misplaced: Default::default(),
            out_of_step: false,
        }),
        Err(e) => Err(e),
    }
}

/// The features a failure passes through: `id`, then each input whose
/// failure made it fail (`dependency_failed`), to the one that failed for
/// its own reasons. `None` when that one is a file the merge left
/// conflicted, which the structural conflicts report.
fn chain(results: &BTreeMap<Id, Result<Output>>, id: &Id) -> Option<Vec<Id>> {
    let mut out = vec![id.clone()];
    loop {
        let at = out.last().expect("not empty");
        let Some(Err(e)) = results.get(at) else {
            return Some(out);
        };
        match e.code {
            Code::MergeConflict => return None,
            Code::DependencyFailed => {
                let next = e.details["dependency"]
                    .as_str()
                    .and_then(|d| Id::parse(d, "feature").ok());
                match next {
                    Some(d) if !out.contains(&d) => out.push(d),
                    _ => return Some(out),
                }
            }
            _ => return Some(out),
        }
    }
}

/// The features `id` reads on any side, directly or not, and itself.
fn closure(sides: &[&Built], id: &Id) -> Vec<Id> {
    let mut seen = std::collections::BTreeSet::new();
    let mut todo = vec![id.clone()];
    while let Some(f) = todo.pop() {
        if !seen.insert(f.clone()) {
            continue;
        }
        for s in sides {
            todo.extend(s.inputs.get(&f).into_iter().flatten().cloned());
        }
    }
    seen.into_iter().collect()
}

/// What a feature's result is on a side that has it.
fn result_on(side: &Side, part: &Id, id: &Id) -> Option<std::result::Result<(), Code>> {
    let r = side.parts.get(part)?.results.get(id)?;
    Some(r.as_ref().map(|_| ()).map_err(|e| e.code))
}

/// The files and parameters behind feature `id` of `part` that `side`
/// changed from `base`: those of every feature it reads, its own, the
/// parameters they read, and the manifest.
fn touched(
    part: &Id,
    reads: &[Id],
    uses: &std::collections::BTreeSet<String>,
    placed: &std::collections::BTreeSet<crate::assembly::Node>,
    side: &Side,
    base: &Side,
) -> Vec<String> {
    let mut out = Vec::new();
    let differs = |path: &str| side.files.get(path) != base.files.get(path);
    if differs(crate::doc::project::MANIFEST) {
        out.push(crate::doc::project::MANIFEST.to_owned());
    }
    // What its instances place: a part's files as they are, or a
    // release's record.
    for (child, rev) in placed {
        match rev {
            crate::doc::assembly::Rev::Working => {
                let dir = format!("parts/{child}/");
                let paths: std::collections::BTreeSet<&String> = side
                    .files
                    .keys()
                    .chain(base.files.keys())
                    .filter(|p| p.starts_with(&dir))
                    .collect();
                out.extend(paths.into_iter().filter(|p| differs(p)).cloned());
            }
            crate::doc::assembly::Rev::Label(l) => {
                let path = format!("releases/{child}/{l}.json");
                if differs(&path) {
                    out.push(path);
                }
            }
            // Another project's release is what the lock says it is.
            crate::doc::assembly::Rev::Foreign { .. } => {
                let path = crate::doc::lock::LOCK.to_owned();
                if differs(&path) && !out.contains(&path) {
                    out.push(path);
                }
            }
        }
    }
    for f in reads {
        let path = format!("parts/{part}/features/{f}.json");
        if differs(&path) {
            out.push(path.clone());
        }
        // The file an import feature reads, as either names it.
        for files in [&side.files, &base.files] {
            let Some(v) = files.get(&path).and_then(|b| json_of(b)) else {
                continue;
            };
            if let Some(file) = v["file"]["path"].as_str().filter(|_| v["type"] == "import")
                && differs(file)
                && !out.iter().any(|o| o == file)
            {
                out.push(file.to_owned());
            }
        }
    }
    let params = format!("parts/{part}/parameters.json");
    let entries = |s: &Side| -> BTreeMap<String, Value> {
        s.files
            .get(&params)
            .and_then(|b| json_of(b))
            .and_then(|v| v["parameters"].as_object().cloned())
            .map(|m| m.into_iter().filter(|(k, _)| uses.contains(k)).collect())
            .unwrap_or_default()
    };
    if entries(side) != entries(base) {
        out.push(params);
    }
    out
}

/// Who broke feature `id` of `part`, which built on the sides `built_on`:
/// each other side whose changes reached what it reads (`both` when that
/// is both, or neither), and the files where they did.
fn blame(
    part: &Id,
    id: &Id,
    built_on: &[&str],
    graphs: [&Built; 3],
    ours: &Side,
    theirs: &Side,
    base: &Side,
) -> (&'static str, Vec<String>) {
    let reads = closure(&graphs, id);
    let uses: std::collections::BTreeSet<String> = reads
        .iter()
        .flat_map(|f| graphs.into_iter().flat_map(move |s| s.uses.get(f)))
        .flatten()
        .cloned()
        .collect();
    // The parts its instances place, and theirs, at any depth, on any
    // side.
    let mut placed = std::collections::BTreeSet::new();
    let mut todo: Vec<crate::assembly::Node> = reads
        .iter()
        .flat_map(|f| graphs.into_iter().filter_map(move |s| s.places.get(f)))
        .cloned()
        .collect();
    while let Some(n) = todo.pop() {
        if !placed.insert(n.clone()) {
            continue;
        }
        if n.1 == crate::doc::assembly::Rev::Working {
            for side in [ours, theirs, base] {
                if let Some(b) = side.parts.get(&n.0) {
                    todo.extend(b.places.values().cloned());
                }
            }
        }
    }
    let mut causes = Vec::new();
    let mut files = Vec::new();
    for (on, other, other_side) in [("ours", "theirs", theirs), ("theirs", "ours", ours)] {
        if built_on.contains(&on) {
            let t = touched(part, &reads, &uses, &placed, other_side, base);
            if !t.is_empty() {
                causes.push(other);
                files.extend(t);
            }
        }
    }
    files.sort();
    files.dedup();
    let cause = match causes.as_slice() {
        [one] => *one,
        _ => "both",
    };
    (cause, files)
}

/// What the merge broke: every feature that built on ours or theirs and
/// fails in the merge, caused by the side whose changes reached what it
/// reads; one that fails only because another the merge broke does is
/// listed under that one. And the features that failed on a side already,
/// or that no side has (work the merge didn't bring), which don't block.
fn broken(
    working: &BTreeMap<Id, Built>,
    ours: &Side,
    theirs: &Side,
    base: &Side,
) -> (Vec<Value>, Vec<Value>) {
    let (mut broke, mut already) = (Vec::new(), Vec::new());
    for (part, w) in working {
        let sides = |id: &Id| {
            let on = [
                ("ours", result_on(ours, part, id)),
                ("theirs", result_on(theirs, part, id)),
            ];
            let pick = |ok: bool| -> Vec<&'static str> {
                on.iter()
                    .filter(|(_, r)| r.as_ref().is_some_and(|r| r.is_ok() == ok))
                    .map(|(s, _)| *s)
                    .collect()
            };
            (pick(true), pick(false))
        };
        // Each failure: already failing on a side, the merge's doing (with
        // the chain it fails through), or a conflicted file's.
        let mut broke_here: BTreeMap<Id, Vec<Id>> = BTreeMap::new();
        for (id, r) in &w.results {
            let Err(e) = r else { continue };
            let (built_on, failed_on) = sides(id);
            // Failing on a side already, or on no side because neither has
            // it (work in the working files the merge didn't bring): not
            // the merge's doing.
            if !failed_on.is_empty() || built_on.is_empty() {
                already.push(json!({"part": part, "feature": id, "error": e.code,
                                    "failed_on": failed_on}));
                continue;
            }
            if let Some(c) = chain(&w.results, id) {
                broke_here.insert(id.clone(), c);
            }
        }
        // Folded under the last feature of its chain that the merge broke.
        let mut also: BTreeMap<Id, Vec<Id>> = BTreeMap::new();
        let mut own = Vec::new();
        for (id, c) in &broke_here {
            let anchor = c
                .iter()
                .rev()
                .find(|f| broke_here.contains_key(*f))
                .expect("it is in its own chain");
            if anchor == id {
                own.push(id.clone());
            } else {
                also.entry(anchor.clone()).or_default().push(id.clone());
            }
        }
        let empty = Built::default();
        let (bo, bt) = (
            ours.parts.get(part).unwrap_or(&empty),
            theirs.parts.get(part).unwrap_or(&empty),
        );
        for id in own {
            let Some(Err(e)) = w.results.get(&id) else {
                continue;
            };
            let (built_on, _) = sides(&id);
            let (cause, files) = blame(part, &id, &built_on, [w, bo, bt], ours, theirs, base);
            let path = format!("parts/{part}/features/{id}.json");
            let mut concerns = json!({"part": part, "feature": id});
            if let Some(r) = e.details.get("reference") {
                concerns["references"] = json!([r]);
            }
            if let Some(a) = also.get(&id) {
                concerns["also"] = json!(a);
            }
            let built = format!("it built on {}", built_on.join(" and "));
            let mut c = conflict(
                format!("{path}#merge_broke"),
                "merge_broke",
                if files.is_empty() {
                    vec![path.clone()]
                } else {
                    files
                },
                concerns,
                cause,
                format!(
                    "{part}/{id} fails in the merge ({}) but {built}: {}",
                    json!(e.code).as_str().unwrap_or_default(),
                    e.message
                ),
            );
            c["error"] = json!(e);
            c["built_on"] = json!(built_on);
            broke.push(c);
        }
    }
    (broke, already)
}

/// Whether a feature is a sketch whose stored positions don't hold, on a
/// side that has it and built it.
fn stale_on(side: &Side, part: &Id, id: &Id) -> Option<bool> {
    let r = side.parts.get(part)?.results.get(id)?;
    Some(r.as_ref().ok()?.sketch()?.stale)
}

/// The sketches the merge left stale that were current on the sides they
/// came from, each with the repair that writes its solved positions.
fn stale(
    merged: &crate::activity::Files,
    working: &BTreeMap<Id, Built>,
    ours: &Side,
    theirs: &Side,
    base: &Side,
) -> Vec<Value> {
    let mut out = Vec::new();
    let empty = Built::default();
    for (part, w) in working {
        for (id, r) in &w.results {
            let Some(solved) = r.as_ref().ok().and_then(|o| o.sketch()) else {
                continue;
            };
            let on = [
                ("ours", stale_on(ours, part, id)),
                ("theirs", stale_on(theirs, part, id)),
            ];
            if !solved.stale || on.iter().any(|(_, s)| *s == Some(true)) {
                continue;
            }
            let built_on: Vec<&str> = on
                .iter()
                .filter(|(_, s)| *s == Some(false))
                .map(|(n, _)| *n)
                .collect();
            let graphs = [
                w,
                ours.parts.get(part).unwrap_or(&empty),
                theirs.parts.get(part).unwrap_or(&empty),
            ];
            let (cause, mut files) = blame(part, id, &built_on, graphs, ours, theirs, base);
            let path = format!("parts/{part}/features/{id}.json");
            files.retain(|f| *f != path);
            files.insert(0, path.clone());
            let cid = format!("{path}#sketch_stale");
            let content = crate::doctor::canonical_bytes(
                &crate::doc::files::Feature::Sketch(solved.sketch.clone()).to_json(),
            );
            let repair = crate::doctor::Repair {
                id: format!("{cid}:solve@{}", crate::doctor::basis_of(merged, &files)),
                summary: format!("write {path} with its positions solved"),
                changes: vec![crate::doctor::Change {
                    path: path.clone(),
                    content: Some(content),
                }],
                params: Vec::new(),
            };
            let mut c = conflict(
                cid,
                "sketch_stale",
                files,
                json!({"part": part, "feature": id}),
                cause,
                format!(
                    "{part}/{id}: the merged positions don't satisfy its constraints, though they did on {}",
                    if built_on.is_empty() {
                        "no side".to_owned()
                    } else {
                        built_on.join(" and ")
                    }
                ),
            );
            c["repairs"] = json!([repair.to_json()]);
            out.push(c);
        }
    }
    out
}

/// The assemblies the merge left stale that were current on the sides
/// that have them, each with the repair that writes its solved poses.
fn stale_assemblies(
    merged: &crate::activity::Files,
    working: &BTreeMap<Id, Built>,
    ours: &Side,
    theirs: &Side,
) -> Vec<Value> {
    let mut out = Vec::new();
    for (part, w) in working {
        let Some((true, writes)) = &w.stale else {
            continue;
        };
        if writes.is_empty() {
            continue;
        }
        let on = |side: &Side| {
            side.parts
                .get(part)
                .and_then(|b| b.stale.as_ref())
                .map(|s| s.0)
        };
        let sides = [("ours", on(ours)), ("theirs", on(theirs))];
        if sides.iter().any(|(_, s)| *s == Some(true)) {
            continue;
        }
        let current: Vec<&str> = sides
            .iter()
            .filter(|(_, s)| *s == Some(false))
            .map(|(n, _)| *n)
            .collect();
        let files: Vec<String> = writes
            .iter()
            .map(|(id, _)| format!("parts/{part}/features/{id}.json"))
            .collect();
        let cid = format!("parts/{part}/part.json#assembly_stale");
        let repair = crate::doctor::Repair {
            id: format!("{cid}:solve@{}", crate::doctor::basis_of(merged, &files)),
            summary: format!("write the poses {part}'s mates solve to"),
            changes: writes
                .iter()
                .zip(&files)
                .map(|((_, f), path)| crate::doctor::Change {
                    path: path.clone(),
                    content: Some(crate::doctor::canonical_bytes(&f.to_json())),
                })
                .collect(),
            params: Vec::new(),
        };
        let mut c = conflict(
            cid,
            "assembly_stale",
            files,
            json!({"part": part}),
            "both",
            format!(
                "{part}: the merged poses don't satisfy its mates, though they did on {}",
                if current.is_empty() {
                    "no side".to_owned()
                } else {
                    current.join(" and ")
                }
            ),
        );
        c["repairs"] = json!([repair.to_json()]);
        out.push(c);
    }
    out
}

/// The assemblies whose fixed imported instances the merge left where
/// their files don't place them, though they were on the sides that have
/// them (the file's bytes from one side, the pose, solver state, from the
/// other), each with the repair that writes the poses the files give.
fn misplaced(
    merged: &crate::activity::Files,
    at: &Session,
    ours: &Side,
    theirs: &Side,
) -> Vec<Value> {
    let found = crate::import::misplaced(at.project());
    let mut out = Vec::new();
    for (assembly, files) in crate::doctor::misplaced_by_assembly(&found) {
        let mine: Vec<&crate::import::Misplaced> =
            found.iter().filter(|m| m.assembly == assembly).collect();
        // Already so on a side: not the merge's doing (doctor says so).
        let on = |side: &Side| {
            mine.iter().any(|m| {
                side.misplaced
                    .contains(&(m.assembly.clone(), m.instance.clone()))
            })
        };
        if on(ours) || on(theirs) {
            continue;
        }
        let cid = format!("parts/{assembly}/part.json#import_misplaced");
        let repair = crate::doctor::Repair {
            id: format!("{cid}:place@{}", crate::doctor::basis_of(merged, &files)),
            summary: format!(
                "write {assembly}'s fixed imported instances at the poses their files place them"
            ),
            changes: crate::doctor::placed_writes(&found, assembly.as_str())
                .into_iter()
                .map(|(path, content)| crate::doctor::Change {
                    path,
                    content: Some(content),
                })
                .collect(),
            params: Vec::new(),
        };
        let instances: Vec<&str> = mine.iter().map(|m| m.instance.as_str()).collect();
        let imports: Vec<&str> = files
            .iter()
            .filter(|f| f.starts_with("imports/"))
            .map(String::as_str)
            .collect();
        let mut c = conflict(
            cid,
            "import_misplaced",
            files.clone(),
            json!({"part": assembly, "instances": instances}),
            "both",
            format!(
                "{assembly}: the merge took {} from one side and the pose of {} from the other: {} not where the file places {}",
                imports.join(" and "),
                instances.join(", "),
                if instances.len() == 1 {
                    "it is"
                } else {
                    "they are"
                },
                if instances.len() == 1 { "it" } else { "them" },
            ),
        );
        c["repairs"] = json!([repair.to_json()]);
        out.push(c);
    }
    out
}

/// What the merge broke in PDM: each fault the merged files have that
/// neither side had ([`ryupdm::merge::validate`]), blamed on the side
/// whose change completes it. A property fault takes a value for the
/// property; a stale or out-of-sequence proposal has the repair
/// `withdraw`, which deletes its record.
fn pdm(
    working: &crate::activity::Files,
    ours: &Side,
    theirs: &Side,
    base: &Side,
    releases: &ryupdm::history::Releases,
) -> Vec<Value> {
    ryupdm::merge::validate(&base.files, &ours.files, &theirs.files, working, releases)
        .into_iter()
        .map(|b| {
            let mut id = format!("{}#pdm_broke/{}", b.path, b.code);
            let mut concerns = json!({"part": b.part});
            if let Some(p) = &b.property {
                id = format!("{id}/{p}");
                concerns["property"] = json!(p);
            }
            if let Some(l) = &b.label {
                concerns["label"] = json!(l);
            }
            if let Some(others) = b.details.get("parts") {
                concerns["parts"] = others.clone();
            }
            let mut c = conflict(
                id.clone(),
                "pdm_broke",
                b.files.clone(),
                concerns,
                b.cause.as_str(),
                format!("{} (neither side had this)", b.message),
            );
            c["error"] = json!({"code": b.code, "message": b.message, "details": b.details});
            if b.property.is_some() {
                c["choices"] = json!(["value"]);
            }
            if b.withdraw {
                let repair = crate::doctor::Repair {
                    id: format!("{id}:withdraw@{}", crate::doctor::basis_of(working, &b.files)),
                    summary: format!(
                        "withdraw the proposal {}: delete it, so the merged part can be proposed again",
                        b.path
                    ),
                    changes: vec![crate::doctor::Change {
                        path: b.path.clone(),
                        content: None,
                    }],
                    params: Vec::new(),
                };
                c["repairs"] = json!([repair.to_json()]);
            }
            c
        })
        .collect()
}

/// Item numbers the merge made shared: each side gave one number to a
/// different part of an assembly, and the merge kept both
/// ([`ryupdm::items::collisions`]). Blamed on both, with the repair
/// `renumber`: theirs' part takes the next free number.
fn item_collisions(working: &crate::activity::Files, ours: &Side, theirs: &Side) -> Vec<Value> {
    ryupdm::items::collisions(&ours.files, &theirs.files, working)
        .into_iter()
        .map(|x| {
            let path = format!("parts/{}/part.json", x.assembly);
            let id = format!("{path}#item_collision/{}", x.item);
            let files = vec![path.clone()];
            let names: Vec<&str> = x.parts.iter().map(Id::as_str).collect();
            let mut c = conflict(
                id.clone(),
                "item_collision",
                files.clone(),
                json!({"part": x.assembly, "parts": x.parts, "item": x.item}),
                "both",
                format!(
                    "{}: item {} names {} after the merge: each side gave it to another part",
                    x.assembly,
                    x.item,
                    names.join(" and ")
                ),
            );
            let repair = crate::doctor::Repair {
                id: format!("{id}:renumber@{}", crate::doctor::basis_of(working, &files)),
                summary: format!(
                    "give {} (theirs) item {}, the next free number",
                    x.theirs, x.to
                ),
                changes: vec![crate::doctor::Change {
                    path,
                    content: Some(crate::doctor::canonical_bytes(&x.repaired)),
                }],
                params: Vec::new(),
            };
            c["repairs"] = json!([repair.to_json()]);
            c
        })
        .collect()
}

/// Whether `path` is a feature file.
fn is_feature(path: &str) -> bool {
    matches!(about(path), (Some(_), Some(_)))
}

/// Features Git took to be renamed: a feature file one side added whose
/// merged content isn't that side's but what merging it with a feature
/// that side deleted and the other changed gives. Git's merge follows
/// renames whatever its config says, so the other side's edits land on the
/// added feature. Each comes with two repairs: `restore` puts the added
/// file back as its side has it and brings the deleted feature back with
/// the other side's edits, listed where that side lists it; `drop` only
/// puts the added file back.
fn renames(
    working: &crate::activity::Files,
    ours: &Side,
    theirs: &Side,
    base: &Side,
) -> Vec<Value> {
    let mut out = Vec::new();
    for (p, merged) in working.iter().filter(|(p, _)| is_feature(p)) {
        for (name, s, other_name, other) in [
            ("ours", ours, "theirs", theirs),
            ("theirs", theirs, "ours", ours),
        ] {
            let Some(added) = s.files.get(p) else {
                continue;
            };
            if base.files.contains_key(p) || other.files.contains_key(p) || merged == added {
                continue;
            }
            let from = base.files.iter().find(|(q, qb)| {
                is_feature(q)
                    && !s.files.contains_key(*q)
                    && !working.contains_key(*q)
                    && other.files.get(*q).is_some_and(|x| x != *qb)
                    && {
                        let changed = &other.files[*q];
                        let (o, t) = if name == "ours" {
                            (added, changed)
                        } else {
                            (changed, added)
                        };
                        crate::merge::bytes(qb, o, t).0[..] == merged[..]
                    }
            });
            let Some((q, _)) = from else { continue };
            let (part, feature) = about(p);
            let (_, deleted) = about(q);
            let index = format!("parts/{}/part.json", part.unwrap_or_default());
            let files = vec![p.clone(), q.clone(), index.clone()];
            let cid = format!("{p}#rename_merged");
            let b = crate::doctor::basis_of(working, &files);
            let back = |path: &str, bytes: &[u8]| crate::doctor::Change {
                path: path.to_owned(),
                content: Some(bytes.to_vec()),
            };
            // The deleted feature listed again, where the other side lists it.
            let mut restored = working.clone();
            restored.insert(q.clone(), other.files[q].clone());
            settle_unmerged(&mut restored, q, other.files.get(q), &other.files);
            let mut changes = vec![back(p, added), back(q, &other.files[q])];
            if let Some(i) = restored
                .get(&index)
                .filter(|i| working.get(&index) != Some(*i))
            {
                changes.push(back(&index, i));
            }
            let repairs = [
                crate::doctor::Repair {
                    id: format!("{cid}:restore@{b}"),
                    summary: format!(
                        "put {p} back as {name} has it, and bring {q} back with {other_name}'s changes"
                    ),
                    changes,
                    params: Vec::new(),
                },
                crate::doctor::Repair {
                    id: format!("{cid}:drop@{b}"),
                    summary: format!(
                        "put {p} back as {name} has it; {q} stays deleted, without {other_name}'s changes"
                    ),
                    changes: vec![back(p, added)],
                    params: Vec::new(),
                },
            ];
            let mut c = conflict(
                cid,
                "rename_merged",
                files,
                json!({"part": part, "feature": feature, "renamed_from": deleted}),
                "both",
                format!(
                    "Git took {q}, which {name} deleted and {other_name} changed, to be renamed to {p}, which {name} added, and merged {other_name}'s changes into it"
                ),
            );
            c["repairs"] = json!(repairs.iter().map(|r| r.to_json()).collect::<Vec<_>>());
            out.push(c);
        }
    }
    out
}

/// The renumbering repair for an ID collision `c`, if one can be planned.
fn renumbering(
    root: &std::path::Path,
    working: &crate::activity::Files,
    ours: &Side,
    theirs: &Side,
    c: &Value,
) -> Vec<Value> {
    let path = c["files"][0].as_str().unwrap_or_default();
    let pointer = c["pointer"].as_str().unwrap_or_default();
    let record = Conflict {
        kind: Kind::IdCollision,
        base: c.get("base").cloned(),
        ours: c.get("ours").cloned(),
        theirs: c.get("theirs").cloned(),
    };
    let Some((r, changes)) = crate::renumber::plan(
        root,
        working,
        &ours.files,
        &theirs.files,
        path,
        pointer,
        &record,
    ) else {
        return Vec::new();
    };
    let mut files: Vec<String> = changes.iter().map(|c| c.path.clone()).collect();
    files.push(path.to_owned());
    files.sort();
    files.dedup();
    let from = match &r {
        crate::renumber::Rename::Feature { from, .. }
        | crate::renumber::Rename::Entity { from, .. } => from.clone(),
    };
    let repair = crate::doctor::Repair {
        id: format!(
            "{}:renumber@{}",
            c["id"].as_str().unwrap_or_default(),
            crate::doctor::basis_of(working, &files)
        ),
        summary: format!(
            "give theirs' {from} the ID {}; theirs' uses of it, and of what it built, follow",
            r.to()
        ),
        changes,
        params: Vec::new(),
    };
    vec![repair.to_json()]
}

/// What a merge status is computed from: the merge (its step and
/// commits), the index (what is staged) and the working files. `None` when
/// Git is in no merge, or the project in no repository.
pub fn key(session: &Session) -> Option<String> {
    let repo = Repo::open(session.root()).ok()??;
    let state = match repo.merge_state() {
        Ok(Some(s)) => format!("{}:{}:{}:{:?}", s.step.as_str(), s.ours, s.theirs, s.base),
        Ok(None) => return None,
        Err(e) => format!("error:{}", e.message),
    };
    Some(format!(
        "{state}:{}:{}",
        repo.index_stamp(),
        session.digest()
    ))
}

/// `merge.status`: what the merge in progress leaves to settle. Kept by
/// [`key`], so asking again about the same state costs nothing.
pub fn status(session: &mut Session) -> Result<Value> {
    let key = key(session);
    if let (Some(k), Some((was, v))) = (&key, &session.merge_status)
        && k == was
    {
        return Ok(v.clone());
    }
    let v = compute(session)?;
    if let Some(k) = key {
        session.merge_status = Some((k, v.clone()));
    }
    Ok(v)
}

/// [`status`], from nothing.
fn compute(session: &mut Session) -> Result<Value> {
    let root = session.root().to_path_buf();
    let (repo, state) = merging(&root)?;
    let working = session.snapshot();
    let paths = repo.unmerged()?;
    // A file staged and then settled again (a conflict record `git add`ed,
    // then resolved) is no longer unmerged, but a commit would still take
    // what the index holds: everything the hook would refuse is listed.
    let staged = repo.unstaged(&working)?;
    let untracked = repo.untracked(&working)?;
    // Four rebuilds: each side, then the merge.
    let job = session.job.clone();
    let stage = job.begin(Phase::Rebuild, "revisions", 4);
    stage.step(|| "ours".into())?;
    let ours = side(session, Some(&state.ours))?;
    stage.step(|| "theirs".into())?;
    let theirs = side(session, Some(&state.theirs))?;
    stage.step(|| "base".into())?;
    let base = side(session, state.base.as_deref())?;
    let sides = Sides { ours, theirs, base };
    session.learn_from_revisions();
    stage.step(|| "merged".into())?;
    let found = findings(session, None, &working, &paths, staged, &sides, true)?;
    drop(stage);
    let commit = |c: Option<&String>| json!({ "commit": c });
    Ok(json!({
        "step": state.step.as_str(),
        "ours": commit(Some(&state.ours)),
        "theirs": commit(Some(&state.theirs)),
        "base": commit(state.base.as_ref()),
        "clean": found.conflicts.is_empty(),
        "conflicts": found.conflicts,
        "already_failing": found.already,
        "unstaged": found.unstaged,
        "untracked": untracked,
    }))
}

/// A merge's three sides, rebuilt.
struct Sides {
    ours: Side,
    theirs: Side,
    base: Side,
}

/// What a merge leaves to settle: its conflicts, sorted by ID, the
/// features already failing on a side, and the files settled but not
/// staged.
struct Findings {
    conflicts: Vec<Value>,
    already: Vec<Value>,
    unstaged: Vec<String>,
}

/// Everything a merge leaves to settle, from its working files and the
/// paths Git left unmerged in them, the files whose working copy the index
/// doesn't hold (`staged`), and its sides. The merge rebuilds in
/// `merged`, a session over its files, or (for a merge in progress) in
/// `session` itself, which explains what failed. Renames Git followed are
/// looked for only where Git ran (`renamed`).
fn findings(
    session: &mut Session,
    mut merged: Option<Session>,
    working: &crate::activity::Files,
    paths: &BTreeMap<String, Unmerged>,
    staged: Vec<String>,
    sides: &Sides,
    renamed: bool,
) -> Result<Findings> {
    let root = session.root().to_path_buf();
    let Sides { ours, theirs, base } = sides;
    let releases = crate::release::history(session)
        .map(|(_, r)| r)
        .unwrap_or_default();
    let mut conflicts = records(working);
    let (more, mut unstaged) = unmerged(working, paths);
    conflicts.extend(more);
    unstaged.extend(staged);
    unstaged.sort();
    unstaged.dedup();
    conflicts.extend(kept_unlisted(
        working,
        paths,
        [&ours.files, &theirs.files, &base.files],
    ));
    if renamed {
        let renamed = renames(working, ours, theirs, base);
        // A renamed file's own conflicts go with it: its repairs rewrite it.
        let gone: Vec<String> = renamed
            .iter()
            .map(|c| format!("{}#", c["files"][0].as_str().unwrap_or_default()))
            .collect();
        conflicts.retain(|c| {
            !gone.iter().any(|g| {
                c["id"]
                    .as_str()
                    .is_some_and(|id| id.starts_with(g.as_str()))
            })
        });
        conflicts.extend(renamed);
    }
    for c in conflicts.iter_mut().filter(|c| c["kind"] == "id_collision") {
        c["repairs"] = json!(renumbering(&root, working, ours, theirs, c));
    }
    let at = match &mut merged {
        Some(m) => m,
        None => session,
    };
    let now = built(at)?;
    let (mut broke, already) = broken(&now, ours, theirs, base);
    // What a name that is gone was, and which feature took it.
    for c in &mut broke {
        let part = c["concerns"]["part"]
            .as_str()
            .and_then(|p| Id::parse(p, "part").ok());
        if let (Some(part), Ok(e)) = (part, serde_json::from_value::<Error>(c["error"].clone())) {
            c["error"] = json!(at.explain(&part, e));
        }
    }
    conflicts.extend(broke);
    conflicts.extend(stale(working, &now, ours, theirs, base));
    conflicts.extend(stale_assemblies(working, &now, ours, theirs));
    conflicts.extend(misplaced(working, at, ours, theirs));
    conflicts.extend(unlocked(working, at, ours, theirs));
    conflicts.extend(pdm(working, ours, theirs, base, &releases));
    conflicts.extend(item_collisions(working, ours, theirs));
    conflicts.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
    Ok(Findings {
        conflicts,
        already,
        unstaged,
    })
}

/// How many merge previews a session keeps, most recently made first.
const PREVIEWS: usize = 4;

/// `merge.preview`: what merging `rev` (a revision, or another worktree's
/// working files as `worktree:<path>`) into this worktree's working files
/// would leave to settle, as `merge.status` would report it after the
/// merge, without merging anything. The base is the merge base of this
/// worktree's head and theirs (the other worktree's head, or the
/// revision's commit); each file merges as Git with ryucad's driver would
/// ([`crate::merge::files_merged`]), and the merge rebuilds in a session over
/// the merged files. Git's rename following isn't previewed. Kept by what
/// it was made from, so asking again about the same files costs nothing.
pub fn preview(session: &mut Session, rev: &str) -> Result<Value> {
    let root = session.root().to_path_buf();
    let repo = crate::git::repo(&root)?;
    let ours_head = repo
        .head_commit()
        .ok_or_else(|| Error::new(Code::NotMerging, "the repository has no commits"))?;
    let (theirs_named, theirs_head, theirs_key) = match session.worktree_files(rev)? {
        Some((w, _, digest)) => {
            if w.current {
                return Err(Error::new(
                    Code::InvalidParam,
                    "that is this worktree: preview a merge with another",
                )
                .with("rev", rev));
            }
            let head = w.head.clone().ok_or_else(|| {
                Error::new(Code::InvalidParam, "that worktree has no commits yet").with("rev", rev)
            })?;
            let key = format!("{head}:{digest}");
            let named = json!({"worktree": w.root, "branch": w.branch, "commit": head});
            (named, head, key)
        }
        None => {
            let commit = repo.commit(rev)?.id.to_string();
            (
                json!({"rev": rev, "commit": commit}),
                commit.clone(),
                commit,
            )
        }
    };
    let key = format!("{ours_head}:{}:{theirs_key}", session.digest());
    if let Some((_, v)) = session.previews.iter().find(|(k, _)| *k == key) {
        return Ok(v.clone());
    }
    let base_commit = repo.merge_base(&ours_head, &theirs_head)?;
    let theirs_rev = match crate::session::worktree_path(rev) {
        Some(_) => rev.to_owned(),
        None => theirs_head.clone(),
    };
    let sides = Sides {
        ours: Side {
            files: session.snapshot(),
            parts: built(session)?,
            misplaced: misplaced_in(session),
            out_of_step: out_of_step(session),
        },
        theirs: side(session, Some(&theirs_rev))?,
        base: side(session, base_commit.as_deref())?,
    };
    let merged =
        crate::merge::files_merged(&sides.base.files, &sides.ours.files, &sides.theirs.files);
    session.learn_from_revisions();
    let at = session.over(merged.files.clone())?;
    let found = findings(
        session,
        Some(at),
        &merged.files,
        &merged.unmerged,
        Vec::new(),
        &sides,
        false,
    )?;
    let mut warnings = Vec::new();
    if let Some(setup) = crate::drivers::setup(&root) {
        if setup.missing.contains(&crate::drivers::RENAMES) {
            warnings.push(json!({
                "code": "git_drivers_missing",
                "message": "this clone's Git follows renames (merge.renames isn't false), which the preview doesn't: run `ryucad git install`",
            }));
        }
        if setup.missing.contains(&crate::drivers::MERGE_DRIVER) {
            warnings.push(json!({
                "code": "git_drivers_missing",
                "message": "this clone's Git has no ryucad merge driver, so a real merge would merge ryucad's files as lines; the preview merges them by value: run `ryucad git install`",
            }));
        }
    }
    let ours_named = json!({"worktree": root, "branch": repo.branch()?, "commit": ours_head});
    let v = json!({
        "schema": 1,
        "ours": ours_named,
        "theirs": theirs_named,
        "base": {"commit": base_commit},
        "clean": found.conflicts.is_empty(),
        "conflicts": found.conflicts,
        "already_failing": found.already,
        "unmerged": merged.unmerged.keys().collect::<Vec<_>>(),
        "warnings": warnings,
    });
    session.previews.insert(0, (key, v.clone()));
    session.previews.truncate(PREVIEWS);
    Ok(v)
}

/// Puts `x` at `pointer` in `v`, making the objects on the way; `None`
/// removes what is there.
fn set_at(v: &mut Value, pointer: &str, x: Option<Value>) {
    if pointer.is_empty() {
        if let Some(x) = x {
            *v = x;
        }
        return;
    }
    let mut seg: Vec<String> = pointer
        .split('/')
        .skip(1)
        .map(crate::merge::unescape)
        .collect();
    let last = seg.pop().expect("a pointer below the root has a segment");
    let mut at = v;
    for k in seg {
        if !at.get(&k).is_some_and(Value::is_object) {
            at[&k] = json!({});
        }
        at = at.get_mut(&k).expect("just made");
    }
    let Some(o) = at.as_object_mut() else { return };
    match x {
        Some(x) => {
            o.insert(last, x);
        }
        None => {
            o.remove(&last);
        }
    }
}

/// Puts feature `id` in `list` where `place` says ([`crate::merge::place`]'s
/// form: first, or after a feature), or takes it out for `None`. After a
/// feature the list doesn't have, or anywhere else, it goes last.
pub(crate) fn put(list: &mut Vec<String>, id: &str, place: Option<&Value>) {
    list.retain(|f| f != id);
    let Some(p) = place else { return };
    let at = match (p.get("first"), p.get("after").and_then(Value::as_str)) {
        (Some(Value::Bool(true)), _) => 0,
        (_, Some(after)) => list
            .iter()
            .position(|f| f == after)
            .map_or(list.len(), |i| i + 1),
        _ => list.len(),
    };
    list.insert(at, id.to_owned());
}

/// A file's JSON from the files being written.
fn file_json(files: &crate::activity::Files, path: &str) -> Result<Value> {
    files
        .get(path)
        .and_then(|b| json_of(b))
        .ok_or_else(|| Error::new(Code::ConflictNotFound, format!("{path} isn't readable now")))
}

/// Settles the conflict `c` of a file's record at `pointer` with `x`, in
/// `files`: the value put in place, the entry gone from the record (and
/// the record when it was the last), the file at the lowest format.
fn settle_record(
    files: &mut crate::activity::Files,
    path: &str,
    pointer: &str,
    kind: Kind,
    x: Option<Value>,
) -> Result<()> {
    let mut v = file_json(files, path)?;
    let mut record = crate::merge::record(&v);
    if record.remove(pointer).is_none() {
        return Err(Error::new(
            Code::ConflictNotFound,
            format!("{path} has no conflict at {pointer} now"),
        )
        .with("conflict", format!("{path}#{pointer}")));
    }
    let mut plain = crate::doc::project::without_conflicts(&v);
    let order = kind == Kind::Order && pointer.starts_with("/features/");
    if order {
        let id = crate::merge::unescape(&pointer["/features/".len()..]);
        let mut list: Vec<String> =
            serde_json::from_value(plain["features"].clone()).unwrap_or_default();
        put(&mut list, &id, x.as_ref().filter(|p| !p.is_null()));
        plain["features"] = json!(list);
    } else {
        set_at(&mut plain, pointer, x);
    }
    if plain["kind"] == crate::doc::files::FEATURE_KIND {
        plain["format"] = json!(crate::doc::files::feature_format(&plain));
    }
    v = crate::merge::Merged {
        value: plain,
        conflicts: record,
    }
    .to_json();
    files.insert(path.to_owned(), crate::doctor::canonical_bytes(&v).into());
    Ok(())
}

/// Sets property `name` in the properties file at `path` to `x` in
/// `files`: `null` clears it, and the file goes with its last property.
fn set_property(
    files: &mut crate::activity::Files,
    path: &str,
    name: &str,
    x: Value,
) -> Result<()> {
    let mut props = match files.get(path) {
        None => ryupdm::props::Properties::default(),
        Some(b) => json_of(b)
            .and_then(|v| ryupdm::props::Properties::from_json(&v).ok())
            .ok_or_else(|| {
                Error::new(Code::ConflictNotFound, format!("{path} isn't readable now"))
            })?,
    };
    if x.is_null() {
        props.values.remove(name);
    } else {
        props.values.insert(name.to_owned(), x);
    }
    if props.values.is_empty() {
        files.remove(path);
    } else {
        files.insert(
            path.to_owned(),
            crate::doctor::canonical_bytes(&props.to_json()).into(),
        );
    }
    Ok(())
}

/// Settles a file Git left unmerged with one side's version: written as
/// that side has it, or deleted where that side deleted it. A feature file
/// kept is listed where that side lists it; one deleted, unlisted.
fn settle_unmerged(
    files: &mut crate::activity::Files,
    path: &str,
    side: Option<&crate::activity::Bytes>,
    side_files: &crate::activity::Files,
) {
    match side {
        Some(b) => {
            files.insert(path.to_owned(), b.clone());
        }
        None => {
            files.remove(path);
        }
    }
    let (Some(part), Some(feature)) = about(path) else {
        return;
    };
    let index = format!("parts/{part}/part.json");
    let Some(mut v) = files.get(&index).and_then(|b| json_of(b)) else {
        return;
    };
    let Ok(mut list) = serde_json::from_value::<Vec<String>>(v["features"].clone()) else {
        return;
    };
    let listed = list.iter().any(|f| f == feature);
    let theirs_list: Vec<String> = side_files
        .get(&index)
        .and_then(|b| json_of(b))
        .and_then(|v| serde_json::from_value(v["features"].clone()).ok())
        .unwrap_or_default();
    match (side.is_some(), listed) {
        (true, false) => {
            let place = crate::merge::place(&theirs_list, feature).unwrap_or(json!({}));
            put(&mut list, feature, Some(&place));
        }
        (false, true) => list.retain(|f| f != feature),
        _ => return,
    }
    v["features"] = json!(list);
    files.insert(index, crate::doctor::canonical_bytes(&v).into());
}

/// `merge.resolve`: settles each conflict as asked, in order, and writes
/// the files as one change, guarded by the part revisions given. Every
/// conflict must be in the merge's status now, and every repair as it was
/// planned there, or nothing is written. Answers what changed and the new
/// status.
pub fn resolve(
    session: &mut Session,
    resolutions: &[crate::proto::Resolution],
    expected: &BTreeMap<String, String>,
) -> Result<Value> {
    for (part, rev) in expected {
        let id = crate::server::part_id(part)?;
        crate::server::check_revision(session.project().part(&id)?, Some(rev))?;
    }
    let root = session.root().to_path_buf();
    let now = status(session)?;
    let (repo, state) = merging(&root)?;
    let unmerged = repo.unmerged()?;
    let before = session.snapshot();
    let mut files = before.clone();
    let mut resolved = Vec::new();
    for r in resolutions {
        let found = now["conflicts"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|c| c["id"] == r.conflict.as_str())
            .ok_or_else(|| {
                Error::new(
                    Code::ConflictNotFound,
                    format!(
                        "the merge has no conflict {} now; run merge status",
                        r.conflict
                    ),
                )
                .with("conflict", &r.conflict)
            })?;
        let kind = found["kind"].as_str().unwrap_or_default();
        let path = found["files"][0].as_str().unwrap_or_default().to_owned();
        let usage = |why: String| Error::new(Code::Usage, why).with("conflict", &r.conflict);
        match (kind, r.choice.as_str()) {
            ("value" | "delete_edit" | "order" | "id_collision", "ours" | "theirs" | "value") => {
                let x = match r.choice.as_str() {
                    "value" => Some(
                        r.value
                            .clone()
                            .ok_or_else(|| usage("choice value needs a value".to_owned()))?,
                    ),
                    side => found.get(side).cloned(),
                };
                let pointer = found["pointer"].as_str().unwrap_or_default();
                let kind = crate::merge::Kind::parse(kind).expect("a record's kind");
                settle_record(&mut files, &path, pointer, kind, x)?;
            }
            ("pdm_broke", "value") if found["concerns"]["property"].is_string() => {
                let x = r
                    .value
                    .clone()
                    .ok_or_else(|| usage("choice value needs a value".to_owned()))?;
                let name = found["concerns"]["property"].as_str().unwrap_or_default();
                set_property(&mut files, &path, name, x)?;
            }
            ("modify_delete" | "merge_unparsed", side @ ("ours" | "theirs")) => {
                let commit = if side == "ours" {
                    &state.ours
                } else {
                    &state.theirs
                };
                let side_files = repo.files(commit)?;
                let bytes = match unmerged.get(&path) {
                    Some(u) if side == "ours" => u.ours.clone().map(Into::into),
                    Some(u) => u.theirs.clone().map(Into::into),
                    // Kept and staged: the file as it is now, unless the
                    // side chosen deleted it.
                    None if found["deleted_by"] == side => None,
                    None => files.get(&path).cloned(),
                };
                settle_unmerged(&mut files, &path, bytes.as_ref(), &side_files);
            }
            (_, "repair") => {
                let id = r
                    .repair
                    .as_deref()
                    .ok_or_else(|| usage("choice repair needs a repair".to_owned()))?;
                let repair = found["repairs"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find(|x| x["id"] == id)
                    .ok_or_else(|| {
                        Error::new(
                            Code::RepairConflict,
                            format!("no repair {id} for {} now: the files changed, or it was never offered; run merge status again", r.conflict),
                        )
                        .with("repair", id)
                    })?;
                for c in repair["changes"].as_array().into_iter().flatten() {
                    let p = c["path"].as_str().unwrap_or_default().to_owned();
                    match c["content"].as_str() {
                        Some(text) => {
                            files.insert(p, text.as_bytes().into());
                        }
                        None => {
                            files.remove(&p);
                        }
                    }
                }
            }
            (_, choice) => {
                let offered = found["choices"]
                    .as_array()
                    .map(|c| {
                        c.iter()
                            .filter_map(Value::as_str)
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .unwrap_or_default();
                return Err(Error::new(
                    Code::InvalidParam,
                    format!(
                        "{} ({kind}) can't be settled by {choice:?}; it takes {}",
                        r.conflict,
                        if offered.is_empty() {
                            "edits to the model, or its repairs".to_owned()
                        } else {
                            format!("{offered}, or its repairs")
                        }
                    ),
                )
                .with("conflict", &r.conflict)
                .with("choice", choice));
            }
        }
        resolved.push(r.conflict.clone());
    }
    let mut changes = Vec::new();
    for (path, bytes) in &files {
        if before.get(path) != Some(bytes) {
            crate::doc::project::write_atomic(&root.join(path), bytes)?;
            changes.push(json!({"path": path, "change": "written"}));
        }
    }
    for path in before.keys().filter(|p| !files.contains_key(*p)) {
        crate::doc::project::remove_file(&root, &root.join(path))?;
        changes.push(json!({"path": path, "change": "deleted"}));
    }
    session.reload()?;
    Ok(json!({"resolved": resolved, "changes": changes, "status": status(session)?}))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::git::tests::git;
    use crate::server::tests::call;
    use std::path::Path;

    /// Runs git, which may fail (a merge that stops for conflicts).
    pub(crate) fn try_git(dir: &Path, args: &[&str]) -> bool {
        crate::git::tests::git_command()
            .args(["-c", "user.name=Test", "-c", "user.email=t@example.com"])
            .args(["-c", "commit.gpgsign=false"])
            .args(args)
            .current_dir(dir)
            .env("GIT_EDITOR", "true")
            .output()
            .unwrap()
            .status
            .success()
    }

    /// A project in a repository whose Git merges ryucad's files with the
    /// merge engine: a driver script that runs `ryucad::merge::bytes`
    /// can't be the test binary, so Git's merge is replaced by writing what
    /// the driver would.
    pub(crate) struct Merging {
        pub dir: tempfile::TempDir,
        pub session: Session,
    }

    impl Merging {
        pub(crate) fn root(&self) -> &Path {
            self.dir.path()
        }

        pub(crate) fn call(&mut self, method: &str, params: Value) -> Result<Value> {
            call(&mut self.session, method, params)
        }

        pub(crate) fn git(&self, args: &[&str]) {
            git(self.root(), args);
        }

        pub(crate) fn commit(&self, message: &str) {
            self.git(&["add", "-A"]);
            self.git(&["commit", "-q", "-m", message]);
        }

        /// Merges `branch` into `main` as Git with ryucad's driver would,
        /// stopping before the commit: Git merges, then every file it left
        /// unmerged is written as the driver writes it.
        pub(crate) fn merge(&mut self, branch: &str) {
            try_git(self.root(), &["merge", "--no-commit", "--no-ff", branch]);
            let repo = Repo::open(self.root()).unwrap().unwrap();
            for (path, u) in repo.unmerged().unwrap() {
                if let (Some(o), Some(t)) = (&u.ours, &u.theirs) {
                    let b = u.base.clone().unwrap_or_default();
                    let (out, outcome) = crate::merge::bytes(&b, o, t);
                    std::fs::write(self.root().join(&path), out).unwrap();
                    // A clean merge is staged, as Git stages a driver's.
                    if outcome == crate::merge::Outcome::Clean {
                        self.git(&["add", &path]);
                    }
                }
            }
            self.session.reload().unwrap();
        }
    }

    /// A project with part `p` holding a box `b`, committed on `main`.
    pub(crate) fn boxed() -> Merging {
        let dir = tempfile::tempdir().unwrap();
        crate::server::init(dir.path(), None).unwrap();
        git(dir.path(), &["init", "-q", "-b", "main"]);
        let session = Session::open(dir.path()).unwrap();
        let mut m = Merging { dir, session };
        m.call("part.create", json!({"part": "p"})).unwrap();
        m.call(
            "feature.add",
            json!({"part": "p", "type": "box", "id": "b",
                   "params": {"x": 10, "y": 20, "z": 5}}),
        )
        .unwrap();
        m.commit("a box");
        m
    }

    /// Edits box `b` on branch `theirs` and on `main`.
    pub(crate) fn both_edit(m: &mut Merging, theirs: Value, ours: Value) {
        m.git(&["checkout", "-q", "-b", "theirs"]);
        m.session.reload().unwrap();
        m.call(
            "feature.edit",
            json!({"part": "p", "id": "b", "params": theirs}),
        )
        .unwrap();
        m.commit("theirs");
        m.git(&["checkout", "-q", "main"]);
        m.session.reload().unwrap();
        m.call(
            "feature.edit",
            json!({"part": "p", "id": "b", "params": ours}),
        )
        .unwrap();
        m.commit("ours");
    }

    #[test]
    fn outside_a_merge_there_is_no_status() {
        let mut m = boxed();
        let e = m.call("merge.status", json!({})).unwrap_err();
        assert_eq!(e.code, Code::NotMerging);
        let dir = tempfile::tempdir().unwrap();
        crate::server::init(dir.path(), None).unwrap();
        let mut s = Session::open(dir.path()).unwrap();
        let e = call(&mut s, "merge.status", json!({})).unwrap_err();
        assert_eq!(e.code, Code::NotAGitRepo);
    }

    #[test]
    fn a_clean_merge_has_nothing_to_settle() {
        let mut m = boxed();
        both_edit(&mut m, json!({"y": 25}), json!({"x": 12}));
        m.merge("theirs");
        let v = m.call("merge.status", json!({})).unwrap();
        assert_eq!(v["step"], "merge");
        assert_eq!(v["clean"], true, "{v}");
        assert_eq!(v["unstaged"], json!([]));
    }

    #[test]
    fn records_and_unmerged_paths_are_conflicts() {
        let mut m = boxed();
        m.call(
            "feature.add",
            json!({"part": "p", "type": "sphere", "id": "s", "params": {"radius": 1}}),
        )
        .unwrap();
        m.commit("a sphere");
        m.git(&["checkout", "-q", "-b", "theirs"]);
        m.session.reload().unwrap();
        m.call(
            "feature.edit",
            json!({"part": "p", "id": "b", "params": {"x": 15}}),
        )
        .unwrap();
        m.call(
            "feature.edit",
            json!({"part": "p", "id": "s", "params": {"radius": 2}}),
        )
        .unwrap();
        m.commit("theirs");
        m.git(&["checkout", "-q", "main"]);
        m.session.reload().unwrap();
        m.call(
            "feature.edit",
            json!({"part": "p", "id": "b", "params": {"x": 12}}),
        )
        .unwrap();
        // Ours deletes the sphere theirs changed: a modify/delete.
        m.call("feature.delete", json!({"part": "p", "id": "s"}))
            .unwrap();
        m.commit("ours");
        m.merge("theirs");
        let v = m.call("merge.status", json!({})).unwrap();
        assert_eq!(v["clean"], false);
        let ids: Vec<&str> = v["conflicts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["id"].as_str().unwrap())
            .collect();
        assert_eq!(
            ids,
            [
                "parts/p/features/b.json#/params/x",
                "parts/p/features/s.json#"
            ]
        );
        let x = &v["conflicts"][0];
        assert_eq!(x["kind"], "value");
        assert_eq!(x["concerns"], json!({"part": "p", "feature": "b"}));
        assert_eq!(
            (x["ours"].as_str(), x["theirs"].as_str()),
            (Some("12 mm"), Some("15 mm"))
        );
        assert_eq!(x["choices"], json!(["ours", "theirs", "value"]));
        assert_eq!(x["cause"], "both");
        let s = &v["conflicts"][1];
        assert_eq!(
            (s["kind"].as_str(), s["deleted_by"].as_str()),
            (Some("modify_delete"), Some("ours"))
        );
        assert_eq!(s["theirs"]["params"]["radius"], "2 mm");
        assert!(s.get("ours").is_none());
        // A file settled by hand but not added is only a reminder.
        let b = m.root().join("parts/p/features/b.json");
        let text = std::fs::read_to_string(&b).unwrap();
        let mut j: Value = serde_json::from_str(&text).unwrap();
        j.as_object_mut().unwrap().remove("conflicts");
        crate::doc::project::write_json(&b, &j).unwrap();
        let v = m.call("merge.status", json!({})).unwrap();
        assert_eq!(v["unstaged"], json!(["parts/p/features/b.json"]));
        assert_eq!(v["conflicts"].as_array().unwrap().len(), 1);
        // Issue #54: the record added to the index, then settled: no longer
        // unmerged, but a commit would take the record.
        let settled = std::fs::read(&b).unwrap();
        std::fs::write(&b, &text).unwrap();
        m.git(&["add", "parts/p/features/b.json"]);
        std::fs::write(&b, &settled).unwrap();
        let v = m.call("merge.status", json!({})).unwrap();
        assert_eq!(v["unstaged"], json!(["parts/p/features/b.json"]), "{v}");
        assert_eq!(v["untracked"], json!([]), "{v}");
        // A new file never added is untracked too: named apart, as nothing
        // the merge settled, and still refused by the hook.
        std::fs::create_dir_all(m.root().join("imports")).unwrap();
        std::fs::write(m.root().join("imports/extra.step"), "ISO-10303-21;\n").unwrap();
        let v = m.call("merge.status", json!({})).unwrap();
        assert_eq!(v["untracked"], json!(["imports/extra.step"]), "{v}");
        assert_eq!(
            v["unstaged"],
            json!(["imports/extra.step", "parts/p/features/b.json"]),
            "{v}"
        );
    }

    #[test]
    fn what_merged_as_lines_is_unparsed() {
        let mut m = boxed();
        m.git(&["checkout", "-q", "-b", "theirs"]);
        let b = m.root().join("parts/p/features/b.json");
        let text = std::fs::read_to_string(&b).unwrap();
        std::fs::write(&b, text.replace("\"10 mm\"", "\"15 mm\"")).unwrap();
        m.commit("theirs");
        m.git(&["checkout", "-q", "main"]);
        // Ours isn't canonical: the driver can't read it, Git's lines clash.
        std::fs::write(
            &b,
            text.replace("\"10 mm\"", "\"12 mm\"").replace("  ", "    "),
        )
        .unwrap();
        m.commit("ours");
        assert!(!try_git(m.root(), &["merge", "--no-edit", "theirs"]));
        m.session.reload().unwrap();
        let v = m.call("merge.status", json!({})).unwrap();
        assert_eq!(v["conflicts"][0]["kind"], "merge_unparsed");
        assert_eq!(v["conflicts"][0]["id"], "parts/p/features/b.json#");
        assert_eq!(v["conflicts"][0]["theirs"]["params"]["x"], "15 mm");
    }

    /// Adds sketch `s` on xy, a 10 mm square drawn with lines `l1`..`l4`,
    /// and an extrude `e` of it.
    fn squared(m: &mut Merging) {
        m.call(
            "feature.add",
            json!({"part": "p", "type": "sketch", "id": "s", "plane": "xy"}),
        )
        .unwrap();
        m.call(
            "sketch.edit",
            json!({"part": "p", "sketch": "s", "ops": [
                {"op": "add", "type": "point", "at": [0, 0]},
                {"op": "add", "type": "point", "at": [10, 0]},
                {"op": "add", "type": "point", "at": [10, 10]},
                {"op": "add", "type": "point", "at": [0, 10]},
                {"op": "add", "type": "line", "start": "p1", "end": "p2"},
                {"op": "add", "type": "line", "start": "p2", "end": "p3"},
                {"op": "add", "type": "line", "start": "p3", "end": "p4"},
                {"op": "add", "type": "line", "start": "p4", "end": "p1"},
            ]}),
        )
        .unwrap();
        m.call(
            "feature.add",
            json!({"part": "p", "type": "extrude", "id": "e", "sketch": "s",
                   "extent": {"type": "blind", "length": "5 mm"}}),
        )
        .unwrap();
    }

    #[test]
    fn what_the_merge_broke_names_the_side_that_broke_it() {
        let mut m = boxed();
        squared(&mut m);
        m.commit("a square, extruded");
        // Theirs sketches on the extrude's top face.
        m.git(&["checkout", "-q", "-b", "theirs"]);
        m.session.reload().unwrap();
        let top = m
            .call("inspect", json!({"part": "p", "feature": "e"}))
            .unwrap();
        let face = top["bodies"][0]["faces"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| {
                f["surface"]["type"] == "plane"
                    && f["surface"]["frame"]["origin"][2]
                        .as_f64()
                        .is_some_and(|z| z > 0.004)
            })
            .map(|f| format!("p/e/{}", f["name"].as_str().unwrap()))
            .expect("a top face");
        m.call(
            "feature.add",
            json!({"part": "p", "type": "sketch", "id": "t", "face": face}),
        )
        .unwrap();
        m.commit("theirs: a sketch on top");
        // Ours deletes the square's last line: the extrude has no loop.
        m.git(&["checkout", "-q", "main"]);
        m.session.reload().unwrap();
        m.call(
            "sketch.edit",
            json!({"part": "p", "sketch": "s", "ops": [{"op": "remove", "id": "l4"}]}),
        )
        .unwrap();
        m.commit("ours: an open square");
        m.merge("theirs");
        let v = m.call("merge.status", json!({})).unwrap();
        // The extrude failed on ours already; the sketch on its face built
        // on theirs and the merge broke it, through ours' change.
        assert_eq!(v["already_failing"][0]["feature"], "e", "{v}");
        assert_eq!(v["already_failing"][0]["failed_on"], json!(["ours"]));
        let c: Vec<&Value> = v["conflicts"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|c| c["kind"] == "merge_broke")
            .collect();
        assert_eq!(c.len(), 1, "{v}");
        assert_eq!(c[0]["id"], "parts/p/features/t.json#merge_broke");
        assert_eq!(c[0]["cause"], "ours");
        assert_eq!(c[0]["built_on"], json!(["theirs"]));
        assert_eq!(c[0]["files"], json!(["parts/p/features/s.json"]));
        assert_eq!(c[0]["error"]["code"], "dependency_failed");
        assert_eq!(v["clean"], false);
    }

    #[test]
    fn a_failing_feature_neither_side_has_does_not_block_the_merge() {
        let mut m = boxed();
        both_edit(&mut m, json!({"x": 12}), json!({"y": 12}));
        // Work in progress, never committed: a part whose cut misses.
        m.call("part.create", json!({"part": "q"})).unwrap();
        for f in [
            json!({"type": "box", "id": "b", "params": {"x": 10, "y": 10, "z": 10}}),
            json!({"type": "cylinder", "id": "miss", "operation": "cut", "targets": ["b"],
                   "params": {"radius": 1, "height": 1}, "placement": {"origin": [50, 50, 50]}}),
        ] {
            let mut f = f;
            f["part"] = json!("q");
            m.call("feature.add", f).unwrap();
        }
        m.merge("theirs");
        let v = m.call("merge.status", json!({})).unwrap();
        assert!(
            !v["conflicts"]
                .as_array()
                .unwrap()
                .iter()
                .any(|c| c["kind"] == "merge_broke"),
            "{v}"
        );
        // Listed, as failing outside the merge: no side has it.
        let a = &v["already_failing"];
        assert_eq!(a.as_array().unwrap().len(), 1, "{v}");
        assert_eq!(a[0]["part"], "q");
        assert_eq!(a[0]["feature"], "miss");
        assert_eq!(a[0]["error"], "operation_no_effect");
        assert_eq!(a[0]["failed_on"], json!([]));
        // Staging it changes nothing: the merge is clean but for it.
        m.git(&["add", "-A"]);
        let v = m.call("merge.status", json!({})).unwrap();
        assert_eq!(v["clean"], true, "{v}");
    }

    #[test]
    fn a_merge_that_closes_a_cycle_of_instances_blames_the_other_side() {
        let mut m = boxed();
        for a in ["frame", "sub"] {
            m.call("part.create", json!({"part": a, "kind": "assembly"}))
                .unwrap();
        }
        m.commit("two assemblies");
        // Theirs places the frame in the sub; ours, the sub in the frame.
        m.git(&["checkout", "-q", "-b", "theirs"]);
        m.session.reload().unwrap();
        m.call(
            "feature.add",
            json!({"part": "sub", "type": "instance", "id": "j", "of": "frame"}),
        )
        .unwrap();
        m.commit("theirs: the frame in the sub");
        m.git(&["checkout", "-q", "main"]);
        m.session.reload().unwrap();
        m.call(
            "feature.add",
            json!({"part": "frame", "type": "instance", "id": "i", "of": "sub"}),
        )
        .unwrap();
        m.commit("ours: the sub in the frame");
        m.merge("theirs");
        let v = m.call("merge.status", json!({})).unwrap();
        let c: Vec<&Value> = v["conflicts"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|c| c["kind"] == "merge_broke")
            .collect();
        assert_eq!(c.len(), 2, "{v}");
        assert_eq!(c[0]["id"], "parts/frame/features/i.json#merge_broke");
        assert_eq!(c[0]["cause"], "theirs");
        assert_eq!(c[0]["error"]["code"], "assembly_cycle");
        assert_eq!(
            c[0]["files"],
            json!(["parts/sub/features/j.json", "parts/sub/part.json"])
        );
        assert_eq!(c[1]["id"], "parts/sub/features/j.json#merge_broke");
        assert_eq!(c[1]["cause"], "ours");
    }

    #[test]
    fn a_cut_the_other_side_added_is_blamed_for_the_face_it_took() {
        let mut m = boxed();
        // Theirs sketches on the box's top.
        m.git(&["checkout", "-q", "-b", "theirs"]);
        m.session.reload().unwrap();
        let v = m.call("inspect", json!({"part": "p"})).unwrap();
        let top = v["bodies"][0]["faces"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| {
                f["surface"]["frame"]["origin"][2]
                    .as_f64()
                    .is_some_and(|z| z > 0.004)
            })
            .map(|f| format!("p/b/{}", f["name"].as_str().unwrap()))
            .expect("a top face");
        m.call(
            "feature.add",
            json!({"part": "p", "type": "sketch", "id": "t", "face": top}),
        )
        .unwrap();
        m.commit("theirs: a sketch on top");
        // Ours cuts the top millimetre away.
        m.git(&["checkout", "-q", "main"]);
        m.session.reload().unwrap();
        m.call(
            "feature.add",
            json!({"part": "p", "type": "box", "id": "skim", "operation": "cut",
                   "params": {"x": 30, "y": 30, "z": 2}, "placement": {"origin": [-5, -5, 4]}}),
        )
        .unwrap();
        m.commit("ours: the top skimmed");
        m.merge("theirs");
        let v = m.call("merge.status", json!({})).unwrap();
        let c: Vec<&Value> = v["conflicts"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|c| c["kind"] == "merge_broke")
            .collect();
        assert_eq!(c.len(), 1, "{v}");
        assert_eq!(c[0]["concerns"]["feature"], "t");
        assert_eq!(c[0]["cause"], "ours", "{v}");
        assert_eq!(c[0]["files"], json!(["parts/p/features/skim.json"]));
        assert_eq!(c[0]["error"]["code"], "reference_unresolved");
        assert_eq!(c[0]["error"]["details"]["removed_by"], "skim");
    }

    /// The name of the edge of an `inspect` body from `a` to `b` (meters).
    fn edge_of(body: &Value, a: [f64; 3], b: [f64; 3]) -> String {
        use crate::sketch::tests::{edge_between, vertex_at};
        let (a, b) = (vertex_at(body, a), vertex_at(body, b));
        edge_between(body, &a, &b)
    }

    #[test]
    fn a_fillet_whose_edge_the_other_side_took_is_blamed_on_it() {
        let mut m = boxed();
        // Theirs rounds the box's top front edge.
        m.git(&["checkout", "-q", "-b", "theirs"]);
        m.session.reload().unwrap();
        let v = m.call("inspect", json!({"part": "p"})).unwrap();
        let front = edge_of(&v["bodies"][0], [0.0, 0.0, 0.005], [0.01, 0.0, 0.005]);
        m.call(
            "feature.add",
            json!({"part": "p", "type": "fillet", "id": "round", "radius": 1,
                   "edges": [format!("p/b/{front}")]}),
        )
        .unwrap();
        m.commit("theirs: a rounded edge");
        // Ours cuts the top millimetre away, and the edge with it.
        m.git(&["checkout", "-q", "main"]);
        m.session.reload().unwrap();
        m.call(
            "feature.add",
            json!({"part": "p", "type": "box", "id": "skim", "operation": "cut",
                   "params": {"x": 30, "y": 30, "z": 2}, "placement": {"origin": [-5, -5, 4]}}),
        )
        .unwrap();
        m.commit("ours: the top skimmed");
        m.merge("theirs");
        let v = m.call("merge.status", json!({})).unwrap();
        let c: Vec<&Value> = v["conflicts"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|c| c["kind"] == "merge_broke")
            .collect();
        assert_eq!(c.len(), 1, "{v}");
        assert_eq!(c[0]["concerns"]["feature"], "round");
        assert_eq!(c[0]["cause"], "ours", "{v}");
        assert_eq!(c[0]["error"]["code"], "reference_unresolved");
        assert_eq!(c[0]["error"]["details"]["role"], "groups.0.edges.0");
        assert_eq!(c[0]["error"]["details"]["removed_by"], "skim");
    }

    /// Issue #87: a parameter conflict fails a feature of a solid's chain
    /// (its founder, or a join on it); what follows it in the chain fails
    /// through it, and the conflict is the only thing to settle.
    #[test]
    fn what_fails_through_a_conflicted_feature_is_left_to_its_conflict() {
        for (param, failed) in [("w", "b"), ("r", "j")] {
            let mut m = boxed();
            m.call(
                "param.set",
                json!({"part": "p", "parameters": {param: "3 mm"}}),
            )
            .unwrap();
            // Only the conflicted feature reads the parameter.
            let read = |f: &str| if f == failed { json!(param) } else { json!(2) };
            m.call(
                "feature.edit",
                json!({"part": "p", "id": "b", "params": {"z": read("b")}}),
            )
            .unwrap();
            m.call(
                "feature.add",
                json!({"part": "p", "type": "cylinder", "id": "j", "operation": "join",
                       "params": {"radius": read("j"), "height": 3},
                       "placement": {"origin": [5, 10, 1]}}),
            )
            .unwrap();
            let v = m.call("inspect", json!({"part": "p"})).unwrap();
            let front = edge_of(&v["bodies"][0], [0.0, 0.0, 0.0], [0.01, 0.0, 0.0]);
            m.call(
                "feature.add",
                json!({"part": "p", "type": "fillet", "id": "round", "radius": 1,
                       "edges": [format!("p/b/{front}")]}),
            )
            .unwrap();
            m.call(
                "feature.add",
                json!({"part": "p", "type": "cylinder", "id": "c", "operation": "cut",
                       "params": {"radius": 1, "height": 10},
                       "placement": {"origin": [5, 15, -2]}}),
            )
            .unwrap();
            m.commit("a parametric part");
            m.git(&["checkout", "-q", "-b", "theirs"]);
            m.session.reload().unwrap();
            m.call(
                "param.set",
                json!({"part": "p", "parameters": {param: "2.5 mm"}}),
            )
            .unwrap();
            m.commit("theirs");
            m.git(&["checkout", "-q", "main"]);
            m.session.reload().unwrap();
            m.call(
                "param.set",
                json!({"part": "p", "parameters": {param: "3.5 mm"}}),
            )
            .unwrap();
            m.commit("ours");
            m.merge("theirs");
            let v = m.call("merge.status", json!({})).unwrap();
            let ids: Vec<&str> = v["conflicts"]
                .as_array()
                .unwrap()
                .iter()
                .map(|c| c["id"].as_str().unwrap())
                .collect();
            let only = format!("parts/p/parameters.json#/parameters/{param}");
            assert_eq!(ids, [only.as_str()], "{v}");
            let r = m.call("rebuild", json!({"part": "p"})).unwrap_err().details;
            let codes: Vec<(&str, Option<&str>)> = r["parts"][0]["features"]
                .as_array()
                .unwrap()
                .iter()
                .map(|f| (f["id"].as_str().unwrap(), f["error"]["code"].as_str()))
                .collect();
            let through = Some("dependency_failed");
            let want = if failed == "b" {
                [("b", Some("merge_conflict")), ("j", through)]
            } else {
                [("b", None), ("j", Some("merge_conflict"))]
            };
            let want: Vec<_> = want
                .into_iter()
                .chain([("round", through), ("c", through)])
                .collect();
            assert_eq!(codes, want, "{r}");
        }
    }

    /// Issue #90: a modify/delete kept with `git add` is no longer unmerged,
    /// but the merged list took the deletion; until the list holds it again
    /// (or the file goes) it is still a conflict, and settling it lists it.
    #[test]
    fn a_kept_modify_delete_staged_unlisted_is_still_a_conflict() {
        let mut m = boxed();
        m.call(
            "feature.add",
            json!({"part": "p", "type": "sphere", "id": "s", "params": {"radius": 1}}),
        )
        .unwrap();
        m.commit("a sphere");
        m.git(&["checkout", "-q", "-b", "theirs"]);
        m.session.reload().unwrap();
        m.call("feature.delete", json!({"part": "p", "id": "s"}))
            .unwrap();
        m.commit("theirs: no sphere");
        m.git(&["checkout", "-q", "main"]);
        m.session.reload().unwrap();
        m.call(
            "feature.edit",
            json!({"part": "p", "id": "s", "params": {"radius": 2}}),
        )
        .unwrap();
        m.commit("ours: a bigger sphere");
        m.merge("theirs");
        let id = "parts/p/features/s.json#";
        // Kept as Git left it (ours' version) and added, the usual Git way.
        m.git(&["add", "-A"]);
        m.session.reload().unwrap();
        let v = m.call("merge.status", json!({})).unwrap();
        assert_eq!(v["clean"], false, "{v}");
        let c = &v["conflicts"][0];
        assert_eq!(c["id"], id, "{v}");
        assert_eq!(c["kind"], "modify_delete");
        assert_eq!(c["deleted_by"], "theirs");
        assert_eq!(c["choices"], json!(["ours", "theirs"]));
        // `check` fails on it too, as on a path Git left unmerged.
        let e = m.call("check", json!({})).unwrap_err();
        let unmerged: Vec<&Value> = e.details["findings"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|f| f["code"] == "unmerged_path")
            .collect();
        assert_eq!(unmerged.len(), 1, "{e:?}");
        assert_eq!(unmerged[0]["files"][0], "parts/p/features/s.json");
        m.call(
            "merge.resolve",
            json!({"resolutions": [{"conflict": id, "choice": "ours"}]}),
        )
        .unwrap();
        let list = m.call("feature.list", json!({"part": "p"})).unwrap();
        let ids: Vec<&str> = list["features"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, ["b", "s"], "{list}");
        let shown = m
            .call("feature.show", json!({"part": "p", "id": "s"}))
            .unwrap();
        assert_eq!(shown["params"]["radius"]["value"], "2 mm", "{shown}");
        m.git(&["add", "-A"]);
        let v = m.call("merge.status", json!({})).unwrap();
        assert_eq!(v["clean"], true, "{v}");
        let v = m.call("check", json!({})).unwrap();
        assert_eq!(v["passed"], true, "{v}");
    }

    #[test]
    fn a_sketch_the_merge_left_stale_is_solved_by_a_repair() {
        let mut m = boxed();
        squared(&mut m);
        m.call(
            "sketch.edit",
            json!({"part": "p", "sketch": "s", "ops": [
                {"op": "add", "type": "horizontal", "a": "l1"},
                {"op": "add", "type": "length", "a": "l1", "value": "10 mm"},
            ]}),
        )
        .unwrap();
        m.commit("a level square");
        // Each side moves p2 a way its own constraints allow: each is
        // current, and the merge takes ours' move with theirs' dimension.
        m.git(&["checkout", "-q", "-b", "theirs"]);
        m.session.reload().unwrap();
        m.call(
            "sketch.edit",
            json!({"part": "p", "sketch": "s", "ops": [{"op": "set", "id": "d1", "value": "12 mm"}]}),
        )
        .unwrap();
        m.commit("theirs: longer");
        m.git(&["checkout", "-q", "main"]);
        m.session.reload().unwrap();
        m.call(
            "sketch.edit",
            json!({"part": "p", "sketch": "s", "ops": [{"op": "move", "id": "p2", "at": [10, 3]},
                                                        {"op": "remove", "id": "c1"}]}),
        )
        .unwrap();
        m.commit("ours: p2 up, no longer level");
        m.merge("theirs");
        let v = m.call("merge.status", json!({})).unwrap();
        let c: Vec<&Value> = v["conflicts"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|c| c["kind"] == "sketch_stale")
            .collect();
        assert_eq!(c.len(), 1, "{v}");
        assert_eq!(c[0]["id"], "parts/p/features/s.json#sketch_stale");
        assert_eq!(c[0]["cause"], "both");
        let r = &c[0]["repairs"][0];
        assert!(
            r["id"]
                .as_str()
                .unwrap()
                .starts_with("parts/p/features/s.json#sketch_stale:solve@")
        );
        assert_eq!(r["changes"][0]["path"], "parts/p/features/s.json");
        // Solved, the file holds: the sketch is current again.
        let solved: Value =
            serde_json::from_str(r["changes"][0]["content"].as_str().unwrap()).unwrap();
        crate::doc::project::write_json(&m.root().join("parts/p/features/s.json"), &solved)
            .unwrap();
        m.session.reload().unwrap();
        let v = m.call("merge.status", json!({})).unwrap();
        assert_eq!(v["clean"], true, "{v}");
    }

    /// Ours took out the only instance of a library's part (and the lock
    /// with it); theirs placed it again elsewhere. Git takes ours' lock
    /// away, and the merge places a label no lock holds.
    #[test]
    fn a_merge_that_leaves_the_lock_behind_is_repaired_from_the_cache() {
        use crate::deps::tests::{depend, library, url};
        let mut m = boxed();
        let root = m.root().to_path_buf();
        std::fs::write(root.join(".git/info/exclude"), "cache/\n").unwrap();
        let (lib_dir, lib) = library();
        let cache = ryupdm::deps::Cache::new(root.join("cache"));
        cache.fetch(&url(lib_dir.path()), &[""]).unwrap();
        m.session.set_deps_cache(cache);
        depend(&mut m.session, &lib, lib_dir.path());
        m.call("part.create", json!({"part": "frame", "kind": "assembly"}))
            .unwrap();
        let place = |m: &mut Merging, id: &str| {
            m.call(
                "feature.add",
                json!({"part": "frame", "type": "instance", "id": id, "of": "lib:p", "rev": "A"}),
            )
            .unwrap();
        };
        place(&mut m, "b");
        m.commit("base");
        m.git(&["checkout", "-q", "-b", "theirs"]);
        m.session.reload().unwrap();
        place(&mut m, "c");
        m.commit("theirs");
        m.git(&["checkout", "-q", "main"]);
        m.session.reload().unwrap();
        m.call("feature.delete", json!({"part": "frame", "id": "b"}))
            .unwrap();
        assert!(!root.join(crate::doc::lock::LOCK).exists());
        m.commit("ours");
        m.merge("theirs");
        let v = m.call("merge.status", json!({})).unwrap();
        let c = v["conflicts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["kind"] == "dependency_unlocked")
            .cloned()
            .unwrap_or_else(|| panic!("{v}"));
        assert_eq!(c["id"], "ryucad.lock#dependency_unlocked");
        assert_eq!(c["concerns"]["unlocked"], json!([format!("{lib}:p@A")]));
        let repair = c["repairs"][0]["id"].as_str().unwrap().to_owned();
        assert!(repair.starts_with("ryucad.lock#dependency_unlocked:deps_lock@"));
        resolve(
            &mut m,
            c["id"].as_str().unwrap(),
            json!({"choice": "repair", "repair": repair}),
        )
        .unwrap();
        assert!(root.join(crate::doc::lock::LOCK).exists());
        let v = m.call("merge.status", json!({})).unwrap();
        assert!(
            v["conflicts"]
                .as_array()
                .unwrap()
                .iter()
                .all(|c| c["kind"] != "dependency_unlocked"),
            "{v}"
        );
    }

    fn resolve(m: &mut Merging, conflict: &str, how: Value) -> Result<Value> {
        let mut r = json!({"conflict": conflict});
        for (k, v) in how.as_object().unwrap() {
            r[k] = v.clone();
        }
        m.call("merge.resolve", json!({"resolutions": [r]}))
    }

    #[test]
    fn a_conflict_takes_a_side_or_a_value() {
        let mut m = boxed();
        both_edit(&mut m, json!({"x": 15, "y": 1}), json!({"x": 12, "y": 2}));
        m.merge("theirs");
        let v = m.call("merge.status", json!({})).unwrap();
        assert_eq!(v["conflicts"].as_array().unwrap().len(), 2);
        let x = "parts/p/features/b.json#/params/x";
        let y = "parts/p/features/b.json#/params/y";
        // Both in one change: theirs for x, a value of the caller's for y.
        let r = m
            .call(
                "merge.resolve",
                json!({"resolutions": [
                    {"conflict": x, "choice": "theirs"},
                    {"conflict": y, "choice": "value", "value": "7 mm"},
                ]}),
            )
            .unwrap();
        assert_eq!(r["resolved"], json!([x, y]));
        assert_eq!(
            r["changes"],
            json!([{"path": "parts/p/features/b.json", "change": "written"}])
        );
        assert_eq!(r["status"]["clean"], true, "{r}");
        assert_eq!(r["status"]["unstaged"], json!(["parts/p/features/b.json"]));
        let b = m
            .call("feature.show", json!({"part": "p", "id": "b"}))
            .unwrap();
        assert_eq!(b["params"]["x"]["value"], "15 mm", "{b}");
        assert_eq!(b["params"]["y"]["value"], "7 mm");
        let text = std::fs::read_to_string(m.root().join("parts/p/features/b.json")).unwrap();
        assert!(crate::doc::canonical::is_canonical(&text));
        // Settled, it is no conflict any more.
        let e = resolve(&mut m, x, json!({"choice": "ours"})).unwrap_err();
        assert_eq!(e.code, Code::ConflictNotFound);
    }

    #[test]
    fn what_cant_be_settled_so_is_refused_and_nothing_is_written() {
        let mut m = boxed();
        both_edit(&mut m, json!({"x": 15}), json!({"x": 12}));
        m.merge("theirs");
        let x = "parts/p/features/b.json#/params/x";
        let before = std::fs::read(m.root().join("parts/p/features/b.json")).unwrap();
        let rev = m
            .session
            .project()
            .parts
            .values()
            .next()
            .unwrap()
            .revision
            .clone();
        for (r, code) in [
            (
                json!([{"conflict": "nowhere#", "choice": "ours"}]),
                Code::ConflictNotFound,
            ),
            (json!([{"conflict": x, "choice": "value"}]), Code::Usage),
            (
                json!([{"conflict": x, "choice": "repair", "repair": "x:y@0"}]),
                Code::RepairConflict,
            ),
            (
                json!([{"conflict": x, "choice": "mine"}]),
                Code::InvalidParam,
            ),
            // The first would do; the second refuses the batch.
            (
                json!([{"conflict": x, "choice": "ours"}, {"conflict": "nowhere#", "choice": "ours"}]),
                Code::ConflictNotFound,
            ),
        ] {
            let e = m
                .call("merge.resolve", json!({"resolutions": r}))
                .unwrap_err();
            assert_eq!(e.code, code, "{r}");
        }
        let e = m
            .call(
                "merge.resolve",
                json!({"resolutions": [{"conflict": x, "choice": "ours"}],
                       "expected_revisions": {"p": "0000"}}),
            )
            .unwrap_err();
        assert_eq!(e.code, Code::Conflict);
        assert_eq!(
            std::fs::read(m.root().join("parts/p/features/b.json")).unwrap(),
            before
        );
        // At the revision read, it goes through.
        m.call(
            "merge.resolve",
            json!({"resolutions": [{"conflict": x, "choice": "ours"}],
                   "expected_revisions": {"p": rev}}),
        )
        .unwrap();
    }

    #[test]
    fn a_feature_deleted_and_changed_is_kept_or_deleted() {
        let mut m = boxed();
        m.call(
            "feature.add",
            json!({"part": "p", "type": "sphere", "id": "s", "params": {"radius": 1}}),
        )
        .unwrap();
        m.call(
            "feature.add",
            json!({"part": "p", "type": "sphere", "id": "t", "params": {"radius": 1}}),
        )
        .unwrap();
        m.commit("two spheres");
        m.git(&["checkout", "-q", "-b", "theirs"]);
        m.session.reload().unwrap();
        for id in ["s", "t"] {
            m.call(
                "feature.edit",
                json!({"part": "p", "id": id, "params": {"radius": 2}}),
            )
            .unwrap();
        }
        m.commit("theirs: bigger");
        m.git(&["checkout", "-q", "main"]);
        m.session.reload().unwrap();
        for id in ["s", "t"] {
            m.call("feature.delete", json!({"part": "p", "id": id}))
                .unwrap();
        }
        m.commit("ours: gone");
        m.merge("theirs");
        let (s, t) = ("parts/p/features/s.json#", "parts/p/features/t.json#");
        // Theirs' sphere s stays, listed where theirs lists it.
        let r = resolve(&mut m, s, json!({"choice": "theirs"})).unwrap();
        assert_eq!(
            r["changes"],
            json!([{"path": "parts/p/part.json", "change": "written"}])
        );
        let list = m.call("feature.list", json!({"part": "p"})).unwrap();
        let ids: Vec<&str> = list["features"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, ["b", "s"]);
        // Ours' deletion of t stands: the file goes.
        let r = resolve(&mut m, t, json!({"choice": "ours"})).unwrap();
        assert_eq!(
            r["changes"],
            json!([{"path": "parts/p/features/t.json", "change": "deleted"}])
        );
        assert!(!m.root().join("parts/p/features/t.json").exists());
        // Kept looks as Git left it: s waits for `git add`, t for `git rm`,
        // and the list theirs' s was put back in for `git add` too.
        let v = &r["status"];
        assert_eq!(v["conflicts"][0]["id"], s);
        assert_eq!(
            v["unstaged"],
            json!(["parts/p/features/t.json", "parts/p/part.json"])
        );
        m.git(&["add", "-A"]);
        let v = m.call("merge.status", json!({})).unwrap();
        assert_eq!(
            (v["clean"].as_bool(), v["unstaged"].clone()),
            (Some(true), json!([]))
        );
    }

    #[test]
    fn a_repair_is_applied_as_planned() {
        let mut m = boxed();
        squared(&mut m);
        m.call(
            "sketch.edit",
            json!({"part": "p", "sketch": "s", "ops": [
                {"op": "add", "type": "length", "a": "l1", "value": "10 mm"},
            ]}),
        )
        .unwrap();
        m.commit("a dimensioned square");
        m.git(&["checkout", "-q", "-b", "theirs"]);
        m.session.reload().unwrap();
        m.call(
            "sketch.edit",
            json!({"part": "p", "sketch": "s", "ops": [{"op": "set", "id": "d1", "value": "12 mm"}]}),
        )
        .unwrap();
        m.commit("theirs");
        m.git(&["checkout", "-q", "main"]);
        m.session.reload().unwrap();
        m.call(
            "sketch.edit",
            json!({"part": "p", "sketch": "s", "ops": [{"op": "move", "id": "p2", "at": [10, 3]}]}),
        )
        .unwrap();
        m.commit("ours");
        m.merge("theirs");
        let v = m.call("merge.status", json!({})).unwrap();
        let c = v["conflicts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["kind"] == "sketch_stale")
            .expect("a stale sketch")
            .clone();
        let id = c["repairs"][0]["id"].as_str().unwrap();
        let r = resolve(
            &mut m,
            c["id"].as_str().unwrap(),
            json!({"choice": "repair", "repair": id}),
        )
        .unwrap();
        assert_eq!(r["status"]["clean"], true, "{r}");
        let g = m
            .call("sketch.get", json!({"part": "p", "sketch": "s"}))
            .unwrap();
        assert_eq!(g["stale"], false, "{g}");
    }

    #[test]
    fn a_feature_git_took_to_be_renamed_is_restored_or_dropped() {
        for repair in ["restore", "drop"] {
            let mut m = boxed();
            m.call(
                "feature.add",
                json!({"part": "p", "type": "sphere", "id": "c", "params": {"radius": 1}}),
            )
            .unwrap();
            m.commit("a sphere");
            m.git(&["checkout", "-q", "-b", "theirs"]);
            m.session.reload().unwrap();
            m.call(
                "feature.edit",
                json!({"part": "p", "id": "c", "params": {"radius": 2}}),
            )
            .unwrap();
            m.commit("theirs: bigger");
            m.git(&["checkout", "-q", "main"]);
            m.session.reload().unwrap();
            m.call("feature.delete", json!({"part": "p", "id": "c"}))
                .unwrap();
            m.call(
                "feature.add",
                json!({"part": "p", "type": "sphere", "id": "d", "params": {"radius": 1}}),
            )
            .unwrap();
            m.commit("ours: c is d now");
            m.merge("theirs");
            let v = m.call("merge.status", json!({})).unwrap();
            let c = &v["conflicts"][0];
            assert_eq!(c["kind"], "rename_merged", "{v}");
            assert_eq!(c["id"], "parts/p/features/d.json#rename_merged");
            assert_eq!(
                c["concerns"],
                json!({"part": "p", "feature": "d", "renamed_from": "c"})
            );
            let id = c["repairs"]
                .as_array()
                .unwrap()
                .iter()
                .map(|r| r["id"].as_str().unwrap().to_owned())
                .find(|r| r.contains(&format!(":{repair}@")))
                .unwrap();
            let r = resolve(
                &mut m,
                c["id"].as_str().unwrap(),
                json!({"choice": "repair", "repair": id}),
            )
            .unwrap();
            assert_eq!(r["status"]["clean"], true, "{r}");
            let radius = |id: &str, m: &mut Merging| {
                m.call("feature.show", json!({"part": "p", "id": id}))
                    .unwrap()["params"]["radius"]["value"]
                    .clone()
            };
            assert_eq!(radius("d", &mut m), "1 mm");
            let list = m.call("feature.list", json!({"part": "p"})).unwrap();
            let ids: Vec<&str> = list["features"]
                .as_array()
                .unwrap()
                .iter()
                .map(|f| f["id"].as_str().unwrap())
                .collect();
            if repair == "restore" {
                assert_eq!(radius("c", &mut m), "2 mm");
                assert_eq!(ids, ["b", "c", "d"]);
            } else {
                assert!(!m.root().join("parts/p/features/c.json").exists());
                assert_eq!(ids, ["b", "d"]);
            }
        }
    }

    /// The renumbering repair of the conflict `id`, applied.
    fn renumber(m: &mut Merging, id: &str) -> Value {
        let v = m.call("merge.status", json!({})).unwrap();
        let c = v["conflicts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["id"] == id)
            .unwrap_or_else(|| panic!("no {id}: {v}"))
            .clone();
        assert_eq!(c["kind"], "id_collision");
        let repair = c["repairs"][0]["id"].as_str().expect("a repair").to_owned();
        assert!(repair.contains(":renumber@"), "{repair}");
        resolve(m, id, json!({"choice": "repair", "repair": repair})).unwrap()
    }

    #[test]
    fn entities_added_under_one_id_are_renumbered() {
        let mut m = boxed();
        squared(&mut m);
        m.commit("a square");
        let add = |m: &mut Merging, at: [i32; 2], from: &str| {
            m.call(
                "sketch.edit",
                json!({"part": "p", "sketch": "s", "ops": [
                    {"op": "add", "type": "point", "at": at},
                    {"op": "add", "type": "line", "start": from, "end": "p5"},
                ]}),
            )
            .unwrap();
        };
        m.git(&["checkout", "-q", "-b", "theirs"]);
        m.session.reload().unwrap();
        add(&mut m, [0, 20], "p4");
        m.commit("theirs: a spur up");
        m.git(&["checkout", "-q", "main"]);
        m.session.reload().unwrap();
        add(&mut m, [20, 0], "p2");
        m.commit("ours: a spur across");
        m.merge("theirs");
        // Theirs' point first, then its line, which names it.
        renumber(&mut m, "parts/p/features/s.json#/entities/p5");
        let r = renumber(&mut m, "parts/p/features/s.json#/entities/l5");
        assert_eq!(r["status"]["clean"], true, "{r}");
        let s: Value = serde_json::from_str(
            &std::fs::read_to_string(m.root().join("parts/p/features/s.json")).unwrap(),
        )
        .unwrap();
        let e = &s["entities"];
        assert_eq!(e["p5"]["m"], json!([0.02, 0]));
        assert_eq!(e["p6"]["m"], json!([0, 0.02]));
        assert_eq!(
            (e["l5"]["start"].as_str(), e["l5"]["end"].as_str()),
            (Some("p2"), Some("p5"))
        );
        assert_eq!(
            (e["l6"]["start"].as_str(), e["l6"]["end"].as_str()),
            (Some("p4"), Some("p6"))
        );
        assert!(s.get("conflicts").is_none());
    }

    #[test]
    fn a_solid_added_under_one_id_is_renumbered_with_its_fillet() {
        let mut m = boxed();
        let add_box = |m: &mut Merging, x: i32| {
            m.call(
                "feature.add",
                json!({"part": "p", "type": "box", "id": "box-2",
                       "params": {"x": x, "y": 10, "z": 10},
                       "placement": {"origin": [40, 0, 0]}}),
            )
            .unwrap();
        };
        m.git(&["checkout", "-q", "-b", "theirs"]);
        m.session.reload().unwrap();
        add_box(&mut m, 30);
        // Theirs rounds its box's vertical edges at x = 70 mm, in two sizes.
        let v = m.call("inspect", json!({"part": "p"})).unwrap();
        let body = v["bodies"]
            .as_array()
            .unwrap()
            .iter()
            .find(|b| b["solid"] == "box-2")
            .unwrap()
            .clone();
        let near = edge_of(&body, [0.07, 0.0, 0.0], [0.07, 0.0, 0.01]);
        let far = edge_of(&body, [0.07, 0.01, 0.0], [0.07, 0.01, 0.01]);
        let groups = json!([{"radius": 2, "edges": [format!("p/box-2/{near}")]},
                            {"radius": 3, "edges": [format!("p/box-2/{far}")]}]);
        m.call(
            "feature.add",
            json!({"part": "p", "type": "fillet", "id": "round", "groups": groups}),
        )
        .unwrap();
        m.commit("theirs: a rounded box");
        m.git(&["checkout", "-q", "main"]);
        m.session.reload().unwrap();
        add_box(&mut m, 5);
        m.commit("ours: a short box");
        m.merge("theirs");
        let r = renumber(&mut m, "parts/p/features/box-2.json#");
        assert_eq!(r["status"]["clean"], true, "{r}");
        // The fillet's edges follow theirs' box to its new ID, in both
        // groups.
        let show = m
            .call("feature.show", json!({"part": "p", "id": "round"}))
            .unwrap();
        for g in show["groups"].as_array().unwrap() {
            assert_eq!(g["edges"][0].as_str().unwrap()[..8], *"p/box-1/", "{g}");
        }
        m.call("rebuild", json!({"part": "p"})).unwrap();
    }

    #[test]
    fn a_solid_added_under_one_id_is_renumbered_in_its_appearance() {
        let mut m = boxed();
        let add_box = |m: &mut Merging, x: i32| {
            m.call(
                "feature.add",
                json!({"part": "p", "type": "box", "id": "box-2",
                       "params": {"x": x, "y": 10, "z": 10},
                       "placement": {"origin": [40, 0, 0]}}),
            )
            .unwrap();
        };
        let a_face = |m: &mut Merging| -> String {
            let v = m.call("inspect", json!({"part": "p"})).unwrap();
            let body = v["bodies"]
                .as_array()
                .unwrap()
                .iter()
                .find(|b| b["solid"] == "box-2")
                .unwrap()
                .clone();
            body["faces"][0]["name"].as_str().unwrap().to_owned()
        };
        m.git(&["checkout", "-q", "-b", "theirs"]);
        m.session.reload().unwrap();
        add_box(&mut m, 30);
        let face = a_face(&mut m);
        m.call(
            "appearance.set",
            json!({"entities": ["p/box-2"], "opacity": 0.5}),
        )
        .unwrap();
        m.call(
            "appearance.set",
            json!({"entities": [format!("p/box-2/{face}")], "colour": "#d03030"}),
        )
        .unwrap();
        m.commit("theirs: a red-faced glass box");
        m.git(&["checkout", "-q", "main"]);
        m.session.reload().unwrap();
        add_box(&mut m, 5);
        m.commit("ours: a short box");
        m.merge("theirs");
        let r = renumber(&mut m, "parts/p/features/box-2.json#");
        assert_eq!(r["status"]["clean"], true, "{r}");
        // Theirs' entries follow its box to its new ID, the face by the
        // name the renumbered box gives it.
        let a = m.call("appearance.get", json!({"part": "p"})).unwrap();
        assert_eq!(a["warnings"], json!([]), "{a}");
        let solid = |key: &str| {
            a["solids"]
                .as_array()
                .unwrap()
                .iter()
                .find(|s| s["solid"] == key)
                .unwrap()
                .clone()
        };
        assert_eq!(solid("box-1")["opacity"], 0.5);
        assert_eq!(solid("box-1")["faces"].as_object().unwrap().len(), 1);
        assert_eq!(solid("box-2")["opacity"], 1);
        assert!(solid("box-2").get("faces").is_none());
    }

    #[test]
    fn an_instance_added_under_one_id_is_renumbered_in_its_explode() {
        let mut m = boxed();
        m.call("part.create", json!({"part": "asm", "kind": "assembly"}))
            .unwrap();
        m.call(
            "feature.add",
            json!({"part": "asm", "type": "instance", "id": "base", "of": "p"}),
        )
        .unwrap();
        m.commit("an assembly");
        let place = |m: &mut Merging, x: i32| {
            m.call(
                "feature.add",
                json!({"part": "asm", "type": "instance", "id": "lid", "of": "p",
                       "placement": {"origin": [x, 0, 10]}}),
            )
            .unwrap();
        };
        m.git(&["checkout", "-q", "-b", "theirs"]);
        m.session.reload().unwrap();
        place(&mut m, 0);
        m.call(
            "explode.save",
            json!({"part": "asm", "explode": "apart",
                   "steps": [{"instances": ["lid"], "translate": [0, 0, 40]},
                             {"instances": ["base"], "translate": [0, 0, -10]}]}),
        )
        .unwrap();
        m.commit("theirs: a lid lifted off");
        m.git(&["checkout", "-q", "main"]);
        m.session.reload().unwrap();
        place(&mut m, 30);
        m.commit("ours: a lid beside");
        m.merge("theirs");
        let r = renumber(&mut m, "parts/asm/features/lid.json#");
        assert_eq!(r["status"]["clean"], true, "{r}");
        // Theirs' explode lifts theirs' lid, under its new ID.
        let list = m.call("feature.list", json!({"part": "asm"})).unwrap();
        let ids: Vec<String> = list["features"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["id"].as_str().unwrap().to_owned())
            .collect();
        let new = ids
            .iter()
            .find(|i| !["base", "lid"].contains(&i.as_str()))
            .unwrap_or_else(|| panic!("{ids:?}"));
        let e: Value = serde_json::from_str(
            &std::fs::read_to_string(m.root().join("parts/asm/explodes/apart.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(e["steps"][0]["instances"], json!([new]), "{e}");
        assert_eq!(e["steps"][1]["instances"], json!(["base"]));
    }

    #[test]
    fn a_solid_added_under_one_id_is_renumbered_in_connectors_that_name_it() {
        let mut m = boxed();
        m.call("part.create", json!({"part": "asm", "kind": "assembly"}))
            .unwrap();
        for (i, at) in [("x", 0), ("y", 100)] {
            m.call(
                "feature.add",
                json!({"part": "asm", "type": "instance", "id": i, "of": "p",
                       "fixed": i == "x", "placement": {"origin": [at, 0, 0]}}),
            )
            .unwrap();
        }
        m.commit("two boxes placed");
        let add_box = |m: &mut Merging, x: i32| {
            m.call(
                "feature.add",
                json!({"part": "p", "type": "box", "id": "box-2",
                       "params": {"x": x, "y": 10, "z": 10},
                       "placement": {"origin": [40, 0, 0]}}),
            )
            .unwrap();
        };
        m.git(&["checkout", "-q", "-b", "theirs"]);
        m.session.reload().unwrap();
        add_box(&mut m, 30);
        // Theirs stands y's origin on the top of x's new box.
        let v = m.call("inspect", json!({"part": "p"})).unwrap();
        let top = v["bodies"]
            .as_array()
            .unwrap()
            .iter()
            .find(|b| b["solid"] == "box-2")
            .unwrap()["faces"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| {
                f["surface"]["frame"]["origin"][2]
                    .as_f64()
                    .is_some_and(|z| z > 0.0099)
            })
            .map(|f| f["name"].as_str().unwrap().to_owned())
            .expect("a face at z = 10 mm");
        m.call(
            "feature.add",
            json!({"part": "asm", "type": "mate", "id": "on", "form": "ball",
                   "a": format!("asm/x/box-2/{top}"), "b": "asm/y"}),
        )
        .unwrap();
        m.commit("theirs: y on x's new box");
        m.git(&["checkout", "-q", "main"]);
        m.session.reload().unwrap();
        add_box(&mut m, 5);
        m.commit("ours: a short box");
        m.merge("theirs");
        let r = renumber(&mut m, "parts/p/features/box-2.json#");
        assert_eq!(r["status"]["clean"], true, "{r}");
        // The connector follows theirs' box to its new ID, under the
        // name its top has there (issue #1145).
        let mate: Value = serde_json::from_str(
            &std::fs::read_to_string(m.root().join("parts/asm/features/on.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(mate["a"]["at"]["feature"], "box-1", "{mate}");
        assert_ne!(mate["a"]["at"]["name"], json!(top));
        let built = m.call("rebuild", json!({"part": "asm"})).unwrap();
        let on = built["parts"][0]["features"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["id"] == "on")
            .unwrap()
            .clone();
        assert_eq!(on["ok"], true, "{on}");
    }

    #[test]
    fn a_solid_added_under_one_id_is_renumbered_with_what_acts_on_it() {
        let mut m = boxed();
        let add_box = |m: &mut Merging, x: i32| {
            m.call(
                "feature.add",
                json!({"part": "p", "type": "box", "id": "box-2",
                       "params": {"x": x, "y": 10, "z": 10}}),
            )
            .unwrap();
        };
        m.git(&["checkout", "-q", "-b", "theirs"]);
        m.session.reload().unwrap();
        add_box(&mut m, 30);
        // Theirs cuts a slot across its box, then sketches on the box's +x
        // face, at x = 30 mm, as the cut left it.
        m.call(
            "feature.add",
            json!({"part": "p", "type": "box", "id": "slot", "operation": "cut",
                   "targets": ["box-2"], "params": {"x": 4, "y": 20, "z": 5},
                   "placement": {"origin": [10, -5, 6]}}),
        )
        .unwrap();
        let faces = m.call("inspect", json!({"part": "p"})).unwrap();
        let body = faces["bodies"]
            .as_array()
            .unwrap()
            .iter()
            .find(|b| b["solid"] == "box-2")
            .unwrap()
            .clone();
        let far = body["faces"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| {
                f["surface"]["frame"]["origin"][0]
                    .as_f64()
                    .is_some_and(|x| x > 0.029)
            })
            .map(|f| f["name"].as_str().unwrap().to_owned())
            .expect("a face at x = 30 mm");
        m.call(
            "feature.add",
            json!({"part": "p", "type": "sketch", "id": "on",
                   "face": format!("p/box-2/{far}")}),
        )
        .unwrap();
        m.commit("theirs: a slotted box, sketched on");
        m.git(&["checkout", "-q", "main"]);
        m.session.reload().unwrap();
        add_box(&mut m, 5);
        m.commit("ours: a short box");
        m.merge("theirs");
        let r = renumber(&mut m, "parts/p/features/box-2.json#");
        assert_eq!(r["status"]["clean"], true, "{r}");
        let show = |m: &mut Merging, id: &str| {
            m.call("feature.show", json!({"part": "p", "id": id}))
                .unwrap()
        };
        // The slot cuts theirs' box, by its new ID; the sketch follows it.
        assert_eq!(show(&mut m, "slot")["targets"], json!(["box-1"]));
        assert_eq!(
            show(&mut m, "on")["face"].as_str().unwrap()[..8],
            *"p/box-1/"
        );
        let built = m.call("rebuild", json!({})).unwrap();
        assert_eq!(built["parts"][0]["failed"], 0, "{built}");
        let solids: Vec<&str> = built["parts"][0]["solids"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["solid"].as_str().unwrap())
            .collect();
        assert_eq!(solids, ["b", "box-2", "box-1"]);
    }

    /// A sketch `s` on box `b`'s top holding points `p1` at (2, 5) and `p2`
    /// at (5, 15) mm, each fixed.
    fn spotted(m: &mut Merging) {
        m.call(
            "feature.add",
            json!({"part": "p", "type": "sketch", "id": "s", "placement": {"origin": [0, 0, 5]}}),
        )
        .unwrap();
        m.call(
            "sketch.edit",
            json!({"part": "p", "sketch": "s", "ops": [
                {"op": "add", "type": "point", "at": [2, 5]},
                {"op": "add", "type": "fix", "a": "p1"},
                {"op": "add", "type": "point", "at": [5, 15]},
                {"op": "add", "type": "fix", "a": "p2"},
            ]}),
        )
        .unwrap();
    }

    #[test]
    fn a_hole_added_under_one_id_is_renumbered_with_its_pattern() {
        let mut m = boxed();
        spotted(&mut m);
        m.commit("a spotted box");
        let hole = |m: &mut Merging, point: &str| {
            m.call(
                "feature.add",
                json!({"part": "p", "type": "hole", "sketch": "s", "points": [point],
                       "diameter": 1, "depth": {"blind": 2}}),
            )
            .unwrap()
        };
        m.git(&["checkout", "-q", "-b", "theirs"]);
        m.session.reload().unwrap();
        assert_eq!(hole(&mut m, "p1")["feature"]["id"], "hole-1");
        m.call(
            "feature.add",
            json!({"part": "p", "type": "pattern", "id": "row", "features": ["hole-1"],
                   "direction": "x", "spacing": 3, "count": 3}),
        )
        .unwrap();
        m.commit("theirs: a row of holes");
        m.git(&["checkout", "-q", "main"]);
        m.session.reload().unwrap();
        hole(&mut m, "p2");
        m.commit("ours: one hole");
        m.merge("theirs");
        let r = renumber(&mut m, "parts/p/features/hole-1.json#");
        assert_eq!(r["status"]["clean"], true, "{r}");
        // The pattern repeats theirs' hole, by its new ID.
        let show = m
            .call("feature.show", json!({"part": "p", "id": "row"}))
            .unwrap();
        assert_eq!(show["features"], json!(["hole-2"]));
        let built = m.call("rebuild", json!({})).unwrap();
        assert_eq!(built["parts"][0]["failed"], 0, "{built}");
    }

    #[test]
    fn a_point_a_hole_drills_at_is_renumbered_with_it() {
        let mut m = boxed();
        spotted(&mut m);
        m.commit("a spotted box");
        let point = |m: &mut Merging, at: [i32; 2]| {
            m.call(
                "sketch.edit",
                json!({"part": "p", "sketch": "s", "ops": [
                    {"op": "add", "type": "point", "at": at},
                ]}),
            )
            .unwrap();
        };
        m.git(&["checkout", "-q", "-b", "theirs"]);
        m.session.reload().unwrap();
        point(&mut m, [8, 8]);
        m.call(
            "feature.add",
            json!({"part": "p", "type": "hole", "id": "h", "sketch": "s", "points": ["p3"],
                   "diameter": 1, "depth": {"blind": 2}}),
        )
        .unwrap();
        m.commit("theirs: a hole at a new point");
        m.git(&["checkout", "-q", "main"]);
        m.session.reload().unwrap();
        point(&mut m, [5, 2]);
        m.commit("ours: another point");
        m.merge("theirs");
        let r = renumber(&mut m, "parts/p/features/s.json#/entities/p3");
        assert_eq!(r["status"]["clean"], true, "{r}");
        let show = m
            .call("feature.show", json!({"part": "p", "id": "h"}))
            .unwrap();
        assert_eq!(show["points"], json!(["p4"]));
    }

    #[test]
    fn a_pattern_the_other_side_left_hanging_is_blamed_on_it() {
        let mut m = boxed();
        spotted(&mut m);
        m.call(
            "feature.add",
            json!({"part": "p", "type": "hole", "id": "h", "sketch": "s", "points": ["p1"],
                   "diameter": 1, "depth": {"blind": 2}}),
        )
        .unwrap();
        m.commit("a drilled box");
        // Theirs repeats the hole along x, to x = 8 mm.
        m.git(&["checkout", "-q", "-b", "theirs"]);
        m.session.reload().unwrap();
        m.call(
            "feature.add",
            json!({"part": "p", "type": "pattern", "id": "row", "features": ["h"],
                   "direction": "x", "spacing": 3, "count": 3}),
        )
        .unwrap();
        m.commit("theirs: a row");
        // Ours makes the box 6 mm long: the last of the row cuts nothing.
        m.git(&["checkout", "-q", "main"]);
        m.session.reload().unwrap();
        m.call(
            "feature.edit",
            json!({"part": "p", "id": "b", "params": {"x": 6}}),
        )
        .unwrap();
        m.commit("ours: a shorter box");
        m.merge("theirs");
        let v = m.call("merge.status", json!({})).unwrap();
        let c: Vec<&Value> = v["conflicts"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|c| c["kind"] == "merge_broke")
            .collect();
        assert_eq!(c.len(), 1, "{v}");
        assert_eq!(c[0]["concerns"]["feature"], "row");
        assert_eq!(c[0]["cause"], "ours", "{v}");
        assert_eq!(c[0]["error"]["code"], "operation_no_effect");
        assert_eq!(c[0]["error"]["details"]["instance"], json!([2]));
    }

    #[test]
    fn a_feature_added_under_one_id_is_renumbered_with_what_reads_it() {
        let mut m = boxed();
        let add_box = |m: &mut Merging, x: i32| {
            m.call(
                "feature.add",
                json!({"part": "p", "type": "box", "id": "box-2",
                       "params": {"x": x, "y": 10, "z": 10}}),
            )
            .unwrap();
        };
        m.git(&["checkout", "-q", "-b", "theirs"]);
        m.session.reload().unwrap();
        add_box(&mut m, 30);
        // Theirs sketches on its box's +x face, at x = 30 mm.
        let faces = m
            .call("inspect", json!({"part": "p", "feature": "box-2"}))
            .unwrap();
        let far = faces["bodies"][0]["faces"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| {
                f["surface"]["frame"]["origin"][0]
                    .as_f64()
                    .is_some_and(|x| x > 0.029)
            })
            .map(|f| f["name"].as_str().unwrap().to_owned())
            .expect("a face at x = 30 mm");
        m.call(
            "feature.add",
            json!({"part": "p", "type": "sketch", "id": "on",
                   "face": format!("p/box-2/{far}")}),
        )
        .unwrap();
        m.commit("theirs: a long box, sketched on");
        m.git(&["checkout", "-q", "main"]);
        m.session.reload().unwrap();
        add_box(&mut m, 5);
        m.commit("ours: a short box");
        m.merge("theirs");
        let r = renumber(&mut m, "parts/p/features/box-2.json#");
        assert_eq!(r["status"]["clean"], true, "{r}");
        let list = m.call("feature.list", json!({"part": "p"})).unwrap();
        let ids: Vec<&str> = list["features"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["id"].as_str().unwrap())
            .collect();
        // The first free ID, as ryucad generates them; after ours' box.
        assert_eq!(ids, ["b", "box-2", "box-1", "on"]);
        let show = |m: &mut Merging, id: &str| {
            m.call("feature.show", json!({"part": "p", "id": id}))
                .unwrap()
        };
        assert_eq!(show(&mut m, "box-2")["params"]["x"]["value"], "5 mm");
        assert_eq!(show(&mut m, "box-1")["params"]["x"]["value"], "30 mm");
        // The sketch follows theirs' box, by the name its face has there now.
        let on: Value = serde_json::from_str(
            &std::fs::read_to_string(m.root().join("parts/p/features/on.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(on["face"]["feature"], "box-1");
        assert_ne!(on["face"]["name"], far.as_str());
        let built = m.call("rebuild", json!({})).unwrap();
        assert_eq!(built["parts"][0]["failed"], 0, "{built}");
        let g = m
            .call("sketch.get", json!({"part": "p", "sketch": "on"}))
            .unwrap();
        assert!(
            (g["frame"]["origin"][0].as_f64().unwrap() - 0.03).abs() < 1e-12,
            "{g}"
        );
    }

    #[test]
    fn values_are_put_at_pointers() {
        let mut v = json!({"a": {"b": 1}});
        set_at(&mut v, "/a/c", Some(json!(2)));
        set_at(&mut v, "/x/y~1z", Some(json!(3)));
        set_at(&mut v, "/a/b", None);
        assert_eq!(v, json!({"a": {"c": 2}, "x": {"y/z": 3}}));
        let mut l: Vec<String> = ["a", "b", "c"].iter().map(|s| s.to_string()).collect();
        put(&mut l, "c", Some(&json!({"first": true})));
        assert_eq!(l, ["c", "a", "b"]);
        put(&mut l, "c", Some(&json!({"after": "a"})));
        assert_eq!(l, ["a", "c", "b"]);
        put(&mut l, "c", None);
        assert_eq!(l, ["a", "b"]);
        put(&mut l, "z", Some(&json!({"after": "gone"})));
        assert_eq!(l, ["a", "b", "z"]);
    }

    #[test]
    fn subscribers_hear_when_a_merge_starts_changes_and_ends() {
        use crate::notify::Topic;
        let mut m = boxed();
        both_edit(&mut m, json!({"x": 15, "y": 1}), json!({"x": 12, "y": 2}));
        let mut live = crate::live::Live::new();
        crate::server::publish(&mut live, &mut m.session);
        let sub = live.hub.subscribe("p-x", &[Topic::Merge]);
        let told = |live: &mut crate::live::Live, m: &mut Merging| -> Vec<Value> {
            m.session.reload().unwrap();
            crate::server::publish(live, &mut m.session);
            sub.drain()
                .iter()
                .map(|l| serde_json::from_str::<Value>(l).unwrap()["params"].clone())
                .collect()
        };
        assert!(told(&mut live, &mut m).is_empty());
        m.merge("theirs");
        let v = told(&mut live, &mut m);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0]["merging"], true);
        assert_eq!(v[0]["status"]["conflicts"].as_array().unwrap().len(), 2);
        // Nothing changed: nothing said, and the status is kept.
        assert!(told(&mut live, &mut m).is_empty());
        let key = key(&m.session).unwrap();
        assert_eq!(m.session.merge_status.as_ref().unwrap().0, key);
        resolve(
            &mut m,
            "parts/p/features/b.json#/params/x",
            json!({"choice": "ours"}),
        )
        .unwrap();
        let v = told(&mut live, &mut m);
        assert_eq!(v[0]["status"]["conflicts"].as_array().unwrap().len(), 1);
        // Staging changes it too, then the merge ends.
        m.git(&["add", "-A"]);
        assert_eq!(told(&mut live, &mut m).len(), 1);
        m.git(&["merge", "--abort"]);
        assert_eq!(told(&mut live, &mut m), [json!({"merging": false})]);
    }

    /// Numbers `part` through the protocol.
    fn number(m: &mut Merging, part: &str, n: &str) {
        m.call(
            "props.set",
            json!({"part": part, "properties": {"number": n}}),
        )
        .unwrap();
    }

    /// The conflicts of `kind` in a status.
    fn of_kind<'a>(v: &'a Value, kind: &str) -> Vec<&'a Value> {
        v["conflicts"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|c| c["kind"] == kind)
            .collect()
    }

    #[test]
    fn a_number_both_sides_gave_is_settled_with_a_value() {
        let mut m = boxed();
        m.call("part.create", json!({"part": "q"})).unwrap();
        // The project's ID first: each branch's first PDM write would give
        // it one of its own.
        m.call("pdm.init", json!({})).unwrap();
        m.commit("a second part, and PDM");
        m.git(&["checkout", "-q", "-b", "theirs"]);
        m.session.reload().unwrap();
        number(&mut m, "q", "300");
        m.commit("theirs: q is 300");
        m.git(&["checkout", "-q", "main"]);
        m.session.reload().unwrap();
        number(&mut m, "p", "300");
        m.commit("ours: p is 300");
        m.merge("theirs");
        let v = m.call("merge.status", json!({})).unwrap();
        let c = of_kind(&v, "pdm_broke");
        let ids: Vec<(&str, &str)> = c
            .iter()
            .map(|c| (c["id"].as_str().unwrap(), c["cause"].as_str().unwrap()))
            .collect();
        let q = "parts/q/properties.json#pdm_broke/part_number_duplicate/number";
        assert_eq!(
            ids,
            [
                (
                    "parts/p/properties.json#pdm_broke/part_number_duplicate/number",
                    "theirs"
                ),
                (q, "ours"),
            ],
            "{v}"
        );
        assert_eq!(
            c[1]["concerns"],
            json!({"part": "q", "property": "number", "parts": ["p"]})
        );
        assert_eq!(
            c[1]["files"],
            json!(["parts/q/properties.json", "parts/p/properties.json"])
        );
        assert_eq!(c[1]["error"]["code"], "part_number_duplicate");
        assert_eq!(c[1]["choices"], json!(["value"]));
        assert_eq!(c[1]["repairs"], json!([]));
        assert_eq!(v["clean"], false);
        // A person picks q's number; both conflicts go.
        let r = resolve(&mut m, q, json!({"choice": "value", "value": "301"})).unwrap();
        assert_eq!(
            r["changes"],
            json!([{"path": "parts/q/properties.json", "change": "written"}])
        );
        assert_eq!(r["status"]["clean"], true, "{r}");
        let got = m.call("props.get", json!({"part": "q"})).unwrap();
        assert_eq!(got["properties"][0]["name"], "number");
        assert_eq!(got["properties"][0]["value"], "301", "{got}");
        // Only a property's conflict takes a value.
        let e = resolve(
            &mut m,
            "parts/q/properties.json#nothing",
            json!({"choice": "value", "value": 1}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::ConflictNotFound);
    }

    #[test]
    fn parts_each_side_numbered_alike_are_renumbered_by_the_repair() {
        let mut m = boxed();
        for p in ["bolt", "nut"] {
            m.call("part.create", json!({"part": p})).unwrap();
            m.call(
                "feature.add",
                json!({"part": p, "type": "box", "id": "b", "params": {"x": 1, "y": 1, "z": 1}}),
            )
            .unwrap();
        }
        m.call("part.create", json!({"part": "frame", "kind": "assembly"}))
            .unwrap();
        let place = |m: &mut Merging, inst: &str, of: &str| {
            m.call(
                "feature.add",
                json!({"part": "frame", "type": "instance", "id": inst, "of": of}),
            )
            .unwrap();
        };
        place(&mut m, "base", "p");
        m.commit("a frame");
        m.git(&["checkout", "-q", "-b", "theirs"]);
        m.session.reload().unwrap();
        place(&mut m, "n1", "nut");
        m.commit("theirs: a nut");
        m.git(&["checkout", "-q", "main"]);
        m.session.reload().unwrap();
        place(&mut m, "b1", "bolt");
        m.commit("ours: a bolt");
        m.merge("theirs");
        let items = |m: &Merging| {
            let v: Value = serde_json::from_str(
                &std::fs::read_to_string(m.root().join("parts/frame/part.json")).unwrap(),
            )
            .unwrap();
            v["items"].clone()
        };
        assert_eq!(items(&m), json!({"p": 1, "bolt": 2, "nut": 2}));
        let v = m.call("merge.status", json!({})).unwrap();
        let c = of_kind(&v, "item_collision");
        assert_eq!(c.len(), 1, "{v}");
        assert_eq!(c[0]["id"], "parts/frame/part.json#item_collision/2");
        assert_eq!(
            (&c[0]["cause"], &c[0]["concerns"]["parts"]),
            (&json!("both"), &json!(["bolt", "nut"]))
        );
        let repair = c[0]["repairs"][0]["id"].as_str().unwrap().to_owned();
        m.call(
            "merge.resolve",
            json!({"resolutions": [{"conflict": c[0]["id"], "choice": "repair", "repair": repair}]}),
        )
        .unwrap();
        assert_eq!(items(&m), json!({"p": 1, "bolt": 2, "nut": 3}));
        let v = m.call("merge.status", json!({})).unwrap();
        assert!(of_kind(&v, "item_collision").is_empty(), "{v}");
    }

    #[test]
    fn a_proposal_the_merge_left_stale_is_withdrawn() {
        let mut m = boxed();
        number(&mut m, "p", "100");
        m.commit("numbered");
        m.git(&["checkout", "-q", "-b", "theirs"]);
        m.session.reload().unwrap();
        m.call("release.propose", json!({"part": "p"})).unwrap();
        m.commit("theirs: propose p A");
        m.git(&["checkout", "-q", "main"]);
        m.session.reload().unwrap();
        m.call(
            "feature.edit",
            json!({"part": "p", "id": "b", "params": {"x": 12}}),
        )
        .unwrap();
        m.commit("ours: longer");
        m.merge("theirs");
        let v = m.call("merge.status", json!({})).unwrap();
        let c = of_kind(&v, "pdm_broke");
        assert_eq!(c.len(), 1, "{v}");
        let id = "releases/p/A.json#pdm_broke/release_stale";
        assert_eq!(c[0]["id"], id);
        assert_eq!(c[0]["cause"], "ours");
        assert_eq!(
            c[0]["files"],
            json!(["releases/p/A.json", "parts/p/features/b.json"])
        );
        assert_eq!(c[0]["concerns"], json!({"part": "p", "label": "A"}));
        assert_eq!(c[0]["error"]["details"]["changes"], json!(["content"]));
        assert_eq!(c[0]["choices"], json!([]));
        let repair = &c[0]["repairs"][0];
        let rid = repair["id"].as_str().unwrap();
        assert!(rid.starts_with(&format!("{id}:withdraw@")), "{rid}");
        assert_eq!(
            repair["changes"],
            json!([{"path": "releases/p/A.json", "action": "delete"}])
        );
        // A value is no way to settle it.
        let e = resolve(&mut m, id, json!({"choice": "value", "value": 1})).unwrap_err();
        assert_eq!(e.code, Code::InvalidParam);
        let r = resolve(&mut m, id, json!({"choice": "repair", "repair": rid})).unwrap();
        assert_eq!(
            r["changes"],
            json!([{"path": "releases/p/A.json", "change": "deleted"}])
        );
        assert_eq!(r["status"]["clean"], true, "{r}");
        // The merged part is proposed again, as it now is.
        let v = m.call("release.propose", json!({"part": "p"})).unwrap();
        assert_eq!(v["record"]["label"], "A");
    }

    #[test]
    fn concerns_name_the_model() {
        assert_eq!(
            concerns("parts/p/features/s.json", "/entities/p7"),
            json!({"part": "p", "feature": "s", "entities": ["p7"]})
        );
        assert_eq!(
            concerns("parts/p/part.json", "/features/b"),
            json!({"part": "p", "feature": "b"})
        );
        assert_eq!(
            concerns("parts/p/parameters.json", "/parameters/wall"),
            json!({"part": "p", "parameter": "wall"})
        );
        assert_eq!(concerns("ryucad.json", "/default_unit"), json!({}));
    }
}
