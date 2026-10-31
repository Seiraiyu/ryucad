//! Dependencies in the window against real daemons and `file://`
//! repositories: the tab's list, a fetch, the picker placing a released
//! part, the tree's marks, an update's dry run and its write, and the
//! BOM's Project column, through the same requests an agent sends.

mod common;

use common::{Project, library, release_again, ryucad};
use ryucad_app::core::workspace::Workspace;
use std::sync::Arc;
use std::time::Duration;

const WAIT: Duration = Duration::from_secs(60);

fn open(p: &Project) -> Workspace {
    let mut ws = Workspace::open(&p.root, ryucad(), "Dana", Arc::new(|| {})).unwrap();
    assert!(ws.wait_until(WAIT, |ws| ws.idle()));
    ws
}

/// The tree's text for the first feature of `frame`.
fn row(ws: &Workspace) -> String {
    match ws.part("frame").and_then(|f| f.features.first()) {
        Some(f) => ryucad_app::core::tree::row_text(f, 0),
        None => String::new(),
    }
}

#[test]
fn a_dependency_is_added_placed_marked_and_updated() {
    let (lib, url) = library();
    let p = Project::new().committed();
    p.ok(&["part", "create", "frame", "--assembly"]);
    let mut ws = open(&p);
    // Added from the tab: listed and fetched.
    ws.deps.add_url = url.clone();
    ws.add_dep();
    assert!(ws.wait_until(WAIT, |ws| ws.idle() && ws.deps.row("fasteners").is_some()));
    assert!(ws.deps.failed.is_none(), "{:?}", ws.deps.failed);
    assert!(ws.deps.row("fasteners").unwrap().fetched);
    // The picker lists p at A, and places it.
    ws.set_focus(Some("frame".into()));
    assert!(ws.wait_until(WAIT, |ws| ws.idle() && ws.shown_assembly().is_some()));
    ws.open_picker();
    ws.pick_dependency("fasteners");
    assert!(ws.wait_until(WAIT, |ws| {
        ws.picker.as_ref().is_some_and(|p| !p.parts.is_empty())
    }));
    let picker = ws.picker.as_mut().unwrap();
    assert_eq!(picker.parts[0].text(), "B-6 (p): A released");
    picker.choose(0);
    ws.place_picked();
    assert!(ws.wait_until(WAIT, |ws| ws.picker.is_none()
        && ws.part("frame").is_some_and(|f| !f.features.is_empty())));
    assert!(
        ws.wait_until(WAIT, |ws| row(ws).contains("fasteners:p @ label A")),
        "{}",
        row(&ws)
    );
    // A newer release, fetched: the instance is outdated.
    release_again(&lib);
    ws.fetch_deps(Vec::new());
    assert!(
        ws.wait_until(WAIT, |ws| ws.idle() && row(ws).ends_with(", outdated)")),
        "{}",
        row(&ws)
    );
    // Update's dry run says what moves, and its write moves it.
    ws.plan_update(Vec::new(), None);
    assert!(ws.wait_until(WAIT, |ws| ws.deps.update.is_some()));
    let id = ws.part("frame").unwrap().features[0].id.clone();
    assert_eq!(
        ws.deps.update.as_ref().unwrap().moved,
        [format!("frame/{id}: fasteners:p A → B")]
    );
    ws.run_update();
    assert!(
        ws.wait_until(WAIT, |ws| ws.idle()
            && row(ws).contains("@ label B")
            && !row(ws).contains("outdated")),
        "{}",
        row(&ws)
    );
    // The BOM names its dependency.
    ws.open_bom();
    assert!(ws.wait_until(WAIT, |ws| ws.idle() && !ws.bom.tree.is_empty()));
    let at = ws.bom.columns.iter().position(|c| c == "project").unwrap();
    assert_eq!(ws.bom.tree[0].cells[at], "fasteners");
    assert_eq!(ws.bom.tree[0].part, "fasteners:p");
    // Removing it while something places it is refused, in the tab.
    ws.remove_dep("fasteners");
    assert!(ws.wait_until(WAIT, |ws| ws.deps.failed.is_some()));
    assert_eq!(
        ws.deps.failed.as_ref().unwrap().code,
        ryucad::error::Code::HasDependents
    );
}

#[test]
fn a_fetch_waits_aside_and_says_how_it_went() {
    let (_lib, url) = library();
    let p = Project::new();
    p.ok(&["deps", "add", &url]);
    let mut ws = open(&p);
    ws.fetch_deps(Vec::new());
    assert!(ws.wait_until(WAIT, |ws| ws.idle() && ws.deps.fetched.is_some()));
    assert_eq!(
        ws.deps.fetched.as_deref(),
        Some("fetched 1 repository, 0 changed")
    );
    assert!(ws.deps.failed.is_none(), "{:?}", ws.deps.failed);
    // A repository that can't be fetched isn't added: the agent's answer
    // says why, and the window, which sent nothing, has nothing to say.
    let gone = p.cli(&["deps", "add", "file:///nowhere/at/all", "--name", "gone"]);
    assert_eq!(gone["ok"], false, "{gone}");
    assert_eq!(gone["error"]["code"], "dependency_fetch_failed", "{gone}");
    assert!(ws.wait_until(Duration::from_secs(5), |ws| ws.idle()));
    assert!(ws.deps.failed.is_none(), "{:?}", ws.deps.failed);
    assert!(ws.deps.row("gone").is_none());
}
