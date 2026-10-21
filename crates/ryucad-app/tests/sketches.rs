//! Sketches in the window: drawn from `sketch.get`, kept per part
//! revision, against a real daemon.

mod common;

use common::{Project, ryucad};
use ryucad::sketch_view::EntityState;
use ryucad_app::core::workspace::Workspace;
use std::sync::Arc;
use std::time::Duration;

const WAIT: Duration = Duration::from_secs(30);

fn open(p: &Project) -> Workspace {
    Workspace::open(&p.root, ryucad(), "Dana", Arc::new(|| {})).unwrap()
}

fn key() -> (String, String) {
    ("p".to_owned(), "s".to_owned())
}

/// Whether the window draws sketch `s` as part `p` is now.
fn drawn(ws: &Workspace) -> bool {
    let rev = ws.part("p").map(|p| p.revision.clone());
    ws.model
        .sketches
        .get(&key())
        .is_some_and(|d| Some(&d.revision) == rev.as_ref())
}

#[test]
fn the_window_draws_sketches() {
    let p = Project::new().with_sketch();
    let mut ws = open(&p);
    assert!(ws.wait_until(WAIT, |ws| ws.idle() && drawn(ws)));
    let part = ws.part("p").unwrap();
    let s = part.feature("s").unwrap();
    assert_eq!(s.sketch.as_ref().unwrap().state, "well");
    assert_eq!(part.feature("e").unwrap().uses, ["s"]);
    let d = &ws.model.sketches[&key()];
    assert_eq!(d.curves.len(), 5);
    assert_eq!(d.dots.len(), 4);
    assert!(d.curves.iter().all(|c| c.state == EntityState::Well));
    assert_eq!(d.curves.iter().filter(|c| c.construction).count(), 1);
    let texts: Vec<&str> = d.labels.iter().map(|l| l.text.as_str()).collect();
    assert_eq!(texts, ["width = 20 mm", "10 mm"]);
    assert_eq!(d.labels[0].value, "width");
}

#[test]
fn a_parameter_edit_redraws_the_sketch() {
    let p = Project::new().with_sketch();
    let mut ws = open(&p);
    assert!(ws.wait_until(WAIT, |ws| ws.idle() && drawn(ws)));
    p.ok(&["param", "set", "p", "width=30 mm"]);
    assert!(ws.wait_until(WAIT, |ws| {
        ws.model
            .sketches
            .get(&key())
            .is_some_and(|d| d.labels[0].text == "width = 30 mm")
    }));
    // The far corner moved with it.
    let d = &ws.model.sketches[&key()];
    let p2 = d.dots.iter().find(|d| d.entity == "p2").unwrap();
    assert!((p2.at[0] - 0.03).abs() < 1e-9, "{:?}", p2.at);
}

#[test]
fn the_default_unit_relabels_dimensions() {
    let p = Project::new().with_sketch();
    let mut ws = open(&p);
    assert!(ws.wait_until(WAIT, |ws| ws.idle() && drawn(ws)));
    p.ok(&["config", "set", "default_unit", "cm"]);
    assert!(ws.wait_until(WAIT, |ws| {
        ws.model
            .sketches
            .get(&key())
            .is_some_and(|d| d.labels[0].text == "width = 2 cm")
    }));
}

/// The window with sketch `s` drawn and the form open for its dimension
/// `d1` (the width).
fn dimension_open(p: &Project) -> Workspace {
    let mut ws = open(p);
    assert!(ws.wait_until(WAIT, |ws| ws.idle() && drawn(ws)));
    ws.open_dimension(Some(("p".into(), "s".into(), "d1".into())));
    assert!(ws.wait_until(WAIT, |ws| ws.dimension.is_some()));
    ws
}

fn width_on_disk(p: &Project) -> String {
    p.ok(&["feature", "show", "p", "s"])["constraints"]["d1"]["value"]["value"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[test]
fn a_dimension_saves_through_sketch_edit() {
    let p = Project::new().with_sketch();
    let mut ws = dimension_open(&p);
    let f = ws.dimension.as_mut().unwrap();
    assert_eq!((f.ty.as_str(), f.value.read.as_str()), ("length", "width"));
    f.value.text = "width + 5 mm".into();
    ws.save_dimension();
    assert!(ws.wait_until(WAIT, |ws| {
        ws.dimension
            .as_ref()
            .is_some_and(|f| !f.saving && f.value.read == "width + 5 mm")
    }));
    assert_eq!(width_on_disk(&p), "width + 5 mm");
    // The sketch was solved and written: the extrude grew with it.
    let log = p.ok(&["activity"]);
    let last = log["entries"].as_array().unwrap().last().unwrap().clone();
    assert_eq!(last["method"], "sketch.edit");
    assert_eq!(last["participant"]["id"], ws.me());
    assert!(ws.wait_until(WAIT, |ws| {
        ws.model
            .sketches
            .get(&key())
            .is_some_and(|d| d.labels[0].text == "width + 5 mm = 25 mm")
    }));
}

#[test]
fn a_stale_dimension_save_conflicts() {
    let p = Project::new().with_sketch();
    let mut ws = dimension_open(&p);
    ws.dimension.as_mut().unwrap().value.text = "30 mm".into();
    // An agent edits the same sketch first.
    let ops = r#"[{"op": "set", "id": "d1", "value": "25 mm"}]"#;
    p.ok(&["sketch", "edit", "p", "s", ops]);
    ws.save_dimension();
    assert!(ws.wait_until(WAIT, |ws| {
        ws.dimension.as_ref().is_some_and(|f| f.changed.is_some())
    }));
    let f = ws.dimension.as_ref().unwrap();
    let c = f.changed.as_ref().unwrap();
    assert_eq!((c.theirs.as_str(), c.yours.as_str()), ("25 mm", "30 mm"));
    assert_eq!(f.value.text, "25 mm");
    assert_eq!(width_on_disk(&p), "25 mm");
}

#[test]
fn a_dimension_that_cant_hold_is_refused() {
    let p = Project::new().with_sketch();
    let mut ws = dimension_open(&p);
    ws.dimension.as_mut().unwrap().value.text = "-3 mm".into();
    ws.save_dimension();
    assert!(ws.wait_until(WAIT, |ws| {
        ws.dimension.as_ref().is_some_and(|f| f.error.is_some())
    }));
    assert!(ws.dimension.as_ref().unwrap().dirty());
    assert_eq!(width_on_disk(&p), "width");
}

#[test]
fn a_feature_chosen_while_a_dimension_loads_stays_chosen() {
    let p = Project::new().with_sketch();
    let mut ws = open(&p);
    assert!(ws.wait_until(WAIT, |ws| ws.idle() && drawn(ws)));
    // A label clicked, then a feature picked before the sketch was read:
    // the dimension's answer, coming later, doesn't cover the feature.
    ws.open_dimension(Some(("p".into(), "s".into(), "d1".into())));
    ws.open_form(Some(("p".into(), "e".into())));
    assert!(ws.wait_until(WAIT, |ws| ws.idle() && ws.form.is_some()));
    assert!(ws.dimension.is_none());
    // And the other way round: the feature's form doesn't wait behind it.
    ws.open_form(Some(("p".into(), "e".into())));
    ws.open_dimension(Some(("p".into(), "s".into(), "d1".into())));
    assert!(ws.wait_until(WAIT, |ws| ws.idle() && ws.dimension.is_some()));
    assert!(ws.form.is_none());
    ws.open_dimension(None);
    assert!(ws.form.is_none());
}
