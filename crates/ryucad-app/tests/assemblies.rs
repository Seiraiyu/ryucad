//! Assemblies in the window against a real daemon: an assembly's
//! occurrences drawn where its instances put them, the project's one root
//! assembly shown by default, and a focus an agent sets followed.

mod common;

use common::{Project, ryucad};
use ryucad_app::core::workspace::Workspace;
use std::sync::Arc;
use std::time::Duration;

const WAIT: Duration = Duration::from_secs(30);

/// A bolt and an assembly placing it twice.
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
    p.ok(&[
        "feature", "add", "frame", "instance", "--id", "b1", "--of", "bolt",
    ]);
    p.ok(&[
        "feature", "add", "frame", "instance", "--id", "b2", "--of", "bolt", "--origin", "40,0,0",
    ]);
    p
}

fn key(part: &str, f: &str) -> (String, String) {
    (part.to_owned(), f.to_owned())
}

#[test]
fn the_window_draws_an_assembly_where_it_places_its_parts() {
    let p = assembled();
    let mut ws = Workspace::open(&p.root, ryucad(), "Dana", Arc::new(|| {})).unwrap();
    assert!(ws.wait_until(WAIT, |ws| {
        ws.idle()
            && ws.model.meshes.contains_key(&key("frame", "b1/shank"))
            && ws.model.meshes.contains_key(&key("frame", "b2/shank"))
    }));
    let frame = ws.part("frame").unwrap();
    assert!(frame.assembly);
    assert_eq!(frame.features[1].instance.as_ref().unwrap().part, "bolt");
    // The second bolt is 40 mm along x from the first.
    let origin = |k: &str| {
        ws.model.meshes[&key("frame", k)]
            .mesh
            .as_ref()
            .unwrap()
            .origin
    };
    let (a, b) = (origin("b1/shank"), origin("b2/shank"));
    assert!((b[0] - a[0] - 0.04).abs() < 1e-9, "{a:?} {b:?}");
    // The frame is the one root: it is what the window draws.
    assert_eq!(ws.model.shown(), ["frame"]);
    let drawn: Vec<_> = ws.model.drawn().map(|(k, _)| k.clone()).collect();
    assert_eq!(drawn, [key("frame", "b1/shank"), key("frame", "b2/shank")]);
    // An agent shows the human the bolt alone.
    p.ok(&["focus", "bolt"]);
    assert!(ws.wait_until(WAIT, |ws| ws.model.focus.as_deref() == Some("bolt")));
    assert!(ws.wait_until(WAIT, |ws| {
        ws.model.meshes.contains_key(&key("bolt", "shank"))
    }));
    let drawn: Vec<_> = ws.model.drawn().map(|(k, _)| k.clone()).collect();
    assert_eq!(drawn, [key("bolt", "shank")]);
    // A move fetches the moved bolt again, where it is now.
    ws.set_focus(None);
    p.ok(&["assembly", "move", "frame", "b2", "--translate", "0,10,0"]);
    assert!(ws.wait_until(WAIT, |ws| {
        ws.idle()
            && ws.model.focus.is_none()
            && ws
                .model
                .meshes
                .get(&key("frame", "b2/shank"))
                .is_some_and(|m| (m.mesh.as_ref().unwrap().origin[1] - 0.01).abs() < 1e-9)
    }));
}

#[test]
fn the_form_edits_an_instance() {
    let p = assembled();
    let mut ws = Workspace::open(&p.root, ryucad(), "Dana", Arc::new(|| {})).unwrap();
    assert!(ws.wait_until(WAIT, |ws| ws.idle() && ws.part("frame").is_some()));
    ws.open_form(Some(key("frame", "b2")));
    assert!(ws.wait_until(WAIT, |ws| ws.form.is_some()));
    let f = ws.form.as_mut().unwrap();
    assert_eq!(f.ty, "instance");
    assert_eq!(f.origin[0].read, "0.04 m");
    assert_eq!(f.rev.as_ref().unwrap().read, "working");
    f.origin[1].text = "15 mm".into();
    f.fixed.as_mut().unwrap().chosen = true;
    ws.save_form();
    assert!(ws.wait_until(WAIT, |ws| {
        ws.form.as_ref().is_some_and(|f| !f.saving && !f.dirty())
    }));
    let shown = p.ok(&["feature", "show", "frame", "b2"]);
    assert_eq!(
        shown["pose"]["origin_m"],
        serde_json::json!([0.04, 0.015, 0])
    );
    assert_eq!(shown["fixed"], true);
    // The tree says so.
    assert!(ws.wait_until(WAIT, |ws| {
        ws.part("frame")
            .and_then(|p| p.feature("b2"))
            .and_then(|f| f.instance.as_ref())
            .is_some_and(|i| i.fixed)
    }));
}
