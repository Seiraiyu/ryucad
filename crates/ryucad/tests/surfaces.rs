//! Sheets and surface sweeps through the protocol (ryucad 18.A), against
//! closed forms: an extruded arc (a curved wall), a revolved quarter
//! circle (a dome), an open line swept along a bend and lofted to another,
//! a square's planar face, a box's face swept as a surface; names across
//! edits; and what refuses a sheet in 18.A (every feature that acts on a
//! body, mass, exports) and what takes one (measure, check, interference
//! leaving it out, reports and inspect saying what it is).

use core::f64::consts::{FRAC_PI_2, PI};
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

/// A sheet's area as ryukei measures it (certified), checked clean and
/// meshed without faults first.
fn area(b: &ryukei::topo::Body) -> f64 {
    use ryukei::measure::{Entity, Properties};
    assert_eq!(ryukei::check::check(b), []);
    assert_eq!(b.kind(), ryukei::topo::BodyKind::Sheet);
    assert_eq!(ryukei::check::check_intersections(b), []);
    let m = ryukei::mesh::tessellate(b, &ryukei::mesh::MeshParams::new(1e-5, 0.2)).unwrap();
    assert!(m.faults.is_empty(), "{:?}", m.faults);
    match ryukei::measure::properties(b, Entity::Body).unwrap() {
        Properties::Sheet { area } => area.value(),
        p => panic!("{p:?}"),
    }
}

/// An arc of radius `r` mm about the origin from angle 0 to `deg`, `a1`.
fn arc(r: f64, deg: f64) -> Value {
    let (s, c) = ryukei::math::sin_cos(deg.to_radians());
    json!([
        {"op": "add", "id": "o", "type": "point", "at": [0, 0]},
        {"op": "add", "id": "p", "type": "point", "at": [r, 0]},
        {"op": "add", "id": "q", "type": "point", "at": [r * c, r * s]},
        {"op": "add", "id": "a1", "type": "arc", "center": "o", "start": "p", "end": "q"}
    ])
}

/// A wall `w`: an open 10 mm line extruded 20 mm as a sheet.
fn wall(s: &mut Session) {
    sketch(s, "s", json!("xy"), line([0.0, 0.0], [10.0, 0.0]));
    add(
        s,
        json!({"type": "extrude", "id": "w", "sketch": "s", "surface": true,
               "extent": {"type": "blind", "length": "20 mm"}}),
    );
}

#[test]
fn an_extruded_arc_is_a_curved_wall() {
    let (_dir, mut s) = project();
    sketch(&mut s, "s", json!("xy"), arc(10.0, 90.0));
    add(
        &mut s,
        json!({"type": "extrude", "id": "w", "sketch": "s", "surface": true,
               "extent": {"type": "blind", "length": "20 mm"}}),
    );
    // A quarter of a 10 mm circle, 20 mm high.
    close(area(&body(&mut s, "w")), FRAC_PI_2 * 0.01 * 0.02, 1e-6);
    let r = reported(&mut s, "w");
    assert_eq!(r["ok"], true, "{r}");
    let shown = &must(&mut s, "rebuild", json!({"part": "p"}))["parts"][0]["solids"][0];
    assert_eq!(
        (
            shown["kind"].as_str(),
            shown["boundaries"].as_u64(),
            shown["closed"].as_bool()
        ),
        (Some("sheet"), Some(1), Some(false)),
        "{shown}"
    );
    // Every face, edge and vertex has an origin naming the extrude.
    let o = must(&mut s, "inspect", json!({"part": "p", "feature": "w"}));
    for list in ["faces", "edges", "vertices"] {
        for e in o["bodies"][0][list].as_array().unwrap() {
            assert_eq!(e["origin"]["feature"], "w", "{list}: {e}");
        }
    }
}

#[test]
fn names_dont_move_with_a_height_or_a_chain_extended_at_its_far_end() {
    let (_dir, mut s) = project();
    sketch(&mut s, "s", json!("xy"), arc(10.0, 90.0));
    add(
        &mut s,
        json!({"type": "extrude", "id": "w", "sketch": "s", "surface": true,
               "extent": {"type": "blind", "length": "20 mm"}}),
    );
    let before = names(&mut s, "w");
    must(
        &mut s,
        "feature.edit",
        json!({"part": "p", "id": "w", "extent": {"length": "35 mm"}}),
    );
    assert_eq!(names(&mut s, "w"), before);
    // A line on from the arc's far end: only the old end's lateral and
    // its two vertices rename.
    must(
        &mut s,
        "sketch.edit",
        json!({"part": "p", "sketch": "s", "ops": [
            {"op": "add", "id": "z", "type": "point", "at": [-10, 10]},
            {"op": "add", "id": "l9", "type": "line", "start": "q", "end": "z"}]}),
    );
    let after = names(&mut s, "w");
    let gone: Vec<_> = before.difference(&after).collect();
    assert_eq!(
        gone.len(),
        3,
        "the far lateral and its two vertices: {gone:?}"
    );
}

#[test]
fn a_revolved_chain_is_a_dome() {
    let (_dir, mut s) = project();
    // A quarter circle in zx from the axis out (sketch x is z), revolved
    // about z: a hemisphere of 10 mm.
    sketch(
        &mut s,
        "s",
        json!("zx"),
        json!([
            {"op": "add", "id": "o", "type": "point", "at": [0, 0]},
            {"op": "add", "id": "p", "type": "point", "at": [0, 10]},
            {"op": "add", "id": "q", "type": "point", "at": [10, 0]},
            {"op": "add", "id": "a1", "type": "arc", "center": "o", "start": "q", "end": "p"},
            {"op": "add", "id": "ax", "type": "line", "start": "o", "end": "q", "construction": true}
        ]),
    );
    add(
        &mut s,
        json!({"type": "revolve", "id": "d", "sketch": "s", "axis": "ax",
               "angle": "full", "surface": true}),
    );
    close(area(&body(&mut s, "d")), 2.0 * PI * 1e-4, 1e-6);
}

#[test]
fn a_chain_swept_along_a_bend() {
    let (_dir, mut s) = project();
    // An open 4 mm line across the path's start, and a bend: 10 mm up z,
    // then a quarter of a 15 mm radius towards +x.
    sketch(&mut s, "s", json!("xy"), line([-2.0, 0.0], [2.0, 0.0]));
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
    add(
        &mut s,
        json!({"type": "sweep", "id": "w", "sketch": "s", "path": "p/pb", "surface": true}),
    );
    // Pappus for a band: its width times the length its middle travels.
    let length = 0.01 + 0.015 * FRAC_PI_2;
    close(area(&body(&mut s, "w")), 0.004 * length, 1e-5);
    let o = must(&mut s, "inspect", json!({"part": "p", "feature": "w"}));
    for list in ["faces", "edges", "vertices"] {
        for e in o["bodies"][0][list].as_array().unwrap() {
            assert_eq!(e["origin"]["feature"], "w", "{list}: {e}");
        }
    }
}

#[test]
fn two_chains_loft_into_a_ruled_skin() {
    let (_dir, mut s) = project();
    sketch(&mut s, "a", json!("xy"), line([0.0, 0.0], [10.0, 0.0]));
    add(
        &mut s,
        json!({"type": "plane", "id": "d", "by": "offset", "from": "xy", "distance": "10 mm"}),
    );
    // Drawn the other way round: the loft runs it from the end nearer
    // where the first starts, so the skin is the flat 10 × 10 mm square.
    sketch(&mut s, "b", json!("d"), line([10.0, 0.0], [0.0, 0.0]));
    add(
        &mut s,
        json!({"type": "loft", "id": "l", "sections": ["a", "b"], "surface": true}),
    );
    close(area(&body(&mut s, "l")), 1e-4, 1e-6);
    let r = reported(&mut s, "l");
    assert_eq!(
        r["loft"]["sections"][1]["chains"][0]["reversed"], true,
        "{r}"
    );
    let o = must(&mut s, "inspect", json!({"part": "p", "feature": "l"}));
    for list in ["faces", "edges", "vertices"] {
        for e in o["bodies"][0][list].as_array().unwrap() {
            assert_eq!(e["origin"]["feature"], "l", "{list}: {e}");
        }
    }
    // Sections of different numbers of chains don't match.
    sketch(
        &mut s,
        "c",
        json!("d"),
        json!([
            {"op": "add", "id": "a", "type": "point", "at": [0, 0]},
            {"op": "add", "id": "b", "type": "point", "at": [5, 0]},
            {"op": "add", "id": "e", "type": "point", "at": [6, 0]},
            {"op": "add", "id": "f", "type": "point", "at": [9, 0]},
            {"op": "add", "id": "l1", "type": "line", "start": "a", "end": "b"},
            {"op": "add", "id": "l2", "type": "line", "start": "e", "end": "f"}
        ]),
    );
    let e = call(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "loft",
               "sections": ["a", {"section": "c", "loops": ["l1", "l2"]}], "surface": true}),
    )
    .unwrap_err();
    assert_eq!(
        (e.code, e.details["reason"].as_str()),
        (Code::LoftFailed, Some("loop_mismatch")),
        "{e}"
    );
}

#[test]
fn a_planar_face_has_its_loops_area_and_a_chain_is_refused() {
    let (_dir, mut s) = project();
    sketch(&mut s, "s", json!("xy"), square(0.0, 0.0, 10.0));
    add(&mut s, json!({"type": "planar", "id": "f", "sketch": "s"}));
    close(area(&body(&mut s, "f")), 1e-4, 1e-9);
    sketch(&mut s, "t", json!("xy"), line([0.0, 0.0], [5.0, 0.0]));
    let e = call(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "planar", "sketch": "t"}),
    )
    .unwrap_err();
    assert_eq!(
        (e.code, e.details["reason"].as_str()),
        (Code::ProfileInvalid, Some("open_profile")),
        "{e}"
    );
}

#[test]
fn a_face_swept_as_a_surface_has_no_caps() {
    let (_dir, mut s) = project();
    add(
        &mut s,
        json!({"type": "box", "id": "b", "params": {"x": 10, "y": 10, "z": 10}}),
    );
    sketch(&mut s, "up", json!("zx"), line([10.0, 0.0], [20.0, 0.0]));
    // The box's top face: its plane's normal along z, 10 mm up.
    let top = {
        let b = body(&mut s, "b");
        let f = b
            .faces()
            .find(|&f| {
                let face = b.face(f).unwrap();
                matches!(b.surface(face.surface), Some(ryukei::geom::Surface::Plane(p))
                    if p.frame().z().get().z.abs() > 0.5 && p.frame().origin().z > 0.005)
            })
            .unwrap();
        b.face(f).unwrap().name.to_string()
    };
    add(
        &mut s,
        json!({"type": "sweep", "id": "w", "face": format!("p/b/{top}"),
               "path": "p/up/l1", "surface": true}),
    );
    let w = body(&mut s, "w");
    assert_eq!(w.faces().count(), 4);
    close(area(&w), 4.0 * 0.01 * 0.01, 1e-6);
}

#[test]
fn features_that_need_a_solid_refuse_a_sheet() {
    // A surface wall `w` and a box `b`; each feature that needs a solid
    // is refused naming `w`, at write, and nothing is written.
    for (f, hint) in [
        (
            json!({"type": "box", "params": {"x": 1, "y": 1, "z": 1}, "operation": "join", "targets": ["w"]}),
            "sew",
        ),
        (
            json!({"type": "boolean", "op": "unite", "target": "w", "tools": ["b"]}),
            "sew",
        ),
        (
            json!({"type": "boolean", "op": "subtract", "target": "b", "tools": ["w"]}),
            "split",
        ),
        (
            json!({"type": "shell", "target": "w", "thickness": "1 mm"}),
            "thicken",
        ),
        (
            json!({"type": "mirror", "solid": "w", "plane": "yz", "operation": "join", "targets": ["b"]}),
            "new",
        ),
    ] {
        let (_dir, mut s) = project();
        wall(&mut s);
        add(
            &mut s,
            json!({"type": "box", "id": "b", "params": {"x": 5, "y": 5, "z": 5}}),
        );
        let before = must(&mut s, "feature.list", json!({"part": "p"}));
        let mut f = f.clone();
        f["part"] = json!("p");
        let e = call(&mut s, "feature.add", f.clone()).unwrap_err();
        assert_eq!(
            (e.code, e.details["solid"].as_str()),
            (Code::WrongBodyKind, Some("w")),
            "{f}: {e}"
        );
        assert_eq!(e.details["got"], "sheet");
        assert!(e.details["hint"].as_str().unwrap().contains(hint), "{e}");
        assert_eq!(
            must(&mut s, "feature.list", json!({"part": "p"})),
            before,
            "{f}: nothing written"
        );
    }
}

#[test]
fn a_cut_trims_a_sheet_at_write_and_at_rebuild() {
    // The wall (x 0..10, z 0..20 on y = 0) less a box over its top half.
    let (_dir, mut s) = project();
    wall(&mut s);
    add(
        &mut s,
        json!({"type": "box", "id": "c", "params": {"x": 20, "y": 4, "z": 20},
               "placement": {"origin": [-5, -2, 10]}, "operation": "cut", "targets": ["w"]}),
    );
    let r = reported(&mut s, "c");
    assert_eq!(r["ok"], true, "{r}");
    close(area(&body(&mut s, "w")), 1e-4, 1e-9);
}

#[test]
fn a_sheet_written_by_hand_where_a_solid_goes_fails_at_rebuild() {
    // The write is refused, so a join to a sheet reaches rebuild only from
    // a file written by hand (a merge, an older build): it fails there,
    // as wrong_body_kind.
    let (dir, mut s) = project();
    wall(&mut s);
    add(
        &mut s,
        json!({"type": "box", "id": "k", "params": {"x": 1, "y": 1, "z": 1}}),
    );
    let feature = dir.path().join("parts/p/features/k.json");
    let mut v: Value = serde_json::from_str(&std::fs::read_to_string(&feature).unwrap()).unwrap();
    v["operation"] = json!("join");
    v["targets"] = json!(["w"]);
    v["format"] = json!(3);
    std::fs::write(&feature, ryucad::doc::canonical::to_string(&v)).unwrap();
    let r = reported(&mut s, "k");
    assert_eq!(r["error"]["code"], "wrong_body_kind", "{r}");
}

#[test]
fn a_sheet_is_measured_by_area_and_left_out_of_mass() {
    let (_dir, mut s) = project();
    wall(&mut s);
    add(
        &mut s,
        json!({"type": "box", "id": "b", "params": {"x": 5, "y": 5, "z": 5}}),
    );
    let m = must(&mut s, "measure", json!({"entities": ["p/w"]}));
    close(m["area"]["m2"].as_f64().unwrap(), 0.01 * 0.02, 1e-9);
    assert!(m.get("volume").is_none(), "{m}");
    let w = must(&mut s, "mass", json!({"of": "p"}));
    close(w["volume"]["m3"].as_f64().unwrap(), 1.25e-7, 1e-6);
    let warned: Vec<&Value> = w["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|x| x["code"] == "mass_sheet_skipped")
        .collect();
    assert_eq!(warned.len(), 1, "{w}");
    assert_eq!(warned[0]["solid"], "w");
    let e = call(&mut s, "mass", json!({"of": "p/w"})).unwrap_err();
    assert_eq!(e.code, Code::WrongBodyKind, "{e}");
    let c = must(&mut s, "check", json!({"part": "p"}));
    assert!(
        c["findings"]
            .as_array()
            .is_none_or(|f| f.iter().all(|x| x["severity"] != "error")),
        "{c}"
    );
}

#[test]
fn an_export_of_a_sheet_is_refused_until_formats_write_it() {
    let (dir, mut s) = project();
    wall(&mut s);
    let out = dir.path().join("w.stl");
    let e = call(
        &mut s,
        "export",
        json!({"part": "p", "out": out.to_str().unwrap()}),
    )
    .unwrap_err();
    assert_eq!(e.code, Code::WrongBodyKind, "{e}");
    assert_eq!(e.details["solid"], "w");
    assert!(!out.exists());
}

#[test]
fn interference_leaves_a_sheet_out() {
    let (_dir, mut s) = project();
    // A wall through the middle of a box, placed by an assembly.
    sketch(&mut s, "s", json!("xy"), line([-5.0, 5.0], [15.0, 5.0]));
    add(
        &mut s,
        json!({"type": "extrude", "id": "w", "sketch": "s", "surface": true,
               "extent": {"type": "symmetric", "length": "30 mm"}}),
    );
    must(&mut s, "part.create", json!({"part": "q"}));
    add(
        &mut s,
        json!({"part": "q", "type": "box", "id": "b", "params": {"x": 10, "y": 10, "z": 10}}),
    );
    must(
        &mut s,
        "part.create",
        json!({"part": "a", "kind": "assembly"}),
    );
    add(
        &mut s,
        json!({"part": "a", "type": "instance", "id": "i1", "of": "p", "fixed": true}),
    );
    add(
        &mut s,
        json!({"part": "a", "type": "instance", "id": "i2", "of": "q", "fixed": true}),
    );
    let v = must(&mut s, "interference", json!({"part": "a"}));
    assert_eq!(v["pairs"], 0, "{v}");
    assert!(v["interferences"].as_array().unwrap().is_empty(), "{v}");
}

#[test]
fn reports_and_inspect_name_a_sheets_laminar_edges() {
    let (_dir, mut s) = project();
    wall(&mut s);
    let i = must(&mut s, "inspect", json!({"part": "p"}));
    let body = &i["bodies"][0];
    assert_eq!(body["kind"], "sheet", "{i}");
    let edges = body["edges"].as_array().unwrap();
    assert_eq!(edges.len(), 4, "{i}");
    assert!(edges.iter().all(|e| e["laminar"] == true), "{i}");
}
