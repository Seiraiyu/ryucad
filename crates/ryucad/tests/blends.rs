//! Fillets and chamfers against exact volumes, through the protocol: every
//! edge of a box rounded and bevelled, a slot's rim rounded all round from
//! one edge, a boss's foot filled. Every result passes the kernel's
//! checks, and `ryucad check` finds nothing wrong.

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

/// A project with part `p`, in millimetres.
fn project() -> (tempfile::TempDir, Session) {
    let dir = tempfile::tempdir().unwrap();
    ryucad::server::init(dir.path(), None).unwrap();
    let mut s = Session::open(dir.path()).unwrap();
    must(&mut s, "part.create", json!({"part": "p"}));
    (dir, s)
}

fn add(s: &mut Session, f: Value) -> Value {
    let mut f = f;
    f["part"] = json!("p");
    must(s, "feature.add", f)
}

/// The body feature `f` leaves, in `inspect`.
fn body(s: &mut Session, f: &str) -> Value {
    must(s, "inspect", json!({"part": "p", "feature": f}))["bodies"][0].clone()
}

/// The name of the vertex of `body` at `p` (meters).
fn vertex(body: &Value, p: [f64; 3]) -> String {
    body["vertices"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| (0..3).all(|k| (v["point"][k].as_f64().unwrap() - p[k]).abs() < 1e-9))
        .map(|v| v["name"].as_str().unwrap().to_owned())
        .unwrap_or_else(|| panic!("no vertex at {p:?}"))
}

/// The reference to the edge of solid `solid` from `a` to `b` (meters).
fn edge(body: &Value, solid: &str, a: [f64; 3], b: [f64; 3]) -> String {
    let (a, b) = (vertex(body, a), vertex(body, b));
    let e = body["edges"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| (e["start"] == a && e["end"] == b) || (e["start"] == b && e["end"] == a))
        .expect("an edge from a to b");
    format!("p/{solid}/{}", e["name"].as_str().unwrap())
}

/// The volume of solid `solid` as feature `f` left it, after the kernel's
/// checks, meshed finely.
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
    ryukei_testkit::mesh::volume_and_area(&m).0
}

/// `got` within a part in `rel` of `want`.
fn near(got: f64, want: f64, rel: f64) {
    assert!((got - want).abs() <= rel * want, "{got} against {want}");
}

/// The corner a fillet of radius `r` rounds off (or fills in) at a square
/// edge: its area, and how far its centroid lies from either face.
fn corner(r: f64) -> (f64, f64) {
    (
        (1.0 - PI / 4.0) * r * r,
        r * (10.0 - 3.0 * PI) / (12.0 - 3.0 * PI),
    )
}

/// A 20 × 10 × 5 mm box `b`, and its twelve edges.
fn twelve(s: &mut Session) -> Vec<String> {
    add(
        s,
        json!({"type": "box", "id": "b", "params": {"x": 20, "y": 10, "z": 5}}),
    );
    let body = body(s, "b");
    body["edges"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| format!("p/b/{}", e["name"].as_str().unwrap()))
        .collect()
}

#[test]
fn every_edge_of_a_box_rounded() {
    let (_dir, mut s) = project();
    let edges = twelve(&mut s);
    add(
        &mut s,
        json!({"type": "fillet", "id": "f", "radius": 1, "edges": edges}),
    );
    // The box shrunk by the radius, swept by a ball of it.
    let (r, [a, b, c]) = (1e-3, [18e-3, 8e-3, 3e-3]);
    let want = a * b * c
        + 2.0 * r * (a * b + b * c + c * a)
        + PI * r * r * (a + b + c)
        + 4.0 / 3.0 * PI * r * r * r;
    near(volume(&mut s, "f", "b"), want, 1e-5);
}

#[test]
fn every_edge_of_a_box_bevelled() {
    let (_dir, mut s) = project();
    let edges = twelve(&mut s);
    add(
        &mut s,
        json!({"type": "chamfer", "id": "c", "d": 1, "edges": edges}),
    );
    // A prism of d²/2 off each edge, mitred at each corner.
    let (d, [a, b, c]) = (1e-3, [20e-3, 10e-3, 5e-3]);
    let want = a * b * c - 2.0 * d * d * (a + b + c) + 6.0 * d * d * d;
    near(volume(&mut s, "c", "b"), want, 1e-9);
}

#[test]
fn a_slots_rim_rounded_all_round_from_one_edge() {
    let (_dir, mut s) = project();
    // A slot of two 5 mm half circles 20 mm apart, 5 mm tall.
    let (big, l, h) = (5.0, 20.0, 5.0);
    add(&mut s, json!({"type": "sketch", "id": "s"}));
    must(
        &mut s,
        "sketch.edit",
        json!({"part": "p", "sketch": "s", "ops": [
            {"op": "add", "id": "c1", "type": "point", "at": [0, 0]},
            {"op": "add", "id": "c2", "type": "point", "at": [l, 0]},
            {"op": "add", "id": "p1", "type": "point", "at": [0, -big]},
            {"op": "add", "id": "p2", "type": "point", "at": [l, -big]},
            {"op": "add", "id": "p3", "type": "point", "at": [l, big]},
            {"op": "add", "id": "p4", "type": "point", "at": [0, big]},
            {"op": "add", "id": "l1", "type": "line", "start": "p1", "end": "p2"},
            {"op": "add", "id": "a1", "type": "arc", "center": "c2", "start": "p2", "end": "p3"},
            {"op": "add", "id": "l2", "type": "line", "start": "p3", "end": "p4"},
            {"op": "add", "id": "a2", "type": "arc", "center": "c1", "start": "p4", "end": "p1"}
        ]}),
    );
    add(
        &mut s,
        json!({"type": "extrude", "id": "e", "sketch": "s", "extent": {"length": h}}),
    );
    let top = edge(
        &body(&mut s, "e"),
        "e",
        [0.0, -0.005, 0.005],
        [0.02, -0.005, 0.005],
    );
    // One edge listed, propagating: the four of the rim.
    let r = add(
        &mut s,
        json!({"type": "fillet", "id": "f", "radius": 1, "edges": [top]}),
    );
    assert_eq!(r["feature"]["groups"][0]["edges"], json!([top]));
    let made = body(&mut s, "f");
    let replaced: Vec<&Value> = made["faces"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["origin"]["feature"] == "f")
        .collect();
    assert_eq!(replaced.len(), 4);
    // The corner taken off, swept along the rim (Pappus round the ends).
    let (rr, big, l, h) = (1e-3, big * 1e-3, l * 1e-3, h * 1e-3);
    let (area, d0) = corner(rr);
    let want = h * (2.0 * big * l + PI * big * big) - area * (2.0 * l + 2.0 * PI * (big - d0));
    near(volume(&mut s, "f", "e"), want, 1e-5);
    // Not propagating, the kernel won't stop at the arcs.
    let e = call(
        &mut s,
        "feature.edit",
        json!({"part": "p", "id": "f", "propagate": false}),
    );
    assert!(e.is_ok(), "{e:?}");
    let e = call(&mut s, "rebuild", json!({"part": "p"})).unwrap_err();
    let f = &e.details["parts"][0]["features"][2]["error"];
    assert_eq!(
        (f["code"].as_str(), f["details"]["reason"].as_str()),
        (Some("blend_failed"), Some("open_tangent_chain")),
        "{f}"
    );
}

#[test]
fn a_bosss_foot_filled() {
    let (_dir, mut s) = project();
    add(
        &mut s,
        json!({"type": "box", "id": "base", "params": {"x": 30, "y": 30, "z": 5}}),
    );
    add(
        &mut s,
        json!({"type": "cylinder", "id": "boss", "operation": "join",
               "params": {"radius": 5, "height": 5}, "placement": {"origin": [15, 15, 5]}}),
    );
    // The circle where the boss meets the base.
    let joined = body(&mut s, "boss");
    let foot = joined["edges"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| {
            e["curve"]["type"] == "circle"
                && (e["curve"]["frame"]["origin"][2].as_f64().unwrap() - 0.005).abs() < 1e-12
        })
        .map(|e| format!("p/base/{}", e["name"].as_str().unwrap()))
        .expect("the boss's foot");
    add(
        &mut s,
        json!({"type": "fillet", "id": "f", "radius": 1, "edges": [foot]}),
    );
    // The corner filled in, swept round the boss outside it.
    let (r, big) = (1e-3, 5e-3);
    let (area, d0) = corner(r);
    let want = 30e-3 * 30e-3 * 5e-3 + PI * big * big * 5e-3 + area * 2.0 * PI * (big + d0);
    near(volume(&mut s, "f", "base"), want, 1e-5);
    // Nothing for `check` to find.
    let v = must(&mut s, "check", json!({"part": "p"}));
    let errors: Vec<&Value> = v["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["severity"] == "error")
        .collect();
    assert!(errors.is_empty(), "{v}");
}
