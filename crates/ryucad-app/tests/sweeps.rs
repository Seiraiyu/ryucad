//! Sweeps and lofts in the window against a real daemon (design 323,
//! phase 6): a chosen loft painted by the faces it made and drawn by its
//! sections, each loop from its start, following an edit of a section; a
//! chosen sweep drawn by its profile and path; and the forms editing a
//! sweep's twist and a loft's weight as an agent's `feature.edit` would.

mod common;

use common::{Project, ryucad};
use ryucad_app::core::workspace::Workspace;
use std::sync::Arc;
use std::time::Duration;

const WAIT: Duration = Duration::from_secs(60);

fn key(f: &str) -> (String, String) {
    ("p".to_owned(), f.to_owned())
}

/// The window, once the model and every solid's mesh have come.
fn open(p: &Project) -> Workspace {
    let mut ws = common::open_editing(&p.root, ryucad(), "Dana", Arc::new(|| {})).unwrap();
    assert!(ws.wait_until(WAIT, |ws| {
        ws.idle()
            && ws.part("p").is_some_and(|p| {
                p.solids
                    .iter()
                    .all(|s| ws.model.meshes.contains_key(&key(&s.id)))
            })
    }));
    ws
}

fn near(got: [f64; 3], want: [f64; 3]) {
    for k in 0..3 {
        assert!((got[k] - want[k]).abs() < 1e-9, "{got:?} against {want:?}");
    }
}

#[test]
fn a_chosen_loft_is_painted_and_drawn_by_its_sections() {
    let p = Project::new().with_loft();
    let mut ws = open(&p);
    let part = ws.part("p").unwrap();
    assert_eq!(part.feature("l").unwrap().ty.as_deref(), Some("loft"));
    // Choosing it paints the faces it made: four sides and two caps.
    ws.choose(Some(key("l")));
    assert!(ws.wait_until(WAIT, |ws| ws.model.made.is_some()
        && ws.model.swept.is_some()));
    assert_eq!(ws.model.made.as_ref().unwrap().faces[&key("l")].len(), 6);
    // And draws its sections in order, each loop from its start, the
    // second starting nearest where the first does.
    let d = ws.model.swept.clone().unwrap();
    assert_eq!((d.kind.as_str(), d.sections.len()), ("loft", 2));
    assert_eq!(d.dots().len(), 2);
    near(d.dots()[0], [-0.005, -0.005, 0.0]);
    near(d.dots()[1], [-0.003, -0.003, 0.01]);
    let labels: Vec<String> = d.labels().into_iter().map(|(t, _)| t).collect();
    assert_eq!(labels, ["1", "2"]);
    // The second section drawn 4 mm across is seen.
    p.ok(&[
        "sketch",
        "edit",
        "p",
        "s2",
        r#"[{"op": "move", "id": "p1", "at": [-2, -2]}, {"op": "move", "id": "p2", "at": [2, -2]},
            {"op": "move", "id": "p3", "at": [2, 2]}, {"op": "move", "id": "p4", "at": [-2, 2]}]"#,
    ]);
    let before = d.revision.clone();
    assert!(ws.wait_until(WAIT, |ws| {
        ws.model
            .swept
            .as_ref()
            .is_some_and(|d| d.revision != before)
    }));
    near(
        ws.model.swept.as_ref().unwrap().dots()[1],
        [-0.002, -0.002, 0.01],
    );
    // Choosing something else draws nothing of it.
    ws.choose(Some(key("s1")));
    assert!(ws.model.swept.is_none());
}

#[test]
fn a_chosen_sweep_is_drawn_by_its_profile_and_path() {
    let p = Project::new().with_loft();
    let mut ws = open(&p);
    ws.choose(Some(key("w")));
    assert!(ws.wait_until(WAIT, |ws| ws.model.swept.is_some()
        && ws.model.made.is_some()));
    let d = ws.model.swept.clone().unwrap();
    assert_eq!(d.kind, "sweep");
    let path = d.path.clone().unwrap();
    near(path[0], [0.02, 0.0, 0.0]);
    near(path[path.len() - 1], [0.02, 0.0, 0.02]);
    // The profile: a circle of 2 mm about (20, 0) on xy; no starts or
    // numbers.
    for q in &d.sections[0].loops[0].points {
        let r = ryu_math::hypot(q[0] - 0.02, q[1]);
        assert!((r - 0.002).abs() < 1e-9 && q[2].abs() < 1e-12, "{q:?}");
    }
    assert!(d.dots().is_empty() && d.labels().is_empty());
    // Its side and its caps.
    assert_eq!(ws.model.made.as_ref().unwrap().faces[&key("w")].len(), 3);
}

#[test]
fn the_forms_edit_a_sweeps_twist_and_a_lofts_weight() {
    let p = Project::new().with_loft();
    let mut ws = open(&p);
    // The loft's start weight.
    ws.open_form(Some(key("l")));
    assert!(ws.wait_until(WAIT, |ws| {
        ws.forms.form.as_ref().is_some_and(|f| f.loft.is_some())
    }));
    let form = ws.forms.form.as_mut().unwrap();
    let l = form.loft.as_mut().unwrap();
    assert_eq!(l.sections, ["sketch s1", "sketch s2"]);
    let start = l.start.as_mut().unwrap();
    assert_eq!(
        (start.direction.as_str(), start.weight.read.as_str()),
        ("normal", "1.5")
    );
    start.weight.text = "2".into();
    ws.save_form();
    assert!(ws.wait_until(WAIT, |ws| {
        ws.forms
            .form
            .as_ref()
            .is_some_and(|f| !f.saving && !f.dirty())
    }));
    let shown = p.ok(&["feature", "show", "p", "l"]);
    assert_eq!(shown["start_tangent"]["weight"]["value"], "2", "{shown}");
    // The sweep: a binormal law needs a direction the form doesn't give,
    // so it is refused and what was chosen kept.
    ws.open_form(Some(key("w")));
    assert!(ws.wait_until(WAIT, |ws| {
        ws.forms.form.as_ref().is_some_and(|f| f.sweep.is_some())
    }));
    let form = ws.forms.form.as_mut().unwrap();
    let w = form.sweep.as_mut().unwrap();
    assert_eq!((w.twist.read.as_str(), w.path.as_str()), ("", "p/s4/l1"));
    assert_eq!(w.law.read, "minimal");
    w.law.chosen = "binormal".into();
    ws.save_form();
    assert!(ws.wait_until(WAIT, |ws| {
        ws.forms
            .form
            .as_ref()
            .is_some_and(|f| !f.saving && f.error.is_some())
    }));
    assert!(ws.forms.form.as_ref().unwrap().dirty());
    // Its twist, last: a twisted sweep rebuilds slowly in a debug build.
    ws.forms.form.as_mut().unwrap().revert();
    ws.forms
        .form
        .as_mut()
        .unwrap()
        .sweep
        .as_mut()
        .unwrap()
        .twist
        .text = "90 deg".into();
    ws.save_form();
    assert!(ws.wait_until(WAIT, |ws| {
        ws.forms
            .form
            .as_ref()
            .is_some_and(|f| !f.saving && !f.dirty())
    }));
    let shown = p.ok(&["feature", "show", "p", "w"]);
    assert_eq!(shown["twist"]["value"], "90 deg", "{shown}");
}
