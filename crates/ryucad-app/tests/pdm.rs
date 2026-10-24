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

/// Opens `p` with the Part section showing part `p`.
fn with_props(p: &Project) -> Workspace {
    let mut ws = open(p);
    ws.show_props(Some("p"));
    assert!(ws.wait_until(WAIT, |ws| ws.props.is_some()));
    ws
}

fn field<'a>(ws: &'a mut Workspace, name: &str) -> &'a mut ryucad_app::core::edit::Field {
    &mut ws
        .props
        .as_mut()
        .unwrap()
        .props
        .iter_mut()
        .find(|r| r.field.name == name)
        .unwrap()
        .field
}

fn number_on_disk(p: &Project) -> serde_json::Value {
    let got = p.ok(&["props", "show", "p"]);
    got["properties"][0]["value"].clone()
}

#[test]
fn a_property_saves_through_props_set() {
    let p = numbered();
    let mut ws = with_props(&p);
    assert_eq!(field(&mut ws, "number").read, "100");
    field(&mut ws, "number").text = "101".into();
    field(&mut ws, "name").text = "Bracket".into();
    ws.save_props();
    assert!(ws.wait_until(WAIT, |ws| {
        ws.props
            .as_ref()
            .is_some_and(|p| !p.saving && !p.dirty() && p.props[0].field.read == "101")
    }));
    assert_eq!(number_on_disk(&p), "101");
    let log = p.ok(&["activity"]);
    let last = log["entries"].as_array().unwrap().last().unwrap().clone();
    assert_eq!(last["method"], "props.set");
    assert_eq!(last["participant"]["id"], ws.me());
}

#[test]
fn a_stale_save_conflicts_and_shows_what_changed() {
    let p = numbered();
    let mut ws = with_props(&p);
    field(&mut ws, "number").text = "101".into();
    p.ok(&["props", "set", "p", "number=102"]);
    ws.save_props();
    assert!(ws.wait_until(WAIT, |ws| {
        ws.props.as_ref().is_some_and(|p| !p.changed.is_empty())
    }));
    let c = &ws.props.as_ref().unwrap().changed[0];
    assert_eq!((c.theirs.as_str(), c.yours.as_str()), ("102", "101"));
    assert_eq!(number_on_disk(&p), "102");
}

#[test]
fn a_value_the_settings_refuse_keeps_what_was_typed() {
    let p = numbered();
    p.ok(&[
        "pdm",
        "configure",
        "--property",
        r#"finish={"type": "enum", "values": ["anodized", "bare"]}"#,
    ]);
    let mut ws = with_props(&p);
    field(&mut ws, "finish").text = "red".into();
    ws.save_props();
    assert!(ws.wait_until(WAIT, |ws| {
        ws.props.as_ref().is_some_and(|p| p.error.is_some())
    }));
    let e = ws.props.as_ref().unwrap().error.as_ref().unwrap();
    assert_eq!(e.code, ryucad::error::Code::PropertyInvalid);
    assert_eq!(field(&mut ws, "finish").text, "red");
}

#[test]
fn pdm_is_set_up_from_the_window() {
    let p = Project::new().with_part();
    let mut ws = open(&p);
    assert!(ws.wait_until(WAIT, |ws| ws.pdm_config.is_some()));
    assert!(!ws.pdm_ready());
    ws.pdm_init();
    assert!(ws.wait_until(WAIT, |ws| ws.pdm_ready()));
    let config = p.ok(&["pdm", "config"]);
    assert!(config["project"].is_string());
    assert_eq!(config["configured"], true);
}
