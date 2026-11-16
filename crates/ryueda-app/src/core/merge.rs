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
/// One thing a row offers: a side taken, or a planned repair.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Action {
    pub label: String,
    /// `ours`, `theirs` or `repair`, as `merge.resolve` takes it.
    pub choice: String,
    pub repair: Option<String>,
    pub recommended: bool,
    /// What it will write, from the status's plan: its summary first.
    pub plan: Vec<String>,
}

/// One item of `merge.status`, as the panel lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub message: String,
    pub cause: String,
    pub files: Vec<String>,
    pub refs: Vec<String>,
    /// The block it concerns, or its references' one block.
    pub block: Option<String>,
    /// A layout area's sheets.
    pub sheets: Vec<String>,
    /// `(side, shortened, in full)` for base, ours and theirs.
    pub values: Vec<(&'static str, String, String)>,
    pub actions: Vec<Action>,
    /// The CLI's line for a value of one's own, when one is taken.
    pub typed: Option<String>,
    /// What a person does when no action here settles it.
    pub hint: Option<String>,
    /// A layout area's fellow areas, itself included: settled together.
    pub with: Vec<String>,
}

impl Row {
    /// Whether Show has something to show.
    pub fn showable(&self) -> bool {
        !self.refs.is_empty() || self.block.is_some()
    }
}

fn owned(v: &Value) -> Vec<String> {
    strings(v).into_iter().map(str::to_owned).collect()
}

/// A value as a row shows it: text bare, else its JSON, cut at 40
/// characters; and in full.
pub fn short(v: &Value) -> (String, String) {
    let full = match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    let short = if full.chars().count() > 40 {
        format!("{}…", full.chars().take(39).collect::<String>())
    } else {
        full.clone()
    };
    (short, full)
}

/// The block an item concerns: its own, or its references' when they
/// share one.
fn block_of(c: &Value) -> Option<String> {
    if let Some(b) = c["concerns"]["block"].as_str() {
        return Some(b.to_owned());
    }
    let refs = owned(&c["concerns"]["refs"]);
    let first = refs.first()?.split('/').next()?.to_owned();
    refs.iter()
        .all(|r| r.split('/').next() == Some(first.as_str()))
        .then_some(first)
}

fn label(repair: &str) -> String {
    match repair {
        "relayout" => "Relay out".to_owned(),
        "merge" => "Merge layouts".to_owned(),
        other => {
            let mut c = other.chars();
            c.next()
                .map(|f| f.to_uppercase().chain(c).collect())
                .unwrap_or_default()
        }
    }
}

/// What a repair will write, from the status's plan: its summary, then a
/// line per change it names.
fn plan(c: &Value, r: &Value) -> Vec<String> {
    let mut out: Vec<String> = r["summary"]
        .as_str()
        .map(str::to_owned)
        .into_iter()
        .collect();
    let text = |v: &Value| v.as_str().unwrap_or_default().to_owned();
    match r["id"].as_str().unwrap_or_default() {
        "renumber" | "annotate" => {
            for x in r["changes"].as_array().into_iter().flatten() {
                let (comp, to) = (text(&x["component"]), text(&x["refdes"]));
                out.push(match c["refdes"].as_str() {
                    Some(from) => format!("{comp}: {from} → {to}"),
                    None => format!("{comp} → {to}"),
                });
            }
        }
        "join" => out.push(format!(
            "theirs' {} joins ours' {}",
            text(&r["net"]),
            text(&r["into"])
        )),
        "rename" => {
            for x in r["renames"].as_array().into_iter().flatten() {
                out.push(format!("theirs' {} → {}", text(&c["name"]), text(&x["to"])));
            }
        }
        "restore" => {
            for f in owned(&r["files"]) {
                out.push(format!("brings back {f}"));
            }
        }
        "relayout" if c["kind"] == "layout_conflict" => {
            if let Some((b, n)) = c["id"].as_str().and_then(|i| i.split_once("#area:")) {
                out.push(format!("relays out area {n} of {b}"));
            }
        }
        _ => {}
    }
    out
}

fn row(c: &Value) -> Row {
    let id = c["id"].as_str().unwrap_or_default().to_owned();
    let kind = c["kind"].as_str().unwrap_or("conflict").to_owned();
    let files = owned(&c["files"]);
    let block = block_of(c);
    let at = block
        .clone()
        .or_else(|| files.first().cloned())
        .unwrap_or_else(|| id.clone());
    let choices = owned(&c["choices"]);
    let mut actions = Vec::new();
    for (side, text) in [("ours", "Take ours"), ("theirs", "Take theirs")] {
        if choices.iter().any(|x| x == side) {
            actions.push(Action {
                label: text.to_owned(),
                choice: side.to_owned(),
                repair: None,
                recommended: false,
                plan: Vec::new(),
            });
        }
    }
    for (i, r) in c["repairs"].as_array().into_iter().flatten().enumerate() {
        let Some(rid) = r["id"].as_str() else {
            continue;
        };
        actions.push(Action {
            label: label(rid),
            choice: "repair".to_owned(),
            repair: Some(rid.to_owned()),
            recommended: i == 0,
            plan: plan(c, r),
        });
    }
    let values = ["base", "ours", "theirs"]
        .into_iter()
        .filter_map(|k| c.get(k).map(|v| (k, short(v))))
        .map(|(k, (s, f))| (k, s, f))
        .collect();
    let typed = choices
        .iter()
        .any(|x| x == "value")
        .then(|| format!("ryueda merge resolve '{id}' --value '<JSON>'"));
    let hint = match kind.as_str() {
        "merge_faults" => Some(
            "Fixed by editing: Show takes you there, and the status is read again as the files change."
                .to_owned(),
        ),
        "annotate_conflict" => Some(
            "Unlock all but one of them (`ryueda component edit <component> --unlock`); the status is read again."
                .to_owned(),
        ),
        "pdm_broke" if actions.is_empty() => Some(
            "Set the property with `ryueda props set`; the status is read again.".to_owned(),
        ),
        _ => None,
    };
    Row {
        title: format!("{kind} in {at}"),
        message: c["message"].as_str().unwrap_or_default().to_owned(),
        cause: c["cause"].as_str().unwrap_or_default().to_owned(),
        refs: owned(&c["concerns"]["refs"]),
        sheets: owned(&c["concerns"]["sheets"]),
        with: owned(&c["with"]),
        id,
        kind,
        files,
        block,
        values,
        actions,
        typed,
        hint,
    }
}

/// Every item of a status, in its order.
pub fn rows(status: &Value) -> Vec<Row> {
    status["conflicts"]
        .as_array()
        .into_iter()
        .flatten()
        .map(row)
        .collect()
}

/// What an area of a block's layout takes in its group's Apply.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Pick {
    #[default]
    Relayout,
    Ours,
    Theirs,
}

impl Pick {
    fn resolution(self) -> (String, Option<String>) {
        match self {
            Pick::Relayout => ("repair".to_owned(), Some("relayout".to_owned())),
            Pick::Ours => ("ours".to_owned(), None),
            Pick::Theirs => ("theirs".to_owned(), None),
        }
    }
}

/// A block's layout areas, settled together.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Group {
    pub block: String,
    pub areas: Vec<Row>,
}

/// What one resolve settles: each conflict's ID, choice and repair.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Batch {
    pub items: Vec<(String, String, Option<String>)>,
}

impl Batch {
    /// A row's action; a layout area's takes every area of its block.
    pub fn one(r: &Row, a: &Action) -> Batch {
        let ids = if r.with.is_empty() {
            vec![r.id.clone()]
        } else {
            r.with.clone()
        };
        Batch {
            items: ids
                .into_iter()
                .map(|id| (id, a.choice.clone(), a.repair.clone()))
                .collect(),
        }
    }

    /// Relay out every area of a block.
    pub fn relayout_all(g: &Group) -> Batch {
        Batch::areas(g, &std::collections::BTreeMap::new())
    }

    /// Each area as picked (relaid out unless picked otherwise).
    pub fn areas(g: &Group, picks: &std::collections::BTreeMap<String, Pick>) -> Batch {
        Batch {
            items: g
                .areas
                .iter()
                .map(|r| {
                    let (choice, repair) =
                        picks.get(&r.id).copied().unwrap_or_default().resolution();
                    (r.id.clone(), choice, repair)
                })
                .collect(),
        }
    }

    pub fn ids(&self) -> Vec<String> {
        self.items.iter().map(|(id, ..)| id.clone()).collect()
    }
}

/// The rows but layout areas, and the areas by block, in the status's
/// order.
pub fn groups(rows: Vec<Row>) -> (Vec<Row>, Vec<Group>) {
    let mut singles = Vec::new();
    let mut groups: Vec<Group> = Vec::new();
    for r in rows {
        if r.kind != "layout_conflict" {
            singles.push(r);
            continue;
        }
        let block = r
            .block
            .clone()
            .unwrap_or_else(|| r.id.split('#').next().unwrap_or_default().to_owned());
        match groups.iter_mut().find(|g| g.block == block) {
            Some(g) => g.areas.push(r),
            None => groups.push(Group {
                block,
                areas: vec![r],
            }),
        }
    }
    (singles, groups)
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

    #[test]
    fn every_kind_becomes_a_row_with_its_actions_in_order() {
        let rows = rows(&status());
        let by = |id: &str| rows.iter().find(|r| r.id == id).unwrap().clone();
        let v = by("blocks/breakout/components/r1.json#/value");
        assert_eq!(v.title, "value in breakout");
        assert_eq!(v.refs, ["breakout/r1"]);
        assert_eq!(
            v.values,
            [
                ("base", "4k7".to_owned(), "4k7".to_owned()),
                ("ours", "10k".to_owned(), "10k".to_owned()),
                ("theirs", "47k".to_owned(), "47k".to_owned()),
            ]
        );
        let labels: Vec<&str> = v.actions.iter().map(|a| a.label.as_str()).collect();
        assert_eq!(labels, ["Take ours", "Take theirs"]);
        assert!(v.actions.iter().all(|a| !a.recommended));
        assert_eq!(
            v.typed.as_deref(),
            Some(
                "ryueda merge resolve 'blocks/breakout/components/r1.json#/value' --value '<JSON>'"
            )
        );
        let r = by("refdes:board/R5");
        assert_eq!(r.title, "refdes_clash in breakout", "refs' one block");
        let labels: Vec<(&str, bool)> = r
            .actions
            .iter()
            .map(|a| (a.label.as_str(), a.recommended))
            .collect();
        assert_eq!(labels, [("Renumber", true), ("Annotate", false)]);
        assert_eq!(
            r.actions[0].plan,
            [
                "give theirs the next free designators",
                "breakout/r9: R5 → R6"
            ]
        );
        assert_eq!(
            (r.actions[0].choice.as_str(), r.actions[0].repair.as_deref()),
            ("repair", Some("renumber"))
        );
        let n = by("name:breakout/AUX");
        assert_eq!(
            n.actions[0].plan[1],
            "theirs' breakout/aux_t joins ours' aux_o"
        );
        assert_eq!(n.actions[1].plan[1], "theirs' AUX → AUX_2");
        let a = by("breakout#area:2");
        assert_eq!(a.sheets, ["s2"]);
        assert_eq!(a.with, ["breakout#area:1", "breakout#area:2"]);
        let labels: Vec<&str> = a.actions.iter().map(|a| a.label.as_str()).collect();
        assert_eq!(labels, ["Take ours", "Take theirs", "Relay out"]);
        assert_eq!(a.actions[2].plan[1], "relays out area 2 of breakout");
        let f = by("faults:erc:e1");
        assert!(f.actions.is_empty() && f.typed.is_none());
        assert!(f.hint.as_deref().unwrap().starts_with("Fixed by editing"));
        assert!(f.showable(), "its references");
        assert_eq!(f.block.as_deref(), Some("breakout"));
        let long = short(&json!({"a": "x".repeat(80)}));
        assert_eq!(long.0.chars().count(), 40);
        assert!(long.0.ends_with('…') && long.1.len() > 80);
    }

    #[test]
    fn a_blocks_areas_are_one_group_and_one_batch() {
        let (singles, groups) = groups(rows(&status()));
        assert_eq!(singles.len(), 4);
        assert_eq!(groups.len(), 1);
        let g = &groups[0];
        assert_eq!(g.block, "breakout");
        assert_eq!(g.areas.len(), 2);
        let all = Batch::relayout_all(g);
        assert_eq!(
            all.items,
            [
                (
                    "breakout#area:1".to_owned(),
                    "repair".to_owned(),
                    Some("relayout".to_owned())
                ),
                (
                    "breakout#area:2".to_owned(),
                    "repair".to_owned(),
                    Some("relayout".to_owned())
                ),
            ]
        );
        let mut picks = std::collections::BTreeMap::new();
        picks.insert("breakout#area:2".to_owned(), Pick::Theirs);
        let mixed = Batch::areas(g, &picks);
        assert_eq!(
            mixed.items[0].2.as_deref(),
            Some("relayout"),
            "relay out unless picked"
        );
        assert_eq!(
            (mixed.items[1].1.as_str(), mixed.items[1].2.as_deref()),
            ("theirs", None)
        );
        // One area's action takes its block's every area.
        let a = &g.areas[0];
        let ours = Batch::one(a, &a.actions[0]);
        assert_eq!(ours.ids(), ["breakout#area:1", "breakout#area:2"]);
        assert!(ours.items.iter().all(|i| i.1 == "ours"));
        let r = singles.iter().find(|r| r.id == "refdes:board/R5").unwrap();
        let b = Batch::one(r, &r.actions[0]);
        assert_eq!(
            b.items,
            [(
                "refdes:board/R5".to_owned(),
                "repair".to_owned(),
                Some("renumber".to_owned())
            )]
        );
    }
}
