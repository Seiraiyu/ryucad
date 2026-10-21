//! The parameter panel against a real daemon: the same `param.set` an
//! agent sends, guarded the same way.

mod common;

use common::{Project, ryucad};
use ryucad_app::core::workspace::Workspace;
use std::sync::Arc;
use std::time::Duration;

const WAIT: Duration = Duration::from_secs(30);

fn open(p: &Project) -> Workspace {
    let mut ws = Workspace::open(&p.root, ryucad(), "Dana", Arc::new(|| {})).unwrap();
    assert!(ws.wait_until(WAIT, |ws| ws.idle() && ws.part("p").is_some()));
    ws.show_params(Some("p"));
    assert!(ws.wait_until(WAIT, |ws| ws.params.is_some()));
    ws
}

fn width(ws: &mut Workspace) -> &mut ryucad_app::core::edit::Field {
    let p = ws.params.as_mut().unwrap();
    &mut p
        .params
        .iter_mut()
        .find(|r| r.field.name == "width")
        .unwrap()
        .field
}

fn on_disk(p: &Project, name: &str) -> String {
    let list = p.ok(&["param", "list", "p"]);
    let row = list["parameters"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"] == name)
        .unwrap()
        .clone();
    row["value"].as_str().unwrap().to_owned()
}

#[test]
fn a_parameter_saves_through_param_set_and_moves_the_sketch() {
    let p = Project::new().with_sketch();
    let mut ws = open(&p);
    let row = &ws.params.as_ref().unwrap().params[0];
    assert_eq!(
        (row.value.as_str(), row.used_by.clone()),
        ("20 mm", vec!["s".to_owned()])
    );
    width(&mut ws).text = "30 mm".into();
    {
        let panel = ws.params.as_mut().unwrap();
        panel.new_name = "depth".into();
        panel.new_value = "width / 2".into();
    }
    ws.save_params();
    assert!(ws.wait_until(WAIT, |ws| {
        ws.params
            .as_ref()
            .is_some_and(|p| !p.saving && !p.dirty() && p.params.len() == 2)
    }));
    assert_eq!(on_disk(&p, "width"), "30 mm");
    assert_eq!(on_disk(&p, "depth"), "width / 2");
    let depth = &ws.params.as_ref().unwrap().params[0];
    assert_eq!(
        (depth.field.name.as_str(), depth.value.as_str()),
        ("depth", "15 mm")
    );
    // The sketch it drives was written back solved, in the same change,
    // which is this window's.
    let log = p.ok(&["activity"]);
    let last = log["entries"].as_array().unwrap().last().unwrap().clone();
    assert_eq!(last["method"], "param.set");
    assert_eq!(last["participant"]["id"], ws.me());
    let shown = p.ok(&["feature", "show", "p", "s"]);
    assert!((shown["entities"]["p2"]["m"][0].as_f64().unwrap() - 0.03).abs() < 1e-9);
}

#[test]
fn a_stale_save_conflicts_and_shows_what_changed() {
    let p = Project::new().with_sketch();
    let mut ws = open(&p);
    width(&mut ws).text = "30 mm".into();
    p.ok(&["param", "set", "p", "width=25 mm"]);
    ws.save_params();
    assert!(ws.wait_until(WAIT, |ws| {
        ws.params.as_ref().is_some_and(|p| !p.changed.is_empty())
    }));
    let c = &ws.params.as_ref().unwrap().changed[0];
    assert_eq!((c.theirs.as_str(), c.yours.as_str()), ("25 mm", "30 mm"));
    assert_eq!(on_disk(&p, "width"), "25 mm");
    assert_eq!(width(&mut ws).text, "25 mm");
}

#[test]
fn a_bad_expression_keeps_what_was_typed() {
    let p = Project::new().with_sketch();
    let mut ws = open(&p);
    width(&mut ws).text = "width + 1 mm".into();
    ws.save_params();
    assert!(ws.wait_until(WAIT, |ws| {
        ws.params.as_ref().is_some_and(|p| p.error.is_some())
    }));
    let e = ws.params.as_ref().unwrap().error.as_ref().unwrap();
    assert_eq!(e.code, ryucad::error::Code::ParameterCycle);
    assert_eq!(width(&mut ws).text, "width + 1 mm");
}

#[test]
fn an_untouched_panel_follows_the_files() {
    let p = Project::new().with_sketch();
    let mut ws = open(&p);
    p.ok(&["param", "set", "p", "width=40 mm"]);
    assert!(
        ws.wait_until(WAIT, |ws| ws.params.as_ref().is_some_and(|p| {
            p.params
                .iter()
                .any(|r| r.field.name == "width" && r.field.read == "40 mm")
        }))
    );
}
