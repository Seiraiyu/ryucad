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

/// The item number the tab shows for `part`.
fn item(ws: &Workspace, part: &str) -> Option<u64> {
    ws.bom.tree.iter().find(|l| l.part == part)?.item
}

/// Types `text` as `part`'s item number and sets it.
fn set_item(ws: &mut Workspace, part: &str, text: &str) {
    let line = ws.bom.tree.iter().find(|l| l.part == part).unwrap().clone();
    ws.bom.edit(&line);
    ws.bom.editing.as_mut().unwrap().text = text.into();
    ws.save_item();
}

#[test]
fn an_item_number_is_set_through_bom_renumber_and_a_stale_one_conflicts() {
    let p = assembled();
    p.ok(&["part", "create", "nut"]);
    p.ok(&[
        "feature", "add", "frame", "instance", "--id", "n1", "--of", "nut",
    ]);
    let mut ws = open(&p);
    ws.load_bom();
    assert!(ws.wait_until(WAIT, |ws| ws.idle() && item(ws, "nut") == Some(2)));
    set_item(&mut ws, "bolt", "7");
    assert!(ws.wait_until(WAIT, |ws| ws.idle() && item(ws, "bolt") == Some(7)));
    assert!(ws.bom.editing.is_none() && !ws.bom.saving);
    let log = p.ok(&["activity"]);
    let last = log["entries"].as_array().unwrap().last().unwrap().clone();
    assert_eq!(last["method"], "bom.renumber");
    assert_eq!(last["participant"]["id"], ws.me());
    // Not an item number: refused here, the text kept.
    set_item(&mut ws, "bolt", "0");
    assert!(ws.bom.edit_error.is_some());
    assert_eq!(ws.bom.editing.as_ref().unwrap().text, "0");
    ws.bom.editing = None;
    // Someone renumbers while the window hears nothing: the save is
    // against the revision it saw, conflicts, and says what changed.
    ws.deaf = true;
    p.ok(&["bom", "renumber", "frame", "bolt=3"]);
    set_item(&mut ws, "bolt", "5");
    assert!(ws.wait_until(WAIT, |ws| ws.idle() && ws.bom.changed.is_some()));
    assert_eq!(
        ws.bom.changed.as_deref(),
        Some("Someone changed frame's item numbers while you edited: bolt is now 3 (you typed 5)")
    );
    assert_eq!(item(&ws, "bolt"), Some(3));
    let bom = p.ok(&["bom", "frame"]);
    assert_eq!(bom["tree"]["children"][0]["item"], 3);
}

#[test]
fn the_csv_written_is_the_clis() {
    let p = assembled();
    let mut ws = open(&p);
    ws.set_bom_flat(true);
    assert!(ws.wait_until(WAIT, |ws| ws.idle() && !ws.bom.flat_lines.is_empty()));
    ws.open_bom_export();
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("bom.csv");
    ws.bom_export.as_mut().unwrap().path = out.display().to_string();
    ws.run_bom_export();
    assert!(ws.wait_until(WAIT, |ws| {
        ws.bom_export.as_ref().is_some_and(|e| e.done.is_some())
    }));
    let cli = p.ok(&["bom", "frame", "--flat", "--format", "csv"]);
    assert_eq!(
        std::fs::read_to_string(&out).unwrap(),
        cli["csv"].as_str().unwrap()
    );
}
