//! The window during a merge, against a real daemon and a real
//! repository: the status it reads and hears of, and conflicts settled
//! from it.

mod common;

use common::{Project, ryucad};
use ryucad_app::core::workspace::Workspace;
use std::sync::Arc;
use std::time::Duration;

const WAIT: Duration = Duration::from_secs(30);

fn open(p: &Project) -> Workspace {
    Workspace::open(&p.root, ryucad(), "Dana", Arc::new(|| {})).unwrap()
}

const CONFLICT: &str = "parts/p/features/base.json#/params/x";

#[test]
fn the_window_follows_a_merge_and_settles_it() {
    let p = Project::new().with_part().committed().diverged();
    let mut ws = open(&p);
    assert!(ws.wait_until(WAIT, |ws| ws.idle() && ws.model.meshes.len() == 2));
    assert!(ws.merge.is_none());
    // Git stops the merge for the conflict; the window hears of it.
    assert!(!p.try_git(&["merge", "--no-edit", "theirs"]));
    assert!(ws.wait_until(WAIT, |ws| {
        ws.merge.as_ref().is_some_and(|m| m.conflicts().len() == 1)
    }));
    let m = ws.merge.as_ref().unwrap();
    assert_eq!(m.summary(), "merge: 1 conflict");
    let c = m.conflict(CONFLICT).unwrap();
    assert_eq!(
        (c["ours"].as_str(), c["theirs"].as_str()),
        (Some("12 mm"), Some("15 mm"))
    );
    // Taking theirs settles it; the file waits for `git add`.
    // Once the window has the merged part, as a person would see it.
    let now = p.ok(&["part", "list"])["parts"][0]["revision"].clone();
    assert!(ws.wait_until(WAIT, |ws| ws.part("p").is_some_and(|p| p.revision == now)));
    ws.resolve(CONFLICT, "theirs", None);
    assert!(ws.wait_until(WAIT, |ws| ws.idle()
        && ws.merge.as_ref().is_some_and(|m| m.clean() && !m.resolving)));
    assert_eq!(
        ws.merge.as_ref().unwrap().unstaged(),
        ["parts/p/features/base.json"]
    );
    let shown = p.ok(&["feature", "show", "p", "base"]);
    assert_eq!(shown["params"]["x"]["value"], "15 mm");
    // Committed, the merge is over.
    p.git(&["add", "."]);
    p.git(&["commit", "-q", "--no-edit"]);
    assert!(ws.wait_until(WAIT, |ws| ws.merge.is_none()));
}

#[test]
fn a_window_opened_during_a_merge_sees_it() {
    let p = Project::new().with_part().committed().diverged();
    assert!(!p.try_git(&["merge", "--no-edit", "theirs"]));
    let mut ws = open(&p);
    assert!(ws.wait_until(WAIT, |ws| ws.idle() && ws.merge.is_some()));
    assert_eq!(ws.merge.as_ref().unwrap().conflicts().len(), 1);
    // A resolution against a revision that moved is refused and the
    // status read again.
    p.ok(&["feature", "edit", "p", "boss", "--param", "radius=2"]);
    let stale = ws.model.parts["p"].revision.clone();
    ws.model.parts.get_mut("p").unwrap().revision = "0".repeat(stale.len());
    ws.resolve(CONFLICT, "ours", None);
    assert!(ws.wait_until(WAIT, |ws| ws.idle()
        && ws.merge.as_ref().is_some_and(|m| m.failed.is_some())));
    let m = ws.merge.as_ref().unwrap();
    assert_eq!(
        m.failed.as_ref().unwrap().code,
        ryucad::error::Code::Conflict
    );
    assert_eq!(m.conflicts().len(), 1);
}
