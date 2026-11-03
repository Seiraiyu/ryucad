//! Lofts through the protocol, against closed forms: a frustum and a
//! prism between two squares (two sections skin ruled), a square to a
//! square pinned a quarter turn round by `loops` (the ruled twist keeps
//! two thirds of the prism, by Simpson's rule on its quadratic area), a
//! section facing back turned over to the same frustum, a planar face of
//! a box lofted to a sketch and joined, three circles on datum planes,
//! end tangents bulging the skin, a closed ring of six squares round an
//! axis (about Pappus's volume); names unchanged across a section's
//! resize and move; and the refusals in the agent's terms. The ring takes
//! minutes in a debug build: it runs in release (`full.yml`).

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

/// Sketch `id` on `plane` (an origin plane, or a datum plane's ID) with
/// `ops`.
fn sketch(s: &mut Session, id: &str, plane: &str, ops: Value) {
    let plane = if ["xy", "yz", "zx"].contains(&plane) {
        plane.to_owned()
    } else {
        format!("p/{plane}")
    };
    add(s, json!({"type": "sketch", "id": id, "plane": plane}));
    must(
        s,
        "sketch.edit",
        json!({"part": "p", "sketch": id, "ops": ops}),
    );
}

/// A datum plane `id` offset `mm` from xy, its normal turned when `flip`.
fn level(s: &mut Session, id: &str, mm: f64, flip: bool) {
    let mut f = json!({"type": "plane", "id": id, "by": "offset", "from": "xy",
                       "distance": format!("{mm} mm")});
    if flip {
        f["flip"] = json!(true);
    }
    add(s, f);
}

/// A circle of radius `r` mm about `(x, y)` in the sketch's plane.
fn circle(x: f64, y: f64, r: f64) -> Value {
    json!([
        {"op": "add", "id": "c", "type": "point", "at": [x, y]},
        {"op": "add", "id": "ci", "type": "circle", "center": "c", "r": r}
    ])
}

/// A square of side `a` mm centred on `(x, y)`, its corners `p1` to `p4`
/// counterclockwise from the bottom left.
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

/// A body's volume from a mesh within a micron.
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

/// A square of side 10 mm on xy and one of side `top` mm 10 mm above it.
fn two_squares(s: &mut Session, top: f64, flip: bool) {
    sketch(s, "s1", "xy", square(0.0, 0.0, 10.0));
    level(s, "d", 10.0, flip);
    sketch(s, "s2", "d", square(0.0, 0.0, top));
}

#[test]
fn a_frustum_and_a_prism_between_two_squares() {
    let (_dir, mut s) = project();
    two_squares(&mut s, 4.0, false);
    let v = add(
        &mut s,
        json!({"type": "loft", "id": "l", "sections": ["s1", "p/s2"]}),
    );
    assert_eq!(
        v["feature"]["sections"],
        json!([{"sketch": "s1"}, {"sketch": "s2"}]),
        "{v}"
    );
    // A square frustum: h / 3 (A1 + A2 + sqrt(A1 A2)).
    close(
        volume(&body(&mut s, "l")),
        0.01 / 3.0 * (1e-4 + 1.6e-5 + 4e-5),
        1e-3,
    );
    let r = reported(&mut s, "l");
    assert_eq!(r["ok"], true, "{r}");
    let l = &r["loft"];
    // The first starts at its anchor, the second at the corner nearest it.
    for k in 0..2 {
        let lp = &l["sections"][k]["loops"][0];
        assert_eq!(lp["start"], "p1", "{r}");
        assert_eq!(lp["entity"], json!(["l1", "l2", "l3", "l4"]), "{r}");
        assert_eq!(lp["segments"], 4, "{r}");
        assert_eq!(l["sections"][k]["turned"], false, "{r}");
    }
    assert_eq!(l["closed"], false, "{r}");
    // Every face's origin names the loft; a side its curve in each
    // section, caps their end.
    let o = must(&mut s, "inspect", json!({"part": "p", "feature": "l"}));
    assert_eq!(o["loft"], *l, "inspect gives the report's loft");
    let faces = o["bodies"][0]["faces"].as_array().unwrap();
    assert_eq!(faces.len(), 6, "{o}");
    let side = faces
        .iter()
        .find(|f| f["origin"]["sections"][0]["entity"] == "l2")
        .unwrap_or_else(|| panic!("{o}"));
    assert_eq!(
        side["origin"]["sections"],
        json!([{"sketch": "s1", "entity": "l2"}, {"sketch": "s2", "entity": "l2"}])
    );
    assert_eq!(side["origin"]["loop"], "p1");
    assert!(faces.iter().any(|f| f["origin"]["cap"] == "end"));
    for list in ["faces", "edges", "vertices"] {
        for e in o["bodies"][0][list].as_array().unwrap() {
            assert_eq!(e["origin"]["feature"], "l", "{list}: {e}");
        }
    }
    // A prism when the squares are alike.
    must(
        &mut s,
        "sketch.edit",
        json!({"part": "p", "sketch": "s2", "ops": [
            {"op": "move", "id": "p1", "at": [-5, -5]}, {"op": "move", "id": "p2", "at": [5, -5]},
            {"op": "move", "id": "p3", "at": [5, 5]}, {"op": "move", "id": "p4", "at": [-5, 5]}]}),
    );
    close(volume(&body(&mut s, "l")), 1e-6, 1e-3);
}

#[test]
fn loops_pin_where_each_section_starts() {
    let (_dir, mut s) = project();
    sketch(&mut s, "s1", "xy", square(0.0, 0.0, 10.0));
    level(&mut s, "d", 10.0, false);
    sketch(&mut s, "s2", "d", square(0.0, 0.0, 10.0));
    // Pinned a quarter turn round: corner p1 to corner p2. At mid height
    // the section is a square on the sides' middles, half the area, so
    // Simpson's rule gives h (4 + 4 × 2 + 4) / 6 of a quarter of the area.
    add(
        &mut s,
        json!({"type": "loft", "id": "l", "sections": [
            {"section": "s1", "loops": ["p1"]}, {"section": "s2", "loops": ["p2"]}]}),
    );
    close(volume(&body(&mut s, "l")), 1e-6 * 2.0 / 3.0, 1e-3);
    let r = reported(&mut s, "l");
    assert_eq!(r["loft"]["sections"][1]["loops"][0]["start"], "p2", "{r}");
    assert_eq!(
        r["loft"]["sections"][1]["loops"][0]["entity"],
        json!(["l2", "l3", "l4", "l1"]),
        "{r}"
    );
    // A curve starts its loop at its first point: l1 starts at p1, back
    // to the prism.
    must(
        &mut s,
        "feature.edit",
        json!({"part": "p", "id": "l", "sections": [
            {"section": "s1", "loops": ["p1"]}, {"section": "s2", "loops": ["l1"]}]}),
    );
    close(volume(&body(&mut s, "l")), 1e-6, 1e-3);
}

#[test]
fn a_section_facing_back_is_turned_over() {
    let (_dir, mut s) = project();
    two_squares(&mut s, 4.0, true);
    add(
        &mut s,
        json!({"type": "loft", "id": "l", "sections": ["s1", "s2"]}),
    );
    close(
        volume(&body(&mut s, "l")),
        0.01 / 3.0 * (1e-4 + 1.6e-5 + 4e-5),
        1e-3,
    );
    let r = reported(&mut s, "l");
    assert_eq!(r["loft"]["sections"][1]["turned"], true, "{r}");
    assert_eq!(r["loft"]["sections"][0]["turned"], false, "{r}");
}

#[test]
fn a_face_of_a_solid_lofted_to_a_sketch_and_joined() {
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
    level(&mut s, "d", 15.0, false);
    sketch(&mut s, "s", "d", square(0.0, 0.0, 4.0));
    add(
        &mut s,
        json!({"type": "loft", "id": "l", "sections": [top, "s"], "operation": "join"}),
    );
    // The box and a frustum of 10 mm on top of it (a ruled skin between
    // parallel squares holds the frustum's volume, off centre or not).
    close(
        volume(&body(&mut s, "b")),
        5e-7 + 0.01 / 3.0 * (1e-4 + 1.6e-5 + 4e-5),
        1e-3,
    );
    let r = reported(&mut s, "l");
    let first = &r["loft"]["sections"][0];
    assert_eq!(first["face"], top, "{r}");
    assert_eq!(
        first["loops"][0]["edge"].as_array().unwrap().len(),
        4,
        "{r}"
    );
}

#[test]
fn three_circles_on_datum_planes() {
    let (_dir, mut s) = project();
    sketch(&mut s, "s1", "xy", circle(0.0, 0.0, 2.0));
    level(&mut s, "d1", 10.0, false);
    sketch(&mut s, "s2", "d1", circle(0.0, 0.0, 3.0));
    level(&mut s, "d2", 20.0, false);
    sketch(&mut s, "s3", "d2", circle(0.0, 0.0, 2.0));
    add(
        &mut s,
        json!({"type": "loft", "id": "l", "sections": ["s1", "s2", "s3"]}),
    );
    let v = volume(&body(&mut s, "l"));
    // Swelling through the middle circle: more than the cylinder of the
    // ends, less than that of the middle.
    assert!(v > PI * 4e-6 * 0.02 && v < PI * 9e-6 * 0.02, "{v}");
    let r = reported(&mut s, "l");
    for k in 0..3 {
        assert_eq!(r["loft"]["sections"][k]["loops"][0]["start"], "ci", "{r}");
    }
}

#[test]
fn end_tangents_bulge_the_skin() {
    let (_dir, mut s) = project();
    two_squares(&mut s, 4.0, false);
    add(
        &mut s,
        json!({"type": "loft", "id": "l", "sections": ["s1", "s2"],
               "start_tangent": "normal", "end_tangent": "z"}),
    );
    let one = volume(&body(&mut s, "l"));
    let frustum = 0.01 / 3.0 * (1e-4 + 1.6e-5 + 4e-5);
    assert!(one > frustum && one < 1e-6, "{one}");
    let r = reported(&mut s, "l");
    let t = &r["loft"]["start_tangent"];
    assert_eq!(
        (&t["direction"], &t["weight"]),
        (&json!("normal"), &json!(1.0)),
        "{r}"
    );
    assert!(
        (t["length_m"].as_f64().unwrap() - 0.01).abs() < 1e-12,
        "{r}"
    );
    // A heavier pull bulges it further.
    must(
        &mut s,
        "feature.edit",
        json!({"part": "p", "id": "l", "start_weight": "2", "end_weight": "2"}),
    );
    let two = volume(&body(&mut s, "l"));
    assert!(two > one, "{two} against {one}");
    // Dropped, the frustum again.
    must(
        &mut s,
        "feature.edit",
        json!({"part": "p", "id": "l", "start_tangent": "", "end_tangent": ""}),
    );
    close(volume(&body(&mut s, "l")), frustum, 1e-3);
}

/// Six squares round the z axis, each on a plane through it a sixth of a
/// turn from the last, 30 mm out.
fn ring(s: &mut Session, n: usize) -> Vec<String> {
    (0..n)
        .map(|k| {
            let id = format!("d{k}");
            add(
                s,
                json!({"type": "plane", "id": id, "by": "angle", "from": "zx", "about": "z",
                       "angle": format!("{} deg", 360.0 * k as f64 / n as f64)}),
            );
            let sk = format!("s{k}");
            sketch(s, &sk, &id, square(0.0, 30.0, 4.0));
            sk
        })
        .collect()
}

#[test]
#[cfg_attr(debug_assertions, ignore = "minutes in a debug build")]
fn a_closed_loft_is_a_ring() {
    let (_dir, mut s) = project();
    let sections = ring(&mut s, 6);
    add(
        &mut s,
        json!({"type": "loft", "id": "l", "sections": sections, "closed": true}),
    );
    let b = body(&mut s, "l");
    assert_eq!(b.faces().count(), 4);
    // About a square ring's: the skin swings a little outside the circle.
    close(volume(&b), TAU * 0.03 * 1.6e-5, 0.03);
    assert_eq!(reported(&mut s, "l")["loft"]["closed"], true);
}

#[test]
fn names_survive_a_sections_resize_and_move() {
    let (_dir, mut s) = project();
    two_squares(&mut s, 4.0, false);
    add(
        &mut s,
        json!({"type": "loft", "id": "l", "sections": ["s1", "s2"]}),
    );
    let before = names(&mut s, "l");
    must(
        &mut s,
        "sketch.edit",
        json!({"part": "p", "sketch": "s2", "ops": [
            {"op": "move", "id": "p1", "at": [-3, -3]}, {"op": "move", "id": "p2", "at": [3, -3]},
            {"op": "move", "id": "p3", "at": [3, 3]}, {"op": "move", "id": "p4", "at": [-3, 3]}]}),
    );
    assert_eq!(names(&mut s, "l"), before, "resized");
    must(
        &mut s,
        "feature.edit",
        json!({"part": "p", "id": "d", "distance": "25 mm"}),
    );
    assert_eq!(names(&mut s, "l"), before, "moved");
    // What a section's loops name stays while the loft names it.
    must(
        &mut s,
        "feature.edit",
        json!({"part": "p", "id": "l", "sections": ["s1", {"section": "s2", "loops": ["p3"]}]}),
    );
    let e = call(
        &mut s,
        "sketch.edit",
        json!({"part": "p", "sketch": "s2", "ops": [{"op": "remove", "id": "p3"}]}),
    )
    .unwrap_err();
    assert_eq!(e.code, Code::HasDependents, "{e}");
}

fn refused(s: &mut Session, f: Value) -> ryucad::error::Error {
    let mut f = f;
    f["part"] = json!("p");
    call(s, "feature.add", f).unwrap_err()
}

#[test]
fn refusals_are_in_the_agents_terms() {
    let (_dir, mut s) = project();
    two_squares(&mut s, 4.0, false);
    level(&mut s, "d3", 20.0, false);
    sketch(&mut s, "round", "d3", circle(0.0, 0.0, 2.0));
    // A circle to a square: as many curves, or split them.
    let e = refused(
        &mut s,
        json!({"type": "loft", "sections": ["s1", "s2", "round"]}),
    );
    assert_eq!(e.code, Code::LoftFailed, "{e}");
    assert_eq!(e.details["reason"], "segment_count_mismatch");
    assert_eq!(e.details["section"]["index"], 2);
    assert_eq!(e.details["section"]["sketch"], "round");
    assert_eq!(e.details["entities"], json!(["ci"]));
    assert_eq!(e.details["counts"], json!({"first": 4, "this": 1}));
    assert!(e.message.contains("split"), "{e}");
    // One section: too few, as read.
    let e = refused(&mut s, json!({"type": "loft", "sections": ["s1"]}));
    assert_eq!(
        (e.code, e.details["pointer"].as_str()),
        (Code::InvalidParam, Some("/sections"))
    );
    // A section of two loops names them; a section naming one loop
    // against one naming two.
    level(&mut s, "d4", 30.0, false);
    let mut two = square(0.0, 0.0, 10.0).as_array().unwrap().clone();
    two.extend(circle(0.0, 0.0, 2.0).as_array().unwrap().iter().cloned());
    sketch(&mut s, "holed", "d4", json!(two));
    let e = refused(&mut s, json!({"type": "loft", "sections": ["s1", "holed"]}));
    assert_eq!(
        (e.code, e.details["pointer"].as_str()),
        (Code::InvalidParam, Some("/sections/1/loops")),
        "{e}"
    );
    let e = refused(
        &mut s,
        json!({"type": "loft", "sections": ["s1", {"section": "holed", "loops": ["p1", "ci"]}]}),
    );
    assert_eq!(
        (e.code, e.details["reason"].as_str()),
        (Code::LoftFailed, Some("loop_mismatch")),
        "{e}"
    );
    // A loop entry the sketch hasn't.
    let e = refused(
        &mut s,
        json!({"type": "loft", "sections": [{"section": "s1", "loops": ["p9"]}, "s2"]}),
    );
    assert_eq!(
        (e.code, e.details["pointer"].as_str()),
        (Code::NotFound, Some("/sections/0/loops/0")),
        "{e}"
    );
    // A section edge-on to the loft: its plane holds the line to the next.
    sketch(&mut s, "edge", "zx", square(10.0, 0.0, 4.0));
    let e = refused(&mut s, json!({"type": "loft", "sections": ["s1", "edge"]}));
    assert_eq!(
        (e.code, e.details["reason"].as_str()),
        (Code::LoftFailed, Some("bad_section")),
        "{e}"
    );
    assert_eq!(e.details["section"]["index"], 1);
    // A tangent in the first section's plane, at rebuild.
    add(
        &mut s,
        json!({"type": "loft", "id": "t", "sections": ["s1", "s2"], "start_tangent": "x"}),
    );
    let r = reported(&mut s, "t");
    assert_eq!(
        (&r["error"]["code"], &r["error"]["details"]["reason"]),
        (&json!("loft_failed"), &json!("bad_tangent")),
        "{r}"
    );
    // A closed loft with a tangent, as read.
    let e = refused(
        &mut s,
        json!({"type": "loft", "sections": ["s1", "s2", "round"], "closed": true,
               "start_tangent": "normal"}),
    );
    assert_eq!(
        (e.code, e.details["pointer"].as_str()),
        (Code::InvalidDocument, Some("/start_tangent"))
    );
}

#[test]
fn a_ring_of_three_turns_too_far_between_sections() {
    let (_dir, mut s) = project();
    let sections = ring(&mut s, 3);
    add(
        &mut s,
        json!({"type": "loft", "id": "l", "sections": sections, "closed": true}),
    );
    let r = reported(&mut s, "l");
    assert_eq!(
        (&r["error"]["code"], &r["error"]["details"]["reason"]),
        (&json!("loft_failed"), &json!("bad_section")),
        "{r}"
    );
    assert!(
        r["error"]["message"]
            .as_str()
            .unwrap()
            .contains("add sections between"),
        "{r}"
    );
}
