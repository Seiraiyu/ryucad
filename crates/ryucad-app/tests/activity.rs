//! The activity log, undo and redo, and Git history from the window,
//! against a real daemon.

mod common;

use common::{Project, ryucad};
use ryucad_app::core::activity::can_undo;
use ryucad_app::core::workspace::Workspace;
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

const WAIT: Duration = Duration::from_secs(30);

fn open(p: &Project) -> Workspace {
    let mut ws = Workspace::open(&p.root, ryucad(), "Dana", Arc::new(|| {})).unwrap();
    assert!(ws.wait_until(WAIT, |ws| ws.idle() && ws.model.meshes.len() == 2));
    ws
}

fn radius(p: &Project) -> String {
    p.ok(&["feature", "show", "p", "boss"])["params"]["radius"]["value"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// The newest entry the window knows.
fn last(ws: &Workspace) -> serde_json::Value {
    ws.activity.entries.last().cloned().unwrap_or_default()
}

#[test]
fn the_window_undoes_an_agents_change_and_redoes_it() {
    let p = Project::new().with_part();
    let mut ws = open(&p);
    p.ok(&["feature", "edit", "p", "boss", "--param", "radius=5"]);
    assert!(ws.wait_until(WAIT, |ws| last(ws)["method"] == "feature.edit"));
    let entry = last(&ws);
    assert_eq!(entry["participant"]["name"], "agent");
    assert!(can_undo(&entry));
    ws.undo(entry["seq"].as_u64());
    assert!(ws.wait_until(WAIT, |ws| last(ws)["kind"] == "undo" && ws.idle()));
    assert_eq!(radius(&p), "3 mm");
    // The entry it undid is marked, so the window offers redo instead.
    assert!(!can_undo(
        ws.activity.get(entry["seq"].as_u64().unwrap()).unwrap()
    ));
    assert_eq!(last(&ws)["participant"]["id"], ws.me());
    ws.redo(None);
    assert!(ws.wait_until(WAIT, |ws| last(ws)["kind"] == "redo" && ws.idle()));
    assert_eq!(radius(&p), "5 mm");
}

#[test]
fn an_undo_that_would_lose_a_later_change_is_refused() {
    let p = Project::new().with_part();
    let mut ws = open(&p);
    p.ok(&["feature", "edit", "p", "boss", "--param", "radius=5"]);
    assert!(ws.wait_until(WAIT, |ws| last(ws)["method"] == "feature.edit"));
    let first = last(&ws)["seq"].as_u64();
    p.ok(&["feature", "edit", "p", "boss", "--param", "radius=6"]);
    assert!(ws.wait_until(WAIT, |ws| last(ws)["seq"].as_u64() > first));
    ws.undo(first);
    assert!(ws.wait_until(WAIT, |ws| !ws.errors.is_empty()));
    assert_eq!(ws.errors[0].code, ryucad::error::Code::UndoConflict);
    assert_eq!(radius(&p), "6 mm");
    // With nothing of its own, the window has nothing to undo.
    ws.undo(None);
    assert!(ws.wait_until(WAIT, |ws| ws.errors.len() == 2));
    assert_eq!(ws.errors[1].code, ryucad::error::Code::NothingToUndo);
}

fn git(p: &Project, args: &[&str]) {
    let st = Command::new("git")
        .args(["-c", "user.name=Test", "-c", "user.email=test@example.com"])
        .args(args)
        .current_dir(&p.root)
        .output()
        .unwrap();
    assert!(st.status.success(), "git {args:?}: {st:?}");
}

#[test]
fn the_history_tab_reads_git() {
    let p = Project::new().with_part();
    let mut ws = open(&p);
    // Its own repository: the tests' folders may be inside another.
    git(&p, &["init", "-q"]);
    git(&p, &["add", "."]);
    git(&p, &["commit", "-q", "-m", "A box and a boss"]);
    p.ok(&["feature", "edit", "p", "boss", "--param", "radius=5"]);
    ws.history = None;
    ws.load_history();
    assert!(ws.wait_until(WAIT, |ws| ws.history.is_some()));
    let h = ws.history.as_ref().unwrap();
    assert_eq!(h["commits"][0]["subject"], "A box and a boss");
    assert_eq!(h["uncommitted"][0]["path"], "parts/p/features/boss.json");
}
