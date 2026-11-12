//! Face tweaks through the protocol (design 400, plan 17.A): a boss slid
//! along its plate, a box's top raised and tilted, a pocket's walls and
//! floor offset with their fillets, a fillet built after a move, names and
//! a hole and a mate surviving edits, refusals; replacements and sheets
//! (Task 13).

mod common;

use common::{call, must};
use core::f64::consts::PI;
use ryucad::doc::id::Id;
use ryucad::error::Code;
use ryucad::session::Session;
use serde_json::{Value, json};

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

fn near(got: f64, want: f64, rel: f64) {
    assert!(
        (got - want).abs() <= rel * want,
        "{got} mm³ against {want} mm³"
    );
}

#[test]
fn a_boss_slides_along_its_plate() {
    let (_dir, mut s) = boxed();
    add(
        &mut s,
        json!({"type": "box", "id": "j", "operation": "join", "targets": ["b"],
                       "params": {"x": 10, "y": 10, "z": 10}, "placement": {"origin": [5, 5, 20]}}),
    );
    let j = body(&mut s, "j");
    let boss = vec![
        plane(&j, "b", 0, -1.0, 5.0),
        plane(&j, "b", 0, 1.0, 15.0),
        plane(&j, "b", 1, -1.0, 5.0),
        plane(&j, "b", 1, 1.0, 15.0),
        plane(&j, "b", 2, 1.0, 30.0),
    ];
    add(
        &mut s,
        json!({"type": "move_face", "id": "m", "by": "translate", "direction": "x",
                       "distance": 5, "faces": boss}),
    );
    near(volume(&mut s, "m", "b"), 25000.0, 1e-9);
    // Slid: its side now at x = 20.
    plane(&body(&mut s, "m"), "b", 0, 1.0, 20.0);
    assert_eq!(names(&mut s, "m"), names(&mut s, "j"));
}

#[test]
fn a_top_raised_then_tilted_about_its_edge() {
    let (_dir, mut s) = boxed();
    let b = body(&mut s, "b");
    let top = plane(&b, "b", 2, 1.0, 20.0);
    add(
        &mut s,
        json!({"type": "axis", "id": "hinge", "by": "two_points",
                       "points": [format!("p/b/{}", vertex(&b, [0.0, 0.0, 20.0])),
                                  format!("p/b/{}", vertex(&b, [40.0, 0.0, 20.0]))]}),
    );
    add(
        &mut s,
        json!({"type": "move_face", "id": "m", "by": "translate", "direction": "z",
                       "distance": 5, "faces": [top]}),
    );
    near(volume(&mut s, "m", "b"), 40.0 * 30.0 * 25.0, 1e-9);
    // A new `by` keeps only what it takes: turned 3° about the edge along
    // +x, the top rises towards y = 30, a wedge on the box.
    must(
        &mut s,
        "feature.edit",
        json!({"part": "p", "id": "m", "by": "rotate",
                                         "axis": "p/hinge", "angle": "3 deg"}),
    );
    let wedge = 0.5 * 30.0 * 30.0 * ryukei::math::tan(3.0f64.to_radians()) * 40.0;
    near(volume(&mut s, "m", "b"), 24000.0 + wedge, 1e-9);
}

/// A box with a pocket 20 × 10 × 5 mm cut from its top, its four upright
/// edges filleted at 2 mm: the floor and the four walls, as references.
fn pocketed(s: &mut Session) -> Vec<String> {
    add(
        s,
        json!({"type": "box", "id": "k", "operation": "cut", "targets": ["b"],
                  "params": {"x": 20, "y": 10, "z": 30}, "placement": {"origin": [10, 10, 15]}}),
    );
    let k = body(s, "k");
    let ups: Vec<String> = [[10.0, 10.0], [30.0, 10.0], [10.0, 20.0], [30.0, 20.0]]
        .iter()
        .map(|[x, y]| edge(&k, "b", [*x, *y, 15.0], [*x, *y, 20.0]))
        .collect();
    add(
        s,
        json!({"type": "fillet", "id": "f", "radius": 2, "edges": ups}),
    );
    let f = body(s, "f");
    vec![
        plane(&f, "b", 2, 1.0, 15.0),
        plane(&f, "b", 0, 1.0, 10.0),
        plane(&f, "b", 0, -1.0, 30.0),
        plane(&f, "b", 1, 1.0, 10.0),
        plane(&f, "b", 1, -1.0, 20.0),
    ]
}

/// A rounded rectangle's area, `w` × `h` with corners of radius `r`.
fn rounded(w: f64, h: f64, r: f64) -> f64 {
    w * h - (4.0 - PI) * r * r
}

#[test]
fn a_pockets_faces_offset_with_their_fillets() {
    let (_dir, mut s) = boxed();
    let faces = pocketed(&mut s);
    near(
        volume(&mut s, "f", "b"),
        24000.0 - rounded(20.0, 10.0, 2.0) * 5.0,
        1e-5,
    );
    // Offset outwards by 0.5 mm, into the pocket: the fillets, tangent to
    // the walls, come with them (tangent is true by default).
    add(
        &mut s,
        json!({"type": "move_face", "id": "m", "by": "offset", "distance": 0.5,
                       "faces": faces}),
    );
    near(
        volume(&mut s, "m", "b"),
        24000.0 - rounded(19.0, 9.0, 1.5) * 4.5,
        1e-5,
    );
    // Without its tangent neighbours, refused: the hint says what to do.
    must(
        &mut s,
        "feature.edit",
        json!({"part": "p", "id": "m", "tangent": false}),
    );
    let e = failure(&mut s, "m");
    assert_eq!(
        (e.code, e.details["reason"].as_str()),
        (Code::TweakFailed, Some("tangency_broken")),
        "{e}"
    );
    assert!(
        e.details["hint"].as_str().unwrap().contains("tangent true"),
        "{e}"
    );
    assert_eq!(e.details["entities"][0]["kind"], "edge", "{e}");
}

#[test]
fn a_fillet_after_a_move_is_built_on_the_moved_edge() {
    let (_dir, mut s) = boxed();
    let top = plane(&body(&mut s, "b"), "b", 2, 1.0, 20.0);
    add(
        &mut s,
        json!({"type": "move_face", "id": "m", "by": "translate", "direction": "z",
                       "distance": 5, "faces": [top]}),
    );
    let e = edge(&body(&mut s, "m"), "b", [0.0, 0.0, 25.0], [40.0, 0.0, 25.0]);
    add(
        &mut s,
        json!({"type": "fillet", "id": "f", "radius": 2, "edges": [e]}),
    );
    near(volume(&mut s, "f", "b"), 30000.0 - (4.0 - PI) * 40.0, 1e-5);
}

#[test]
fn names_a_hole_and_a_mate_survive_distance_edits() {
    let (_dir, mut s) = boxed();
    let top = plane(&body(&mut s, "b"), "b", 2, 1.0, 20.0);
    add(
        &mut s,
        json!({"type": "move_face", "id": "m", "by": "translate", "direction": "z",
                       "distance": 5, "faces": [top.clone()]}),
    );
    // A hole drilled at a point sketched on the moved top.
    add(&mut s, json!({"type": "sketch", "id": "s", "face": top}));
    must(
        &mut s,
        "sketch.edit",
        json!({"part": "p", "sketch": "s",
        "ops": [{"op": "add", "id": "q", "type": "point", "at": [10, 10]}]}),
    );
    add(
        &mut s,
        json!({"type": "hole", "id": "h", "sketch": "s", "diameter": 4,
                       "depth": "through_all", "targets": ["b"]}),
    );
    // A lid mated flat on the moved top.
    must(&mut s, "part.create", json!({"part": "lid"}));
    add(
        &mut s,
        json!({"part": "lid", "type": "box", "id": "c", "params": {"x": 10, "y": 10, "z": 2}}),
    );
    let under = plane(
        &must(&mut s, "inspect", json!({"part": "lid", "feature": "c"}))["bodies"][0],
        "c",
        2,
        -1.0,
        0.0,
    )
    .replacen("p/", "", 1);
    must(
        &mut s,
        "part.create",
        json!({"part": "asm", "kind": "assembly"}),
    );
    add(
        &mut s,
        json!({"part": "asm", "type": "instance", "id": "base", "of": "p", "fixed": true}),
    );
    add(
        &mut s,
        json!({"part": "asm", "type": "instance", "id": "top", "of": "lid"}),
    );
    add(
        &mut s,
        json!({"part": "asm", "type": "mate", "id": "on", "form": "planar",
                       "a": format!("asm/base/{}", top.replacen("p/", "", 1)),
                       "b": format!("asm/top/{under}")}),
    );
    let lid_z = |s: &mut Session| -> f64 {
        let r = must(s, "rebuild", json!({"part": "asm"}));
        let asm = r["parts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["part"] == "asm")
            .unwrap()
            .clone();
        let i = asm["features"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["id"] == "top")
            .unwrap()
            .clone();
        i["instance"]["pose"]["origin_m"][2].as_f64().unwrap()
    };
    let (before, z0) = (names(&mut s, "m"), lid_z(&mut s));
    must(
        &mut s,
        "feature.edit",
        json!({"part": "p", "id": "m", "distance": "8 mm"}),
    );
    assert_eq!(names(&mut s, "m"), before);
    let r = must(&mut s, "rebuild", json!({"part": "p"}));
    assert_eq!(r["parts"][0]["failed"], 0, "{r}");
    // The lid follows the top up 3 mm (the edit keeps the assembly current).
    assert!((lid_z(&mut s) - z0 - 0.003).abs() < 1e-9);
}

#[test]
fn refusals() {
    let (_dir, mut s) = boxed();
    let top = plane(&body(&mut s, "b"), "b", 2, 1.0, 20.0);
    // Nothing moving: refused when written.
    let e = call(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "move_face", "by": "translate",
        "direction": "z", "distance": 0, "faces": [top.clone()]}),
    )
    .unwrap_err();
    assert_eq!(
        (e.code, e.details["pointer"].as_str()),
        (Code::InvalidParam, Some("/distance"))
    );
    // The top pushed down past the bottom: a topology change, with its hint.
    add(
        &mut s,
        json!({"type": "move_face", "id": "m", "by": "translate", "direction": "z",
                       "distance": -25, "faces": [top]}),
    );
    let e = failure(&mut s, "m");
    assert_eq!(e.code, Code::TweakFailed, "{e}");
    assert!(
        matches!(
            e.details["reason"].as_str(),
            Some("edge_vanishes" | "vertex_splits" | "face_collapses")
        ),
        "{e}"
    );
    assert!(
        e.details["hint"].as_str().unwrap().contains("topology"),
        "{e}"
    );
    assert_eq!(e.details["kernel"], "tweak");
}
#[test]
fn a_top_made_flush_with_another_solids_face_and_with_a_datum_plane() {
    let (_dir, mut s) = boxed();
    add(
        &mut s,
        json!({"type": "box", "id": "c", "params": {"x": 10, "y": 10, "z": 25},
                       "placement": {"origin": [60, 0, 0]}}),
    );
    let top = plane(&body(&mut s, "b"), "b", 2, 1.0, 20.0);
    let lid = plane(&body(&mut s, "c"), "c", 2, 1.0, 25.0);
    add(
        &mut s,
        json!({"type": "replace_face", "id": "r",
                       "groups": [{"with": lid, "faces": [top.clone()]}]}),
    );
    near(volume(&mut s, "r", "b"), 40.0 * 30.0 * 25.0, 1e-9);
    // c, its source, is untouched (the replacement acts on b alone).
    near(volume(&mut s, "c", "c"), 10.0 * 10.0 * 25.0, 1e-9);
    add(
        &mut s,
        json!({"type": "plane", "id": "d", "by": "offset", "from": "xy", "distance": 22}),
    );
    must(
        &mut s,
        "feature.move",
        json!({"part": "p", "id": "d", "after": "c"}),
    );
    must(
        &mut s,
        "feature.edit",
        json!({"part": "p", "id": "r",
        "groups": [{"with": "p/d", "faces": [top]}]}),
    );
    near(volume(&mut s, "r", "b"), 40.0 * 30.0 * 22.0, 1e-9);
}

/// Sketch `id` on xy with a square of side `a` mm centred on the origin.
fn square(s: &mut Session, id: &str, a: f64) {
    add(s, json!({"type": "sketch", "id": id, "plane": "xy"}));
    let h = a / 2.0;
    must(
        s,
        "sketch.edit",
        json!({"part": "p", "sketch": id, "ops": [
            {"op": "add", "id": "p1", "type": "point", "at": [-h, -h]},
            {"op": "add", "id": "p2", "type": "point", "at": [h, -h]},
            {"op": "add", "id": "p3", "type": "point", "at": [h, h]},
            {"op": "add", "id": "p4", "type": "point", "at": [-h, h]},
            {"op": "add", "id": "l1", "type": "line", "start": "p1", "end": "p2"},
            {"op": "add", "id": "l2", "type": "line", "start": "p2", "end": "p3"},
            {"op": "add", "id": "l3", "type": "line", "start": "p3", "end": "p4"},
            {"op": "add", "id": "l4", "type": "line", "start": "p4", "end": "p1"}
        ]}),
    );
}

#[test]
fn a_sheet_moves_but_its_boundarys_face_isnt_replaced() {
    let dir = tempfile::tempdir().unwrap();
    ryucad::server::init(dir.path(), None).unwrap();
    let mut s = Session::open(dir.path()).unwrap();
    must(&mut s, "part.create", json!({"part": "p"}));
    square(&mut s, "q", 10.0);
    add(&mut s, json!({"type": "planar", "id": "f", "sketch": "q"}));
    let face = format!(
        "p/f/{}",
        body(&mut s, "f")["faces"][0]["name"].as_str().unwrap()
    );
    add(
        &mut s,
        json!({"type": "move_face", "id": "m", "by": "translate", "direction": "z",
                       "distance": 5, "faces": [face.clone()]}),
    );
    let moved = body(&mut s, "m");
    assert_eq!(moved["kind"], "sheet", "{moved}");
    let z = moved["faces"][0]["surface"]["frame"]["origin"][2]
        .as_f64()
        .unwrap();
    assert!((z - 0.005).abs() < 1e-12, "{moved}");
    // A feature that needs a solid still refuses a sheet.
    let e = call(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "shell", "target": "f", "thickness": 1}),
    )
    .unwrap_err();
    assert_eq!(e.code, Code::WrongBodyKind, "{e}");
    // A face with a laminar edge has no replacement: the kernel's refusal.
    add(
        &mut s,
        json!({"type": "replace_face", "id": "r",
                       "groups": [{"with": "zx", "faces": [face]}]}),
    );
    let e = failure(&mut s, "r");
    assert_eq!(
        (e.code, e.details["reason"].as_str()),
        (Code::TweakFailed, Some("laminar_face")),
        "{e}"
    );
}
