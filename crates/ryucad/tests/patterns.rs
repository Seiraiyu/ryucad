//! Patterns against exact volumes, through the protocol: a grid of holes
//! with an instance skipped, a bolt circle round a boss's axis, and a
//! pocket mirrored, each against the same features modelled one by one.
//! Every result passes the kernel's checks, and `ryucad check` finds
//! nothing wrong.

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

fn add(s: &mut Session, f: Value) -> Value {
    let mut f = f;
    f["part"] = json!("p");
    must(s, "feature.add", f)
}

/// Part `p`: a plate `b` 40 × 30 × 10 mm about the origin in x and y,
/// and a sketch `s` on its top with a fixed point at each of `points`.
fn plate(points: &[[f64; 2]]) -> (tempfile::TempDir, Session) {
    let dir = tempfile::tempdir().unwrap();
    ryucad::server::init(dir.path(), None).unwrap();
    let mut s = Session::open(dir.path()).unwrap();
    must(&mut s, "part.create", json!({"part": "p"}));
    add(
        &mut s,
        json!({"type": "box", "id": "b", "params": {"x": 40, "y": 30, "z": 10},
               "placement": {"origin": [-20, -15, 0]}}),
    );
    add(
        &mut s,
        json!({"type": "sketch", "id": "s", "placement": {"origin": [0, 0, 10]}}),
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

/// Solid `b` as the part shows it: its volume in mm³ from a fine mesh,
/// and its counts, after the kernel's checks and `ryucad check`.
fn shown(s: &mut Session) -> (f64, [u64; 3]) {
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
    let v = call(s, "check", json!({"part": "p"}));
    assert!(v.is_ok(), "{v:?}");
    let c = b.body.counts();
    (
        ryukei_testkit::mesh::volume_and_area(&m).0 * 1e9,
        [c.faces as u64, c.edges as u64, c.vertices as u64],
    )
}

fn near(got: f64, want: f64) {
    assert!((got - want).abs() < 2e-4 * want, "{got} mm³ != {want} mm³");
}

fn hole(id: &str, points: &[&str]) -> Value {
    json!({"type": "hole", "id": id, "sketch": "s", "points": points, "diameter": 3,
           "depth": {"blind": 5}})
}

#[test]
fn a_grid_of_holes_skips_one() {
    let (_dir, mut s) = plate(&[[-15.0, -10.0]]);
    add(&mut s, hole("h", &["p1"]));
    add(
        &mut s,
        json!({"type": "pattern", "id": "grid", "features": ["h"], "direction": "x",
               "spacing": 6, "count": 5, "direction2": "y", "spacing2": 5, "count2": 4,
               "skip": [[2, 1], [4, 3]]}),
    );
    let (v, _) = shown(&mut s);
    near(v, 12000.0 - 18.0 * PI * 2.25 * 5.0);
    // Two more rows renames nothing of those already there.
    let names = |s: &mut Session| {
        let v = must(s, "inspect", json!({"part": "p"}));
        v["bodies"][0]["faces"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["name"].as_str().unwrap().to_owned())
            .collect::<std::collections::BTreeSet<_>>()
    };
    let before = names(&mut s);
    must(
        &mut s,
        "feature.edit",
        json!({"part": "p", "id": "grid", "count2": 5}),
    );
    let (v, _) = shown(&mut s);
    near(v, 12000.0 - 23.0 * PI * 2.25 * 5.0);
    assert!(before.is_subset(&names(&mut s)));
}

#[test]
fn a_bolt_circle_round_a_boss() {
    let (_dir, mut s) = plate(&[[8.0, 0.0]]);
    // A boss on the plate, and six holes round its axis through the plate.
    add(
        &mut s,
        json!({"type": "cylinder", "id": "boss", "operation": "join", "targets": ["b"],
               "params": {"radius": 4, "height": 3}, "placement": {"origin": [0, 0, 10]}}),
    );
    add(&mut s, hole("h", &["p1"]));
    let v = must(&mut s, "inspect", json!({"part": "p", "feature": "h"}));
    let side = v["bodies"][0]["faces"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| {
            f["surface"]["type"] == "cylinder"
                && (f["surface"]["radius"].as_f64().unwrap() - 0.004).abs() < 1e-12
        })
        .map(|f| format!("p/b/{}", f["name"].as_str().unwrap()))
        .expect("the boss's side");
    add(
        &mut s,
        json!({"type": "pattern", "id": "circle", "features": ["h"], "axis": side,
               "count": 6, "angle": 360}),
    );
    let (by_pattern, counts) = shown(&mut s);
    near(
        by_pattern,
        12000.0 + PI * 16.0 * 3.0 - 6.0 * PI * 2.25 * 5.0,
    );
    // The same six holes, drilled one by one.
    let (_dir2, mut t) = plate(
        &(0..6)
            .map(|k| {
                let a = f64::from(k) * PI / 3.0;
                [8.0 * ryukei::math::cos(a), 8.0 * ryukei::math::sin(a)]
            })
            .collect::<Vec<_>>(),
    );
    add(
        &mut t,
        json!({"type": "cylinder", "id": "boss", "operation": "join", "targets": ["b"],
               "params": {"radius": 4, "height": 3}, "placement": {"origin": [0, 0, 10]}}),
    );
    add(&mut t, hole("h", &["p1", "p2", "p3", "p4", "p5", "p6"]));
    let (by_hand, hand_counts) = shown(&mut t);
    near(by_pattern, by_hand);
    assert_eq!(counts, hand_counts);
}

#[test]
fn a_pocket_mirrored() {
    let (_dir, mut s) = plate(&[]);
    // An L-shaped pocket off to one side, mirrored in the yz plane.
    add(
        &mut s,
        json!({"type": "box", "id": "pocket", "operation": "cut", "targets": ["b"],
               "params": {"x": 6, "y": 4, "z": 3}, "placement": {"origin": [5, 2, 7]}}),
    );
    add(
        &mut s,
        json!({"type": "box", "id": "leg", "operation": "cut", "targets": ["b"],
               "params": {"x": 2, "y": 8, "z": 3}, "placement": {"origin": [9, -6, 7]}}),
    );
    add(
        &mut s,
        json!({"type": "pattern", "id": "m", "features": ["pocket", "leg"], "plane": "yz"}),
    );
    let (by_pattern, counts) = shown(&mut s);
    let pocket = 6.0 * 4.0 * 3.0 + 2.0 * 8.0 * 3.0;
    near(by_pattern, 12000.0 - 2.0 * pocket);
    // The mirrored pocket, modelled at x = -11 to -5 and -11 to -9.
    let (_dir2, mut t) = plate(&[]);
    for (id, x, y, ox, oy) in [
        ("pocket", 6, 4, 5, 2),
        ("leg", 2, 8, 9, -6),
        ("pocket2", 6, 4, -11, 2),
        ("leg2", 2, 8, -11, -6),
    ] {
        add(
            &mut t,
            json!({"type": "box", "id": id, "operation": "cut", "targets": ["b"],
                   "params": {"x": x, "y": y, "z": 3}, "placement": {"origin": [ox, oy, 7]}}),
        );
    }
    let (by_hand, hand_counts) = shown(&mut t);
    near(by_pattern, by_hand);
    assert_eq!(counts, hand_counts);
}
