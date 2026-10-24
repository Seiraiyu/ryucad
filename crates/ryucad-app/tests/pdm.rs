//! ryupdm in the window against a real daemon: release states, part
//! properties and the Releases tab, through the same requests an agent
//! sends.

mod common;

use common::{Project, ryucad};
use ryucad_app::core::workspace::Workspace;
use std::sync::Arc;
use std::time::Duration;

const WAIT: Duration = Duration::from_secs(30);

fn open(p: &Project) -> Workspace {
    let mut ws = Workspace::open(&p.root, ryucad(), "Dana", Arc::new(|| {})).unwrap();
    assert!(ws.wait_until(WAIT, |ws| ws.idle() && ws.part("p").is_some()));
    ws
}

/// A part `p` with a box and a cylinder, numbered 100, committed on `main`.
fn numbered() -> Project {
    let p = Project::new().with_part();
    p.ok(&["pdm", "init"]);
    p.ok(&["props", "set", "p", "number=100"]);
    p.committed()
}

fn badge(ws: &Workspace) -> Option<String> {
    ws.states.badge("p")
}

#[test]
fn badges_follow_proposals_and_the_release_branch() {
    let p = numbered();
    let mut ws = open(&p);
    assert_eq!(badge(&ws).as_deref(), Some("in work"));
    p.ok(&["release", "propose", "p"]);
    assert!(ws.wait_until(WAIT, |ws| badge(ws).as_deref()
        == Some("in work, A proposed")));
    // Official once main holds it; work on it is work towards B.
    p.git(&["add", "."]);
    p.git(&["commit", "-q", "-m", "release p A"]);
    assert!(ws.wait_until(WAIT, |ws| badge(ws).as_deref() == Some("A")));
    p.ok(&["feature", "edit", "p", "base", "--param", "x=30"]);
    assert!(ws.wait_until(WAIT, |ws| badge(ws).as_deref() == Some("A modified")));
}
