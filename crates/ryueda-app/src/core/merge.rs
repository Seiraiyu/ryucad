//! A merge in progress as the window knows it (design 688, Conflict
//! panel): `merge.status`'s answer, read when the window opens and again
//! whenever the session announces it moved (`merge.changed`), and what the
//! panel's last resolve met. ryueda never stages or commits: once settled,
//! `git add` and the commit are the person's.

use ryu_doc::error::{Code, Error};
use serde_json::Value;

/// The merge as the window knows it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Merge {
    /// `merge.status`'s answer; `null` when it couldn't be made.
    pub status: Value,
    /// Why there is no status.
    pub error: Option<Error>,
    /// A resolve is on its way.
    pub resolving: bool,
    /// The conflicts the last resolve named, and why it was refused: kept
    /// on their rows until the next resolve.
    pub refused: Option<(Vec<String>, Error)>,
}

fn strings(v: &Value) -> Vec<&str> {
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect()
}

impl Merge {
    pub fn of(status: Value) -> Merge {
        Merge {
            status,
            ..Merge::default()
        }
    }

    pub fn broken(e: Error) -> Merge {
        Merge {
            error: Some(e),
            ..Merge::default()
        }
    }

    /// This merge, keeping what the window had in hand: a resolve on its
    /// way and the last refusal.
    pub fn keeping(mut self, old: Option<&Merge>) -> Merge {
        if let Some(old) = old {
            self.resolving = old.resolving;
            self.refused = old.refused.clone();
        }
        self
    }

    /// `merge.status`'s answer: no merge outside one (`not_merging`) or
    /// outside a repository (`not_a_git_repo`).
    pub fn answered(old: Option<&Merge>, a: Result<Value, Error>) -> Option<Merge> {
        match a {
            Ok(v) => Some(Merge::of(v).keeping(old)),
            Err(e) if matches!(e.code, Code::NotMerging | Code::NotAGitRepo) => None,
            Err(e) => Some(Merge::broken(e).keeping(old)),
        }
    }

    /// `merge.changed`'s params.
    pub fn noted(old: Option<&Merge>, params: &Value) -> Option<Merge> {
        if params["merging"] != true {
            return None;
        }
        if let Some(e) = params.get("error") {
            let e = serde_json::from_value(e.clone())
                .unwrap_or_else(|_| Error::new(Code::Internal, e.to_string()));
            return Some(Merge::broken(e).keeping(old));
        }
        Some(Merge::of(params["status"].clone()).keeping(old))
    }

    /// What Git is in the middle of: `merge`, `cherry-pick`, `revert` or
    /// `rebase`.
    pub fn step(&self) -> &str {
        self.status["step"].as_str().unwrap_or("merge")
    }

    pub fn conflicts(&self) -> &[Value] {
        self.status["conflicts"]
            .as_array()
            .map_or(&[][..], Vec::as_slice)
    }

    pub fn conflict(&self, id: &str) -> Option<&Value> {
        self.conflicts().iter().find(|c| c["id"] == id)
    }

    pub fn clean(&self) -> bool {
        self.error.is_none() && self.status["clean"] == true
    }

    pub fn unstaged(&self) -> Vec<&str> {
        strings(&self.status["unstaged"])
    }

    /// New project files the index doesn't hold at all.
    pub fn untracked(&self) -> Vec<&str> {
        strings(&self.status["untracked"])
    }

    /// What the merge settled that waits for `git add`.
    pub fn settled(&self) -> Vec<&str> {
        let untracked = self.untracked();
        self.unstaged()
            .into_iter()
            .filter(|f| !untracked.contains(f))
            .collect()
    }

    /// Why the last resolve was refused, when it named `id`.
    pub fn refused_for(&self, id: &str) -> Option<&Error> {
        self.refused
            .as_ref()
            .filter(|(ids, _)| ids.iter().any(|x| x == id))
            .map(|(_, e)| e)
    }

    /// The status line's words: `3 to settle · 2 to stage`.
    pub fn summary(&self) -> String {
        if self.error.is_some() {
            return format!("{}: can't tell what is left", self.step());
        }
        let mut parts = Vec::new();
        let (n, s) = (self.conflicts().len(), self.settled().len());
        if n > 0 {
            parts.push(format!("{n} to settle"));
        }
        if s > 0 {
            parts.push(format!("{s} to stage"));
        }
        if parts.is_empty() {
            "settled: ready to commit".to_owned()
        } else {
            parts.join(" · ")
        }
    }
}
#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use ryu_doc::error::Code;
    use serde_json::json;

    /// A status with one item of every kind the panel shows, in the
    /// daemon's shapes (`validate.rs`, `clash.rs`).
    pub(crate) fn status() -> Value {
        json!({
            "step": "merge",
            "ours": {"commit": "a".repeat(40)}, "theirs": {"commit": "b".repeat(40)},
            "base": {"commit": "c".repeat(40)},
            "clean": false,
            "conflicts": [
                {"id": "blocks/breakout/components/r1.json#/value", "kind": "value",
                 "files": ["blocks/breakout/components/r1.json"],
                 "concerns": {"block": "breakout", "refs": ["breakout/r1"]}, "cause": "both",
                 "message": "blocks/breakout/components/r1.json at /value: changed differently on each side",
                 "pointer": "/value", "base": "4k7", "ours": "10k", "theirs": "47k",
                 "choices": ["ours", "theirs", "value"], "repairs": []},
                {"id": "breakout#area:1", "kind": "layout_conflict", "files": ["blocks/breakout/layout.json"],
                 "concerns": {"block": "breakout", "sheets": ["s1"], "refs": ["breakout/c97", "breakout/c98"]},
                 "cause": "both", "message": "both sides changed this area", "with": ["breakout#area:1", "breakout#area:2"],
                 "choices": ["ours", "theirs"], "repairs": [{"id": "relayout", "summary": "lay this area out again"}]},
                {"id": "breakout#area:2", "kind": "layout_conflict", "files": ["blocks/breakout/layout.json"],
                 "concerns": {"block": "breakout", "sheets": ["s2"], "refs": []},
                 "cause": "both", "message": "both sides changed this area", "with": ["breakout#area:1", "breakout#area:2"],
                 "choices": ["ours", "theirs"], "repairs": [{"id": "relayout", "summary": "lay this area out again"}]},
                {"id": "faults:erc:e1", "kind": "merge_faults", "files": ["blocks/breakout/components/u1.json"],
                 "concerns": {"design": "board", "refs": ["breakout/u1"]}, "cause": "theirs",
                 "message": "ERC finds what neither side had: pin conflict", "choices": [], "repairs": []},
                {"id": "name:breakout/AUX", "kind": "name_clash", "files": ["blocks/breakout/nets/aux_o.json", "blocks/breakout/nets/aux_t.json"],
                 "concerns": {"block": "breakout", "refs": ["breakout/aux_o", "breakout/aux_t"]}, "cause": "both",
                 "message": "breakout/aux_o, breakout/aux_t take the name AUX", "name": "AUX", "choices": [],
                 "repairs": [{"id": "join", "net": "breakout/aux_t", "into": "aux_o", "summary": "theirs' net joins ours'"},
                             {"id": "rename", "renames": [{"path": "blocks/breakout/nets/aux_t.json", "pointer": "/name", "to": "AUX_2"}],
                              "summary": "theirs' take the next free name"}]},
                {"id": "refdes:board/R5", "kind": "refdes_clash", "files": ["blocks/breakout/components/r8.json", "blocks/breakout/components/r9.json"],
                 "concerns": {"design": "board", "refs": ["breakout/r8", "breakout/r9"]}, "cause": "both",
                 "message": "2 occurrences share R5 in the merge", "refdes": "R5", "choices": [],
                 "repairs": [{"id": "renumber", "changes": [{"component": "breakout/r9", "refdes": "R6"}],
                              "summary": "give theirs the next free designators"},
                             {"id": "annotate", "changes": [{"component": "breakout/r8", "refdes": "R5"}, {"component": "breakout/r9", "refdes": "R6"}],
                              "summary": "annotate them in canvas order"}]}
            ],
            "already_failing": [{"code": "erc_floating", "message": "a floating input"}],
            "warnings": [],
            "unstaged": ["blocks/breakout/nets/sda.json", "blocks/breakout/new.json"],
            "untracked": ["blocks/breakout/new.json"]
        })
    }

    #[test]
    fn a_status_read_and_summed_up() {
        let m = Merge::of(status());
        assert_eq!(m.step(), "merge");
        assert_eq!(m.conflicts().len(), 6);
        assert!(!m.clean());
        assert_eq!(m.settled(), ["blocks/breakout/nets/sda.json"]);
        assert_eq!(m.untracked(), ["blocks/breakout/new.json"]);
        assert_eq!(m.summary(), "6 to settle · 1 to stage");
        assert!(m.conflict("refdes:board/R5").is_some());
        let mut done = status();
        done["conflicts"] = json!([]);
        done["clean"] = json!(true);
        done["unstaged"] = json!([]);
        done["untracked"] = json!([]);
        assert_eq!(Merge::of(done).summary(), "settled: ready to commit");
        let broken = Merge::broken(Error::new(Code::Internal, "x"));
        assert_eq!(broken.summary(), "merge: can't tell what is left");
    }

    #[test]
    fn answers_and_notes_start_keep_and_end_a_merge() {
        let none = Merge::answered(None, Err(Error::new(Code::NotMerging, "no")));
        assert_eq!(none, None);
        let none = Merge::answered(None, Err(Error::new(Code::NotAGitRepo, "no")));
        assert_eq!(none, None);
        let mut m = Merge::answered(None, Ok(status())).unwrap();
        m.resolving = true;
        m.refused = Some((
            vec!["refdes:board/R5".into()],
            Error::new(Code::Conflict, "moved"),
        ));
        // A status the session publishes keeps what the window had.
        let kept = Merge::noted(Some(&m), &json!({"merging": true, "status": status()})).unwrap();
        assert!(kept.resolving);
        assert_eq!(
            kept.refused_for("refdes:board/R5").unwrap().code,
            Code::Conflict
        );
        assert!(kept.refused_for("name:breakout/AUX").is_none());
        let e = Merge::noted(
            Some(&m),
            &json!({"merging": true, "error": {"code": "internal", "message": "x"}}),
        )
        .unwrap();
        assert_eq!(e.error.unwrap().code, Code::Internal);
        assert_eq!(Merge::noted(Some(&m), &json!({"merging": false})), None);
        let other = Merge::answered(Some(&m), Err(Error::new(Code::Internal, "x"))).unwrap();
        assert!(other.error.is_some() && other.resolving);
    }
}
