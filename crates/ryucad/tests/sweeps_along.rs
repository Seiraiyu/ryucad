//! Sweeps along paths through the protocol, against closed forms: a
//! circle along a straight line (a cylinder) and round a bend of a line
//! and a tangent quarter arc (Pappus along the path), a square twisted
//! along a line (the twisted prism keeps its area at every height),
//! scaled (a frustum), a spring up a helix (binormal about its axis by
//! default: Pappus about the axis), the bend cut from a block (checked
//! clean, the block less the tube), a face of a solid swept and joined
//! to it; names unchanged across edits of the path, the twist, the scale
//! and the profile's size; and the refusals in the agent's terms. Two kernel limits are pinned so a fix shows: a
//! flat-sided thread up a helix is refused as `uncertain` (its sides
//! meet where the check can't part them), and a round one, which sweeps,
//! can't be cut from a shank (`boolean_failed`). The spring, the twisted
//! square and the threads take minutes in a debug build: they run in
//! release (`full.yml`).

use core::f64::consts::{PI, TAU};
use ryucad::doc::id::Id;
use ryucad::error::Code;
use ryucad::proto::Request;
use ryucad::server::handle;
use ryucad::session::Session;
use serde_json::{Value, json};
use std::collections::BTreeSet;

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

/// A project with an empty part `p`.
fn project() -> (tempfile::TempDir, Session) {
    let dir = tempfile::tempdir().unwrap();
    ryucad::server::init(dir.path(), None).unwrap();
    let mut s = Session::open(dir.path()).unwrap();
    must(&mut s, "part.create", json!({"part": "p"}));
    (dir, s)
}

/// Sketch `id` on `plane` with `ops`.
fn sketch(s: &mut Session, id: &str, plane: Value, ops: Value) {
    let mut f = json!({"type": "sketch", "id": id});
    match plane {
        Value::String(p) => f["plane"] = json!(p),
        Value::Object(m) => {
            for (k, x) in m {
                f[k] = x;
            }
        }
        _ => unreachable!(),
    }
    add(s, f);
    must(
        s,
        "sketch.edit",
        json!({"part": "p", "sketch": id, "ops": ops}),
    );
}

/// A circle of radius `r` mm about `(x, y)` in the sketch's plane.
fn circle(x: f64, y: f64, r: f64) -> Value {
    json!([
        {"op": "add", "id": "c", "type": "point", "at": [x, y]},
        {"op": "add", "id": "ci", "type": "circle", "center": "c", "r": r}
    ])
}

/// A square of side `a` mm centred on `(x, y)`.
fn square(x: f64, y: f64, a: f64) -> Value {
    let h = a / 2.0;
    json!([
        {"op": "add", "id": "p1", "type": "point", "at": [x - h, y - h]},
        {"op": "add", "id": "p2", "type": "point", "at": [x + h, y - h]},
        {"op": "add", "id": "p3", "type": "point", "at": [x + h, y + h]},
        {"op": "add", "id": "p4", "type": "point", "at": [x - h, y + h]},
        {"op": "add", "id": "l1", "type": "line", "start": "p1", "end": "p2"},
        {"op": "add", "id": "l2", "type": "line", "start": "p2", "end": "p3"},
        {"op": "add", "id": "l3", "type": "line", "start": "p3", "end": "p4"},
        {"op": "add", "id": "l4", "type": "line", "start": "p4", "end": "p1"}
    ])
}

/// A line from `a` to `b` in the sketch's plane, `l1`.
fn line(a: [f64; 2], b: [f64; 2]) -> Value {
    json!([
        {"op": "add", "id": "a", "type": "point", "at": a},
        {"op": "add", "id": "b", "type": "point", "at": b},
        {"op": "add", "id": "l1", "type": "line", "start": "a", "end": "b"}
    ])
}

/// The body solid `solid` of part `p` shows.
fn body(s: &mut Session, solid: &str) -> ryukei::topo::Body {
    let part = Id::parse("p", "part").unwrap();
    let r = s.rebuilt(&part).unwrap().clone();
    assert_eq!(r.failed(), 0, "{:?}", r.features);
    let solids = ryucad::solid::Solids::of(s.project().part(&part).unwrap());
    let shown = r.shown(&solids);
    let (_, b) = shown
        .iter()
        .find(|x| x.solid.as_str() == solid)
        .and_then(|x| x.state)
        .unwrap_or_else(|| panic!("{solid} shows"));
    b.body.clone()
}

/// A body's volume from a mesh within a micron: within a part in a
/// thousand of a profile a millimetre across.
fn volume(b: &ryukei::topo::Body) -> f64 {
    assert_eq!(ryukei::check::check(b), []);
    let m = ryukei::mesh::tessellate(b, &ryukei::mesh::MeshParams::new(1e-6, 0.1)).unwrap();
    assert!(m.faults.is_empty(), "{:?}", m.faults);
    ryukei_testkit::mesh::volume_and_area(&m).0
}

fn close(got: f64, want: f64, rel: f64) {
    assert!(
        (got - want).abs() <= rel * want.abs(),
        "{got} against {want}: {}",
        (got - want) / want
    );
}

/// Feature `f` of part `p` as rebuild reports it, failed or not.
fn reported(s: &mut Session, f: &str) -> Value {
    let r = match call(s, "rebuild", json!({"part": "p"})) {
        Ok(r) => r,
        Err(e) => e.details,
    };
    r["parts"][0]["features"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["id"] == f)
        .unwrap_or_else(|| panic!("{r}"))
        .clone()
}

/// The names of every face, edge and vertex solid `solid` shows.
fn names(s: &mut Session, solid: &str) -> BTreeSet<String> {
    use ryukei::topo::EntityId;
    let b = body(s, solid);
    let faces = b.faces().map(EntityId::Face);
    let edges = b.edges().map(EntityId::Edge);
    let vertices = b.vertices().map(EntityId::Vertex);
    faces
        .chain(edges)
        .chain(vertices)
        .map(|e| b.name_of(e).unwrap().to_string())
        .collect()
}

#[test]
fn a_circle_swept_along_a_line_is_a_cylinder() {
    let (_dir, mut s) = project();
    // The profile on xy, the path up z in a sketch on zx (sketch x is z).
    sketch(&mut s, "s", json!("xy"), circle(0.0, 0.0, 2.0));
    sketch(&mut s, "up", json!("zx"), line([0.0, 0.0], [30.0, 0.0]));
    let v = add(
        &mut s,
        json!({"type": "sweep", "id": "w", "sketch": "s", "path": "p/up/l1"}),
    );
    // The law it chose is written: minimal, the path being no helix.
    assert_eq!(v["feature"]["law"], "minimal", "{v}");
    close(volume(&body(&mut s, "w")), PI * 4e-6 * 0.03, 1e-3);
    let r = reported(&mut s, "w");
    assert_eq!(r["ok"], true, "{r}");
    let w = &r["sweep"];
    assert_eq!(w["profile"]["sketch"], "s", "{r}");
    assert_eq!(w["profile"]["loops"][0]["anchor"], "ci", "{r}");
    assert_eq!(w["path"]["segments"][0]["from"], "p/up/l1", "{r}");
    assert!((w["path"]["length_m"].as_f64().unwrap() - 0.03).abs() < 1e-15);
    // Every face's origin names the sweep, and the side its path segment.
    let o = must(&mut s, "inspect", json!({"part": "p", "feature": "w"}));
    let faces = o["bodies"][0]["faces"].as_array().unwrap();
    assert_eq!(faces.len(), 3, "{o}");
    let side = faces
        .iter()
        .find(|f| f["origin"]["entity"] == "ci")
        .unwrap_or_else(|| panic!("{o}"));
    assert_eq!(side["origin"]["feature"], "w");
    assert_eq!(side["origin"]["path_segment"], "p/up/l1");
    // Every face, edge and vertex has an origin naming the sweep: they are
    // derived from the keys as the kernel names them.
    for list in ["faces", "edges", "vertices"] {
        for e in o["bodies"][0][list].as_array().unwrap() {
            assert_eq!(e["origin"]["feature"], "w", "{list}: {e}");
            assert_eq!(e["origin"]["sketch"], "s", "{list}: {e}");
        }
    }
}

#[test]
#[cfg_attr(debug_assertions, ignore = "minutes in a debug build")]
fn a_twisted_square_keeps_its_area_and_a_scaled_one_is_a_frustum() {
    let (_dir, mut s) = project();
    sketch(&mut s, "s", json!("xy"), square(0.0, 0.0, 4.0));
    sketch(&mut s, "up", json!("zx"), line([0.0, 0.0], [20.0, 0.0]));
    add(
        &mut s,
        json!({"type": "sweep", "id": "w", "sketch": "s", "path": "p/up/l1",
               "twist": "90 deg"}),
    );
    close(volume(&body(&mut s, "w")), 16e-6 * 0.02, 1e-3);
    let twisted = names(&mut s, "w");
    // Twisted further and scaled to half: a frustum's volume, and no name
    // moved.
    must(
        &mut s,
        "feature.edit",
        json!({"part": "p", "id": "w", "twist": "", "scale": "0.5"}),
    );
    let (a1, a2) = (16e-6, 4e-6);
    close(
        volume(&body(&mut s, "w")),
        0.02 / 3.0 * (a1 + a2 + f64::sqrt(a1 * a2)),
        1e-3,
    );
    assert_eq!(names(&mut s, "w"), twisted);
    // A longer path and a bigger square rename nothing either.
    must(
        &mut s,
        "sketch.edit",
        json!({"part": "p", "sketch": "up", "ops": [{"op": "move", "id": "b", "at": [25, 0]}]}),
    );
    assert_eq!(names(&mut s, "w"), twisted);
}

#[test]
#[cfg_attr(debug_assertions, ignore = "minutes in a debug build")]
fn a_spring_up_a_helix_turns_with_it() {
    let (_dir, mut s) = project();
    // A helix of radius 10 mm, pitch 4 mm, two turns, about z, starting on
    // x; the wire's section on zx about where it starts, radius 1 mm.
    add(
        &mut s,
        json!({"type": "helix", "id": "h", "axis": "z", "radius": "10 mm",
               "pitch": "4 mm", "turns": 2}),
    );
    sketch(&mut s, "s", json!("zx"), circle(0.0, 10.0, 1.0));
    let v = add(
        &mut s,
        json!({"type": "sweep", "id": "w", "sketch": "s", "path": "h"}),
    );
    // Binormal about the helix's axis, by default, written into its file.
    assert_eq!(v["feature"]["law"], "binormal", "{v}");
    assert_eq!(v["feature"]["direction"], "p/h", "{v}");
    // Pappus about the axis: the section's area round its centre's circle,
    // twice; the climb lies in the section's plane and adds nothing.
    close(
        volume(&body(&mut s, "w")),
        PI * 1e-6 * TAU * 0.01 * 2.0,
        2e-3,
    );
}

/// Sketch `bend` on zx (sketch x is z, sketch y is x): a line up z
/// 10 mm, then a quarter turn of radius 15 mm towards +x, tangent where
/// they meet; a path `pb` of the two; and a circle of radius 2 mm about
/// the origin on xy, sketch `s`. The path's length is
/// `10 mm + 15 mm × π/2`.
fn bend() -> (tempfile::TempDir, Session, f64) {
    let (dir, mut s) = project();
    sketch(&mut s, "s", json!("xy"), circle(0.0, 0.0, 2.0));
    sketch(
        &mut s,
        "bend",
        json!("zx"),
        json!([
            {"op": "add", "id": "a", "type": "point", "at": [0, 0]},
            {"op": "add", "id": "b", "type": "point", "at": [10, 0]},
            {"op": "add", "id": "c", "type": "point", "at": [10, 15]},
            {"op": "add", "id": "e", "type": "point", "at": [25, 15]},
            {"op": "add", "id": "l1", "type": "line", "start": "a", "end": "b"},
            {"op": "add", "id": "a1", "type": "arc", "center": "c", "start": "b", "end": "e"}
        ]),
    );
    add(
        &mut s,
        json!({"type": "path", "id": "pb", "segments": ["p/bend/l1", "p/bend/a1"]}),
    );
    (dir, s, 0.01 + 0.015 * PI / 2.0)
}

#[test]
fn a_circle_round_a_bend_is_pappus_along_the_path() {
    let (_dir, mut s, length) = bend();
    add(
        &mut s,
        json!({"type": "sweep", "id": "w", "sketch": "s", "path": "pb"}),
    );
    // Pappus: the disc's area along its centre's path, the line and the
    // quarter arc.
    close(volume(&body(&mut s, "w")), PI * 4e-6 * length, 1e-3);
    let r = reported(&mut s, "w");
    let segs = r["sweep"]["path"]["segments"].as_array().unwrap();
    assert_eq!(segs.len(), 2, "{r}");
    assert!((r["sweep"]["path"]["length_m"].as_f64().unwrap() - length).abs() < 1e-12);
    // Each leg's side says which segment it follows.
    let o = must(&mut s, "inspect", json!({"part": "p", "feature": "w"}));
    let along: BTreeSet<String> = o["bodies"][0]["faces"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|f| f["origin"]["path_segment"].as_str().map(str::to_owned))
        .collect();
    assert_eq!(
        along,
        BTreeSet::from(["p/bend/l1".to_owned(), "p/bend/a1".to_owned()]),
        "{o}"
    );
}

#[test]
fn a_bend_cut_from_a_block_takes_away_its_tube() {
    let (_dir, mut s, length) = bend();
    // A block from z = 2 mm up, holding all of the tube above it: the cut
    // enters through its bottom and ends inside.
    add(
        &mut s,
        json!({"type": "box", "id": "block", "params": {"x": 45, "y": 10, "z": 28},
               "placement": {"origin": [-5, -5, 2]}}),
    );
    add(
        &mut s,
        json!({"type": "sweep", "id": "w", "sketch": "s", "path": "pb",
               "operation": "cut", "targets": ["block"]}),
    );
    let b = body(&mut s, "block");
    assert_eq!(ryukei::check::check_intersections(&b), []);
    let m = ryukei::measure::mass(&b, 1.0, &ryukei::measure::MassOptions::default()).unwrap();
    // The block less the tube above z = 2 mm: the disc along the path, its
    // first 2 mm below the block.
    let want = 0.045 * 0.01 * 0.028 - PI * 4e-6 * (length - 0.002);
    assert!(
        (m.volume.value() - want).abs() <= m.volume.bound() + 1e-9 * want,
        "{:?} against {want}",
        m.volume
    );
    // The cut's faces are the sweep's, named by it.
    let o = must(&mut s, "inspect", json!({"part": "p", "feature": "w"}));
    assert!(
        o["bodies"][0]["faces"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["origin"]["feature"] == "w" && f["origin"]["entity"] == "ci"),
        "{o}"
    );
    assert!(call(&mut s, "check", json!({"part": "p"})).is_ok());
}

/// A shank of radius 5 mm, 20 mm long, up z from the origin; a helix `h`
/// about z from (5, 0, 4) mm, pitch 2 mm, three turns; a sketch `tooth`
/// on zx (sketch x is z, sketch y is x) holding `ops`.
fn shank_and_helix(ops: Value) -> (tempfile::TempDir, Session) {
    let (dir, mut s) = project();
    add(
        &mut s,
        json!({"type": "cylinder", "id": "shank", "params": {"radius": 5, "height": 20}}),
    );
    add(
        &mut s,
        json!({"type": "point", "id": "st", "by": "coordinates",
               "coordinates": ["5 mm", "0", "4 mm"]}),
    );
    add(
        &mut s,
        json!({"type": "helix", "id": "h", "axis": "z", "start": "p/st", "pitch": "2 mm",
               "turns": 3}),
    );
    sketch(&mut s, "tooth", json!("zx"), ops);
    (dir, s)
}

/// A limit of ryukei's booleans, pinned so a fix shows: a round thread
/// sweeps up the helix, but cutting it from the shank is refused, the
/// boolean unable to certify where the swept tube meets the shank's
/// cylinder (`boolean_failed`, `uncertain`). Fewer turns, a coarser pitch,
/// a deeper groove and a minimal law were all refused alike on cox-worker-1.
/// When it builds, check it clean and that it takes away the segment of
/// the circle inside the shank by Pappus about the axis.
#[test]
#[cfg_attr(debug_assertions, ignore = "minutes in a debug build")]
fn a_thread_cut_from_a_shank_is_refused_as_uncertain() {
    // The thread's section where the helix starts: a circle of radius
    // 0.4 mm about (z, x) = (4, 5.1) mm, reaching 0.3 mm into the shank.
    let (_dir, mut s) = shank_and_helix(json!([
        {"op": "add", "id": "o", "type": "point", "at": [4, 5.1]},
        {"op": "add", "id": "ci", "type": "circle", "center": "o", "r": 0.4}
    ]));
    let v = add(
        &mut s,
        json!({"type": "sweep", "id": "thread", "sketch": "tooth", "path": "h",
               "operation": "cut", "targets": ["shank"]}),
    );
    assert_eq!(v["feature"]["law"], "binormal", "{v}");
    let r = reported(&mut s, "thread");
    let e = &r["error"];
    assert_eq!(e["code"], "boolean_failed", "{r}");
    assert!(
        e["details"]["kernel"]
            .as_str()
            .unwrap()
            .starts_with("InvalidBoolean(Uncertain"),
        "{r}"
    );
    // The sweep alone builds: the same thread as a solid of its own.
    must(
        &mut s,
        "feature.edit",
        json!({"part": "p", "id": "thread", "operation": "new"}),
    );
    let r = reported(&mut s, "thread");
    assert_eq!(r["ok"], true, "{r}");
}

/// A limit of ryukei 8's sweeps, pinned so a fix shows: two flat sides of
/// a profile swept up a helix (a triangular thread's) meet along their
/// lateral where the self-intersection check can't certify them apart,
/// so the sweep is refused as `uncertain`, naming both sides by their
/// sketch lines. A round thread sweeps (above), but its cut is refused.
#[test]
#[cfg_attr(debug_assertions, ignore = "minutes in a debug build")]
fn a_flat_sided_thread_up_a_helix_is_refused_as_uncertain() {
    let (_dir, mut s) = shank_and_helix(json!([
        {"op": "add", "id": "p1", "type": "point", "at": [3.6, 5.5]},
        {"op": "add", "id": "p2", "type": "point", "at": [4.4, 5.5]},
        {"op": "add", "id": "p3", "type": "point", "at": [4, 4.6]},
        {"op": "add", "id": "l1", "type": "line", "start": "p1", "end": "p2"},
        {"op": "add", "id": "l2", "type": "line", "start": "p2", "end": "p3"},
        {"op": "add", "id": "l3", "type": "line", "start": "p3", "end": "p1"}
    ]));
    add(
        &mut s,
        json!({"type": "sweep", "id": "thread", "sketch": "tooth", "path": "h",
               "operation": "cut", "targets": ["shank"]}),
    );
    let r = reported(&mut s, "thread");
    let e = &r["error"];
    assert_eq!(e["code"], "sweep_failed", "{r}");
    assert_eq!(e["details"]["reason"], "uncertain", "{r}");
    let sides: BTreeSet<&str> = e["details"]["faces"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["origin"]["entity"].as_str().unwrap())
        .collect();
    assert_eq!(sides, BTreeSet::from(["l2", "l3"]), "{r}");
}

#[test]
fn a_face_of_a_solid_swept_and_joined() {
    let (_dir, mut s) = project();
    add(
        &mut s,
        json!({"type": "box", "id": "b", "params": {"x": 10, "y": 10, "z": 5}}),
    );
    let o = must(&mut s, "inspect", json!({"part": "p", "feature": "b"}));
    let top = o["bodies"][0]["faces"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| {
            let z = &f["surface"]["frame"]["z"];
            let sign = if f["reversed"] == true { -1.0 } else { 1.0 };
            f["surface"]["type"] == "plane" && z[2].as_f64().unwrap() * sign > 0.5
        })
        .map(|f| format!("p/b/{}", f["name"].as_str().unwrap()))
        .unwrap_or_else(|| panic!("{o}"));
    sketch(&mut s, "up", json!("zx"), line([5.0, 2.0], [15.0, 2.0]));
    add(
        &mut s,
        json!({"type": "sweep", "id": "w", "face": top, "path": "p/up/l1",
               "operation": "join"}),
    );
    close(volume(&body(&mut s, "b")), 1e-4 * 0.015, 1e-3);
    let r = reported(&mut s, "w");
    assert_eq!(
        r["sweep"]["profile"]["loops"][0]["edge"]
            .as_array()
            .unwrap()
            .len(),
        4,
        "{r}"
    );
}

#[test]
fn refusals_are_in_the_agents_terms() {
    let (_dir, mut s) = project();
    sketch(&mut s, "s", json!("xy"), circle(0.0, 0.0, 2.0));
    // A path in the profile's own plane.
    sketch(&mut s, "flat", json!("xy"), line([0.0, 0.0], [30.0, 0.0]));
    add(
        &mut s,
        json!({"type": "sweep", "id": "w", "sketch": "s", "path": "p/flat/l1"}),
    );
    let r = reported(&mut s, "w");
    assert_eq!(r["error"]["code"], "sweep_failed", "{r}");
    assert_eq!(r["error"]["details"]["reason"], "profile_along_path", "{r}");
    // A path that names a sketch, not its curve.
    let e = call(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "sweep", "sketch": "s", "path": "p/flat"}),
    )
    .unwrap_err();
    assert_eq!(e.code, Code::ReferenceWrongKind, "{e}");
    // No path.
    let e = call(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "sweep", "sketch": "s"}),
    )
    .unwrap_err();
    assert_eq!(
        (e.code, &e.details["param"]),
        (Code::InvalidParam, &json!("path"))
    );
    // A twist round a closed path that isn't whole turns.
    sketch(&mut s, "ring", json!("zx"), circle(20.0, 0.0, 15.0));
    let e = call(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "sweep", "sketch": "s", "path": "p/ring/ci",
               "twist": "90 deg"}),
    )
    .unwrap_err();
    assert_eq!(
        (e.code, &e.details["pointer"]),
        (Code::InvalidParam, &json!("/twist")),
        "{e}"
    );
}
