//! Offsets against exact volumes, through the protocol: a box grown by
//! Steiner's formula and shrunk, a pocket's floor raised, a boss's top
//! pulled down, a cylinder grown; names kept across distance edits;
//! refusals.

mod common;

use common::{call, must, project};
use core::f64::consts::PI;
use ryucad::doc::id::Id;
use ryucad::error::Code;
use ryucad::session::Session;
use serde_json::{Value, json};

fn add(s: &mut Session, f: Value) -> Value {
    let mut f = f;
    f["part"] = json!("p");
    must(s, "feature.add", f)
}

/// [`project`] with a box `b` 40 × 30 × 20 mm.
fn boxed() -> (tempfile::TempDir, Session) {
    let (dir, mut s) = project();
    add(
        &mut s,
        json!({"type": "box", "id": "b", "params": {"x": 40, "y": 30, "z": 20}}),
    );
    (dir, s)
}

/// What feature `f` leaves, in `inspect`.
fn body(s: &mut Session, f: &str) -> Value {
    must(s, "inspect", json!({"part": "p", "feature": f}))["bodies"][0].clone()
}

/// `body`'s planar face whose outward normal along z is `n`, at `z` mm,
/// as a reference into `solid`.
fn plane(body: &Value, solid: &str, n: f64, z: f64) -> String {
    let f = body["faces"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| {
            let sign = if f["reversed"] == true { -1.0 } else { 1.0 };
            f["surface"]["type"] == "plane"
                && f["surface"]["frame"]["z"][2].as_f64().unwrap() * sign == n
                && (f["surface"]["frame"]["origin"][2].as_f64().unwrap() - z * 1e-3).abs() < 1e-9
        })
        .unwrap_or_else(|| panic!("no face facing {n} at {z} mm"));
    format!("p/{solid}/{}", f["name"].as_str().unwrap())
}

/// The volume in mm³ of solid `solid` as feature `f` left it, after the
/// kernel's checks, meshed finely.
fn volume(s: &mut Session, f: &str, solid: &str) -> f64 {
    let part = Id::parse("p", "part").unwrap();
    let r = s.rebuilt(&part).unwrap();
    let (_, out) = r
        .features
        .iter()
        .find(|(i, _)| i.as_str() == f)
        .expect("listed");
    let out = out.as_ref().unwrap_or_else(|e| panic!("{f}: {e}"));
    let b = &out
        .solid(&Id::parse(solid, "feature").unwrap())
        .expect("the solid")
        .body;
    assert!(ryukei::check::check(b).is_empty(), "{f}");
    assert_eq!(ryukei::check::check_intersections(b), [], "{f}");
    let m = ryukei::mesh::tessellate(b, &ryukei::mesh::MeshParams::new(2e-7, 0.05)).unwrap();
    assert!(m.faults.is_empty(), "{f}: {:?}", m.faults);
    ryukei_testkit::mesh::volume_and_area(&m).0 * 1e9
}

/// `got` within a part in `rel` of `want`.
fn near(got: f64, want: f64, rel: f64) {
    assert!(
        (got - want).abs() <= rel * want,
        "{got} mm³ against {want} mm³"
    );
}

#[test]
fn a_box_grown_and_shrunk() {
    let (_dir, mut s) = boxed();
    add(&mut s, json!({"type": "offset", "id": "o", "distance": 1}));
    let (a, b, c, d) = (40.0, 30.0, 20.0, 1.0);
    let steiner = a * b * c
        + 2.0 * (a * b + b * c + c * a) * d
        + PI * d * d * (a + b + c)
        + 4.0 / 3.0 * PI * d * d * d;
    near(volume(&mut s, "o", "b"), steiner, 1e-4);
    must(
        &mut s,
        "feature.edit",
        json!({"part": "p", "id": "o", "distance": "-1 mm"}),
    );
    near(volume(&mut s, "o", "b"), 38.0 * 28.0 * 18.0, 1e-9);
}

#[test]
fn a_pockets_floor_raised_and_a_bosss_top_pulled() {
    let (_dir, mut s) = boxed();
    add(
        &mut s,
        json!({"type": "box", "id": "k", "operation": "cut", "targets": ["b"],
               "params": {"x": 20, "y": 10, "z": 30}, "placement": {"origin": [10, 10, 15]}}),
    );
    let floor = plane(&body(&mut s, "k"), "b", 1.0, 15.0);
    add(
        &mut s,
        json!({"type": "offset", "id": "o", "distance": 0,
               "faces": [{"distance": 3, "faces": [floor]}]}),
    );
    near(volume(&mut s, "o", "b"), 24000.0 - 1000.0 + 600.0, 1e-9);
    let (_dir, mut s) = boxed();
    add(
        &mut s,
        json!({"type": "box", "id": "j", "operation": "join", "targets": ["b"],
               "params": {"x": 10, "y": 10, "z": 10}, "placement": {"origin": [5, 5, 20]}}),
    );
    let lid = plane(&body(&mut s, "j"), "b", 1.0, 30.0);
    add(
        &mut s,
        json!({"type": "offset", "id": "o", "distance": 0,
               "faces": [{"distance": "-2 mm", "faces": [lid]}]}),
    );
    near(volume(&mut s, "o", "b"), 25000.0 - 200.0, 1e-9);
}

#[test]
fn a_cylinder_grown() {
    let (_dir, mut s) = project();
    add(
        &mut s,
        json!({"type": "cylinder", "id": "c", "params": {"radius": 10, "height": 20}}),
    );
    add(&mut s, json!({"type": "offset", "id": "o", "distance": 1}));
    let (r, h, d) = (10.0, 20.0, 1.0);
    let want = PI * r * r * h
        + (2.0 * PI * r * h + 2.0 * PI * r * r) * d
        + PI * (PI * r + h) * d * d
        + 4.0 / 3.0 * PI * d * d * d;
    near(volume(&mut s, "o", "c"), want, 1e-4);
}

#[test]
fn names_last_across_distance_edits() {
    let (_dir, mut s) = boxed();
    add(&mut s, json!({"type": "offset", "id": "o", "distance": 1}));
    let names = |s: &mut Session| -> Vec<String> {
        let b = body(s, "o");
        let mut n: Vec<String> = ["faces", "edges", "vertices"]
            .iter()
            .flat_map(|k| b[*k].as_array().unwrap().clone())
            .map(|e| e["name"].as_str().unwrap().to_owned())
            .collect();
        n.sort();
        n
    };
    let before = names(&mut s);
    must(
        &mut s,
        "feature.edit",
        json!({"part": "p", "id": "o", "distance": "2.5 mm"}),
    );
    assert_eq!(names(&mut s), before);
}

#[test]
fn refusals() {
    let (_dir, mut s) = boxed();
    // Nothing moving: refused when written.
    let e = call(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "offset", "distance": 0}),
    )
    .unwrap_err();
    assert_eq!(
        (e.code, e.details["pointer"].as_str()),
        (Code::InvalidParam, Some("/distance"))
    );
    // Shrunk past half the box: the rebuild says the kernel's reason.
    add(
        &mut s,
        json!({"type": "offset", "id": "o", "distance": -11}),
    );
    add(
        &mut s,
        json!({"type": "cylinder", "id": "c", "params": {"radius": 3, "height": 10},
               "placement": {"origin": [60, 0, 0]}}),
    );
    // A cylinder shrunk past its axis.
    add(
        &mut s,
        json!({"type": "offset", "id": "thin", "target": "c", "distance": -3.5}),
    );
    let part = Id::parse("p", "part").unwrap();
    let r = s.rebuilt(&part).unwrap();
    let failed = |f: &str| {
        let (_, out) = r.features.iter().find(|(i, _)| i.as_str() == f).unwrap();
        out.as_ref().unwrap_err().clone()
    };
    let e = failed("o");
    assert_eq!(
        (e.code, e.details["reason"].as_str()),
        (Code::OffsetFailed, Some("empty")),
        "{e}"
    );
    assert!(e.details.get("hint").is_none(), "{e}");
    let e = failed("thin");
    assert_eq!(
        (e.code, e.details["reason"].as_str()),
        (Code::OffsetFailed, Some("empty")),
        "{e}"
    );
}
