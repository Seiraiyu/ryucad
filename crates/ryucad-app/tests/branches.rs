//! The Branches tab against real daemons and a real repository with two
//! worktrees: the rows `worktree.list` gives, merge previews made again as
//! either side changes, and a compare with the other worktree's working
//! files that follows its edits.

mod common;

use common::{Project, ryucad};
use ryucad_app::core::branches::{compare_rev, summary, who};
use ryucad_app::core::workspace::Workspace;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

const WAIT: Duration = Duration::from_secs(60);

/// Runs the CLI in `dir`; its result, which must be ok.
fn ok_in(dir: &Path, args: &[&str]) -> Value {
    let out = std::process::Command::new(&ryucad().exe)
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["ok"], true, "ryucad {args:?}: {v}");
    v["result"].clone()
}

/// A committed project with a worktree `arm` on branch `arm`, where agent
/// `agent-b` is expected; the worktree's project folder.
fn two_worktrees() -> (Project, PathBuf) {
    let p = Project::new().with_part().committed();
    let v = p.ok(&[
        "--as",
        "agent-b",
        "worktree",
        "add",
        "../arm",
        "-b",
        "arm",
        "--no-warm",
    ]);
    (p, PathBuf::from(v["path"].as_str().unwrap()))
}

/// The preview of `arm` once it is made from the files as they are.
fn previewed(ws: &Workspace, arm: &str) -> bool {
    let b = &ws.branches;
    let Some(w) = b.others().find(|w| w["path"] == arm) else {
        return false;
    };
    b.previews
        .get(arm)
        .is_some_and(|p| Some(&p.key) == b.key(w).as_ref())
        && b.previewing.is_none()
}

#[test]
fn the_tab_lists_the_other_worktree_and_previews_merging_it() {
    let (p, arm) = two_worktrees();
    let arm_s = arm.to_str().unwrap().to_owned();
    let mut ws = common::open_editing(&p.root, ryucad(), "Dana", Arc::new(|| {})).unwrap();
    assert!(ws.wait_until(WAIT, |ws| ws.idle()));
    ws.load_branches();
    assert!(ws.wait_until(WAIT, |ws| previewed(ws, &arm_s)));
    let b = &ws.branches;
    let row = b.others().next().unwrap();
    assert_eq!(row["branch"], "arm");
    assert_eq!(who(row), "agent-b (expected)");
    assert_eq!(summary(&b.previews[&arm_s]), "merges cleanly");
    assert_eq!(b.current().unwrap()["branch"], "main");

    // Both sides set the box's length: the preview is made again (this
    // window's own edit through its daemon, theirs read on the next
    // reading) and says so.
    ok_in(&arm, &["feature", "edit", "p", "base", "--param", "x=15"]);
    p.ok(&["feature", "edit", "p", "base", "--param", "x=12"]);
    assert!(ws.wait_until(WAIT, |ws| {
        ws.branches
            .previews
            .get(&arm_s)
            .is_some_and(|p| summary(p) == "1 conflict")
    }));
    ws.branches.shown = Some(arm_s.clone());
    let v = ws.branches.shown_preview().unwrap();
    assert_eq!(v["conflicts"][0]["kind"], "value");
    assert_eq!(v["conflicts"][0]["theirs"], "15 mm");

    // A compare with the other worktree's working files follows its edits.
    ws.set_compare(Some(compare_rev(&arm_s)));
    assert!(ws.wait_until(WAIT, |ws| {
        ws.idle()
            && ws
                .compare
                .as_ref()
                .is_some_and(|c| c.loaded && !c.wanted.is_empty())
    }));
    let key = ("p".to_owned(), "base".to_owned());
    let before = ws.compare.as_ref().unwrap().wanted[&key].clone();
    ok_in(&arm, &["feature", "edit", "p", "base", "--param", "x=16"]);
    assert!(ws.wait_until(WAIT, |ws| {
        ws.compare
            .as_ref()
            .is_some_and(|c| c.wanted.get(&key).is_some_and(|h| *h != before))
    }));
}

#[test]
fn a_branch_is_checked_out_from_the_window_and_refused_while_dirty() {
    use ryucad::error::Code;
    use serde_json::json;
    let p = Project::new().with_part().committed();
    p.git(&["switch", "-q", "-c", "arm"]);
    p.ok(&["feature", "edit", "p", "base", "--param", "x=15"]);
    p.git(&["commit", "-q", "-am", "longer"]);
    p.git(&["switch", "-q", "main"]);
    let mut ws = common::open_editing(&p.root, ryucad(), "Dana", Arc::new(|| {})).unwrap();
    assert!(ws.wait_until(WAIT, |ws| ws.idle()));
    p.ok(&["--as", "claude-1", "status", "reviewing"]);
    ws.load_branches();
    assert!(ws.wait_until(WAIT, |ws| {
        ws.branches.branch_list.is_some() && ws.people.others(ws.me()).any(|p| p.name == "claude-1")
    }));
    assert_eq!(ws.branches.current_branch(), Some("main"));
    // The dialog names who else is attached.
    ws.ask_checkout("arm");
    let c = ws.branches.checkout.clone().unwrap();
    assert_eq!(c.standing, "1 ahead");
    assert!(
        c.attached.contains(&"claude-1".to_owned()),
        "{:?}",
        c.attached
    );
    // An uncommitted change: refused, the file named, nothing switched.
    p.ok(&["feature", "edit", "p", "boss", "--param", "radius=4"]);
    ws.confirm_checkout();
    assert!(ws.wait_until(WAIT, |ws| {
        ws.branches
            .checkout
            .as_ref()
            .is_some_and(|c| c.refused.is_some() && !c.busy)
    }));
    let e = ws
        .branches
        .checkout
        .as_ref()
        .unwrap()
        .refused
        .clone()
        .unwrap();
    assert_eq!(e.code, Code::CheckoutDirty);
    assert_eq!(e.details["files"], json!(["parts/p/features/boss.json"]));
    // Committed in Git, the switch goes through and the window follows.
    p.git(&["commit", "-q", "-am", "radius"]);
    ws.confirm_checkout();
    assert!(ws.wait_until(WAIT, |ws| {
        ws.idle() && ws.branches.checkout.is_none() && ws.branches.current_branch() == Some("arm")
    }));
    let shown = p.ok(&["feature", "show", "p", "base"]);
    assert_eq!(shown["params"]["x"]["value"], "15 mm");
    let log = p.ok(&["activity"]);
    let last = log["entries"].as_array().unwrap().last().unwrap().clone();
    assert_eq!(last["kind"], "switch");
    assert_eq!(last["participant"]["kind"], "human");
}
