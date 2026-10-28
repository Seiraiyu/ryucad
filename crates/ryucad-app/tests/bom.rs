//! The BOM tab against a real daemon: the BOM of the part the window
//! shows, indented and flat, read again as what it reads changes, through
//! the same requests an agent sends.

mod common;

use common::{Project, ryucad};
use ryucad_app::core::workspace::Workspace;
use std::sync::Arc;
use std::time::Duration;

const WAIT: Duration = Duration::from_secs(30);

/// A bolt and a frame placing it twice: the project's one root assembly.
fn assembled() -> Project {
    let p = Project::new();
    p.ok(&["part", "create", "bolt"]);
    p.ok(&[
        "feature",
        "add",
        "bolt",
        "cylinder",
        "--id",
        "shank",
        "--param",
        "radius=3",
        "--param",
        "height=20",
    ]);
    p.ok(&["part", "create", "frame", "--assembly"]);
    for (id, at) in [("b1", "0,0,0"), ("b2", "40,0,0")] {
        p.ok(&[
            "feature", "add", "frame", "instance", "--id", id, "--of", "bolt", "--origin", at,
        ]);
    }
    p
}

fn open(p: &Project) -> Workspace {
    let mut ws = Workspace::open(&p.root, ryucad(), "Dana", Arc::new(|| {})).unwrap();
    assert!(ws.wait_until(WAIT, |ws| ws.idle() && ws.part("frame").is_some()));
    ws
}

/// The quantity cell of the tab's line for `part`.
fn count(ws: &Workspace, part: &str) -> Option<String> {
    let q = ws.bom.columns.iter().position(|c| c == "quantity")?;
    let line = ws.bom.shown().iter().find(|l| l.part == part)?;
    Some(line.cells[q].clone())
}

#[test]
fn the_tab_shows_the_shown_assemblys_bom_and_follows_it() {
    let p = assembled();
    let mut ws = open(&p);
    ws.load_bom();
    assert!(ws.wait_until(WAIT, |ws| ws.idle() && ws.bom.root.is_some()));
    assert_eq!(ws.bom.root.as_deref(), Some("frame"));
    assert_eq!(count(&ws, "bolt").as_deref(), Some("2"));
    let bolt = &ws.bom.tree[0];
    assert_eq!(bolt.occurrences, ["frame/b1", "frame/b2"]);
    assert_eq!(bolt.item, Some(1));
    // An agent adds a bolt: the tab reads it again.
    p.ok(&[
        "feature", "add", "frame", "instance", "--id", "b3", "--of", "bolt",
    ]);
    assert!(ws.wait_until(WAIT, |ws| count(ws, "bolt").as_deref() == Some("3")));
    // Flat: one line, its occurrences from the indented lines.
    ws.set_bom_flat(true);
    assert!(ws.wait_until(WAIT, |ws| ws.idle() && !ws.bom.flat_lines.is_empty()));
    assert_eq!(ws.bom.shown().len(), 1);
    assert_eq!(ws.bom.shown()[0].occurrences.len(), 3);
    // A property declared in the settings is a column, as in the CSV.
    p.ok(&["pdm", "init"]);
    p.ok(&[
        "pdm",
        "configure",
        "--property",
        r#"finish={"type": "string"}"#,
    ]);
    assert!(
        ws.wait_until(WAIT, |ws| ws.bom.columns.last().map(String::as_str)
            == Some("finish"))
    );
}

#[test]
fn several_roots_are_offered_and_a_focus_chooses() {
    let p = assembled();
    p.ok(&["part", "create", "other", "--assembly"]);
    let mut ws = open(&p);
    ws.load_bom();
    assert!(ws.wait_until(WAIT, |ws| ws.idle() && !ws.bom.roots.is_empty()));
    assert_eq!(ws.bom.roots, ["frame", "other"]);
    // Showing one assembly makes it the tab's.
    ws.set_focus(Some("frame".into()));
    assert!(ws.wait_until(WAIT, |ws| ws.bom.root.as_deref() == Some("frame")));
    assert!(ws.bom.roots.is_empty());
}
