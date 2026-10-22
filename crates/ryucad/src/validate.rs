//! Merge validation: what a merge in progress leaves to settle, and
//! settling it. `merge.status` lists every conflict: those in the files'
//! conflict records, the paths Git left unmerged (a file deleted on one
//! side and changed on the other, one merged as lines), and, from
//! rebuilding both sides and the merge, what the merge broke that built on
//! either side. Each conflict has a stable ID, the files and the parts of
//! the model it concerns, the side that caused it, and its choices.
//!
//! ryucad never touches the index: once a file is settled, `git add` and
//! the commit are the agent's or the human's.

use crate::doc::id::Id;
use crate::error::{Code, Error, Result};
use crate::git::{MergeState, Repo, Unmerged};
use crate::merge::{Conflict, Kind};
use crate::rebuild::Output;
use crate::session::Session;
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// Every kind of conflict `merge.status` reports. The strings are stable.
pub const KINDS: [&str; 8] = [
    "value",
    "delete_edit",
    "order",
    "id_collision",
    "modify_delete",
    "merge_unparsed",
    "merge_broke",
    "sketch_stale",
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
fn shown(bytes: Option<&Vec<u8>>) -> Option<Value> {
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
    root: &std::path::Path,
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
        let file = std::fs::read(root.join(path)).ok();
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
            if let Some(s) = shown(side.as_ref()) {
                x[k] = s;
            }
        }
        x["choices"] = json!(["ours", "theirs"]);
        conflicts.push(x);
    }
    (conflicts, unstaged)
}

/// One side of a merge, rebuilt: its files, and each part's features by
/// ID with what they built and what they read.
struct Side {
    files: crate::activity::Files,
    parts: BTreeMap<Id, Built>,
}

/// A part on one side: each listed feature's result, and the features and
/// parameters each one reads.
#[derive(Default)]
struct Built {
    results: BTreeMap<Id, Result<Output>>,
    inputs: BTreeMap<Id, Vec<Id>>,
    uses: BTreeMap<Id, std::collections::BTreeSet<String>>,
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
        for (fid, f) in &p.features {
            if let Ok(f) = f {
                b.inputs.insert(
                    fid.clone(),
                    crate::graph::inputs(f).into_iter().cloned().collect(),
                );
            }
        }
        // A part whose list can't be read builds nothing.
        if p.file.is_ok() {
            b.results = s.rebuilt(&id)?.features.iter().cloned().collect();
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
        });
    };
    match session.at(Some(c)) {
        Ok((_, s)) => Ok(Side {
            files: s.snapshot(),
            parts: built(s)?,
        }),
        // A commit with no project here has nothing of it.
        Err(e) if e.code == Code::NotFound => Ok(Side {
            files: Default::default(),
            parts: BTreeMap::new(),
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
    side: &Side,
    base: &Side,
) -> Vec<String> {
    let mut out = Vec::new();
    let differs = |path: &str| side.files.get(path) != base.files.get(path);
    if differs(crate::doc::project::MANIFEST) {
        out.push(crate::doc::project::MANIFEST.to_owned());
    }
    for f in reads {
        let path = format!("parts/{part}/features/{f}.json");
        if differs(&path) {
            out.push(path);
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
    let mut causes = Vec::new();
    let mut files = Vec::new();
    for (on, other, other_side) in [("ours", "theirs", theirs), ("theirs", "ours", ours)] {
        if built_on.contains(&on) {
            let t = touched(part, &reads, &uses, other_side, base);
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
/// which don't block.
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
            let (_, failed_on) = sides(id);
            if !failed_on.is_empty() {
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
            let built = if built_on.is_empty() {
                "neither side has it".to_owned()
            } else {
                format!("it built on {}", built_on.join(" and "))
            };
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
    root: &std::path::Path,
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
                id: format!("{cid}:solve@{}", crate::doctor::basis(root, &files)),
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

/// `merge.status`: what the merge in progress leaves to settle.
pub fn status(session: &mut Session) -> Result<Value> {
    let root = session.root().to_path_buf();
    let (repo, state) = merging(&root)?;
    let working = session.snapshot();
    let mut conflicts = records(&working);
    let (more, unstaged) = unmerged(&root, &repo.unmerged()?);
    conflicts.extend(more);
    let ours = side(session, Some(&state.ours))?;
    let theirs = side(session, Some(&state.theirs))?;
    let base = side(session, state.base.as_deref())?;
    let now = built(session)?;
    let (broke, already) = broken(&now, &ours, &theirs, &base);
    conflicts.extend(broke);
    conflicts.extend(stale(&root, &now, &ours, &theirs, &base));
    conflicts.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
    let commit = |c: Option<&String>| json!({ "commit": c });
    Ok(json!({
        "step": state.step.as_str(),
        "ours": commit(Some(&state.ours)),
        "theirs": commit(Some(&state.theirs)),
        "base": commit(state.base.as_ref()),
        "clean": conflicts.is_empty(),
        "conflicts": conflicts,
        "already_failing": already,
        "unstaged": unstaged,
    }))
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
fn put(list: &mut Vec<String>, id: &str, place: Option<&Value>) {
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
    files.insert(path.to_owned(), crate::doctor::canonical_bytes(&v));
    Ok(())
}

/// Settles a file Git left unmerged with one side's version: written as
/// that side has it, or deleted where that side deleted it. A feature file
/// kept is listed where that side lists it; one deleted, unlisted.
fn settle_unmerged(
    files: &mut crate::activity::Files,
    path: &str,
    side: Option<&Vec<u8>>,
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
    files.insert(index, crate::doctor::canonical_bytes(&v));
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
            ("modify_delete" | "merge_unparsed", side @ ("ours" | "theirs")) => {
                let u = unmerged.get(&path).cloned().unwrap_or_default();
                let commit = if side == "ours" {
                    &state.ours
                } else {
                    &state.theirs
                };
                let side_files = repo.files(commit)?;
                let bytes = if side == "ours" { u.ours } else { u.theirs };
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
                            files.insert(p, text.as_bytes().to_vec());
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
        std::process::Command::new("git")
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
        // Kept looks as Git left it: s waits for `git add`, t for `git rm`.
        let v = &r["status"];
        assert_eq!(v["conflicts"][0]["id"], s);
        assert_eq!(v["unstaged"], json!(["parts/p/features/t.json"]));
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
