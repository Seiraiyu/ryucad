//! Solids in the window against a real daemon: one mesh per solid as the
//! model shows it, features grouped under the solids they act on, a
//! chosen feature painted by the faces it made, and the form editing what
//! a feature acts on, a fillet's sizes and edges, and a hole's and a
//! pattern's sizes, counts and skips.

mod common;

use common::{Project, ryucad};
use ryucad_app::core::workspace::Workspace;
use std::sync::Arc;
use std::time::Duration;

const WAIT: Duration = Duration::from_secs(30);

fn key(f: &str) -> (String, String) {
    ("p".to_owned(), f.to_owned())
}

/// The window, once the model and every solid's mesh have come.
fn open(p: &Project) -> Workspace {
    let mut ws = Workspace::open(&p.root, ryucad(), "Dana", Arc::new(|| {})).unwrap();
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

#[test]
fn the_window_shows_solids() {
    let p = Project::new().with_part();
    // A slot cut across the base, and the boss joined to it.
    p.ok(&[
        "feature",
        "add",
        "p",
        "box",
        "--id",
        "slot",
        "--operation",
        "cut",
        "--target",
        "base",
        "--param",
        "x=4",
        "--param",
        "y=20",
        "--param",
        "z=2",
        "--origin",
        "8,-5,4",
    ]);
    p.ok(&[
        "feature", "add", "p", "boolean", "--id", "whole", "--op", "unite", "--target", "base",
        "--tool", "boss",
    ]);
    let mut ws = open(&p);
    // One solid, and its mesh; the boss is the base's now.
    let part = ws.part("p").unwrap();
    assert_eq!(part.solids.len(), 1);
    assert_eq!(part.solids[0].features, ["base", "slot", "whole"]);
    assert_eq!(ws.model.meshes.len(), 1);
    assert!(ws.model.meshes.contains_key(&key("base")));
    let slot = part.feature("slot").unwrap();
    assert_eq!(slot.operation.as_deref(), Some("cut"));
    assert_eq!(part.feature("whole").unwrap().consumes, ["boss"]);
    // A feature shows in its solid.
    assert_eq!(ws.model.solid_of(&key("slot")), Some(key("base")));
    assert_eq!(ws.model.solid_of(&key("boss")), None);
    // Choosing the slot paints the faces it made: its walls and floor, in
    // the base.
    ws.choose(Some(key("slot")));
    assert!(ws.wait_until(WAIT, |ws| ws.made.is_some()));
    let made = ws.made.clone().unwrap();
    assert_eq!(made.feature, key("slot"));
    let faces = &made.faces[&key("base")];
    assert_eq!(faces.len(), 3, "{faces:?}");
    // An edit moves what it made with it.
    p.ok(&[
        "feature", "edit", "p", "slot", "--param", "z=10", "--origin", "8,-5,-1",
    ]);
    let before = made.revision.clone();
    assert!(ws.wait_until(WAIT, |ws| {
        ws.made.as_ref().is_some_and(|m| m.revision != before)
    }));
    // Through the slot: its floor is gone, its walls go right through.
    assert_eq!(ws.made.as_ref().unwrap().faces[&key("base")].len(), 2);
    ws.choose(None);
    assert!(ws.made.is_none());
}

#[test]
fn the_form_edits_what_a_feature_acts_on() {
    let p = Project::new().with_part();
    p.ok(&[
        "feature",
        "add",
        "p",
        "box",
        "--id",
        "slot",
        "--operation",
        "cut",
        "--target",
        "base",
        "--param",
        "x=4",
        "--param",
        "y=20",
        "--param",
        "z=4",
        "--origin=-2,-5,3",
    ]);
    let mut ws = open(&p);
    ws.open_form(Some(key("slot")));
    assert!(ws.wait_until(WAIT, |ws| ws.form.is_some()));
    let form = ws.form.as_mut().unwrap();
    assert_eq!(form.operation.as_ref().unwrap().chosen, "cut");
    let targets = form.targets.as_mut().unwrap();
    assert_eq!(targets.options, ["base", "boss"]);
    // The slot cuts the boss too: the same feature.edit an agent sends.
    targets.toggle("boss");
    ws.save_form();
    assert!(ws.wait_until(WAIT, |ws| {
        ws.form.as_ref().is_some_and(|f| !f.saving && !f.dirty())
    }));
    let shown = p.ok(&["feature", "show", "p", "slot"]);
    assert_eq!(shown["targets"], serde_json::json!(["base", "boss"]));
    // A refusal says why, and keeps what was chosen.
    let form = ws.form.as_mut().unwrap();
    form.operation.as_mut().unwrap().chosen = "intersect".into();
    form.targets.as_mut().unwrap().toggle("base");
    form.targets.as_mut().unwrap().toggle("boss");
    ws.save_form();
    assert!(ws.wait_until(WAIT, |ws| {
        ws.form
            .as_ref()
            .is_some_and(|f| !f.saving && f.error.is_some())
    }));
    assert!(ws.form.as_ref().unwrap().dirty());
}

/// The reference to the base's vertical edge at (`x`, `y`) mm.
fn upright(p: &Project, x: f64, y: f64) -> String {
    let body = p.ok(&["inspect", "p", "base"])["bodies"][0].clone();
    let at = |v: &serde_json::Value, z: f64| {
        let want = [x * 1e-3, y * 1e-3, z * 1e-3];
        (0..3).all(|k| (v["point"][k].as_f64().unwrap() - want[k]).abs() < 1e-9)
    };
    let vertex = |z: f64| {
        body["vertices"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| at(v, z))
            .map(|v| v["name"].clone())
            .unwrap()
    };
    let (a, b) = (vertex(0.0), vertex(5.0));
    let e = body["edges"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| (e["start"] == a && e["end"] == b) || (e["start"] == b && e["end"] == a))
        .unwrap();
    format!("p/base/{}", e["name"].as_str().unwrap())
}

#[test]
fn a_fillet_is_painted_and_its_form_edits_it() {
    let p = Project::new().with_part();
    let (near, far) = (upright(&p, 20.0, 0.0), upright(&p, 20.0, 10.0));
    p.ok(&[
        "feature", "add", "p", "fillet", "--id", "round", "--radius", "1", "--edge", &near,
        "--edge", &far,
    ]);
    let mut ws = open(&p);
    let part = ws.part("p").unwrap();
    assert_eq!(part.solids[0].features, ["base", "round"]);
    assert_eq!(ws.model.solid_of(&key("round")), Some(key("base")));
    // Choosing the fillet paints the faces that took its edges' places.
    ws.choose(Some(key("round")));
    assert!(ws.wait_until(WAIT, |ws| ws.made.is_some()));
    assert_eq!(ws.made.as_ref().unwrap().faces[&key("base")].len(), 2);
    // Its form: the radius as written, the edges it rounds.
    ws.open_form(Some(key("round")));
    assert!(ws.wait_until(WAIT, |ws| ws.form.is_some()));
    let form = ws.form.as_mut().unwrap();
    let b = form.blend.as_mut().unwrap();
    assert_eq!(b.groups[0].sizes[0].1.read, "1 mm");
    assert_eq!(b.groups[0].edges.read, [near.clone(), far.clone()]);
    // A person makes it larger and takes one edge out: the same
    // feature.edit an agent sends.
    b.groups[0].sizes[0].1.text = "1.5 mm".into();
    b.groups[0].edges.toggle(&far);
    ws.save_form();
    assert!(ws.wait_until(WAIT, |ws| {
        ws.form.as_ref().is_some_and(|f| !f.saving && !f.dirty())
    }));
    let shown = p.ok(&["feature", "show", "p", "round"]);
    assert_eq!(shown["groups"][0]["radius"]["value"], "1.5 mm");
    assert_eq!(shown["groups"][0]["edges"], serde_json::json!([near]));
    // And what it made follows: one face now.
    assert!(ws.wait_until(WAIT, |ws| {
        ws.made
            .as_ref()
            .is_some_and(|m| m.faces.get(&key("base")).is_some_and(|f| f.len() == 1))
    }));
}

#[test]
fn a_pattern_of_holes_is_painted_and_its_forms_edit_it() {
    let p = Project::new().with_part();
    // Holes from the base's top at (14, 3) mm, repeated along y.
    p.ok(&[
        "feature", "add", "p", "sketch", "--id", "s", "--origin", "0,0,5",
    ]);
    let ops = r#"[{"op": "add", "type": "point", "at": [14, 3]},
                  {"op": "add", "type": "fix", "a": "p1"}]"#;
    p.ok(&["sketch", "edit", "p", "s", ops]);
    p.ok(&[
        "feature",
        "add",
        "p",
        "hole",
        "--id",
        "h",
        "--sketch",
        "s",
        "--target",
        "base",
        "--diameter",
        "1",
        "--depth",
        "2",
    ]);
    p.ok(&[
        "feature",
        "add",
        "p",
        "pattern",
        "--id",
        "row",
        "--seed",
        "h",
        "--direction",
        "y",
        "--spacing",
        "2",
        "--count",
        "3",
    ]);
    let mut ws = open(&p);
    let part = ws.part("p").unwrap();
    let base = part.solids.iter().find(|s| s.id == "base").unwrap();
    assert_eq!(base.features, ["base", "h", "row"]);
    // Choosing the pattern paints what its two instances made: a wall
    // and a floor each.
    ws.choose(Some(key("row")));
    assert!(ws.wait_until(WAIT, |ws| ws.made.is_some()));
    assert_eq!(ws.made.as_ref().unwrap().faces[&key("base")].len(), 4);
    // The hole's form: a person makes it wider.
    ws.open_form(Some(key("h")));
    assert!(ws.wait_until(WAIT, |ws| ws.form.is_some()));
    let h = ws.form.as_mut().unwrap().hole.as_mut().unwrap();
    assert_eq!(h.sizes[0].1.read, "1 mm");
    h.sizes[0].1.text = "1.2 mm".into();
    ws.save_form();
    assert!(ws.wait_until(WAIT, |ws| {
        ws.form.as_ref().is_some_and(|f| !f.saving && !f.dirty())
    }));
    assert_eq!(
        p.ok(&["feature", "show", "p", "h"])["diameter"]["value"],
        "1.2 mm"
    );
    // The pattern's: one more, and the middle one skipped.
    ws.open_form(Some(key("row")));
    assert!(ws.wait_until(WAIT, |ws| {
        ws.form.as_ref().is_some_and(|f| f.feature == "row")
    }));
    let r = ws.form.as_mut().unwrap().pattern.as_mut().unwrap();
    r.sizes[1].1.text = "4".into();
    r.skip.text = "[2]".into();
    ws.save_form();
    assert!(ws.wait_until(WAIT, |ws| {
        ws.form.as_ref().is_some_and(|f| !f.saving && !f.dirty())
    }));
    let shown = p.ok(&["feature", "show", "p", "row"]);
    assert_eq!(shown["count"]["value"], "4");
    assert_eq!(shown["skip"], serde_json::json!([2]));
    assert!(ws.wait_until(WAIT, |ws| {
        ws.made
            .as_ref()
            .is_some_and(|m| m.faces.get(&key("base")).is_some_and(|f| f.len() == 4))
    }));
}
