//! Sheets through other features (ryucad 18.C, design 388's table):
//! cuts, holes and patterns of them trim a sheet; a boolean subtract
//! trims a sheet target; a fillet rounds a sheet's inner edge and
//! refuses its boundary; an offset of a sheet is a sheet; a mirror of a
//! sheet is a new sheet; a split takes a sheet target by a plane.

mod common;

use common::{call, must, project};
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

/// A wall `w`: an open 10 mm line extruded 20 mm as a sheet.
fn wall(s: &mut Session) {
    sketch(s, "s", json!("xy"), line([0.0, 0.0], [10.0, 0.0]));
    add(
        s,
        json!({"type": "extrude", "id": "w", "sketch": "s", "surface": true,
               "extent": {"type": "blind", "length": "20 mm"}}),
    );
}

/// A sketch of one free point `p1` at `p` mm (a hole drills there).
fn point_at(p: [f64; 2]) -> Value {
    json!([{"op": "add", "id": "p1", "type": "point", "at": p}])
}

/// Two lines (0, 0)-(10, 0)-(10, 10), `l1` and `l2`, sharing point `b`.
fn ell() -> Value {
    json!([
        {"op": "add", "id": "a", "type": "point", "at": [0, 0]},
        {"op": "add", "id": "b", "type": "point", "at": [10, 0]},
        {"op": "add", "id": "c", "type": "point", "at": [10, 10]},
        {"op": "add", "id": "l1", "type": "line", "start": "a", "end": "b"},
        {"op": "add", "id": "l2", "type": "line", "start": "b", "end": "c"}
    ])
}

/// Sheet `w`'s one inner edge (two faces) and one of its boundary
/// (laminar) edges, as references.
fn edges_of(s: &mut Session, w: &str) -> (String, String) {
    let b = body(s, w);
    let text = |e| format!("p/{w}/{}", b.edge(e).unwrap().name);
    let inner = b
        .edges()
        .find(|&e| !b.is_laminar(e))
        .expect("an inner edge");
    let boundary = b.edges().find(|&e| b.is_laminar(e)).expect("a boundary");
    (text(inner), text(boundary))
}

#[test]
fn a_hole_and_a_pattern_of_cuts_drill_a_sheet() {
    let (_dir, mut s) = project();
    // A 20 x 20 sheet on z = 0, drilled by a 2 mm hole at its middle
    // and a row of three 1 mm cuts.
    sketch(&mut s, "s", json!("xy"), square(0.0, 0.0, 20.0));
    add(&mut s, json!({"type": "planar", "id": "w", "sketch": "s"}));
    // The hole's sketch 1 mm above the sheet: a hole drills against its
    // sketch's normal, and a sheet lying in that plane is behind nothing.
    add(
        &mut s,
        json!({"type": "plane", "id": "up", "by": "offset", "from": "xy", "distance": "1 mm"}),
    );
    sketch(&mut s, "h", json!("up"), point_at([0.0, 0.0]));
    add(
        &mut s,
        json!({"type": "hole", "id": "d", "sketch": "h", "diameter": "2 mm",
               "depth": "through_all", "targets": ["w"]}),
    );
    let before = area(&body(&mut s, "w"));
    close(before, 4e-4 - std::f64::consts::PI * 1e-6, 1e-6);
    add(
        &mut s,
        json!({"type": "box", "id": "c", "params": {"x": 1, "y": 1, "z": 4},
               "placement": {"origin": [5, 5, -2]}, "operation": "cut", "targets": ["w"]}),
    );
    add(
        &mut s,
        json!({"type": "pattern", "id": "pat", "features": ["c"], "form": "linear",
               "direction": "x", "spacing": "2 mm", "count": 3}),
    );
    close(area(&body(&mut s, "w")), before - 3e-6, 1e-6);
}

#[test]
fn a_subtract_trims_a_sheet() {
    let (_dir, mut s) = project();
    wall(&mut s);
    add(
        &mut s,
        json!({"type": "box", "id": "k", "params": {"x": 20, "y": 4, "z": 10},
               "placement": {"origin": [-5, -2, 10]}}),
    );
    add(
        &mut s,
        json!({"type": "boolean", "id": "bo", "op": "subtract", "target": "w", "tools": ["k"]}),
    );
    close(area(&body(&mut s, "w")), 1e-4, 1e-9);
}

#[test]
fn a_sheets_inner_edge_is_rounded_and_its_boundary_refused() {
    let (_dir, mut s) = project();
    // An L of two walls: a chain of two lines extruded, its inner edge
    // between them.
    sketch(&mut s, "s", json!("xy"), ell());
    add(
        &mut s,
        json!({"type": "extrude", "id": "w", "sketch": "s", "surface": true,
               "extent": {"type": "blind", "length": "10 mm"}}),
    );
    let (inner, boundary) = edges_of(&mut s, "w");
    add(
        &mut s,
        json!({"type": "fillet", "id": "f", "radius": "2 mm", "edges": [inner]}),
    );
    let b = body(&mut s, "w");
    assert_eq!(b.faces().count(), 3, "two walls and the round");
    let e = call(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "fillet", "id": "g", "radius": "1 mm", "edges": [boundary]}),
    )
    .unwrap_err();
    assert_eq!(
        (e.code, e.details["reason"].as_str()),
        (Code::BlendFailed, Some("unsupported")),
        "{e}"
    );
    assert!(
        e.details["hint"].as_str().unwrap().contains("laminar"),
        "{e}"
    );
}

#[test]
fn an_offset_of_a_sheet_is_a_sheet_and_a_mirror_a_new_one() {
    let (_dir, mut s) = project();
    wall(&mut s);
    add(
        &mut s,
        json!({"type": "offset", "id": "o", "target": "w", "distance": "2 mm"}),
    );
    close(area(&body(&mut s, "w")), 2e-4, 1e-9);
    add(
        &mut s,
        json!({"type": "mirror", "id": "m", "solid": "w", "plane": "yz"}),
    );
    close(area(&body(&mut s, "m")), 2e-4, 1e-9);
    let m = must(&mut s, "feature.show", json!({"part": "p", "id": "m"}));
    assert_eq!(m["operation"], "new", "{m}");
}

#[test]
fn a_sheet_is_split_by_a_plane() {
    let (_dir, mut s) = project();
    wall(&mut s);
    add(
        &mut s,
        json!({"type": "plane", "id": "p4", "by": "offset", "from": "yz",
               "distance": "4 mm"}),
    );
    add(
        &mut s,
        json!({"type": "split", "id": "sp", "target": "w", "plane": "p4"}),
    );
    // The wall runs x 0..10: behind 4 mm of it, in front 6 mm.
    let (b, f) = (area(&body(&mut s, "w")), area(&body(&mut s, "sp")));
    close(b, 0.8e-4, 1e-9);
    close(f, 1.2e-4, 1e-9);
    let sides = reported(&mut s, "sp")["split"]["sides"].clone();
    assert_eq!(sides[0]["kind"], "sheet", "{sides}");
    assert_eq!(sides[0]["lumps"], 1, "{sides}");
}
