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
fn the_disk_cache_is_shown_and_cleared() {
    let p = Project::new().with_part();
    let mut ws = open(&p);
    assert!(ws.wait_until(WAIT, |ws| ws.idle() && ws.model.meshes.len() == 2));
    ws.read_cache();
    assert!(ws.wait_until(WAIT, |ws| ws.cache.is_some()));
    let held = ws.cache.clone().unwrap();
    assert_eq!(held["enabled"], true);
    assert_eq!(held["kinds"]["features"]["entries"], 2, "{held}");
    assert_eq!(held["kinds"]["meshes"]["entries"], 2, "{held}");
    assert!(
        ryucad_app::ui::dialogs::cache_text(Some(&held)).starts_with("4 entries, "),
        "{held}"
    );
    ws.clear_cache();
    assert!(ws.wait_until(WAIT, |ws| {
        ws.cache.as_ref().is_some_and(|c| c["entries"] == 0)
    }));
    assert_eq!(ws.cache.as_ref().unwrap()["removed"]["entries"], 4);
    assert_eq!(p.ok(&["cache", "stats"])["entries"], 0);
    assert_eq!(ryucad_app::ui::dialogs::cache_text(None), "reading…");
}

#[test]
fn the_shared_cache_is_set_and_its_token_stored() {
    use ryucad_app::ui::dialogs::{shared_text, token_text};
    let p = Project::new();
    let mut ws = open(&p);
    let url = "https://cache.example.com";
    ws.set_config("cache.remote", json!(url));
    ws.read_cache();
    assert!(ws.wait_until(WAIT, |ws| {
        ws.cache.as_ref().is_some_and(|c| c["remote"]["url"] == url)
    }));
    // The app's daemon is a development build: it says so.
    let text = shared_text(ws.cache.as_ref());
    assert!(text.contains("development build"), "{text}");
    ws.login_cache(" ryc_secret ");
    assert!(ws.wait_until(WAIT, |ws| ws.cache_token.is_some()));
    let t = ws.cache_token.take().unwrap();
    assert_eq!(t["token"], "file", "{t}");
    assert_eq!(token_text(&t), "Saved");
    let file = std::path::PathBuf::from(t["file"].as_str().unwrap());
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "ryc_secret\n");
    ws.login_cache("two words");
    assert!(ws.wait_until(WAIT, |ws| ws.cache_token.is_some()));
    let t = ws.cache_token.take().unwrap();
    assert!(token_text(&t).starts_with("Not saved: "), "{t}");
    ws.logout_cache();
    assert!(ws.wait_until(WAIT, |ws| ws.cache_token.is_some()));
    assert_eq!(
        token_text(ws.cache_token.as_ref().unwrap()),
        "No token stored"
    );
    assert!(!file.exists());
    ws.set_config("cache.remote", json!(""));
    assert!(ws.wait_until(WAIT, |ws| ws.idle()
        && ws.settings.text("cache.remote", "?").is_empty()));
}

#[test]
fn the_shared_cache_says_what_it_did() {
    use ryucad_app::ui::dialogs::{shared_text, token_text};
    assert_eq!(shared_text(None), "reading…");
    assert_eq!(
        shared_text(Some(&json!({"remote": {"enabled": false}}))),
        "None set: this machine builds what it lacks itself"
    );
    let on = json!({"remote": {"enabled": true, "url": "https://c", "write": true,
        "hits": 7, "puts": 2, "bad": 0, "problem": null}});
    assert_eq!(
        shared_text(Some(&on)),
        "https://c, reading and sending: 7 read, 2 sent, 0 bad"
    );
    let skipped = json!({"remote": {"enabled": true, "url": "https://c", "write": false,
        "hits": 0, "puts": 0, "bad": 1, "problem": {"message": "it answered 503"}}});
    assert_eq!(
        shared_text(Some(&skipped)),
        "https://c, skipped for now: it answered 503: 0 read, 0 sent, 1 bad"
    );
    let read = json!({"token": "file", "server": {"write": false, "entries": 3}});
    assert_eq!(token_text(&read), "Saved: the server lets it read only");
    let env = json!({"token": "variable", "overridden_by": "RYUCAD_CACHE_TOKEN",
        "server": {"error": {"message": "the shared cache at https://c refused the token"}}});
    assert_eq!(
        token_text(&env),
        "Saved, but RYUCAD_CACHE_TOKEN is sent instead; the server says: \
         the shared cache at https://c refused the token"
    );
    // Forgetting the stored token while the variable is set: removed, and
    // the variable's token still goes.
    let gone = json!({"token": "variable", "overridden_by": "RYUCAD_CACHE_TOKEN",
        "removed": true, "server": {"write": true, "entries": 3}});
    assert_eq!(
        token_text(&gone),
        "Removed; RYUCAD_CACHE_TOKEN is still sent: the server lets it read and write"
    );
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
