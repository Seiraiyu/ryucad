//! Datum planes, axes and points through the protocol, against exact
//! volumes: sketches on offset, angled and three-point planes extruded, a
//! revolve about a datum axis, a mirror in a midplane, a bolt circle about
//! a two-point axis, an extrude up to a datum plane and a mate to one; a
//! datum following its face when the box is resized, renaming nothing of
//! its sketch's; and the refusals.

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
    if f.get("part").is_none() {
        f["part"] = json!("p");
    }
    must(s, "feature.add", f)
}

/// A project with part `p`: a box `b` 20 × 10 × 5 mm from the origin.
fn boxed() -> (tempfile::TempDir, Session) {
    let dir = tempfile::tempdir().unwrap();
    ryucad::server::init(dir.path(), None).unwrap();
    let mut s = Session::open(dir.path()).unwrap();
    must(&mut s, "part.create", json!({"part": "p"}));
    add(
        &mut s,
        json!({"type": "box", "id": "b", "params": {"x": 20, "y": 10, "z": 5}}),
    );
    (dir, s)
}

/// The box's face, edge or vertex where `pick` holds, by name.
fn named(s: &mut Session, list: &str, pick: impl Fn(&Value) -> bool) -> String {
    let v = must(s, "inspect", json!({"part": "p", "feature": "b"}));
    let found = v["bodies"][0][list]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| pick(e))
        .unwrap_or_else(|| panic!("no such entity in {v}"));
    format!("p/b/{}", found["name"].as_str().unwrap())
}

/// The box's planar face whose outward normal is `n`.
fn facing(s: &mut Session, n: [f64; 3]) -> String {
    named(s, "faces", |f| {
        let z = &f["surface"]["frame"]["z"];
        let sign = if f["reversed"] == true { -1.0 } else { 1.0 };
        f["surface"]["type"] == "plane"
            && (0..3).all(|k| (z[k].as_f64().unwrap() * sign - n[k]).abs() < 1e-12)
    })
}

fn top(s: &mut Session) -> String {
    facing(s, [0.0, 0.0, 1.0])
}

fn vertex(s: &mut Session, at: [f64; 3]) -> String {
    named(s, "vertices", |v| {
        (0..3).all(|k| (v["point"][k].as_f64().unwrap() - at[k]).abs() < 1e-12)
    })
}

/// A rectangle `w` × `h` mm with its corner at `(x, y)` in sketch `id`.
fn rectangle(s: &mut Session, id: &str, [x, y]: [f64; 2], [w, h]: [f64; 2]) {
    let mut ops = Vec::new();
    for (i, at) in [[x, y], [x + w, y], [x + w, y + h], [x, y + h]]
        .into_iter()
        .enumerate()
    {
        ops.push(json!({"op": "add", "id": format!("p{}", i + 1), "type": "point", "at": at}));
        ops.push(json!({"op": "add", "type": "fix", "a": format!("p{}", i + 1)}));
    }
    for i in 1..=4 {
        let j = i % 4 + 1;
        ops.push(json!({"op": "add", "id": format!("l{i}"), "type": "line",
                        "start": format!("p{i}"), "end": format!("p{j}")}));
    }
    must(
        s,
        "sketch.edit",
        json!({"part": "p", "sketch": id, "ops": ops}),
    );
}

/// Solid `solid` as the part shows it: its volume in mm³ from a fine mesh
/// and its box in mm, after the kernel's checks and `ryucad check`.
fn shown(s: &mut Session, solid: &str) -> (f64, [[f64; 3]; 2]) {
    let part = Id::parse("p", "part").unwrap();
    let r = s.rebuilt(&part).unwrap().clone();
    assert_eq!(r.failed(), 0, "{:?}", r.features);
    let solids = ryucad::solid::Solids::of(s.project().part(&part).unwrap());
    let shown = r.shown(&solids);
    let (_, b) = shown
        .iter()
        .find(|x| x.solid.as_str() == solid)
        .and_then(|x| x.state)
        .unwrap();
    assert!(ryukei::check::check(&b.body).is_empty());
    assert_eq!(ryukei::check::check_intersections(&b.body), []);
    let m = ryukei::mesh::tessellate(&b.body, &ryukei::mesh::MeshParams::new(2e-6, 0.1)).unwrap();
    assert!(m.faults.is_empty(), "{:?}", m.faults);
    assert!(call(s, "check", json!({"part": "p"})).is_ok());
    let (lo, hi) = ryucad::inspect::body_extent(&b.body).unwrap();
    (
        ryukei_testkit::mesh::volume_and_area(&m).0 * 1e9,
        [lo.map(|x| x * 1e3), hi.map(|x| x * 1e3)],
    )
}

fn near(got: f64, want: f64) {
    assert!(
        (got - want).abs() <= 2e-4 * want.abs().max(1.0),
        "{got} != {want}"
    );
}

#[test]
fn sketches_on_derived_planes_extrude_where_their_planes_are() {
    let (_dir, mut s) = boxed();
    let top = top(&mut s);
    // 30 mm above the top, carrying its frame across.
    add(
        &mut s,
        json!({"type": "plane", "id": "d1", "by": "offset", "from": top, "distance": "30 mm"}),
    );
    add(
        &mut s,
        json!({"type": "sketch", "id": "s1", "plane": "p/d1"}),
    );
    rectangle(&mut s, "s1", [1.0, 2.0], [4.0, 3.0]);
    add(
        &mut s,
        json!({"type": "extrude", "id": "e1", "sketch": "s1", "extent": {"length": 2}}),
    );
    let (v, [lo, hi]) = shown(&mut s, "e1");
    near(v, 24.0);
    near(lo[2], 35.0);
    near(hi[2], 37.0);
    near(lo[0], 1.0);
    // xy turned 90° about x: it faces -y.
    add(
        &mut s,
        json!({"type": "plane", "id": "d2", "by": "angle", "from": "xy", "about": "x",
               "angle": "90 deg"}),
    );
    add(
        &mut s,
        json!({"type": "sketch", "id": "s2", "plane": "p/d2"}),
    );
    rectangle(&mut s, "s2", [0.0, 0.0], [4.0, 3.0]);
    add(
        &mut s,
        json!({"type": "extrude", "id": "e2", "sketch": "s2", "extent": {"length": 2}}),
    );
    let (v, [lo, hi]) = shown(&mut s, "e2");
    near(v, 24.0);
    near(lo[1], -2.0);
    near(hi[1], 0.0);
    // Through three of the top's corners: the top itself.
    let corners = [
        vertex(&mut s, [0.0, 0.0, 0.005]),
        vertex(&mut s, [0.02, 0.0, 0.005]),
        vertex(&mut s, [0.0, 0.01, 0.005]),
    ];
    add(
        &mut s,
        json!({"type": "plane", "id": "d3", "by": "three_points", "points": corners}),
    );
    add(
        &mut s,
        json!({"type": "sketch", "id": "s3", "plane": "p/d3"}),
    );
    rectangle(&mut s, "s3", [2.0, 2.0], [4.0, 3.0]);
    add(
        &mut s,
        json!({"type": "extrude", "id": "e3", "sketch": "s3", "extent": {"length": 2}}),
    );
    let (v, [lo, hi]) = shown(&mut s, "e3");
    near(v, 24.0);
    near(lo[2], 5.0);
    near(hi[2], 7.0);
}

#[test]
fn a_revolve_about_a_datum_axis() {
    let (_dir, mut s) = boxed();
    add(
        &mut s,
        json!({"type": "axis", "id": "a1", "by": "of", "axis": "z"}),
    );
    // On zx the sketch's x is the part's z and its y the part's x: a ring
    // 10 to 12 mm out, 5 mm high.
    add(&mut s, json!({"type": "sketch", "id": "s", "plane": "zx"}));
    rectangle(&mut s, "s", [0.0, 10.0], [5.0, 2.0]);
    add(
        &mut s,
        json!({"type": "revolve", "id": "r", "sketch": "s", "axis": "p/a1", "angle": "full"}),
    );
    let (v, _) = shown(&mut s, "r");
    near(v, PI * (144.0 - 100.0) * 5.0);
}

#[test]
fn a_mirror_in_a_midplane_and_a_bolt_circle_about_a_two_point_axis() {
    let (_dir, mut s) = boxed();
    // A pocket at x 14 to 18, mirrored in the plane halfway between the
    // box's ends (x = 10), lands at 2 to 6.
    add(
        &mut s,
        json!({"type": "box", "id": "c", "operation": "cut", "targets": ["b"],
               "params": {"x": 4, "y": 4, "z": 2}, "placement": {"origin": [14, 3, 3]}}),
    );
    let ends = [
        facing(&mut s, [-1.0, 0.0, 0.0]),
        facing(&mut s, [1.0, 0.0, 0.0]),
    ];
    add(
        &mut s,
        json!({"type": "plane", "id": "mid", "by": "midplane", "between": ends}),
    );
    add(
        &mut s,
        json!({"type": "pattern", "id": "m", "form": "mirror", "features": ["c"],
               "plane": "p/mid"}),
    );
    let (v, [lo, _]) = shown(&mut s, "b");
    near(v, 1000.0 - 2.0 * 32.0);
    near(lo[0], 0.0);
    // Four 1 mm holes round the axis through (10, 5, 0) and (10, 5, 5),
    // the first at (13, 5), clear of both pockets.
    add(
        &mut s,
        json!({"type": "point", "id": "q1", "by": "coordinates",
               "coordinates": ["10 mm", "5 mm", "0 mm"]}),
    );
    add(
        &mut s,
        json!({"type": "point", "id": "q2", "by": "coordinates",
               "coordinates": ["10 mm", "5 mm", "5 mm"]}),
    );
    add(
        &mut s,
        json!({"type": "axis", "id": "ax", "by": "two_points", "points": ["p/q1", "p/q2"]}),
    );
    add(
        &mut s,
        json!({"type": "sketch", "id": "top", "placement": {"origin": [0, 0, 5]}}),
    );
    must(
        &mut s,
        "sketch.edit",
        json!({"part": "p", "sketch": "top", "ops": [
            {"op": "add", "id": "p1", "type": "point", "at": [13, 5]},
            {"op": "add", "type": "fix", "a": "p1"}]}),
    );
    add(
        &mut s,
        json!({"type": "hole", "id": "h", "sketch": "top", "points": ["p1"], "diameter": 1,
               "depth": {"blind": 1}, "targets": ["b"]}),
    );
    add(
        &mut s,
        json!({"type": "pattern", "id": "ring", "form": "circular", "features": ["h"],
               "axis": "p/ax", "count": 4, "angle": 360}),
    );
    let (v, _) = shown(&mut s, "b");
    near(v, 1000.0 - 64.0 - 4.0 * PI * 0.25);
}

#[test]
fn an_extrude_up_to_a_datum_plane_and_a_datum_following_its_face() {
    let (_dir, mut s) = boxed();
    let top = top(&mut s);
    add(
        &mut s,
        json!({"type": "plane", "id": "d1", "by": "offset", "from": top, "distance": "2 mm"}),
    );
    add(&mut s, json!({"type": "sketch", "id": "s", "plane": "xy"}));
    rectangle(&mut s, "s", [25.0, 0.0], [4.0, 3.0]);
    add(
        &mut s,
        json!({"type": "extrude", "id": "e", "sketch": "s",
               "extent": {"type": "up_to", "target": "p/d1"}}),
    );
    let (v, [_, hi]) = shown(&mut s, "e");
    near(v, 4.0 * 3.0 * 7.0);
    near(hi[2], 7.0);
    // A sketch on the datum keeps its names when the box grows: the datum
    // follows the top, and the sketch's plane the datum.
    add(
        &mut s,
        json!({"type": "sketch", "id": "on", "plane": "p/d1"}),
    );
    rectangle(&mut s, "on", [1.0, 1.0], [2.0, 2.0]);
    add(
        &mut s,
        json!({"type": "extrude", "id": "boss", "sketch": "on", "extent": {"length": 1}}),
    );
    let names = |s: &mut Session| {
        let v = must(s, "inspect", json!({"part": "p", "feature": "boss"}));
        let mut n: Vec<String> = v["bodies"][0]["faces"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["name"].as_str().unwrap().to_owned())
            .collect();
        n.sort();
        n
    };
    let before = names(&mut s);
    must(
        &mut s,
        "feature.edit",
        json!({"part": "p", "id": "b", "params": {"z": 8}}),
    );
    let (_, [lo, hi]) = shown(&mut s, "boss");
    near(lo[2], 10.0);
    near(hi[2], 11.0);
    assert_eq!(names(&mut s), before);
}

#[test]
fn an_instance_mates_to_a_datum_plane() {
    let dir = tempfile::tempdir().unwrap();
    ryucad::server::init(dir.path(), None).unwrap();
    let mut s = Session::open(dir.path()).unwrap();
    for p in ["base", "pin"] {
        must(&mut s, "part.create", json!({"part": p}));
    }
    add(
        &mut s,
        json!({"part": "base", "type": "box", "id": "b", "params": {"x": 10, "y": 10, "z": 2}}),
    );
    add(
        &mut s,
        json!({"part": "pin", "type": "cylinder", "id": "c", "params": {"radius": 1, "height": 6}}),
    );
    // The pin's datum 4 mm up its axis.
    add(
        &mut s,
        json!({"part": "pin", "type": "plane", "id": "d", "by": "offset", "from": "xy",
               "distance": 4}),
    );
    must(
        &mut s,
        "part.create",
        json!({"part": "asm", "kind": "assembly"}),
    );
    add(
        &mut s,
        json!({"part": "asm", "type": "instance", "id": "base", "of": "base", "fixed": true}),
    );
    add(
        &mut s,
        json!({"part": "asm", "type": "instance", "id": "pin", "of": "pin"}),
    );
    // Fastened: the pin's datum on the base's origin, so the pin sits 4 mm
    // below it.
    let m = add(
        &mut s,
        json!({"part": "asm", "type": "mate", "id": "m", "form": "fastened",
               "a": "asm/base", "b": "asm/pin/d"}),
    );
    assert_eq!(m["feature"]["b"]["text"], "asm/pin/d", "{m}");
    let file: Value = serde_json::from_str(
        &std::fs::read_to_string(dir.path().join("parts/asm/features/m.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        file["b"],
        json!({"instance": ["pin"], "at": {"feature": "d"}})
    );
    assert_eq!(file["format"], 7);
    let r = must(&mut s, "rebuild", json!({"part": "asm"}));
    let pin = r["parts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["part"] == "asm")
        .unwrap()["features"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["id"] == "pin")
        .unwrap()
        .clone();
    let z = pin["instance"]["pose"]["origin_m"][2].as_f64().unwrap();
    // Within the mate solver's tolerance.
    assert!((z + 0.004).abs() < 1e-9, "{pin}");
}

#[test]
fn datums_that_wont_do_say_why() {
    let (_dir, mut s) = boxed();
    let top = top(&mut s);
    // A point where a plane goes, refused when written.
    let e = call(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "plane", "by": "offset", "from": "origin", "distance": 1}),
    )
    .unwrap_err();
    assert_eq!(e.code, ryucad::error::Code::ReferenceWrongKind);
    assert_eq!(
        (&e.details["got"], &e.details["role"]),
        (&json!("origin"), &json!("from"))
    );
    // A solid is never a plane.
    let e = call(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "plane", "by": "offset", "from": "p/b", "distance": 1}),
    )
    .unwrap_err();
    assert_eq!(e.code, ryucad::error::Code::ReferenceWrongKind);
    // What the box's top makes of itself, only rebuild tells.
    add(
        &mut s,
        json!({"type": "axis", "id": "a", "by": "planes", "between": [top.clone(), "xy"]}),
    );
    let e = call(&mut s, "rebuild", json!({"part": "p"})).unwrap_err();
    let a = &e.details["parts"][0]["features"][1];
    assert_eq!(a["error"]["code"], "datum_degenerate", "{a}");
    // What a datum reads can't go, nor move after it.
    must(&mut s, "feature.delete", json!({"part": "p", "id": "a"}));
    add(
        &mut s,
        json!({"type": "plane", "id": "d", "by": "offset", "from": top, "distance": 1}),
    );
    let e = call(&mut s, "feature.delete", json!({"part": "p", "id": "b"})).unwrap_err();
    assert_eq!(e.code, ryucad::error::Code::HasDependents);
    let e = call(
        &mut s,
        "feature.move",
        json!({"part": "p", "id": "b", "after": "d"}),
    )
    .unwrap_err();
    assert_eq!(e.code, ryucad::error::Code::ReferenceForward);
    // A datum's sketch fails with it.
    add(&mut s, json!({"type": "sketch", "id": "s", "plane": "p/d"}));
    must(
        &mut s,
        "feature.edit",
        json!({"part": "p", "id": "d", "by": "midplane", "between": ["xy", "xy"]}),
    );
    let v = must(&mut s, "feature.show", json!({"part": "p", "id": "d"}));
    assert!(
        v.get("distance").is_none() && v.get("from").is_none(),
        "{v}"
    );
    let e = call(
        &mut s,
        "feature.edit",
        json!({"part": "p", "id": "d", "by": "three_points", "points": ["origin", "origin"]}),
    )
    .unwrap_err();
    assert_eq!(e.code, ryucad::error::Code::InvalidDocument, "{e}");
    // A sketch's point a datum reads stays while it does.
    must(
        &mut s,
        "sketch.edit",
        json!({"part": "p", "sketch": "s", "ops": [
            {"op": "add", "id": "p1", "type": "point", "at": [1, 1]}]}),
    );
    add(
        &mut s,
        json!({"type": "point", "id": "q", "by": "of", "point": "p/s/p1"}),
    );
    let e = call(
        &mut s,
        "sketch.edit",
        json!({"part": "p", "sketch": "s", "ops": [{"op": "remove", "id": "p1"}]}),
    )
    .unwrap_err();
    assert_eq!(e.code, ryucad::error::Code::HasDependents, "{e}");
    assert_eq!(e.details["features"], json!(["q"]));
}
