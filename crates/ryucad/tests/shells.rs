//! Shells against exact volumes, through the protocol: a box open at its
//! top with a thicker floor, outwards, closed into a cavity; a cylinder
//! cup; a rounded tray; names kept across thickness edits, a hole on the
//! inner floor following them; refusals. Every result passes the kernel's
//! checks, the self-intersection check among them.

use core::f64::consts::PI;
use ryucad::doc::id::Id;
use ryucad::error::Code;
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

/// A project with part `p`, in millimetres.
fn project() -> (tempfile::TempDir, Session) {
    let dir = tempfile::tempdir().unwrap();
    ryucad::server::init(dir.path(), None).unwrap();
    let mut s = Session::open(dir.path()).unwrap();
    must(&mut s, "part.create", json!({"part": "p"}));
    (dir, s)
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

/// The name of the vertex of `body` at `p` (mm).
fn vertex(body: &Value, p: [f64; 3]) -> String {
    body["vertices"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| (0..3).all(|k| (v["point"][k].as_f64().unwrap() - p[k] * 1e-3).abs() < 1e-9))
        .map(|v| v["name"].as_str().unwrap().to_owned())
        .unwrap_or_else(|| panic!("no vertex at {p:?}"))
}

/// The reference to the edge of `solid` from `a` to `b` (mm).
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
fn a_box_open_at_its_top_with_a_thicker_floor() {
    let (_dir, mut s) = boxed();
    let b = body(&mut s, "b");
    let (top, bottom) = (plane(&b, "b", 1.0, 20.0), plane(&b, "b", -1.0, 0.0));
    let r = add(
        &mut s,
        json!({"type": "shell", "id": "sh", "thickness": 2, "open": [top],
               "faces": [{"thickness": "4 mm", "faces": [bottom]}]}),
    );
    // The part's one solid, filled in.
    assert_eq!(r["feature"]["target"], "b");
    assert_eq!(r["feature"]["thickness"]["m"], 0.002);
    near(
        volume(&mut s, "sh", "b"),
        24000.0 - 36.0 * 26.0 * 16.0,
        1e-9,
    );
    // Without the thicker floor: 2 mm all round.
    must(
        &mut s,
        "feature.edit",
        json!({"part": "p", "id": "sh", "faces": []}),
    );
    near(
        volume(&mut s, "sh", "b"),
        24000.0 - 36.0 * 26.0 * 18.0,
        1e-9,
    );
}

#[test]
fn outwards_then_closed_into_a_cavity() {
    let (_dir, mut s) = boxed();
    let top = plane(&body(&mut s, "b"), "b", 1.0, 20.0);
    add(
        &mut s,
        json!({"type": "shell", "id": "sh", "thickness": 2, "direction": "outward",
               "open": [top]}),
    );
    // The box grown below its top by Steiner's formula, less the box.
    let (a, b, c, t) = (40.0, 30.0, 20.0, 2.0);
    let steiner = a * b * c
        + 2.0 * (a * b + b * c + c * a) * t
        + PI * t * t * (a + b + c)
        + 4.0 / 3.0 * PI * t * t * t;
    let above = a * b * t + PI * t * t / 4.0 * 2.0 * (a + b) + 2.0 / 3.0 * PI * t * t * t;
    near(volume(&mut s, "sh", "b"), steiner - above - a * b * c, 1e-4);
    // Inwards and closed: a cavity.
    must(
        &mut s,
        "feature.edit",
        json!({"part": "p", "id": "sh", "direction": "inward", "open": []}),
    );
    near(
        volume(&mut s, "sh", "b"),
        24000.0 - 36.0 * 26.0 * 16.0,
        1e-9,
    );
}

#[test]
fn a_cylinder_cup() {
    let (_dir, mut s) = project();
    add(
        &mut s,
        json!({"type": "cylinder", "id": "c", "params": {"radius": 10, "height": 20}}),
    );
    let top = plane(&body(&mut s, "c"), "c", 1.0, 20.0);
    add(
        &mut s,
        json!({"type": "shell", "id": "cup", "thickness": 2, "open": [top]}),
    );
    near(
        volume(&mut s, "cup", "c"),
        PI * (100.0 * 20.0 - 64.0 * 18.0),
        1e-4,
    );
}

#[test]
fn a_rounded_tray() {
    let (_dir, mut s) = boxed();
    let b = body(&mut s, "b");
    let ups: Vec<String> = [[0.0, 0.0], [40.0, 0.0], [0.0, 30.0], [40.0, 30.0]]
        .iter()
        .map(|[x, y]| edge(&b, "b", [*x, *y, 0.0], [*x, *y, 20.0]))
        .collect();
    add(
        &mut s,
        json!({"type": "fillet", "id": "f", "radius": 5, "edges": ups}),
    );
    let top = plane(&body(&mut s, "f"), "b", 1.0, 20.0);
    add(
        &mut s,
        json!({"type": "shell", "id": "tray", "thickness": 2, "open": [top]}),
    );
    let plan = |a: f64, b: f64, r: f64| a * b - (4.0 - PI) * r * r;
    near(
        volume(&mut s, "tray", "b"),
        plan(40.0, 30.0, 5.0) * 20.0 - plan(36.0, 26.0, 3.0) * 18.0,
        1e-4,
    );
}

#[test]
fn names_last_across_thickness_edits_and_a_hole_on_the_inner_floor_follows() {
    let (_dir, mut s) = boxed();
    let b = body(&mut s, "b");
    let (top, bottom) = (plane(&b, "b", 1.0, 20.0), plane(&b, "b", -1.0, 0.0));
    add(
        &mut s,
        json!({"type": "shell", "id": "sh", "thickness": 2, "open": [top],
               "faces": [{"thickness": "4 mm", "faces": [bottom]}]}),
    );
    let shelled = body(&mut s, "sh");
    let inner = plane(&shelled, "b", 1.0, 4.0);
    // The inner floor says it is the wall of the bottom.
    let name = |r: &str| r.rsplit('/').next().unwrap().to_owned();
    let floor = shelled["faces"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["name"] == name(&inner))
        .unwrap();
    assert_eq!(
        floor["origin"],
        json!({"feature": "sh", "wall_of": name(&bottom)})
    );
    add(&mut s, json!({"type": "sketch", "id": "k", "face": inner}));
    must(
        &mut s,
        "sketch.edit",
        json!({"part": "p", "sketch": "k", "ops": [
            {"op": "add", "type": "point", "at": [20, 15]},
            {"op": "add", "type": "fix", "a": "p1"}
        ]}),
    );
    add(
        &mut s,
        json!({"type": "hole", "id": "h", "sketch": "k", "points": ["p1"], "diameter": 4,
               "depth": {"blind": 2}, "targets": ["b"]}),
    );
    let names = |s: &mut Session| -> Vec<String> {
        let mut n: Vec<String> = body(s, "sh")["faces"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["name"].as_str().unwrap().to_owned())
            .collect();
        n.sort();
        n
    };
    let before = names(&mut s);
    near(volume(&mut s, "h", "b"), 9024.0 - PI * 4.0 * 2.0, 1e-6);
    for (t, inside) in [("3 mm", 34.0 * 24.0), ("1.5 mm", 37.0 * 27.0)] {
        must(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "sh", "thickness": t}),
        );
        assert_eq!(names(&mut s), before, "{t}");
        near(
            volume(&mut s, "h", "b"),
            24000.0 - inside * 16.0 - PI * 4.0 * 2.0,
            1e-6,
        );
    }
}

#[test]
fn refusals() {
    let (_dir, mut s) = boxed();
    add(
        &mut s,
        json!({"type": "cylinder", "id": "c", "params": {"radius": 3, "height": 10},
               "placement": {"origin": [60, 0, 0]}}),
    );
    let b = body(&mut s, "b");
    let top = plane(&b, "b", 1.0, 20.0);
    let lid = plane(&body(&mut s, "c"), "c", 1.0, 10.0);
    let refused = |s: &mut Session, f: Value| {
        let mut f = f;
        f["part"] = json!("p");
        f["type"] = json!("shell");
        call(s, "feature.add", f).unwrap_err()
    };
    // Two solids: the target must be named.
    let e = refused(&mut s, json!({"thickness": 2}));
    assert_eq!(
        (e.code, e.details["param"].as_str()),
        (Code::InvalidParam, Some("target"))
    );
    // A face of another solid.
    let e = refused(
        &mut s,
        json!({"target": "b", "thickness": 2, "open": [lid]}),
    );
    assert_eq!(e.code, Code::TargetsMixed, "{e}");
    // An edge where a face goes.
    let e = refused(
        &mut s,
        json!({"target": "b", "thickness": 2,
               "open": [format!("p/b/{}", b["edges"][0]["name"].as_str().unwrap())]}),
    );
    assert_eq!(
        (e.code, e.details["role"].as_str()),
        (Code::ReferenceWrongKind, Some("open.0")),
        "{e}"
    );
    // Open and given a wall at once.
    let e = refused(
        &mut s,
        json!({"target": "b", "thickness": 2, "open": [top],
               "faces": [{"thickness": 4, "faces": [top]}]}),
    );
    assert_eq!(
        (e.code, e.details["pointer"].as_str()),
        (Code::InvalidParam, Some("/faces/0/faces/0")),
        "{e}"
    );
    // A wall past half the box: written, and the rebuild says why.
    add(
        &mut s,
        json!({"type": "shell", "id": "deep", "target": "b", "thickness": 11}),
    );
    let part = Id::parse("p", "part").unwrap();
    let r = s.rebuilt(&part).unwrap();
    let (_, out) = r
        .features
        .iter()
        .find(|(i, _)| i.as_str() == "deep")
        .unwrap();
    let e = out.as_ref().unwrap_err();
    assert_eq!(e.code, Code::ShellFailed, "{e}");
    assert_eq!(
        (e.details["reason"].as_str(), e.details["kernel"].as_str()),
        (Some("edge_vanishes"), Some("offset")),
        "{e}"
    );
    assert!(e.details["hint"].is_string(), "{e}");
}
