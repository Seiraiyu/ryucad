//! Settings and preferences from the window, against a real daemon. The
//! tests in this file share one preferences folder, so each uses its own
//! keys.

mod common;

use common::{Project, ryucad};
use ryucad_app::core::workspace::Workspace;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;

const WAIT: Duration = Duration::from_secs(30);

fn open(p: &Project) -> Workspace {
    let mut ws = Workspace::open(&p.root, ryucad(), "Dana", Arc::new(|| {})).unwrap();
    assert!(ws.wait_until(WAIT, |ws| ws.idle() && !ws.settings.all.is_empty()));
    ws
}

#[test]
fn preferences_go_both_ways() {
    let p = Project::new();
    let mut ws = open(&p);
    assert_eq!(ws.settings.text("ui.navigation", ""), "ryucad");
    ws.set_config("ui.navigation", json!("onshape"));
    assert!(
        ws.wait_until(WAIT, |ws| ws.settings.text("ui.navigation", "")
            == "onshape")
    );
    assert_eq!(
        p.ok(&["config", "get", "ui.navigation"])["value"],
        "onshape"
    );
    // An agent's change reaches the window.
    p.ok(&["config", "set", "ui.navigation", "solidworks"]);
    assert!(
        ws.wait_until(WAIT, |ws| ws.settings.text("ui.navigation", "")
            == "solidworks")
    );
    // A bad value is refused, and says why.
    ws.set_config("ui.navigation", json!("blender"));
    assert!(ws.wait_until(WAIT, |ws| !ws.errors.is_empty()));
    assert_eq!(ws.errors[0].code, ryucad::error::Code::InvalidSetting);
}

#[test]
fn a_project_setting_is_guarded_by_its_revision() {
    let p = Project::new();
    let mut ws = open(&p);
    ws.set_config("default_unit", json!("in"));
    assert!(ws.wait_until(WAIT, |ws| ws.idle()
        && ws.settings.text("default_unit", "") == "in"));
    assert_eq!(p.ok(&["config", "get", "default_unit"])["value"], "in");
    // A stale revision: someone changed ryucad.json since the window read it.
    let stale = ws.settings.project_revision.clone();
    p.ok(&["config", "set", "default_unit", "cm"]);
    assert!(ws.wait_until(WAIT, |ws| ws.settings.project_revision != stale));
    ws.settings.project_revision = stale;
    ws.set_config("default_unit", json!("ft"));
    assert!(ws.wait_until(WAIT, |ws| !ws.errors.is_empty()));
    assert_eq!(ws.errors[0].code, ryucad::error::Code::Conflict);
    assert_eq!(p.ok(&["config", "get", "default_unit"])["value"], "cm");
}
