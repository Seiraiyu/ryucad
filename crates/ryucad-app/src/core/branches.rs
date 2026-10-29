//! The Branches tab: every worktree of the project's repository (each an
//! agent's branch, with its own daemon) as `worktree.list` gives it, and
//! for each other worktree a preview of merging its working files into
//! this window's (`merge.preview`), made again when either side's files
//! or head move, one at a time. Rows open the worktree in a new window
//! (`ryucad open`), compare with it (`compare.set` with
//! `worktree:<path>`) or list its preview's conflicts, read only: there is
//! no merge to settle yet.

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
}

/// The revision a compare with worktree `path` names.
pub fn compare_rev(path: &str) -> String {
    format!("{}{path}", ryucad::session::WORKTREE)
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
    use ryucad::error::Code;
    use serde_json::json;

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
