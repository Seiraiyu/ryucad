//! Inserts and deletes in the window against a real daemon (design 521,
//! The app): an insert read as one row, its solids meshed by key and
//! painted when it is chosen, a delete's solid ghosted, and the insert's
//! and the delete's forms editing as an agent's writes would.

mod common;

use common::{Project, ryucad};
use ryucad_app::core::workspace::Workspace;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;

const WAIT: Duration = Duration::from_secs(120);

fn key(f: &str) -> (String, String) {
    ("e".to_owned(), f.to_owned())
}

/// The window, once e's model and every solid's mesh have come.
fn open(p: &Project) -> Workspace {
    let mut ws = common::open_editing(&p.root, ryucad(), "Dana", Arc::new(|| {})).unwrap();
    assert!(ws.wait_until(WAIT, |ws| {
        ws.idle()
            && ws.part("e").is_some_and(|p| {
                !p.solids.is_empty()
                    && p.solids
                        .iter()
                        .all(|s| ws.model.meshes.contains_key(&key(&s.id)))
            })
    }));
    ws
}

/// Saves the form and waits for the window to settle.
fn saved(ws: &mut Workspace) {
    ws.save_form();
    assert!(ws.wait_until(WAIT, |ws| ws.idle()
        && ws.form.as_ref().is_some_and(|f| !f.saving && !f.dirty())));
}

#[test]
fn an_insert_is_read_meshed_by_key_and_painted_when_chosen() {
    let p = Project::new().with_inserts();
    let ws = open(&p);
    let part = ws.part("e").unwrap().clone();
    let brd = part.feature("brd").unwrap().insert.clone().unwrap();
    assert_eq!(
        (brd.part.as_str(), brd.label.as_deref(), brd.count),
        ("pcb", Some("A"), Some(2))
    );
    // The boss is deleted: the model shows the inserted base and the lid.
    let ids: Vec<&str> = part.solids.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(ids, ["brd/base", "lid"]);
    assert_eq!(
        part.feature("d").unwrap().delete.as_ref().unwrap().text(),
        "brd/boss"
    );
    assert!(ws.model.meshes.contains_key(&key("brd/base")));
    assert_eq!(
        ryucad_app::core::inserts::painted(&part, &key("brd")),
        [key("brd/base")]
    );
}

#[test]
fn a_chosen_delete_ghosts_the_solid_it_deletes_where_it_was() {
    let p = Project::new().with_inserts();
    let mut ws = open(&p);
    ws.choose(Some(key("d")));
    assert!(ws.wait_until(WAIT, |ws| {
        ws.deleted
            .as_ref()
            .is_some_and(|d| d.ghosts.contains_key(&key("brd/boss")))
    }));
    let ghost = ws.deleted.as_ref().unwrap().ghosts[&key("brd/boss")].clone();
    let m = ghost.mesh.as_ref().unwrap();
    // Raised 20 mm with the insert: the boss's top is at 33 mm.
    let top = m
        .positions
        .iter()
        .map(|q| f64::from(q[2]) + m.origin[2])
        .fold(f64::MIN, f64::max);
    assert!((top - 0.033).abs() < 1e-6, "{top}");
    ws.choose(None);
    assert!(ws.deleted.is_none());
}

#[test]
fn the_inserts_form_offers_its_labels_previews_a_move_and_applies_it() {
    let p = Project::new().with_inserts();
    let mut ws = open(&p);
    ws.open_form(Some(key("brd")));
    assert!(ws.wait_until(WAIT, |ws| ws.form.as_ref().is_some_and(|f| {
        f.feature == "brd"
            && f.rev
                .as_ref()
                .is_some_and(|r| r.options.contains(&"B".to_owned()))
    })));
    let mut f = ws.form.clone().unwrap();
    assert_eq!(f.of.as_deref(), Some("pcb"));
    let rev = f.rev.as_ref().unwrap();
    assert_eq!(rev.read, "A");
    for l in ["working", "A", "B"] {
        assert!(rev.options.contains(&l.to_owned()), "{rev:?}");
    }
    assert!(f.origin[2].read.starts_with("20"), "{:?}", f.origin[2]);
    // Choosing B previews what moving the pin changes, writing nothing.
    f.rev.as_mut().unwrap().chosen = "B".into();
    ws.form = Some(f);
    ws.preview_insert();
    assert!(ws.wait_until(WAIT, |ws| {
        ws.insert_preview
            .as_ref()
            .is_some_and(|p| p.answer.is_some())
    }));
    let moved = ws.insert_preview.clone().unwrap().answer.unwrap().unwrap();
    assert_eq!(
        (moved.changed, moved.added, moved.removed),
        (1, 0, 0),
        "{moved:?}"
    );
    assert!(moved.failing.is_empty(), "{moved:?}");
    assert_eq!(p.ok(&["feature", "show", "e", "brd"])["rev"]["label"], "A");
    saved(&mut ws);
    assert_eq!(p.ok(&["feature", "show", "e", "brd"])["rev"]["label"], "B");
    ws.preview_insert();
    assert!(ws.insert_preview.is_none());
}

#[test]
fn a_deletes_solids_are_taken_out_from_its_form() {
    let p = Project::new().with_inserts();
    p.ok(&[
        "feature", "add", "e", "box", "--id", "x2", "--param", "x=1", "--param", "y=1", "--param",
        "z=1", "--origin", "50,0,0",
    ]);
    p.ok(&[
        "feature",
        "add",
        "e",
        "delete_body",
        "--id",
        "d2",
        "--solid",
        "x2",
        "--solid",
        "lid",
    ]);
    let mut ws = open_without(&p, &["lid", "x2"]);
    ws.open_form(Some(key("d2")));
    assert!(ws.wait_until(WAIT, |ws| {
        ws.form
            .as_ref()
            .is_some_and(|f| f.feature == "d2" && f.solids.is_some())
    }));
    let mut f = ws.form.clone().unwrap();
    let s = f.solids.as_mut().unwrap();
    assert_eq!(s.read, ["x2", "lid"]);
    s.toggle("lid");
    ws.form = Some(f);
    saved(&mut ws);
    assert_eq!(
        p.ok(&["feature", "show", "e", "d2"])["solids"],
        json!(["x2"])
    );
}

/// [`open`], for a project whose `gone` solids are deleted.
fn open_without(p: &Project, gone: &[&str]) -> Workspace {
    let ws = open(p);
    let part = ws.part("e").unwrap();
    assert!(
        part.solids.iter().all(|s| !gone.contains(&s.id.as_str())),
        "{:?}",
        part.solids
    );
    ws
}
