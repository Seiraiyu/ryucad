//! The Branches tab: every worktree of the project's repository (each an
//! agent's branch, with its own daemon) as `worktree.list` gives it, and
//! for each other worktree a preview of merging its working files into
//! this window's (`merge.preview`), made again when either side's files
//! or head move, one at a time. Rows open the worktree in a new window
//! (`ryucad open`), compare with it (`compare.set` with
//! `worktree:<path>`) or list its preview's conflicts, read only: there is
//! no merge to settle yet.
//!
//! Above the worktrees, the repository's branches (`branch.list`, design
//! 447): Check out opens a dialog naming every other participant
//! attached, then switches this folder (`branch.switch`) guarded by the
//! project revision the list was read at; Compare compares with the
//! branch; Open worktree opens the worktree that has it.

use ryucad::error::Code;
use ryucad::error::Error;
use serde_json::Value;
use std::collections::BTreeMap;
use std::time::Duration;

/// How often the tab reads the worktrees again while it is shown: another
/// agent's work changes no file of this project, so no notification says
/// so.
pub const BRANCHES_EVERY: Duration = Duration::from_secs(5);

/// A worktree's merge preview, and what it was made from.
#[derive(Clone, Debug, PartialEq)]
pub struct Preview {
    /// Both sides' heads and states when it was asked for.
    pub key: String,
    /// `merge.preview`'s answer, or why there is none.
    pub answer: Result<Value, Error>,
}

/// The Check out dialog (design 447): the branch, how it stands against
/// this worktree's head, who else is attached, and the project revision
/// the list was read at, which guards the switch.
#[derive(Clone, Debug, PartialEq)]
pub struct Checkout {
    pub branch: String,
    /// `1 ahead`, `even`: as [`standing`] says it.
    pub standing: String,
    /// Every other participant attached to this daemon, by name.
    pub attached: Vec<String>,
    /// `branch.list`'s `project`, sent as `expected_revision`.
    pub expected: Option<String>,
    /// The switch is on its way.
    pub busy: bool,
    /// Why the daemon, or the window's mode, refused it.
    pub refused: Option<Error>,
}

/// What the tab knows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Branches {
    /// `worktree.list`'s answer, once read.
    pub list: Option<Value>,
    /// Why the list couldn't be read (outside a repository, say).
    pub error: Option<Error>,
    /// Each other worktree's preview, by its project folder.
    pub previews: BTreeMap<String, Preview>,
    /// The preview on its way: the worktree and its key.
    pub previewing: Option<(String, String)>,
    /// The worktree whose preview's conflicts are listed.
    pub shown: Option<String>,
    /// `branch.list`'s answer, once read.
    pub branch_list: Option<Value>,
    /// Why the branches couldn't be read (outside a repository, say).
    pub branch_error: Option<Error>,
    /// The Check out dialog, while it is open.
    pub checkout: Option<Checkout>,
}

impl Branches {
    /// Every worktree as listed, this one among them.
    pub fn all(&self) -> &[Value] {
        self.list
            .as_ref()
            .and_then(|l| l["worktrees"].as_array())
            .map_or(&[][..], Vec::as_slice)
    }

    /// This window's worktree.
    pub fn current(&self) -> Option<&Value> {
        self.all().iter().find(|w| w["current"] == true)
    }

    /// The other worktrees, in Git's order.
    pub fn others(&self) -> impl Iterator<Item = &Value> {
        self.all().iter().filter(|w| w["current"] != true)
    }

    /// What a preview of merging `w` into this worktree is made from:
    /// both heads and both states.
    pub fn key(&self, w: &Value) -> Option<String> {
        let me = self.current()?;
        let part = |v: &Value, k: &str| v[k].as_str().unwrap_or_default().to_owned();
        Some(format!(
            "{}:{}:{}:{}",
            part(me, "head"),
            part(me, "state"),
            part(w, "head"),
            part(w, "state")
        ))
    }

    /// The next preview to ask for: the first other worktree whose preview
    /// is missing or was made from other files, while none is on its way.
    pub fn next_preview(&self) -> Option<(String, String)> {
        if self.previewing.is_some() {
            return None;
        }
        self.others().find_map(|w| {
            let path = w["path"].as_str()?.to_owned();
            let key = self.key(w)?;
            let current = self.previews.get(&path).is_some_and(|p| p.key == key);
            (!current).then_some((path, key))
        })
    }

    /// Takes a preview's answer.
    pub fn previewed(&mut self, path: String, key: String, answer: Result<Value, Error>) {
        if self
            .previewing
            .as_ref()
            .is_some_and(|(p, k)| *p == path && *k == key)
        {
            self.previewing = None;
        }
        self.previews.insert(path, Preview { key, answer });
    }

    /// Takes a new list: previews of worktrees that are gone go.
    pub fn listed(&mut self, list: Value) {
        self.error = None;
        self.list = Some(list);
        let paths: Vec<String> = self
            .others()
            .filter_map(|w| w["path"].as_str().map(str::to_owned))
            .collect();
        self.previews.retain(|p, _| paths.contains(p));
        if self.shown.as_ref().is_some_and(|s| !paths.contains(s)) {
            self.shown = None;
        }
    }

    /// The preview whose conflicts are listed, if it has an answer.
    pub fn shown_preview(&self) -> Option<&Value> {
        let p = self.previews.get(self.shown.as_ref()?)?;
        p.answer.as_ref().ok()
    }

    /// `branch.list`'s branches: local ones by name, then remote-tracking
    /// ones with no local branch of their name.
    pub fn branches(&self) -> &[Value] {
        self.branch_list
            .as_ref()
            .and_then(|l| l["branches"].as_array())
            .map_or(&[][..], Vec::as_slice)
    }

    /// The branch checked out here: `None` on a detached head, or before
    /// the list is read.
    pub fn current_branch(&self) -> Option<&str> {
        self.branch_list.as_ref()?["current"].as_str()
    }

    /// The Check out dialog for `branch`, naming `attached`: `None` for
    /// the branch checked out here, or one the list hasn't.
    pub fn checkout_of(&self, branch: &str, attached: Vec<String>) -> Option<Checkout> {
        let row = self
            .branches()
            .iter()
            .find(|b| b["name"] == branch && b["current"] != true)?;
        Some(Checkout {
            branch: branch.to_owned(),
            standing: standing(row),
            attached,
            expected: self
                .branch_list
                .as_ref()
                .and_then(|l| l["project"].as_str())
                .map(str::to_owned),
            busy: false,
            refused: None,
        })
    }

    /// The branches other than this one whose names `query` matches,
    /// best first, for File > Switch branch… in the palette.
    pub fn matching(&self, query: &str) -> Vec<String> {
        let mut scored: Vec<(i32, String)> = self
            .branches()
            .iter()
            .filter(|b| b["current"] != true)
            .filter_map(|b| {
                let n = b["name"].as_str()?;
                Some((crate::core::palette::score(query, n)?, n.to_owned()))
            })
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        scored.dedup_by(|a, b| a.1 == b.1);
        scored.into_iter().map(|(_, n)| n).collect()
    }
}

/// The revision a compare with worktree `path` names.
pub fn compare_rev(path: &str) -> String {
    format!("{}{path}", ryucad::session::WORKTREE)
}

/// A branch row's name for people: `arm`, or `wing (origin)` for a
/// branch only a remote has.
pub fn branch_label(b: &Value) -> String {
    let name = b["name"].as_str().unwrap_or_default();
    match b["remote"].as_str() {
        Some(r) => format!("{name} ({r})"),
        None => name.to_owned(),
    }
}

/// What a branch row's Compare compares with: a local branch by its name,
/// a remote's by `<remote>/<name>`.
pub fn branch_rev(b: &Value) -> String {
    let name = b["name"].as_str().unwrap_or_default();
    match b["remote"].as_str() {
        Some(r) => format!("{r}/{name}"),
        None => name.to_owned(),
    }
}

/// What the Check out dialog warns of: each other participant attached
/// (question 436: a switch is allowed while agents are attached).
pub fn attached_lines(c: &Checkout) -> Vec<String> {
    c.attached
        .iter()
        .map(|n| format!("{n} is attached and may be writing"))
        .collect()
}

/// A refused switch for people: what is in the way, and what to do.
pub fn refusal(e: &Error) -> Vec<String> {
    let detail = |k: &str, or: &str| e.details[k].as_str().unwrap_or(or).to_owned();
    match e.code {
        Code::CheckoutDirty => {
            let mut out = vec!["These project files differ from HEAD:".to_owned()];
            out.extend(
                e.details["files"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|f| f.as_str().map(|s| format!("  {s}"))),
            );
            out.push("Commit or stash them in Git first: ryucad never commits.".to_owned());
            out
        }
        Code::BranchInWorktree => vec![format!(
            "{} is checked out in the worktree at {}: open it there.",
            detail("branch", "?"),
            detail("worktree", "?")
        )],
        Code::MergeInProgress => vec![format!(
            "A {} is in progress: finish or abort it in Git first.",
            detail("step", "merge")
        )],
        Code::Conflict => vec![
            "The project changed since the list was read: look again, then check out.".to_owned(),
        ],
        _ => vec![e.message.clone()],
    }
}

/// A worktree's name for people: its branch, else `detached at <head>`.
pub fn name(w: &Value) -> String {
    match w["branch"].as_str() {
        Some(b) => b.to_owned(),
        None => {
            let head = w["head"].as_str().unwrap_or("?");
            format!("detached at {}", &head[..head.len().min(10)])
        }
    }
}

/// Who works in a worktree: the participants its running daemon has,
/// else the name `worktree add --as` recorded, `(expected)`.
pub fn who(w: &Value) -> String {
    let attached: Vec<&str> = w["daemon"]["participants"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| p["name"].as_str())
        .collect();
    if !attached.is_empty() {
        return attached.join(", ");
    }
    match w["as"].as_str() {
        Some(a) => format!("{a} (expected)"),
        None if w["daemon"]["busy"] == true => "busy".to_owned(),
        None => String::new(),
    }
}

/// How a worktree stands against this one: commits ahead and behind, and
/// its uncommitted changes: `2 ahead, 1 behind, 3 uncommitted`, or `even`.
pub fn standing(w: &Value) -> String {
    let n = |k: &str| w[k].as_u64().unwrap_or_default();
    let mut parts = Vec::new();
    if n("ahead") > 0 {
        parts.push(format!("{} ahead", n("ahead")));
    }
    if n("behind") > 0 {
        parts.push(format!("{} behind", n("behind")));
    }
    if n("changes") > 0 {
        parts.push(format!("{} uncommitted", n("changes")));
    }
    if parts.is_empty() {
        "even".to_owned()
    } else {
        parts.join(", ")
    }
}

/// A worktree's last change through its daemon, for people: `feature.edit
/// by agent-b, 2 min ago`, or nothing when its daemon isn't running.
pub fn last_change(w: &Value) -> Option<String> {
    let a = &w["daemon"]["last_activity"];
    let method = a["method"].as_str()?;
    let by = a["participant"]["name"]
        .as_str()
        .unwrap_or("an outside change");
    let ago = crate::ui::panels::ago(a["time_ms"].as_u64().unwrap_or_default());
    Some(format!("{method} by {by}, {ago}"))
}

/// A preview for people: `clean`, `2 conflicts, 1 broken feature`, or why
/// it couldn't be made.
pub fn summary(p: &Preview) -> String {
    let v = match &p.answer {
        Ok(v) => v,
        Err(e) => return format!("can't preview: {}", e.message),
    };
    if v["clean"] == true {
        return "merges cleanly".to_owned();
    }
    let conflicts = v["conflicts"].as_array().map_or(&[][..], Vec::as_slice);
    let broke = conflicts
        .iter()
        .filter(|c| c["kind"] == "merge_broke")
        .count();
    let other = conflicts.len() - broke;
    let mut parts = Vec::new();
    match other {
        0 => {}
        1 => parts.push("1 conflict".to_owned()),
        n => parts.push(format!("{n} conflicts")),
    }
    match broke {
        0 => {}
        1 => parts.push("1 broken feature".to_owned()),
        n => parts.push(format!("{n} broken features")),
    }
    parts.join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn branch_list() -> Value {
        json!({"current": "main", "head": "aaaa", "project": "r1", "branches": [
            {"name": "arm", "ref": "refs/heads/arm", "remote": null, "head": "bbbb",
             "upstream": "origin/arm", "ahead": 1, "behind": 0, "current": false, "worktree": null},
            {"name": "leg", "ref": "refs/heads/leg", "remote": null, "head": "cccc",
             "upstream": null, "ahead": 0, "behind": 2, "current": false, "worktree": "/r/leg"},
            {"name": "main", "ref": "refs/heads/main", "remote": null, "head": "aaaa",
             "upstream": null, "ahead": 0, "behind": 0, "current": true, "worktree": "/r/main"},
            {"name": "wing", "ref": "refs/remotes/origin/wing", "remote": "origin", "head": "dddd",
             "upstream": null, "ahead": 3, "behind": 0, "current": false, "worktree": null},
        ]})
    }

    #[test]
    fn branch_rows_name_compare_and_check_out() {
        let mut b = Branches::default();
        assert_eq!(b.current_branch(), None);
        assert!(b.branches().is_empty());
        b.branch_list = Some(branch_list());
        assert_eq!(b.current_branch(), Some("main"));
        let rows = b.branches();
        assert_eq!(rows.len(), 4);
        assert_eq!(branch_label(&rows[0]), "arm");
        assert_eq!(branch_label(&rows[3]), "wing (origin)");
        assert_eq!(branch_rev(&rows[0]), "arm");
        assert_eq!(branch_rev(&rows[3]), "origin/wing");
        assert_eq!(standing(&rows[0]), "1 ahead");
        assert_eq!(standing(&rows[1]), "2 behind");
        // The palette's list: every branch but this one, best match first.
        assert_eq!(b.matching(""), ["arm", "leg", "wing"]);
        assert_eq!(b.matching("w"), ["wing"]);
        // The dialog: not for this branch, nor one not listed.
        assert!(b.checkout_of("main", vec![]).is_none());
        assert!(b.checkout_of("nope", vec![]).is_none());
        let c = b.checkout_of("arm", vec!["claude-1".into()]).unwrap();
        assert_eq!(
            (
                c.standing.as_str(),
                c.expected.as_deref(),
                c.busy,
                c.refused.is_none()
            ),
            ("1 ahead", Some("r1"), false, true)
        );
        assert_eq!(
            attached_lines(&c),
            ["claude-1 is attached and may be writing"]
        );
    }

    #[test]
    fn a_refusal_says_what_to_do() {
        let dirty = Error::new(Code::CheckoutDirty, "1 project file(s) differ from HEAD")
            .with("files", ["parts/p/features/b.json"]);
        assert_eq!(
            refusal(&dirty),
            [
                "These project files differ from HEAD:",
                "  parts/p/features/b.json",
                "Commit or stash them in Git first: ryucad never commits.",
            ]
        );
        let held = Error::new(Code::BranchInWorktree, "held")
            .with("branch", "leg")
            .with("worktree", "/r/leg");
        assert_eq!(
            refusal(&held),
            ["leg is checked out in the worktree at /r/leg: open it there."]
        );
        let merging = Error::new(Code::MergeInProgress, "merging").with("step", "cherry-pick");
        assert_eq!(
            refusal(&merging),
            ["A cherry-pick is in progress: finish or abort it in Git first."]
        );
        let stale = Error::new(Code::Conflict, "the project is at r2");
        assert_eq!(
            refusal(&stale),
            ["The project changed since the list was read: look again, then check out."]
        );
        let other = Error::new(Code::GitFailed, "git switch arm failed: boom");
        assert_eq!(refusal(&other), ["git switch arm failed: boom"]);
    }

    fn list() -> Value {
        json!({"worktrees": [
            {"path": "/r/main", "current": true, "branch": "main", "head": "aaaa", "state": "s1",
             "ahead": 0, "behind": 0, "changes": 0, "daemon": {"running": true,
             "participants": [{"name": "Dana (window 1)"}]}},
            {"path": "/r/arm", "current": false, "branch": "arm", "head": "bbbb", "state": "s2",
             "ahead": 2, "behind": 1, "changes": 3, "as": "agent-b",
             "daemon": {"running": true, "participants": [{"name": "agent-b"}],
                        "last_activity": {"method": "feature.edit", "time_ms": 0,
                                          "participant": {"name": "agent-b"}}}},
            {"path": "/r/leg", "current": false, "branch": null, "head": "cccccccccccccccc",
             "state": "s3", "ahead": 0, "behind": 0, "changes": 0, "as": "agent-c",
             "daemon": null},
        ]})
    }

    #[test]
    fn rows_say_who_works_where_and_how_it_stands() {
        let mut b = Branches::default();
        b.listed(list());
        assert_eq!(b.current().unwrap()["branch"], "main");
        let others: Vec<&Value> = b.others().collect();
        assert_eq!(others.len(), 2);
        let (arm, leg) = (others[0], others[1]);
        assert_eq!(name(arm), "arm");
        assert_eq!(name(leg), "detached at cccccccccc");
        assert_eq!(who(arm), "agent-b");
        assert_eq!(who(leg), "agent-c (expected)");
        assert_eq!(standing(arm), "2 ahead, 1 behind, 3 uncommitted");
        assert_eq!(standing(leg), "even");
        assert!(
            last_change(arm)
                .unwrap()
                .starts_with("feature.edit by agent-b, ")
        );
        assert_eq!(last_change(leg), None);
        assert_eq!(compare_rev("/r/arm"), "worktree:/r/arm");
    }

    #[test]
    fn previews_are_asked_for_one_at_a_time_and_again_when_files_move() {
        let mut b = Branches::default();
        b.listed(list());
        let (path, key) = b.next_preview().unwrap();
        assert_eq!((path.as_str(), key.as_str()), ("/r/arm", "aaaa:s1:bbbb:s2"));
        b.previewing = Some((path.clone(), key.clone()));
        assert_eq!(b.next_preview(), None);
        b.previewed(path, key, Ok(json!({"clean": true, "conflicts": []})));
        let (path, key) = b.next_preview().unwrap();
        assert_eq!(path, "/r/leg");
        b.previewed(
            path,
            key,
            Err(Error::new(
                Code::InvalidParam,
                "that worktree has no commits yet",
            )),
        );
        assert_eq!(b.next_preview(), None);
        // This worktree's files moved: every preview is asked for again.
        let mut l = list();
        l["worktrees"][0]["state"] = json!("s9");
        b.listed(l);
        assert_eq!(b.next_preview().unwrap().1, "aaaa:s9:bbbb:s2");
        // A worktree that is gone takes its preview with it.
        b.shown = Some("/r/leg".into());
        let mut l = list();
        l["worktrees"].as_array_mut().unwrap().pop();
        b.listed(l);
        assert!(!b.previews.contains_key("/r/leg"));
        assert_eq!(b.shown, None);
    }

    #[test]
    fn a_preview_says_what_a_merge_would_leave() {
        let p = |answer| Preview {
            key: String::new(),
            answer,
        };
        assert_eq!(summary(&p(Ok(json!({"clean": true})))), "merges cleanly");
        let v = json!({"clean": false, "conflicts": [
            {"kind": "value"}, {"kind": "merge_broke"}, {"kind": "order"}]});
        assert_eq!(summary(&p(Ok(v))), "2 conflicts, 1 broken feature");
        let e = Error::new(Code::RevNotFound, "no revision x");
        assert_eq!(summary(&p(Err(e))), "can't preview: no revision x");
    }
}
