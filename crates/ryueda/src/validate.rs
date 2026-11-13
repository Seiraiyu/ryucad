//! Merge validation (design 518): what a merge in progress leaves to
//! settle in a ryueda project, and settling it. `merge.status` lists the
//! conflicts in the files' records, the paths Git left unmerged (a file
//! deleted on one side and changed on the other, one merged as lines, a
//! model file), and each block whose layout the merge leaves to merge
//! (Task 13). Each item has a stable ID, its files, what it concerns as
//! reference paths, the side that caused it, its choices and its planned
//! repairs. ryueda never touches the index: once a file is settled, `git
//! add` and the commit are the agent's or the person's.

use crate::error::{Code, Error, Result};
use crate::git::Repo;
use crate::id::Id;
use crate::project::Project;
use crate::session::Session;
use ryu_git::value::{Conflict, Kind};
use ryu_git::{MergeState, Unmerged};
use ryu_session::fresh::Files;
use ryusch::layout::Layout;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::path::Path;

/// Every kind `merge.status` reports. The strings are stable.
pub const KINDS: [&str; 13] = [
    "value",
    "delete_edit",
    "order",
    "id_collision",
    "modify_delete",
    "merge_unparsed",
    "layout_pending",
    "layout_conflict",
    "layout_stale",
    "refdes_clash",
    "annotate_conflict",
    "name_clash",
    "merge_faults",
];

/// The error for a request that needs a merge when there is none.
pub fn not_merging(root: &Path) -> Error {
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
fn merging(root: &Path) -> Result<(Repo, MergeState)> {
    let repo: Repo = ryu_git::repo::repo(root)?;
    let state = repo.merge_state()?.ok_or_else(|| not_merging(root))?;
    Ok((repo, state))
}

/// The project's own files: never an alias's (`../…`), which this
/// project's merge doesn't hold.
pub fn own(files: &Files) -> Files {
    files
        .iter()
        .filter(|(p, _)| !p.starts_with("../"))
        .map(|(p, b)| (p.clone(), b.clone()))
        .collect()
}

/// What a file, and a pointer in it, are about, as stage 3's reference
/// paths: a block's component (a pin of it), net, instance, port or bus.
pub fn concerns(path: &str, pointer: &str) -> Value {
    let seg: Vec<&str> = path.trim_end_matches(".json").split('/').collect();
    let ptr: Vec<String> = pointer
        .split('/')
        .skip(1)
        .map(ryu_git::value::unescape)
        .collect();
    let first = |name: &str| match ptr.as_slice() {
        [s, id, ..] if s == name => Some(id.clone()),
        _ => None,
    };
    let mut refs = Vec::new();
    let block = match seg.as_slice() {
        ["blocks", b, "components", c] => {
            refs.push(match first("pins") {
                Some(pin) => format!("{b}/{c}/{pin}"),
                None => format!("{b}/{c}"),
            });
            Some(*b)
        }
        ["blocks", b, "nets", n] => {
            refs.push(format!("{b}/{n}"));
            Some(*b)
        }
        ["blocks", b, "instances", i] => {
            refs.push(format!("{b}/instance:{i}"));
            Some(*b)
        }
        ["blocks", b, "block"] => {
            if let Some(p) = first("ports") {
                refs.push(format!("{b}/port:{p}"));
            } else if let Some(x) = first("buses") {
                refs.push(format!("{b}/bus:{x}"));
            }
            Some(*b)
        }
        ["blocks", b, ..] => Some(*b),
        _ => None,
    };
    let mut v = json!({ "refs": refs });
    if let Some(b) = block {
        v["block"] = json!(b);
    }
    v
}

/// What `--expect` guards for a conflict ID: its block (`blocks/<b>/…`,
/// `<b>#…`, `name:<b>/…`), else the project (`refdes:…`, `name:/…`,
/// `faults:erc:…`, and every file outside `blocks/`).
pub fn scope(conflict: &str) -> String {
    if let Some(rest) = conflict.strip_prefix("name:") {
        let block = rest.split('/').next().unwrap_or_default();
        return if block.is_empty() {
            "project".to_owned()
        } else {
            block.to_owned()
        };
    }
    if conflict.starts_with("refdes:") || conflict.starts_with("faults:erc:") {
        return "project".to_owned();
    }
    let conflict = conflict.strip_prefix("faults:").unwrap_or(conflict);
    let path = conflict.split('#').next().unwrap_or_default();
    match path.strip_prefix("blocks/") {
        Some(rest) => rest.split('/').next().unwrap_or_default().to_owned(),
        // `<block>#area:<n>`, `<block>#layout`: a block ID has no `/` or `.`.
        None if !path.is_empty() && !path.contains(['/', '.']) => path.to_owned(),
        None => "project".to_owned(),
    }
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
        Kind::DeleteEdit => format!("{at}: removed on one side, and changed or used on the other"),
        Kind::Order => format!("{at}: each side puts this somewhere else in its list"),
        Kind::IdCollision => format!("{at}: each side added something different under this ID"),
        Kind::LayoutPending => format!("{at}: a layout both sides changed"),
    }
}

/// An item as `merge.status` reports it.
pub(crate) fn item(
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
pub(crate) fn json_of(bytes: &[u8]) -> Option<Value> {
    crate::canonical::parse(std::str::from_utf8(bytes).ok()?).ok()
}

/// A side's version of a file, as an item shows it: its JSON, or its text
/// when it isn't JSON; `None` when the side has no file.
fn shown(bytes: Option<&[u8]>) -> Option<Value> {
    let b = bytes?;
    Some(json_of(b).unwrap_or_else(|| Value::String(String::from_utf8_lossy(b).into_owned())))
}

/// The conflicts in the working files' records, but for a layout's
/// pending entry (Task 13 lists those by block).
fn records(working: &Files) -> Vec<Value> {
    let mut out = Vec::new();
    for (path, bytes) in working {
        let Some(v) = json_of(bytes) else { continue };
        for (pointer, c) in ryu_git::value::record(&v) {
            if c.kind == Kind::LayoutPending {
                continue;
            }
            let mut x = item(
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

/// What Git left unmerged that the records don't say: files deleted on
/// one side and changed on the other, files it merged as lines, model
/// files (binary to Git). Those now settled in the working files wait for
/// `git add` (`unstaged`).
fn unmerged(working: &Files, paths: &BTreeMap<String, Unmerged>) -> (Vec<Value>, Vec<String>) {
    let (mut conflicts, mut unstaged) = (Vec::new(), Vec::new());
    for (path, u) in paths {
        let file = working.get(path).map(|b| b.to_vec());
        let what = concerns(path, "");
        let mut x = if u.ours.is_none() || u.theirs.is_none() {
            // Deleted in the working files: settled, waiting for `git rm`.
            if file.is_none() {
                unstaged.push(path.clone());
                continue;
            }
            let by = if u.ours.is_none() { "ours" } else { "theirs" };
            let mut x = item(
                format!("{path}#"),
                "modify_delete",
                vec![path.clone()],
                what,
                "both",
                format!(
                    "{path} was deleted by {by} and changed by the other side (once settled, `git add` or `git rm` it: kept, it looks as Git left it)"
                ),
            );
            x["deleted_by"] = json!(by);
            x
        } else if path.starts_with("library/models/") {
            // Binary to Git: settled, it is either side's bytes.
            if file.is_some() && file == u.theirs && u.theirs != u.ours {
                unstaged.push(path.clone());
                continue;
            }
            let mut x = item(
                format!("{path}#"),
                "merge_unparsed",
                vec![path.clone()],
                what,
                "both",
                format!("{path} is a model file, which Git doesn't merge: take one side's bytes"),
            );
            for (k, side) in [("base", &u.base), ("ours", &u.ours), ("theirs", &u.theirs)] {
                if let Some(b) = side {
                    x[k] = json!({"blake3": blake3::hash(b).to_hex().to_string()});
                }
            }
            x["choices"] = json!(["ours", "theirs"]);
            conflicts.push(x);
            continue;
        } else {
            match file.as_deref().map(json_of) {
                Some(Some(v)) if v.get(ryu_git::value::CONFLICTS).is_some() => continue,
                Some(Some(_)) => {
                    unstaged.push(path.clone());
                    continue;
                }
                _ => item(
                    format!("{path}#"),
                    "merge_unparsed",
                    vec![path.clone()],
                    what,
                    "both",
                    format!(
                        "{path} couldn't be merged by value (a version isn't a ryueda file this build reads, or ryueda's merge driver isn't installed: `ryueda git install`), so Git merged it as lines"
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

/// Each side's files, from the merge's commits; the base's are empty
/// when the sides share no ancestor.
pub(crate) struct Sides {
    pub base: Files,
    pub ours: Files,
    pub theirs: Files,
}

pub(crate) fn sides(root: &Path, state: &MergeState) -> Result<Sides> {
    let read = |c: Option<&String>| -> Result<Files> {
        match c {
            Some(c) => Ok(own(&crate::git::revision(root, c)?.files)),
            None => Ok(Files::new()),
        }
    };
    Ok(Sides {
        base: read(state.base.as_ref())?,
        ours: read(Some(&state.ours))?,
        theirs: read(Some(&state.theirs))?,
    })
}

/// A side's layout at `path`: an empty one where the side has no file.
fn layout_on(files: &Files, path: &str) -> Result<Layout> {
    match files.get(path) {
        None => Ok(Layout::default()),
        Some(b) => crate::schem::read_layout(b, path),
    }
}

/// Block `id`'s layout merged three ways over the merged connectivity:
/// the block as ryusch reads it (the merged instructions' notes filled),
/// each side's layout, and what `ryusch::merge` makes of them.
pub(crate) struct BlockMerge {
    pub built: crate::schem::Built,
    pub base: Layout,
    pub ours: Layout,
    pub theirs: Layout,
    pub merged: ryusch::merge::Merged,
}

pub(crate) fn block_merge(p: &Project, id: &Id, sides: &Sides) -> Result<BlockMerge> {
    let path = crate::schem::layout_path(id);
    let b = p.conn.blocks.get(id).ok_or_else(|| {
        Error::new(
            Code::NotFound,
            format!("block {id} isn't in the merged project"),
        )
        .with("block", id)
    })?;
    let mut built = crate::schem::build(p, b)?;
    let base = layout_on(&sides.base, &path)?;
    let ours = layout_on(&sides.ours, &path)?;
    let theirs = layout_on(&sides.theirs, &path)?;
    let merged = ryusch::merge::merge(&built.input, &base, &ours, &theirs);
    // The merged instructions' notes, as `schem::lay` fills a file's.
    built.input.instructions = merged.layout.instructions.clone();
    let _ = crate::schem::fill_notes(p, b, &mut built.input);
    Ok(BlockMerge {
        built,
        base,
        ours,
        theirs,
        merged,
    })
}

/// `layout` for block `id`, laid out once more over `files` with it
/// written, as a later status will read it: so what a resolution writes
/// is current.
fn current(root: &Path, files: &Files, id: &Id, layout: &Layout) -> Result<Layout> {
    let mut files = files.clone();
    files.insert(
        crate::schem::layout_path(id),
        crate::schem::layout_text(layout).into_bytes().into(),
    );
    let p = project_over(root, &files)?;
    let b = p.conn.blocks.get(id).ok_or_else(|| {
        Error::new(
            Code::NotFound,
            format!("block {id} isn't in the merged project"),
        )
        .with("block", id)
    })?;
    Ok(
        crate::schem::lay(&p, b, None, &[], ryusch::input::Relayout::None)?
            .outcome
            .layout,
    )
}

/// The project loaded over `files` (a session's files with a batch's
/// changes), its aliases resolved as the manifest there names them.
pub(crate) fn project_over(root: &Path, files: &Files) -> Result<Project> {
    let manifest = files
        .get(crate::project::MANIFEST)
        .map(|b| crate::project::Manifest::read(b))
        .transpose()?
        .ok_or_else(|| Error::new(Code::ProjectNotFound, "the merge left no manifest"))?;
    Project::load_from(root, files, &crate::project::resolve(root, &manifest))
}

/// What settling a layout needs beyond the files: the project's folder,
/// the merge, and the merged project (`None` when it doesn't load).
pub(crate) struct Merging<'a> {
    pub root: &'a Path,
    pub state: &'a MergeState,
    pub project: Option<&'a Project>,
}

/// The blocks whose layout file holds a pending entry (the driver's).
fn pending(working: &Files) -> Vec<Id> {
    working
        .iter()
        .filter_map(|(path, bytes)| {
            let b = path.strip_prefix("blocks/")?.strip_suffix("/layout.json")?;
            let v = json_of(bytes)?;
            ryu_git::value::record(&v)
                .get("")
                .filter(|c| c.kind == Kind::LayoutPending)?;
            Id::parse(b, "block").ok()
        })
        .collect()
}

/// Whether the file at `path` can move block `block`'s drawing: the
/// block's own files, every block's `block.json` (an instance's ports),
/// and everything outside `blocks/` (rails, the library, the style, the
/// manifest's sheet) but ERC's, lint's and sourcing's settings and the
/// attributes.
pub(crate) fn affects(path: &str, block: &str) -> bool {
    match path.strip_prefix("blocks/") {
        Some(rest) => {
            rest.strip_prefix(block).is_some_and(|r| r.starts_with('/'))
                || rest.ends_with("/block.json")
        }
        None => !matches!(
            path,
            crate::project::ERC
                | crate::project::LINT
                | crate::project::PARTS
                | crate::project::ATTRIBUTES
        ),
    }
}

/// Whether `a` and `b` agree on every file block `block`'s drawing reads.
fn same(a: &Files, b: &Files, block: &str) -> bool {
    a.keys()
        .chain(b.keys())
        .filter(|p| affects(p, block))
        .all(|p| a.get(p) == b.get(p))
}

/// The layout items: pending, conflicting by area, or stale (see the
/// module's doc and design 518's phase 6). `blocked` when other
/// conflicts are listed: no layout merges until they settle.
fn layouts(
    root: &Path,
    state: &MergeState,
    working: &Files,
    project: Option<&Project>,
    blocked: bool,
) -> Result<Vec<Value>> {
    let pending = pending(working);
    let mut out = Vec::new();
    let waiting = |id: &Id, why: String| {
        let mut x = item(
            format!("{id}#layout"),
            "layout_pending",
            vec![crate::schem::layout_path(id)],
            json!({"block": id, "refs": []}),
            "both",
            why,
        );
        x["choices"] = json!(["ours", "theirs"]);
        x
    };
    let Some(p) = project.filter(|_| !blocked) else {
        for id in &pending {
            out.push(waiting(id, format!(
                "both sides changed block {id}'s layout: it merges once the merge's other conflicts are settled (or take one side's whole)"
            )));
        }
        return Ok(out);
    };
    if pending.is_empty() && p.conn.blocks.values().all(|b| b.layout.is_none()) {
        return Ok(out);
    }
    let sides = sides(root, state)?;
    for id in &pending {
        let path = crate::schem::layout_path(id);
        let m = match block_merge(p, id, &sides) {
            Ok(m) => m,
            Err(e) => {
                let mut x = waiting(
                    id,
                    format!(
                        "both sides changed block {id}'s layout, and the merged block can't be laid out yet: {}",
                        e.message
                    ),
                );
                x["error"] = json!(e);
                out.push(x);
                continue;
            }
        };
        if m.merged.conflicts.is_empty() {
            let mut x = item(
                format!("{id}#layout"),
                "layout_stale",
                vec![path],
                json!({"block": id, "refs": []}),
                "both",
                format!(
                    "both sides changed block {id}'s layout in separate areas: merge them over the merged connectivity (repair merge)"
                ),
            );
            x["choices"] = json!(["ours", "theirs"]);
            x["repairs"] = json!([{"id": "merge", "summary": format!(
                "merge both sides' layouts of block {id} and lay it out over them, incrementally"
            )}]);
            out.push(x);
            continue;
        }
        let ids: Vec<String> = (1..=m.merged.conflicts.len())
            .map(|n| format!("{id}#area:{n}"))
            .collect();
        for (i, c) in m.merged.conflicts.iter().enumerate() {
            let refs: BTreeSet<String> = c
                .ours
                .iter()
                .chain(&c.theirs)
                .filter_map(|k| crate::schem::key_ref(id, k))
                .collect();
            let sheets: Vec<&String> = c.areas.keys().collect();
            let mut x = item(
                ids[i].clone(),
                "layout_conflict",
                vec![path.clone()],
                json!({"block": id, "sheets": sheets, "refs": refs}),
                "both",
                format!(
                    "both sides changed this area of block {id}'s layout ({} key(s) ours, {} theirs); settle every area of the block in one resolve",
                    c.ours.len(),
                    c.theirs.len()
                ),
            );
            x["areas"] = json!(c.areas);
            x["keys"] = json!({"ours": c.ours, "theirs": c.theirs});
            x["with"] = json!(ids);
            x["choices"] = json!(["ours", "theirs"]);
            x["repairs"] = json!([{"id": "relayout", "summary":
                "lay this area out again from the base layout, with the merged connectivity and both sides' instructions"}]);
            out.push(x);
        }
    }
    // Blocks no record names whose drawing the merge moved past.
    for (id, b) in &p.conn.blocks {
        if pending.contains(id) || !matches!(b.layout, Some(Ok(_))) {
            continue;
        }
        if same(working, &sides.ours, id.as_str()) || same(working, &sides.theirs, id.as_str()) {
            continue;
        }
        let path = crate::schem::layout_path(id);
        let laid = crate::schem::lay(p, b, None, &[], ryusch::input::Relayout::None);
        if laid.as_ref().is_ok_and(|l| !l.stale && l.invalid.is_none()) {
            continue;
        }
        let kept = working.get(&path);
        let cause = if kept == sides.ours.get(&path) {
            "theirs"
        } else if kept == sides.theirs.get(&path) {
            "ours"
        } else {
            "both"
        };
        let mut x = item(
            format!("{id}#layout"),
            "layout_stale",
            vec![path],
            json!({"block": id, "refs": []}),
            cause,
            format!(
                "block {id}'s layout no longer matches the merged connectivity: lay it out again (repair relayout), or the next write will, in someone else's diff"
            ),
        );
        match laid {
            Ok(_) => {
                x["repairs"] = json!([{"id": "relayout", "summary": format!(
                    "lay block {id} out over its file, incrementally"
                )}]);
            }
            Err(e) => x["error"] = json!(e),
        }
        out.push(x);
    }
    Ok(out)
}

/// What `merge.status` is kept by: the merge Git is in, the index's
/// stamp and the working files' digest; `None` outside a repository or a
/// merge.
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

/// What a failed status is asked again on: the files Git keeps while it
/// waits, by size and time.
pub fn retry_stamp(session: &Session) -> String {
    match Repo::open(session.root()) {
        Ok(Some(repo)) => repo.merge_stamp(),
        _ => String::new(),
    }
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
    let root = session.root().to_path_buf();
    let (repo, state) = merging(&root)?;
    let full = session.snapshot();
    let working = own(&full);
    let v = findings(&root, &repo, &state, &working, &full, Some(session.project()))?;
    if let Some(k) = key {
        session.merge_status = Some((k, v.clone()));
    }
    Ok(v)
}

/// Everything that blocks the merge's commit, from the project's own
/// working files (`full`: with its aliases'); `project` is the project
/// loaded over them, `None` when it doesn't load (its manifest holds a
/// record).
fn findings(
    root: &Path,
    repo: &Repo,
    state: &MergeState,
    working: &Files,
    full: &Files,
    project: Option<&Project>,
) -> Result<Value> {
    let paths = repo.unmerged()?;
    let mut unstaged = repo.unstaged(working)?;
    let untracked = repo.untracked(working)?;
    let mut conflicts = records(working);
    let (more, settled) = unmerged(working, &paths);
    conflicts.extend(more);
    unstaged.extend(settled);
    unstaged.sort();
    unstaged.dedup();
    let blocked = !conflicts.is_empty();
    // What the merged project holds that neither side did: judged only
    // once every file merged and the project loads.
    let mut found = crate::clash::Found::default();
    if let (false, Some(p)) = (blocked, project) {
        found = crate::clash::find(root, state, p, full)?;
    }
    let held = blocked || !found.clashes.is_empty();
    conflicts.extend(std::mem::take(&mut found.clashes));
    conflicts.extend(std::mem::take(&mut found.faults));
    conflicts.extend(layouts(root, state, working, project, held)?);
    conflicts.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
    let commit = |c: Option<&String>| json!({ "commit": c });
    Ok(json!({
        "step": state.step.as_str(),
        "ours": commit(Some(&state.ours)),
        "theirs": commit(Some(&state.theirs)),
        "base": commit(state.base.as_ref()),
        "clean": conflicts.is_empty(),
        "conflicts": conflicts,
        "already_failing": found.already,
        "warnings": found.warnings,
        "unstaged": unstaged,
        "untracked": untracked,
    }))
}

/// A file's JSON from the files being written.
fn file_json(files: &Files, path: &str) -> Result<Value> {
    files
        .get(path)
        .and_then(|b| json_of(b))
        .ok_or_else(|| Error::new(Code::ConflictNotFound, format!("{path} isn't readable now")))
}

/// Settles the conflict at `pointer` of `path`'s record with `x`, in
/// `files`: the value put in place ([`crate::merge::settle`]), the entry
/// gone from the record (and the record with its last entry), the file
/// canonical at the lowest format.
pub(crate) fn settle_record(
    files: &mut Files,
    path: &str,
    pointer: &str,
    kind: Kind,
    x: Option<Value>,
) -> Result<()> {
    let v = file_json(files, path)?;
    let mut record = ryu_git::value::record(&v);
    if record.remove(pointer).is_none() {
        return Err(Error::new(
            Code::ConflictNotFound,
            format!("{path} has no conflict at {pointer} now"),
        )
        .with("conflict", format!("{path}#{pointer}")));
    }
    let mut plain = v;
    if let Some(o) = plain.as_object_mut() {
        o.remove(ryu_git::value::CONFLICTS);
    }
    let out = ryu_git::value::Merged {
        value: crate::merge::settle(&plain, pointer, kind, x),
        conflicts: record,
    }
    .to_json();
    files.insert(
        path.to_owned(),
        crate::canonical::to_string(&out).into_bytes().into(),
    );
    Ok(())
}

/// The item `id` of a status, or `conflict_not_found`.
fn find<'a>(now: &'a Value, id: &str) -> Result<&'a Value> {
    now["conflicts"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|c| c["id"] == id)
        .ok_or_else(|| {
            Error::new(
                Code::ConflictNotFound,
                format!("the merge has no conflict {id} now; run merge status"),
            )
            .with("conflict", id)
        })
}

/// Settles each of `resolutions`, in order, in `files`: every conflict
/// must be in `now` (the status) and every choice one it offers. Answers
/// the IDs settled.
fn settle(
    repo: &Repo,
    now: &Value,
    files: &mut Files,
    resolutions: &[crate::proto::Resolution],
    at: &Merging,
) -> Result<Vec<String>> {
    let unmerged = repo.unmerged()?;
    let mut resolved = Vec::new();
    // A block's areas (by number) and what each takes; a stale block's repair.
    let mut areas: BTreeMap<String, BTreeMap<usize, ryusch::merge::Repair>> = BTreeMap::new();
    let mut stale: Vec<(String, String)> = Vec::new();
    for r in resolutions {
        let found = find(now, &r.conflict)?;
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
                let kind = Kind::parse(kind).expect("a record's kind");
                settle_record(files, &path, pointer, kind, x)?;
            }
            ("modify_delete" | "merge_unparsed", side @ ("ours" | "theirs")) => {
                let bytes = match unmerged.get(&path) {
                    Some(u) if side == "ours" => u.ours.clone(),
                    Some(u) => u.theirs.clone(),
                    // Kept and staged: the file as it is now, unless the
                    // side chosen deleted it.
                    None if found["deleted_by"] == side => None,
                    None => files.get(&path).map(|b| b.to_vec()),
                };
                match bytes {
                    Some(b) => {
                        files.insert(path.clone(), b.into());
                    }
                    None => {
                        files.remove(&path);
                    }
                }
            }
            ("layout_pending" | "layout_stale", side @ ("ours" | "theirs"))
                if found["choices"]
                    .as_array()
                    .is_some_and(|c| c.iter().any(|x| x == side)) =>
            {
                let commit = if side == "ours" {
                    &at.state.ours
                } else {
                    &at.state.theirs
                };
                let rel = crate::source::join(repo.prefix().trim_end_matches('/'), &path);
                match repo.file_at(commit, &rel)? {
                    Some(b) => {
                        files.insert(path.clone(), b.into());
                    }
                    None => {
                        files.remove(&path);
                    }
                }
            }
            ("layout_conflict", "ours" | "theirs" | "repair") => {
                let repair = match (r.choice.as_str(), r.repair.as_deref()) {
                    ("ours", _) => ryusch::merge::Repair::Ours,
                    ("theirs", _) => ryusch::merge::Repair::Theirs,
                    (_, Some("relayout")) => ryusch::merge::Repair::Relayout,
                    _ => {
                        return Err(Error::new(
                            Code::RepairConflict,
                            format!(
                                "{} has no repair {:?}: its repair is relayout",
                                r.conflict, r.repair
                            ),
                        )
                        .with("conflict", &r.conflict));
                    }
                };
                let (block, n) = r.conflict.split_once("#area:").unwrap_or_default();
                let n: usize = n.parse().unwrap_or_default();
                areas.entry(block.to_owned()).or_default().insert(n, repair);
            }
            ("layout_stale", "repair") => {
                let id = r.repair.clone().unwrap_or_default();
                if !found["repairs"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .any(|x| x["id"] == id.as_str())
                {
                    return Err(Error::new(
                        Code::RepairConflict,
                        format!(
                            "no repair {id} for {} now: run merge status again",
                            r.conflict
                        ),
                    )
                    .with("repair", id));
                }
                let block = r.conflict.split('#').next().unwrap_or_default().to_owned();
                stale.push((block, id));
            }
            (_, choice) => {
                let offered: Vec<&str> = found["choices"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .chain(
                        found["repairs"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(|x| x["id"].as_str()),
                    )
                    .collect();
                return Err(Error::new(
                    Code::InvalidParam,
                    format!(
                        "{} ({kind}) can't be settled by {choice:?}; it takes {}",
                        r.conflict,
                        offered.join(", ")
                    ),
                )
                .with("conflict", &r.conflict)
                .with("choice", choice));
            }
        }
        resolved.push(r.conflict.clone());
    }
    if areas.is_empty() && stale.is_empty() {
        return Ok(resolved);
    }
    let p = at.project.ok_or_else(|| {
        Error::new(
            Code::InvalidParam,
            "a layout merges over the merged project, which doesn't load yet: settle ryueda.json first",
        )
    })?;
    let sides = sides(at.root, at.state)?;
    for (block, chosen) in &areas {
        let listed: Vec<&str> = now["conflicts"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|c| c["id"].as_str())
            .filter(|c| c.starts_with(&format!("{block}#area:")))
            .collect();
        if chosen.len() != listed.len() {
            let named: BTreeSet<String> =
                chosen.keys().map(|n| format!("{block}#area:{n}")).collect();
            let rest: Vec<&str> = listed
                .iter()
                .copied()
                .filter(|c| !named.contains(*c))
                .collect();
            return Err(Error::new(
                Code::InvalidParam,
                format!(
                    "block {block}'s layout areas are settled together: add {}",
                    rest.join(", ")
                ),
            )
            .with("block", block)
            .with("areas", &listed));
        }
        let id = Id::parse(block, "block")?;
        let m = block_merge(p, &id, &sides)?;
        if m.merged.conflicts.len() != listed.len() {
            return Err(Error::new(
                Code::RepairConflict,
                format!(
                    "block {block}'s layout merges into other areas now: run merge status again"
                ),
            )
            .with("block", block));
        }
        let repairs: Vec<ryusch::merge::Repair> = chosen.values().copied().collect();
        let out = ryusch::merge::resolve(
            &m.built.input,
            &m.base,
            &m.ours,
            &m.theirs,
            &m.merged,
            &repairs,
        )
        .map_err(|refused| crate::schem::refusal(&id, refused))?;
        let l = current(at.root, files, &id, &out.layout)?;
        files.insert(
            crate::schem::layout_path(&id),
            crate::schem::layout_text(&l).into_bytes().into(),
        );
    }
    for (block, repair) in &stale {
        let id = Id::parse(block, "block")?;
        let layout = if repair == "merge" {
            let m = block_merge(p, &id, &sides)?;
            ryusch::merge::resolve(&m.built.input, &m.base, &m.ours, &m.theirs, &m.merged, &[])
                .map_err(|refused| crate::schem::refusal(&id, refused))?
                .layout
        } else {
            let b = p.conn.blocks.get(&id).ok_or_else(|| {
                Error::new(Code::NotFound, format!("no block {block}")).with("block", block)
            })?;
            crate::schem::lay(p, b, None, &[], ryusch::input::Relayout::None)?
                .outcome
                .layout
        };
        let l = current(at.root, files, &id, &layout)?;
        files.insert(
            crate::schem::layout_path(&id),
            crate::schem::layout_text(&l).into_bytes().into(),
        );
    }
    Ok(resolved)
}

/// Writes what `files` changed from `before`, each atomically.
fn write(root: &Path, before: &Files, files: &Files) -> Result<Vec<Value>> {
    let mut changes = Vec::new();
    for (path, bytes) in files {
        if before.get(path) != Some(bytes) {
            crate::source::write_atomic(&crate::session::path_of(root, path), bytes)?;
            changes.push(json!({"path": path, "change": "written"}));
        }
    }
    for path in before.keys().filter(|p| !files.contains_key(*p)) {
        crate::source::remove_file(&crate::session::path_of(root, path))?;
        changes.push(json!({"path": path, "change": "deleted"}));
    }
    Ok(changes)
}

/// The revision `--expect` names for `scope` ([`scope`]): a block's, or
/// the project's.
fn revision_of(p: &Project, scope: &str) -> Result<String> {
    if scope == "project" {
        return Ok(p.revision.clone());
    }
    let id = crate::id::Id::parse(scope, "block")?;
    p.conn
        .blocks
        .get(&id)
        .map(|b| b.revision.clone())
        .ok_or_else(|| Error::new(Code::NotFound, format!("no block {scope}")).with("block", scope))
}

/// `merge.resolve`: settles each conflict as asked, in order, and writes
/// the files as one change, guarded by the revisions given. Every
/// conflict must be in the merge's status now, or nothing is written.
/// Answers what changed and the new status.
pub fn resolve(
    session: &mut Session,
    resolutions: &[crate::proto::Resolution],
    expected: &BTreeMap<String, String>,
) -> Result<Value> {
    for (scope, rev) in expected {
        let actual = revision_of(session.project(), scope)?;
        crate::server::check_revision(scope, &actual, Some(rev))?;
    }
    let root = session.root().to_path_buf();
    let now = status(session)?;
    let (repo, state) = merging(&root)?;
    let before = session.snapshot();
    let mut files = before.clone();
    let at = Merging {
        root: &root,
        state: &state,
        project: Some(session.project()),
    };
    let resolved = settle(&repo, &now, &mut files, resolutions, &at)?;
    let changes = write(&root, &before, &files)?;
    session.reload()?;
    Ok(json!({"resolved": resolved, "changes": changes, "status": status(session)?}))
}

/// `merge.status` or `merge.resolve` for a project that doesn't open (its
/// manifest holds a record): from the files alone, records and unmerged
/// paths only, the resolution written but not logged (no session holds
/// the project to log it against).
pub fn unloaded(root: &Path, req: crate::proto::Request) -> Result<Value> {
    // `--expect` guards a revision, and a project that doesn't open has
    // none: refused, never written unguarded.
    if let crate::proto::Request::MergeResolve {
        expected_revisions, ..
    } = &req
        && !expected_revisions.is_empty()
    {
        return Err(Error::new(
            Code::InvalidParam,
            format!(
                "--expect can't be checked while {} holds a merge's conflict record: the project has no revision until it is settled; resolve without --expect",
                crate::project::MANIFEST
            ),
        )
        .with("expected_revisions", expected_revisions));
    }
    let (repo, state) = merging(root)?;
    let working = own(&crate::session::current(root)?);
    let now = findings(root, &repo, &state, &working, &working, None)?;
    let crate::proto::Request::MergeResolve { resolutions, .. } = req else {
        return Ok(now);
    };
    let mut files = working.clone();
    let at = Merging {
        root,
        state: &state,
        project: None,
    };
    let resolved = settle(&repo, &now, &mut files, &resolutions, &at)?;
    let changes = write(root, &working, &files)?;
    let after_files = own(&crate::session::current(root)?);
    let after = findings(root, &repo, &state, &after_files, &after_files, None)?;
    Ok(json!({"resolved": resolved, "changes": changes, "status": after, "unloaded": true}))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::project::ProjectKind;
    use crate::project::tests::manifest;

    #[test]
    fn concerns_and_scopes_are_reference_paths() {
        assert_eq!(
            concerns("blocks/b/components/r1.json", "/pins/3"),
            json!({"block": "b", "refs": ["b/r1/3"]})
        );
        assert_eq!(
            concerns("blocks/b/block.json", "/ports/p1/name"),
            json!({"block": "b", "refs": ["b/port:p1"]})
        );
        assert_eq!(
            concerns("blocks/b/instances/i1.json", ""),
            json!({"block": "b", "refs": ["b/instance:i1"]})
        );
        assert_eq!(concerns("erc.json", "/severity/x"), json!({"refs": []}));
        assert_eq!(scope("blocks/b/nets/n.json#/name"), "b");
        assert_eq!(scope("b#area:2"), "b");
        assert_eq!(scope("erc.json#/severity/x"), "project");
        // Plan 4.D's items: a designator crosses blocks (the project's
        // revision), a name its block's (the project's for rails), what
        // the merge broke its file's.
        assert_eq!(scope("refdes:board/R5"), "project");
        assert_eq!(scope("name:breakout/AUX"), "breakout");
        assert_eq!(scope("name:/GND"), "project");
        assert_eq!(scope("faults:blocks/b/nets/n.json#/class"), "b");
        assert_eq!(scope("faults:erc:0123456789abcdef"), "project");
        assert_eq!(scope("faults:rails/gnd.json#/name"), "project");
        for k in ["refdes_clash", "annotate_conflict", "name_clash", "merge_faults"] {
            assert!(KINDS.contains(&k), "{k}");
        }
    }

    /// `erc.json` at a severity, as ryueda writes it.
    pub(crate) fn erc(level: &str) -> String {
        format!(
            "{{\n  \"format\": 1,\n  \"kind\": \"ryueda.erc\",\n  \"severity\": {{\n    \"pin_conflict\": \"{level}\"\n  }}\n}}\n"
        )
    }

    /// A design project in a repository, mid-merge: `erc.json` changed on
    /// both sides (`error` ours, `off` theirs, `warning` the base) and,
    /// with no driver in this clone, merged by Git as lines.
    pub(crate) fn erc_conflict(r: &Path) {
        let git = |a: &[&str]| crate::git::tests::git(r, a);
        git(&["init", "-q"]);
        manifest(r, ProjectKind::Design, &[]);
        std::fs::write(r.join("erc.json"), erc("warning")).unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "base"]);
        git(&["checkout", "-q", "-b", "other"]);
        std::fs::write(r.join("erc.json"), erc("off")).unwrap();
        git(&["commit", "-q", "-a", "-m", "theirs"]);
        git(&["checkout", "-q", "main"]);
        std::fs::write(r.join("erc.json"), erc("error")).unwrap();
        git(&["commit", "-q", "-a", "-m", "ours"]);
        let out = crate::git::tests::git_command()
            .current_dir(r)
            .args(["merge", "-q", "other"])
            .output()
            .unwrap();
        assert!(!out.status.success());
    }

    #[test]
    fn a_text_conflict_and_a_record_are_listed() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        let git = |a: &[&str]| crate::git::tests::git(r, a);
        erc_conflict(r);
        let mut s = Session::open(r).unwrap();
        let v = status(&mut s).unwrap();
        assert_eq!(
            (v["already_failing"].clone(), v["warnings"].clone()),
            (json!([]), json!([]))
        );
        assert_eq!(
            (v["step"].as_str(), v["clean"].as_bool()),
            (Some("merge"), Some(false))
        );
        let c = &v["conflicts"][0];
        assert_eq!(
            (c["id"].as_str(), c["kind"].as_str()),
            (Some("erc.json#"), Some("merge_unparsed"))
        );
        assert_eq!(c["choices"], json!(["ours", "theirs"]));
        // Asked again with nothing moved: the same answer, kept.
        assert_eq!(status(&mut s).unwrap(), v);
        // A record in the file: listed by pointer, with its sides.
        let rec = json!({"format": 1, "kind": "ryueda.erc", "severity": {"pin_conflict": "error"},
            "conflicts": {"/severity/pin_conflict": {"kind": "value", "base": "warning", "ours": "error", "theirs": "off"}}});
        std::fs::write(r.join("erc.json"), crate::canonical::to_string(&rec)).unwrap();
        s.reload().unwrap();
        let v = status(&mut s).unwrap();
        let ids: Vec<&str> = v["conflicts"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|c| c["id"].as_str())
            .collect();
        assert_eq!(ids, ["erc.json#/severity/pin_conflict"]);
        assert_eq!(v["conflicts"][0]["theirs"], "off");
        assert_eq!(
            v["conflicts"][0]["choices"],
            json!(["ours", "theirs", "value"])
        );
        // Outside a merge: not_merging.
        git(&["merge", "--abort"]);
        s.reload().unwrap();
        assert_eq!(status(&mut s).unwrap_err().code, Code::NotMerging);
    }

    fn res(conflict: &str, choice: &str) -> crate::proto::Resolution {
        crate::proto::Resolution {
            conflict: conflict.to_owned(),
            choice: choice.to_owned(),
            value: None,
            repair: None,
        }
    }

    fn record(r: &Path) {
        let rec = json!({"format": 1, "kind": "ryueda.erc", "severity": {"pin_conflict": "error"},
            "conflicts": {"/severity/pin_conflict": {"kind": "value", "base": "warning", "ours": "error", "theirs": "off"}}});
        std::fs::write(r.join("erc.json"), crate::canonical::to_string(&rec)).unwrap();
    }

    #[test]
    fn resolutions_settle_records_and_unmerged_paths() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        erc_conflict(r);
        let mut s = Session::open(r).unwrap();
        let none = BTreeMap::new();
        let e = resolve(&mut s, &[res("nope#", "ours")], &none).unwrap_err();
        assert_eq!(e.code, Code::ConflictNotFound);
        // Theirs of a file Git merged as lines: the index's stage 3.
        let v = resolve(&mut s, &[res("erc.json#", "theirs")], &none).unwrap();
        assert_eq!(v["resolved"], json!(["erc.json#"]));
        assert_eq!(
            v["changes"],
            json!([{"path": "erc.json", "change": "written"}])
        );
        assert_eq!(
            std::fs::read_to_string(r.join("erc.json")).unwrap(),
            erc("off")
        );
        assert_eq!(
            (
                v["status"]["clean"].clone(),
                v["status"]["unstaged"].clone()
            ),
            (json!(true), json!(["erc.json"]))
        );
        // A record's entry settled by a value: the record goes with it.
        record(r);
        s.reload().unwrap();
        let mut by_value = res("erc.json#/severity/pin_conflict", "value");
        by_value.value = Some(json!("warning"));
        resolve(&mut s, &[by_value], &none).unwrap();
        assert_eq!(
            std::fs::read_to_string(r.join("erc.json")).unwrap(),
            erc("warning")
        );
        // A choice the item doesn't offer, and a stale project revision.
        record(r);
        s.reload().unwrap();
        let e = resolve(
            &mut s,
            &[res("erc.json#/severity/pin_conflict", "repair")],
            &none,
        )
        .unwrap_err();
        assert_eq!(e.code, Code::InvalidParam);
        let stale = BTreeMap::from([("project".to_owned(), "0".repeat(32))]);
        let e = resolve(
            &mut s,
            &[res("erc.json#/severity/pin_conflict", "ours")],
            &stale,
        )
        .unwrap_err();
        assert_eq!(e.code, Code::Conflict);
        assert_eq!(
            std::fs::read_to_string(r.join("erc.json")).unwrap(),
            crate::canonical::to_string(
                &json!({"format": 1, "kind": "ryueda.erc", "severity": {"pin_conflict": "error"},
            "conflicts": {"/severity/pin_conflict": {"kind": "value", "base": "warning", "ours": "error", "theirs": "off"}}})
            ),
            "refused: nothing written"
        );
    }

    #[test]
    fn a_manifest_record_is_settled_without_the_project() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        erc_conflict(r);
        let mut m: Value =
            serde_json::from_slice(&std::fs::read(r.join("ryueda.json")).unwrap()).unwrap();
        m["format"] = json!(2);
        m["properties"] = json!({"a": "1"});
        m["conflicts"] =
            json!({"/properties/a": {"kind": "value", "base": "0", "ours": "1", "theirs": "2"}});
        std::fs::write(r.join("ryueda.json"), crate::canonical::to_string(&m)).unwrap();
        assert_eq!(
            Session::open(r).err().map(|e| e.code),
            Some(Code::MergeConflict)
        );
        let ask = |req| crate::server::handle_safely(&mut None, None, None, r, req);
        let v = ask(crate::proto::Request::MergeStatus {}).unwrap().result;
        let ids: Vec<&str> = v["conflicts"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|c| c["id"].as_str())
            .collect();
        assert_eq!(ids, ["erc.json#", "ryueda.json#/properties/a"]);
        // `--expect` has no revision to guard here: refused, nothing written.
        let before = std::fs::read(r.join("ryueda.json")).unwrap();
        let e = ask(crate::proto::Request::MergeResolve {
            resolutions: vec![res("ryueda.json#/properties/a", "theirs")],
            expected_revisions: BTreeMap::from([("project".to_owned(), "0".repeat(32))]),
        })
        .unwrap_err();
        assert_eq!(e.code, Code::InvalidParam, "{}", e.message);
        assert_eq!(std::fs::read(r.join("ryueda.json")).unwrap(), before);
        let v = ask(crate::proto::Request::MergeResolve {
            resolutions: vec![res("ryueda.json#/properties/a", "theirs")],
            expected_revisions: BTreeMap::new(),
        })
        .unwrap()
        .result;
        assert_eq!(v["unloaded"], true);
        let p = Project::load(r).unwrap();
        assert_eq!(p.manifest.properties["a"], "2");
    }

    #[test]
    fn what_a_blocks_layout_depends_on() {
        assert!(affects("blocks/b/components/r1.json", "b"));
        assert!(affects("blocks/c/block.json", "b"), "an instance's ports");
        assert!(!affects("blocks/c/components/r1.json", "b"));
        assert!(!affects("blocks/bb/layout.json", "b"));
        assert!(affects("rails/gnd.json", "b") && affects("../lib/library/symbols/r.json", "b"));
        assert!(!affects("erc.json", "b") && affects("style.json", "b"));
        let f = |pairs: &[(&str, &str)]| -> Files {
            pairs
                .iter()
                .map(|(p, b)| (p.to_string(), std::sync::Arc::from(b.as_bytes())))
                .collect()
        };
        let a = f(&[("blocks/b/layout.json", "1"), ("blocks/c/nets/n.json", "1")]);
        let b = f(&[("blocks/b/layout.json", "1"), ("blocks/c/nets/n.json", "2")]);
        assert!(same(&a, &b, "b") && !same(&a, &b, "c"));
        let rec = json!({"kind": "ryueda.layout", "format": 1,
            "conflicts": {"": {"kind": "layout_pending", "ours": {}, "theirs": {}}}});
        let w = f(&[
            ("blocks/b/layout.json", &crate::canonical::to_string(&rec)),
            ("blocks/c/layout.json", "{}"),
        ]);
        assert_eq!(pending(&w), [crate::id::Id::parse("b", "block").unwrap()]);
    }
}
