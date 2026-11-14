//! The hole checks end to end (ryucad 28.B), in process: one part per
//! finding, each found with its code and severity, every fix it offers
//! sent as it is, and the part then clean of that code. A plate of many
//! well-placed holes checks clean at the defaults.

mod common;

use common::must;
use ryucad::session::Session;
use serde_json::{Value, json};

/// Part `p`: a plate `pl` `x` × `y` × `z` mm and a sketch `t` on its top
/// with a fixed point at each of `points` (mm).
fn plate(size: [f64; 3], points: &[[f64; 2]]) -> (tempfile::TempDir, Session) {
    let dir = tempfile::tempdir().unwrap();
    ryucad::server::init(dir.path(), None).unwrap();
    let mut s = Session::open(dir.path()).unwrap();
    must(&mut s, "part.create", json!({"part": "p"}));
    must(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "box", "id": "pl",
               "params": {"x": size[0], "y": size[1], "z": size[2]}}),
    );
    must(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "sketch", "id": "t",
               "placement": {"origin": [0, 0, size[2]]}}),
    );
    let mut ops = Vec::new();
    for (i, at) in points.iter().enumerate() {
        ops.push(json!({"op": "add", "type": "point", "at": at}));
        ops.push(json!({"op": "add", "type": "fix", "a": format!("p{}", i + 1)}));
    }
    must(
        &mut s,
        "sketch.edit",
        json!({"part": "p", "sketch": "t", "ops": ops}),
    );
    (dir, s)
}

fn findings(s: &mut Session, params: Value) -> Vec<Value> {
    must(s, "holes.check", params)["findings"]
        .as_array()
        .unwrap()
        .clone()
}

/// The finding of `code`, with its severity checked.
fn found(s: &mut Session, params: Value, code: &str, severity: &str) -> Value {
    let f = findings(s, params.clone());
    let x = f
        .iter()
        .find(|x| x["code"] == code)
        .unwrap_or_else(|| panic!("{code}: {f:?}"))
        .clone();
    assert_eq!(x["severity"], severity, "{x}");
    x
}

fn clean_of(s: &mut Session, params: Value, code: &str) {
    let f = findings(s, params);
    assert!(!f.iter().any(|x| x["code"] == code), "{code}: {f:?}");
}

#[test]
fn a_thread_past_its_material_and_its_fixes() {
    // The issue's case: an M6 thread 15 mm into 12 mm of material.
    let (_dir, mut s) = plate([40.0, 40.0, 12.0], &[[20.0, 20.0]]);
    must(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "hole", "id": "m6", "sketch": "t",
               "spec": "M6x1.0-6H 15 DEEP, DRILL 18 DEEP", "targets": ["pl"]}),
    );
    let q = json!({"part": "p"});
    let past = found(&mut s, q.clone(), "hole_thread_past_material", "error");
    found(&mut s, q.clone(), "hole_breaks_through", "error");
    // Its first fix: through, the thread full.
    let fix = &past["fixes"][0];
    must(
        &mut s,
        fix["method"].as_str().unwrap(),
        fix["params"].clone(),
    );
    clean_of(&mut s, q.clone(), "hole_thread_past_material");
    clean_of(&mut s, q, "hole_breaks_through");
}

#[test]
fn bores_that_meet_and_the_fix_that_parts_them() {
    let (_dir, mut s) = plate([40.0, 40.0, 12.0], &[[20.0, 20.0]]);
    must(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "hole", "id": "v", "sketch": "t",
               "spec": "DIA 4 10 DEEP", "targets": ["pl"]}),
    );
    must(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "sketch", "id": "side",
               "placement": {"origin": [40, 0, 0], "x": [0, 1, 0], "z": [1, 0, 0]}}),
    );
    must(
        &mut s,
        "sketch.edit",
        json!({"part": "p", "sketch": "side", "ops": [
            {"op": "add", "type": "point", "at": [20, 6]},
            {"op": "add", "type": "fix", "a": "p1"}]}),
    );
    must(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "hole", "id": "x", "sketch": "side",
               "spec": "DIA 4 25 DEEP", "targets": ["pl"]}),
    );
    let q = json!({"part": "p"});
    let b = found(&mut s, q.clone(), "hole_breakout", "error");
    assert_eq!(b["holes"], json!(["p/v/p1", "p/x/p1"]));
    for fix in b["fixes"].as_array().unwrap() {
        must(
            &mut s,
            fix["method"].as_str().unwrap(),
            fix["params"].clone(),
        );
    }
    clean_of(&mut s, q, "hole_breakout");
}

#[test]
fn a_hole_near_an_edge_says_how_far_to_move() {
    // A Ø4 hole 1 mm from the plate's x = 0 side: its wall is 1 mm off it.
    let (_dir, mut s) = plate([40.0, 40.0, 12.0], &[[3.0, 20.0]]);
    must(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "hole", "id": "e", "sketch": "t",
               "spec": "DIA 4 THRU", "targets": ["pl"]}),
    );
    let q = json!({"part": "p"});
    let f = found(&mut s, q.clone(), "hole_edge_distance", "warning");
    assert_eq!(f["fixes"], json!([]));
    assert!(
        (f["details"]["distance_m"].as_f64().unwrap() - 0.001).abs() < 1e-6,
        "{f}"
    );
    assert!(f["details"]["hint"].as_str().is_some(), "{f}");
    // A smaller --edge-factor, as the hint offers: 0.2 x 4 mm is under
    // the 1 mm there is.
    clean_of(
        &mut s,
        json!({"part": "p", "edge_factor": 0.2}),
        "hole_edge_distance",
    );
}

#[test]
fn a_short_thread_and_a_drill_off_the_drill_series() {
    let (_dir, mut s) = plate([40.0, 40.0, 12.0], &[[20.0, 20.0]]);
    // 1.5 mm of M3 (0.5 x D), on a 2.52 mm drill that's no standard drill.
    must(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "hole", "id": "m3", "sketch": "t",
               "spec": "M3x0.5-6H 1.5 DEEP, DRILL 2.52 6 DEEP", "targets": ["pl"]}),
    );
    let q = json!({"part": "p"});
    let eng = found(&mut s, q.clone(), "hole_thread_engagement", "warning");
    let drill = found(&mut s, q.clone(), "hole_tap_drill_nonstandard", "info");
    for fix in [&eng["fixes"][0], &drill["fixes"][0]] {
        must(
            &mut s,
            fix["method"].as_str().unwrap(),
            fix["params"].clone(),
        );
    }
    clean_of(&mut s, q.clone(), "hole_thread_engagement");
    clean_of(&mut s, q, "hole_tap_drill_nonstandard");
}

#[test]
fn a_plate_of_well_placed_holes_checks_clean() {
    // 6 x 4 holes on a 15 mm grid, 10 mm in from every edge.
    let mut points = Vec::new();
    for i in 0..6 {
        for j in 0..4 {
            points.push([10.0 + 15.0 * f64::from(i), 10.0 + 15.0 * f64::from(j)]);
        }
    }
    let (_dir, mut s) = plate([95.0, 65.0, 12.0], &points);
    must(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "hole", "id": "g", "sketch": "t",
               "spec": "M5 THRU", "targets": ["pl"]}),
    );
    let v = must(&mut s, "holes.check", json!({"part": "p"}));
    assert_eq!(v["checked"], 24);
    assert_eq!(v["findings"], json!([]), "{v}");
    assert_eq!(v["passed"], true);
}
