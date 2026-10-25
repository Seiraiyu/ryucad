//! Holes against exact volumes, through the protocol: a plate drilled
//! with every form and bottom, blind, through all and up to a face, from
//! a sketch on its top face. Every result passes the kernel's checks, and
//! `ryucad check` finds nothing wrong.

use core::f64::consts::PI;
use ryucad::doc::id::Id;
use ryucad::proto::Request;
use ryucad::server::handle;
use ryucad::session::Session;
use serde_json::{Value, json};

fn call(s: &mut Session, method: &str, params: Value) -> ryucad::error::Result<Value> {
    handle(s, Request::from_rpc(method, Some(params)).unwrap())
}

fn must(s: &mut Session, method: &str, params: Value) -> Value {
    call(s, method, params).unwrap_or_else(|e| panic!("{method}: {e}"))
}

/// Part `p`: a plate `b` 40 × 30 × 10 mm and a sketch `s` on its top face
/// with a fixed point at each of `points` (mm).
fn plate(points: &[[f64; 2]]) -> (tempfile::TempDir, Session) {
    let dir = tempfile::tempdir().unwrap();
    ryucad::server::init(dir.path(), None).unwrap();
    let mut s = Session::open(dir.path()).unwrap();
    must(&mut s, "part.create", json!({"part": "p"}));
    must(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "box", "id": "b", "params": {"x": 40, "y": 30, "z": 10}}),
    );
    let body = must(&mut s, "inspect", json!({"part": "p", "feature": "b"}))["bodies"][0].clone();
    let top = face(&body, 1.0);
    must(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "sketch", "id": "s", "face": format!("p/b/{top}")}),
    );
    let mut ops = Vec::new();
    for (i, at) in points.iter().enumerate() {
        ops.push(json!({"op": "add", "type": "point", "at": at}));
        ops.push(json!({"op": "add", "type": "fix", "a": format!("p{}", i + 1)}));
    }
    must(
        &mut s,
        "sketch.edit",
        json!({"part": "p", "sketch": "s", "ops": ops}),
    );
    (dir, s)
}

/// The name of `body`'s planar face whose outward normal is `z` along
/// the z axis.
fn face(body: &Value, z: f64) -> String {
    body["faces"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| {
            let sign = if f["reversed"] == true { -1.0 } else { 1.0 };
            f["surface"]["type"] == "plane"
                && f["surface"]["frame"]["z"][2].as_f64().unwrap() * sign == z
        })
        .map(|f| f["name"].as_str().unwrap().to_owned())
        .unwrap()
}

/// Solid `b` as the part shows it: its volume in mm³ from a fine mesh,
/// after the kernel's checks, the self-intersection check among them.
fn volume(s: &mut Session) -> f64 {
    let part = Id::parse("p", "part").unwrap();
    let r = s.rebuilt(&part).unwrap().clone();
    assert_eq!(r.failed(), 0, "{:?}", r.features);
    let (_, last) = r.features.last().unwrap();
    let b = last
        .as_ref()
        .unwrap()
        .solid(&Id::parse("b", "feature").unwrap())
        .unwrap();
    assert!(ryukei::check::check(&b.body).is_empty());
    assert_eq!(ryukei::check::check_intersections(&b.body), []);
    let m = ryukei::mesh::tessellate(&b.body, &ryukei::mesh::MeshParams::new(2e-6, 0.1)).unwrap();
    assert!(m.faults.is_empty(), "{:?}", m.faults);
    ryukei_testkit::mesh::volume_and_area(&m).0 * 1e9
}

fn near(got: f64, want: f64) {
    assert!((got - want).abs() < 2e-4 * want, "{got} mm³ != {want} mm³");
}

#[test]
fn every_form_against_its_volume() {
    let (_dir, mut s) = plate(&[
        [8.0, 8.0],
        [20.0, 8.0],
        [32.0, 8.0],
        [8.0, 22.0],
        [20.0, 22.0],
    ]);
    let plate = 40.0 * 30.0 * 10.0;
    let add = |s: &mut Session, f: Value| {
        let mut f = f;
        f["part"] = json!("p");
        f["type"] = json!("hole");
        f["sketch"] = json!("s");
        must(s, "feature.add", f)
    };
    // A blind hole with a drill point: a cylinder and a cone.
    add(
        &mut s,
        json!({"id": "a", "points": ["p1"], "diameter": 4, "depth": {"blind": 6},
               "bottom": {"drill": "118 deg"}}),
    );
    let tip = 2.0 / ryukei::math::tan(59f64.to_radians());
    let mut removed = PI * 4.0 * 6.0 + PI * 4.0 * tip / 3.0;
    near(volume(&mut s), plate - removed);
    // Counterbored through the plate: two cylinders.
    add(
        &mut s,
        json!({"id": "c", "points": ["p2"], "form": "counterbore", "diameter": 6.6,
               "counterbore": {"diameter": 11, "depth": 6.8}, "depth": "through_all"}),
    );
    removed += PI * 5.5 * 5.5 * 6.8 + PI * 3.3 * 3.3 * 3.2;
    near(volume(&mut s), plate - removed);
    // Countersunk at 90 deg: a frustum, then the bore.
    add(
        &mut s,
        json!({"id": "k", "points": ["p3"], "form": "countersink", "diameter": 4,
               "countersink": {"diameter": 8, "angle": 90}, "depth": {"blind": 8}}),
    );
    removed += PI * 2.0 * (16.0 + 8.0 + 4.0) / 3.0 + PI * 4.0 * 6.0;
    near(volume(&mut s), plate - removed);
    // Up to the plate's bottom face, 2 mm short, at two points at once.
    let body = must(&mut s, "inspect", json!({"part": "p", "feature": "b"}))["bodies"][0].clone();
    let bottom = format!("p/b/{}", face(&body, -1.0));
    add(
        &mut s,
        json!({"id": "u", "points": ["p4", "p5"], "diameter": 3,
               "depth": {"up_to": {"target": bottom, "offset": "-2 mm"}}}),
    );
    removed += 2.0 * PI * 2.25 * 8.0;
    near(volume(&mut s), plate - removed);
    let v = call(&mut s, "check", json!({"part": "p"}));
    assert!(v.is_ok(), "{v:?}");
}

#[test]
fn resizing_a_hole_renames_nothing() {
    let (_dir, mut s) = plate(&[[10.0, 10.0]]);
    must(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "hole", "id": "h", "sketch": "s", "diameter": 4,
               "form": "counterbore", "counterbore": {"diameter": 8, "depth": 3},
               "depth": {"blind": 6}}),
    );
    let names = |s: &mut Session| {
        let v = must(s, "inspect", json!({"part": "p"}));
        let mut n: Vec<String> = ["faces", "edges", "vertices"]
            .iter()
            .flat_map(|k| v["bodies"][0][k].as_array().unwrap().clone())
            .map(|e| e["name"].as_str().unwrap().to_owned())
            .collect();
        n.sort();
        n
    };
    let before = names(&mut s);
    must(
        &mut s,
        "feature.edit",
        json!({"part": "p", "id": "h", "diameter": 5, "counterbore": {"diameter": 9, "depth": 2},
               "depth": {"blind": 7}}),
    );
    assert_eq!(names(&mut s), before);
}
