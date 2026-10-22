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

use crate::error::{Code, Error, Result};
use crate::git::{MergeState, Repo, Unmerged};
use crate::merge::{Conflict, Kind};
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
        let mut x = if u.ours.is_none() || u.theirs.is_none() {
            let by = if u.ours.is_none() { "ours" } else { "theirs" };
            let mut x = conflict(
                format!("{path}#"),
                "modify_delete",
                vec![path.clone()],
                concerns,
                "both",
                format!("{path} was deleted by {by} and changed by the other side"),
            );
            x["deleted_by"] = json!(by);
            x
        } else {
            let file = std::fs::read(root.join(path)).ok();
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

/// `merge.status`: what the merge in progress leaves to settle.
pub fn status(session: &mut Session) -> Result<Value> {
    let root = session.root().to_path_buf();
    let (repo, state) = merging(&root)?;
    let working = session.snapshot();
    let mut conflicts = records(&working);
    let (more, unstaged) = unmerged(&root, &repo.unmerged()?);
    conflicts.extend(more);
    conflicts.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
    let side = |c: Option<&String>| json!({ "commit": c });
    Ok(json!({
        "step": state.step.as_str(),
        "ours": side(Some(&state.ours)),
        "theirs": side(Some(&state.theirs)),
        "base": side(state.base.as_ref()),
        "clean": conflicts.is_empty(),
        "conflicts": conflicts,
        "unstaged": unstaged,
    }))
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
