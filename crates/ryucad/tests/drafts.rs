//! Drafts through the protocol (design 400, plan 17.B): a box's walls
//! about its bottom and its middle against prismatoids, a boss and the
//! hole through it about the plate's top against frusta, a rounded
//! rectangle's walls with their corners by tangency; then (Task 11)
//! parting lines, groups and flips, fillets, names, refusals, sheets and
//! a varying angle's warning.

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
    if f.get("part").is_none() {
        f["part"] = json!("p");
    }
    must(s, "feature.add", f)
}

/// A project with part `p` and a box `b` 40 × 30 × 20 mm from the origin.
fn boxed() -> (tempfile::TempDir, Session) {
    let dir = tempfile::tempdir().unwrap();
    ryucad::server::init(dir.path(), None).unwrap();
    let mut s = Session::open(dir.path()).unwrap();
    must(&mut s, "part.create", json!({"part": "p"}));
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

/// The planar face of `body` whose outward normal along `axis` is `n`, at
/// `at` mm along it, as a reference into `solid`.
fn plane(body: &Value, solid: &str, axis: usize, n: f64, at: f64) -> String {
    let f = body["faces"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| {
            let sign = if f["reversed"] == true { -1.0 } else { 1.0 };
            f["surface"]["type"] == "plane"
                && f["surface"]["frame"]["z"][axis].as_f64().unwrap() * sign == n
                && (f["surface"]["frame"]["origin"][axis].as_f64().unwrap() - at * 1e-3).abs()
                    < 1e-9
        })
        .unwrap_or_else(|| panic!("no face facing {n} at {at} mm along {axis}"));
    format!("p/{solid}/{}", f["name"].as_str().unwrap())
}

/// The face of `body` on a `kind` surface (`cylinder`, `cone`) of radius
/// `r` mm, as a reference into `solid`.
fn round(body: &Value, solid: &str, kind: &str, r: f64) -> String {
    let f = body["faces"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| {
            f["surface"]["type"] == kind
                && (f["surface"]["radius"].as_f64().unwrap() - r * 1e-3).abs() < 1e-9
        })
        .unwrap_or_else(|| panic!("no {kind} face of radius {r} mm"));
    format!("p/{solid}/{}", f["name"].as_str().unwrap())
}

/// The name of the vertex of `body` at `p` mm.
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

/// Every name of feature `f`'s body, sorted.
fn names(s: &mut Session, f: &str) -> Vec<String> {
    let b = body(s, f);
    let mut n: Vec<String> = ["faces", "edges", "vertices"]
        .iter()
        .flat_map(|k| b[*k].as_array().unwrap().clone())
        .map(|e| e["name"].as_str().unwrap().to_owned())
        .collect();
    n.sort();
    n
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

/// Feature `f`'s refusal at rebuild.
fn failure(s: &mut Session, f: &str) -> ryucad::error::Error {
    let part = Id::parse("p", "part").unwrap();
    let r = s.rebuilt(&part).unwrap();
    let (_, out) = r
        .features
        .iter()
        .find(|(i, _)| i.as_str() == f)
        .expect("listed");
    out.as_ref()
        .err()
        .unwrap_or_else(|| panic!("{f} built"))
        .clone()
}

/// Part `p`'s rebuild report: draft `f`'s, and the rebuild's warnings.
fn report(s: &mut Session, f: &str) -> (Value, Vec<Value>) {
    let r = must(s, "rebuild", json!({"part": "p"}));
    let d = r["parts"][0]["features"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["id"] == f)
        .unwrap()["draft"]
        .clone();
    // A rebuild's answer gives its parts' warnings at its top.
    let w = r["warnings"].as_array().cloned().unwrap_or_default();
    (d, w)
}

fn near(got: f64, want: f64, rel: f64) {
    assert!(
        (got - want).abs() <= rel * want,
        "{got} mm³ against {want} mm³"
    );
}

/// A prismatoid's volume from its areas at its ends and middle.
fn prismatoid(h: f64, a: impl Fn(f64) -> f64) -> f64 {
    h / 6.0 * (a(0.0) + 4.0 * a(h / 2.0) + a(h))
}

fn frustum(h: f64, r0: f64, r1: f64) -> f64 {
    PI * h / 3.0 * (r0 * r0 + r0 * r1 + r1 * r1)
}

fn tan(deg: f64) -> f64 {
    ryukei::math::tan(deg.to_radians())
}

/// The box's four walls: +x, -x, +y, -y.
fn walls(s: &mut Session) -> Vec<String> {
    let b = body(s, "b");
    vec![
        plane(&b, "b", 0, 1.0, 40.0),
        plane(&b, "b", 0, -1.0, 0.0),
        plane(&b, "b", 1, 1.0, 30.0),
        plane(&b, "b", 1, -1.0, 0.0),
    ]
}

#[test]
fn a_boxs_walls_drafted_about_its_bottom_and_its_middle() {
    let (_dir, mut s) = boxed();
    let w = walls(&mut s);
    add(
        &mut s,
        json!({"type": "draft", "id": "d", "angle": "2 deg", "neutral": "xy", "faces": w}),
    );
    let t = tan(2.0);
    let a = |z: f64| (40.0 - 2.0 * z * t) * (30.0 - 2.0 * z * t);
    near(volume(&mut s, "d", "b"), prismatoid(20.0, a), 1e-9);
    assert_eq!(names(&mut s, "d"), names(&mut s, "b"));
    let (d, warnings) = report(&mut s, "d");
    let angles = d["angles"].as_array().unwrap();
    assert_eq!(angles.len(), 4);
    assert!(
        angles
            .iter()
            .all(|x| x["exact"] == true && x["least"]["value"] == "2 deg"),
        "{d}"
    );
    assert!(warnings.is_empty(), "{warnings:?}");
    // About a datum plane through its middle (listed before the draft: a
    // draft reading a plane after it is reference_forward): above it the
    // walls lean in, below it out.
    let (_dir2, mut s) = boxed();
    add(
        &mut s,
        json!({"type": "plane", "id": "mid", "by": "offset", "from": "xy", "distance": 10}),
    );
    let w = walls(&mut s);
    add(
        &mut s,
        json!({"type": "draft", "id": "d", "angle": "2 deg", "neutral": "p/mid", "faces": w}),
    );
    let b = |z: f64| (40.0 - 2.0 * z * t) * (30.0 - 2.0 * z * t);
    near(
        volume(&mut s, "d", "b"),
        prismatoid(10.0, b) + prismatoid(10.0, |z| b(-z)),
        1e-9,
    );
    let (d, _) = report(&mut s, "d");
    assert_eq!(d["neutral"]["origin_m"], json!([0.0, 0.0, 0.01]), "{d}");
}

#[test]
fn a_boss_and_the_hole_through_it_drafted_about_the_plates_top() {
    let (_dir, mut s) = boxed();
    // A 40 × 40 × 10 plate, a boss of radius 5 and height 10 on its middle,
    // a hole of radius 3 through both.
    must(
        &mut s,
        "feature.edit",
        json!({"part": "p", "id": "b", "params": {"x": 40, "y": 40, "z": 10}}),
    );
    add(
        &mut s,
        json!({"type": "cylinder", "id": "boss", "operation": "join", "targets": ["b"],
               "params": {"radius": 5, "height": 10}, "placement": {"origin": [20, 20, 10]}}),
    );
    add(
        &mut s,
        json!({"type": "cylinder", "id": "bore", "operation": "cut", "targets": ["b"],
               "params": {"radius": 3, "height": 40}, "placement": {"origin": [20, 20, -10]}}),
    );
    let j = body(&mut s, "bore");
    let (side, wall, top) = (
        round(&j, "b", "cylinder", 5.0),
        round(&j, "b", "cylinder", 3.0),
        plane(&j, "b", 2, 1.0, 10.0),
    );
    // About the plate's top face, its normal the pull.
    add(
        &mut s,
        json!({"type": "draft", "id": "d", "angle": "3 deg", "neutral": top,
               "faces": [side, wall]}),
    );
    let t = tan(3.0);
    let want = 40.0 * 40.0 * 10.0 - frustum(10.0, 3.0 - 10.0 * t, 3.0)
        + frustum(10.0, 5.0, 5.0 - 10.0 * t)
        - frustum(10.0, 3.0, 3.0 + 10.0 * t);
    near(volume(&mut s, "d", "b"), want, 1e-5);
    // Both are cones now, at their angle exactly.
    let d = body(&mut s, "d");
    let cones = d["faces"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["surface"]["type"] == "cone")
        .count();
    assert_eq!(cones, 2, "{d}");
    let (r, _) = report(&mut s, "d");
    assert!(
        r["angles"]
            .as_array()
            .unwrap()
            .iter()
            .all(|x| x["exact"] == true),
        "{r}"
    );
    assert_eq!(names(&mut s, "d"), names(&mut s, "bore"));
}

#[test]
fn a_rounded_rectangles_walls_drafted_with_their_corners_by_tangency() {
    let (_dir, mut s) = boxed();
    let b = body(&mut s, "b");
    let ups: Vec<String> = [[0.0, 0.0], [40.0, 0.0], [40.0, 30.0], [0.0, 30.0]]
        .iter()
        .map(|[x, y]| edge(&b, "b", [*x, *y, 0.0], [*x, *y, 20.0]))
        .collect();
    add(
        &mut s,
        json!({"type": "fillet", "id": "f", "radius": 5, "edges": ups}),
    );
    let f = body(&mut s, "f");
    let w = vec![
        plane(&f, "b", 0, 1.0, 40.0),
        plane(&f, "b", 0, -1.0, 0.0),
        plane(&f, "b", 1, 1.0, 30.0),
        plane(&f, "b", 1, -1.0, 0.0),
    ];
    // tangent is true by default: the corners join the draft.
    add(
        &mut s,
        json!({"type": "draft", "id": "d", "angle": "2 deg", "neutral": "xy", "faces": w}),
    );
    let t = tan(2.0);
    let a = |z: f64| {
        (40.0 - 2.0 * z * t) * (30.0 - 2.0 * z * t) - (4.0 - PI) * (5.0 - z * t) * (5.0 - z * t)
    };
    near(volume(&mut s, "d", "b"), prismatoid(20.0, a), 1e-4);
    let (r, _) = report(&mut s, "d");
    assert_eq!(r["angles"].as_array().unwrap().len(), 8, "{r}");
    assert_eq!(r["modified"]["faces"], 8, "{r}");
}

#[test]
fn a_parting_line_round_a_ledged_box_drafts_both_halves() {
    let (_dir, mut s) = boxed();
    // A 40 × 30 × 10 box with a 30 × 20 × 10 box on it, inset 5 all round.
    must(
        &mut s,
        "feature.edit",
        json!({"part": "p", "id": "b", "params": {"x": 40, "y": 30, "z": 10}}),
    );
    add(
        &mut s,
        json!({"type": "box", "id": "u", "operation": "join", "targets": ["b"],
               "params": {"x": 30, "y": 20, "z": 10}, "placement": {"origin": [5, 5, 10]}}),
    );
    let u = body(&mut s, "u");
    let ring = |c: [[f64; 2]; 4]| -> Vec<String> {
        (0..4)
            .map(|k| {
                let ([x0, y0], [x1, y1]) = (c[k], c[(k + 1) % 4]);
                edge(&u, "b", [x0, y0, 10.0], [x1, y1, 10.0])
            })
            .collect()
    };
    let mut parting = ring([[0.0, 0.0], [40.0, 0.0], [40.0, 30.0], [0.0, 30.0]]);
    parting.extend(ring([[5.0, 5.0], [35.0, 5.0], [35.0, 25.0], [5.0, 25.0]]));
    let faces = vec![
        plane(&u, "b", 0, 1.0, 40.0),
        plane(&u, "b", 0, -1.0, 0.0),
        plane(&u, "b", 1, 1.0, 30.0),
        plane(&u, "b", 1, -1.0, 0.0),
        plane(&u, "b", 0, 1.0, 35.0),
        plane(&u, "b", 0, -1.0, 5.0),
        plane(&u, "b", 1, 1.0, 25.0),
        plane(&u, "b", 1, -1.0, 5.0),
    ];
    add(
        &mut s,
        json!({"type": "draft", "id": "d", "angle": "2 deg", "parting": parting,
               "pull": "z", "faces": faces}),
    );
    let t = tan(2.0);
    // The top half narrows going up from the ledge, the bottom going down.
    let want = prismatoid(10.0, |z| (30.0 - 2.0 * z * t) * (20.0 - 2.0 * z * t))
        + prismatoid(10.0, |z| (40.0 - 2.0 * z * t) * (30.0 - 2.0 * z * t));
    near(volume(&mut s, "d", "b"), want, 1e-9);
    assert_eq!(names(&mut s, "d"), names(&mut s, "u"));
    // A ledge square to the pull holds the angle: no warning.
    let (_, warnings) = report(&mut s, "d");
    assert!(warnings.is_empty(), "{warnings:?}");
}

#[test]
fn walls_at_their_own_angles_and_one_flipped() {
    let (_dir, mut s) = boxed();
    let w = walls(&mut s);
    // +x and -x at 2°, +y at 5° (a group), -y at 2° against the pull.
    add(
        &mut s,
        json!({"type": "draft", "id": "d", "angle": "2 deg", "neutral": "xy",
               "faces": [w[0], w[1], w[3]],
               "groups": [{"angle": "5 deg", "faces": [w[2]]}],
               "flip": [w[3]]}),
    );
    let (t2, t5) = (tan(2.0), tan(5.0));
    let a = |z: f64| (40.0 - 2.0 * z * t2) * (30.0 - z * (t5 - t2));
    near(volume(&mut s, "d", "b"), prismatoid(20.0, a), 1e-9);
    // Each face at its own angle from its own pull.
    let (r, warnings) = report(&mut s, "d");
    let by = |f: &str| {
        r["angles"]
            .as_array()
            .unwrap()
            .iter()
            .find(|x| x["face"] == f)
            .unwrap()["least"]["value"]
            .clone()
    };
    assert_eq!(
        (by(&w[2]), by(&w[3])),
        (json!("5 deg"), json!("2 deg")),
        "{r}"
    );
    assert!(warnings.is_empty());
}

#[test]
fn a_fillet_after_a_draft_and_one_before_it() {
    let (_dir, mut s) = boxed();
    let b = body(&mut s, "b");
    let w = walls(&mut s);
    let rim = edge(&b, "b", [40.0, 0.0, 20.0], [40.0, 30.0, 20.0]);
    // After: built on the drafted edge, which keeps its name.
    add(
        &mut s,
        json!({"type": "draft", "id": "d", "angle": "2 deg", "neutral": "xy", "faces": w}),
    );
    add(
        &mut s,
        json!({"type": "fillet", "id": "f", "radius": 2, "edges": [rim]}),
    );
    let drafted = volume(&mut s, "d", "b");
    assert!(volume(&mut s, "f", "b") < drafted);
    // Before, with tangent false: the fillet beside the east wall can't
    // stay one, and the hint says what to do.
    let (_dir2, mut s) = boxed();
    let b = body(&mut s, "b");
    let up = edge(&b, "b", [40.0, 0.0, 0.0], [40.0, 0.0, 20.0]);
    add(
        &mut s,
        json!({"type": "fillet", "id": "f", "radius": 2, "edges": [up]}),
    );
    let east = plane(&body(&mut s, "f"), "b", 0, 1.0, 40.0);
    add(
        &mut s,
        json!({"type": "draft", "id": "d", "angle": "2 deg", "neutral": "xy",
               "faces": [east], "tangent": false}),
    );
    let e = failure(&mut s, "d");
    assert_eq!(
        (e.code, e.details["reason"].as_str()),
        (Code::TweakFailed, Some("tangency_broken")),
        "{e}"
    );
    assert!(
        e.details["hint"].as_str().unwrap().contains("tangent true"),
        "{e}"
    );
    // With tangency (the default), the fillet joins the draft.
    must(
        &mut s,
        "feature.edit",
        json!({"part": "p", "id": "d", "tangent": true}),
    );
    volume(&mut s, "d", "b");
}

#[test]
fn a_new_angle_renames_nothing_and_a_cut_on_a_drafted_wall_survives_it() {
    let (_dir, mut s) = boxed();
    must(
        &mut s,
        "param.set",
        json!({"part": "p", "parameters": {"a": "2 deg"}}),
    );
    let w = walls(&mut s);
    add(
        &mut s,
        json!({"type": "draft", "id": "d", "angle": "a", "neutral": "xy", "faces": w}),
    );
    // A bore into the east wall, along -x.
    add(
        &mut s,
        json!({"type": "cylinder", "id": "bore", "operation": "cut", "targets": ["b"],
               "params": {"radius": 3, "height": 10},
               "placement": {"origin": [45, 15, 10], "z": [-1, 0, 0], "x": [0, 1, 0]}}),
    );
    let before = names(&mut s, "d");
    for a in ["3 deg", "5 deg", "1 deg"] {
        must(
            &mut s,
            "param.set",
            json!({"part": "p", "parameters": {"a": a}}),
        );
        assert_eq!(names(&mut s, "d"), before, "{a}");
        volume(&mut s, "bore", "b");
    }
}

#[test]
fn refusals() {
    let (_dir, mut s) = boxed();
    let w = walls(&mut s);
    let b = body(&mut s, "b");
    let top = plane(&b, "b", 2, 1.0, 20.0);
    // An angle out of range is refused when written.
    let e = call(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "draft", "angle": "90 deg", "neutral": "xy",
               "faces": [w[0]]}),
    )
    .unwrap_err();
    assert_eq!(
        (e.code, e.details["pointer"].as_str()),
        (Code::InvalidParam, Some("/angle"))
    );
    // A neutral plane above the box crosses no wall.
    add(
        &mut s,
        json!({"type": "plane", "id": "high", "by": "offset", "from": "xy", "distance": 50}),
    );
    add(
        &mut s,
        json!({"type": "draft", "id": "d1", "angle": "2 deg", "neutral": "p/high",
               "faces": [w[0]]}),
    );
    let e = failure(&mut s, "d1");
    assert_eq!(e.details["reason"], "no_hinge", "{e}");
    assert_eq!(e.details["entities"][0]["reference"], json!(w[0]), "{e}");
    must(&mut s, "feature.delete", json!({"part": "p", "id": "d1"}));
    // The top about one of its own edges, pulled along its normal.
    let rim = edge(&b, "b", [0.0, 0.0, 20.0], [40.0, 0.0, 20.0]);
    add(
        &mut s,
        json!({"type": "draft", "id": "d2", "angle": "2 deg", "parting": [rim],
               "pull": "z", "faces": [top]}),
    );
    let e = failure(&mut s, "d2");
    assert_eq!(e.details["reason"], "hinge_along_pull", "{e}");
    assert!(e.details["hint"].as_str().unwrap().contains("pull"), "{e}");
    must(&mut s, "feature.delete", json!({"part": "p", "id": "d2"}));
    // Two opposite walls drafted past each other: a topology change the
    // fixed engine refuses.
    add(
        &mut s,
        json!({"type": "draft", "id": "d3", "angle": "80 deg", "neutral": "xy",
               "faces": [w[0], w[1]]}),
    );
    let e = failure(&mut s, "d3");
    let reason = e.details["reason"].as_str().unwrap();
    assert!(
        [
            "edge_vanishes",
            "vertex_splits",
            "face_collapses",
            "self_intersecting",
            "uncertain"
        ]
        .contains(&reason),
        "{e}"
    );
    if ["edge_vanishes", "vertex_splits", "face_collapses"].contains(&reason) {
        assert!(
            e.details["hint"].as_str().unwrap().contains("topology"),
            "{e}"
        );
    }
}

#[test]
fn a_closed_sheet_drafts_and_an_open_ones_rim_is_refused() {
    let (_dir, mut s) = boxed();
    let b = body(&mut s, "b");
    let all: Vec<String> = b["faces"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| format!("p/b/{}", f["name"].as_str().unwrap()))
        .collect();
    let top = plane(&b, "b", 2, 1.0, 20.0);
    let w = walls(&mut s);
    add(
        &mut s,
        json!({"type": "extract", "id": "skin", "faces": all}),
    );
    let on = |f: &str| f.replacen("p/b/", "p/skin/", 1);
    add(
        &mut s,
        json!({"type": "draft", "id": "d", "angle": "2 deg", "neutral": "xy",
               "faces": w.iter().map(|f| on(f)).collect::<Vec<_>>()}),
    );
    let r = must(&mut s, "rebuild", json!({"part": "p"}));
    let skin = r["parts"][0]["solids"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["solid"] == "skin")
        .unwrap()
        .clone();
    assert_eq!(
        (skin["ok"].as_bool(), skin["kind"].as_str()),
        (Some(true), Some("sheet")),
        "{skin}"
    );
    // An open box: its walls' top edges are laminar.
    let open: Vec<String> = all.iter().filter(|f| **f != top).cloned().collect();
    add(
        &mut s,
        json!({"type": "extract", "id": "tray", "faces": open}),
    );
    let on = |f: &str| f.replacen("p/b/", "p/tray/", 1);
    add(
        &mut s,
        json!({"type": "draft", "id": "d2", "angle": "2 deg", "neutral": "xy",
               "faces": [on(&w[0])]}),
    );
    let e = failure(&mut s, "d2");
    assert_eq!(e.details["reason"], "laminar_face", "{e}");
}

#[test]
fn a_parting_line_climbing_along_the_pull_warns_its_angle_varies() {
    let (_dir, mut s) = boxed();
    let b = body(&mut s, "b");
    let top = plane(&b, "b", 2, 1.0, 20.0);
    let east = plane(&b, "b", 0, 1.0, 40.0);
    let rim = edge(&b, "b", [40.0, 0.0, 20.0], [40.0, 30.0, 20.0]);
    // The top tilted 10° about its edge along x at y = 0: the east wall's
    // top edge climbs along z.
    add(
        &mut s,
        json!({"type": "axis", "id": "hinge", "by": "two_points",
               "points": [format!("p/b/{}", vertex(&b, [0.0, 0.0, 20.0])),
                          format!("p/b/{}", vertex(&b, [40.0, 0.0, 20.0]))]}),
    );
    add(
        &mut s,
        json!({"type": "move_face", "id": "m", "by": "rotate", "axis": "p/hinge",
               "angle": "10 deg", "faces": [top]}),
    );
    add(
        &mut s,
        json!({"type": "draft", "id": "d", "angle": "3 deg", "parting": [rim],
               "pull": "z", "faces": [east.clone()]}),
    );
    volume(&mut s, "d", "b");
    let (r, warnings) = report(&mut s, "d");
    let w = warnings
        .iter()
        .find(|w| w["code"] == "draft_angle_varies")
        .unwrap_or_else(|| panic!("{warnings:?}"));
    assert_eq!(
        (w["feature"].as_str(), w["face"].as_str()),
        (Some("d"), Some(east.as_str()))
    );
    // Its angle from the pull isn't the 3° asked, and the report says so.
    let a = &r["angles"][0];
    assert!(
        (a["least"]["rad"].as_f64().unwrap() - 3f64.to_radians()).abs() > 1e-3f64.to_radians(),
        "{r}"
    );
}
