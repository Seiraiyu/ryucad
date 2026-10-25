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

/// The planar face of `part`'s solid `solid` whose outward normal is `z`
/// along z (`1` up, `-1` down).
fn face_up(p: &Project, part: &str, solid: &str, z: f64) -> String {
    let v = p.ok(&["inspect", part, solid]);
    v["bodies"][0]["faces"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| {
            let sign = if f["reversed"] == true { -1.0 } else { 1.0 };
            f["surface"]["type"] == "plane"
                && (f["surface"]["frame"]["z"][2].as_f64().unwrap() * sign - z).abs() < 1e-12
        })
        .map(|f| f["name"].as_str().unwrap().to_owned())
        .unwrap()
}

/// A plate fixed in `frame` and a bolt stood on its top by revolute `m1`,
/// with three copies of the bolt round the plate's z (`ring`).
fn mated() -> Project {
    let p = Project::new();
    p.ok(&["part", "create", "plate"]);
    p.ok(&[
        "feature", "add", "plate", "box", "--id", "slab", "--param", "x=100", "--param", "y=50",
        "--param", "z=5",
    ]);
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
        "feature", "add", "frame", "instance", "--id", "base", "--of", "plate", "--fixed",
    ]);
    p.ok(&[
        "feature", "add", "frame", "instance", "--id", "pin", "--of", "bolt", "--origin", "50,0,0",
    ]);
    let top = format!("frame/base/slab/{}", face_up(&p, "plate", "slab", 1.0));
    let foot = format!("frame/pin/shank/{}", face_up(&p, "bolt", "shank", -1.0));
    p.ok(&[
        "feature",
        "add",
        "frame",
        "mate",
        "--id",
        "m1",
        "--form",
        "revolute",
        "--a",
        &top,
        "--a-offset",
        r#"{"origin": ["20 mm", "10 mm", "0 mm"]}"#,
        "--b",
        &foot,
        "--b-offset",
        r#"{"flip": true}"#,
    ]);
    p.ok(&[
        "feature",
        "add",
        "frame",
        "instance_pattern",
        "--id",
        "ring",
        "--instance",
        "pin",
        "--axis",
        "z",
        "--count",
        "4",
        "--angle",
        "360deg",
    ]);
    p
}

#[test]
fn the_tree_says_what_mates_leave_and_a_mate_paints_its_faces() {
    let p = mated();
    let mut ws = Workspace::open(&p.root, ryucad(), "Dana", Arc::new(|| {})).unwrap();
    assert!(ws.wait_until(WAIT, |ws| {
        ws.idle()
            && ws
                .part("frame")
                .is_some_and(|f| f.feature("ring").is_some())
    }));
    let frame = ws.part("frame").unwrap();
    let solve = frame.solve.as_ref().unwrap();
    assert!(solve.converged && !solve.stale, "{solve:?}");
    let pin = frame.feature("pin").unwrap().instance.as_ref().unwrap();
    assert_eq!(pin.solve, Some(("under".to_owned(), 1)));
    let m = frame.feature("m1").unwrap().mate.as_ref().unwrap();
    assert_eq!(
        (m.form.as_str(), m.state.as_deref()),
        ("revolute", Some("well"))
    );
    assert_eq!(
        frame.feature("ring").unwrap().copies,
        ["ring:1/pin", "ring:2/pin", "ring:3/pin"]
    );
    // The copies are drawn, under the pattern.
    assert!(ws.wait_until(WAIT, |ws| {
        ws.model
            .meshes
            .contains_key(&key("frame", "ring:2/pin/shank"))
    }));
    // Chosen, the mate paints the faces its connectors are on.
    ws.choose(Some(key("frame", "m1")));
    let made = ws.made.as_ref().expect("the mate's faces");
    let bodies: Vec<&String> = made.faces.keys().map(|(_, b)| b).collect();
    assert_eq!(bodies, ["base/slab", "pin/shank"]);
}
