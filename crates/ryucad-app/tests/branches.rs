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
    let mut ws = Workspace::open(&p.root, ryucad(), "Dana", Arc::new(|| {})).unwrap();
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
