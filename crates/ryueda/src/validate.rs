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
use crate::project::Project;
use crate::session::Session;
use ryu_git::value::{Conflict, Kind};
use ryu_git::{MergeState, Unmerged};
use ryu_session::fresh::Files;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::Path;

/// Every kind `merge.status` reports. The strings are stable.
pub const KINDS: [&str; 9] = [
    "value",
    "delete_edit",
    "order",
    "id_collision",
    "modify_delete",
    "merge_unparsed",
    "layout_pending",
    "layout_conflict",
    "layout_stale",
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
pub(crate) fn own(files: &Files) -> Files {
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
/// `<b>#…`), else the project.
pub fn scope(conflict: &str) -> String {
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
fn json_of(bytes: &[u8]) -> Option<Value> {
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
    let working = own(&session.snapshot());
    let v = findings(&root, &repo, &state, &working, Some(session.project()))?;
    if let Some(k) = key {
        session.merge_status = Some((k, v.clone()));
    }
    Ok(v)
}

/// Everything that blocks the merge's commit, from the project's own
/// working files; `project` is the project loaded over them, `None` when
/// it doesn't load (its manifest holds a record).
fn findings(
    root: &Path,
    repo: &Repo,
    state: &MergeState,
    working: &Files,
    project: Option<&Project>,
) -> Result<Value> {
    let _ = (root, project);
    let paths = repo.unmerged()?;
    let mut unstaged = repo.unstaged(working)?;
    let untracked = repo.untracked(working)?;
    let mut conflicts = records(working);
    let (more, settled) = unmerged(working, &paths);
    conflicts.extend(more);
    unstaged.extend(settled);
    unstaged.sort();
    unstaged.dedup();
    conflicts.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
    let commit = |c: Option<&String>| json!({ "commit": c });
    Ok(json!({
        "step": state.step.as_str(),
        "ours": commit(Some(&state.ours)),
        "theirs": commit(Some(&state.theirs)),
        "base": commit(state.base.as_ref()),
        "clean": conflicts.is_empty(),
        "conflicts": conflicts,
        "unstaged": unstaged,
        "untracked": untracked,
    }))
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
}
