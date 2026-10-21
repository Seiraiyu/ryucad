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
