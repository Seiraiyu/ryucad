//! Trims and splits by tools, through the protocol (ryucad 18.C): a
//! sheet trimmed by a solid both ways, the areas summing to the sheet's;
//! a solid split by a curved sheet into two solids whose volumes sum to
//! the target's; a split by another body's faces; names kept across a
//! move of the tool sheet; a tool that doesn't divide refused.

mod common;

use common::{call, must, project};
use ryucad::doc::id::Id;
use ryucad::session::Session;
use serde_json::{Value, json};
use std::collections::BTreeSet;

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

/// The volume, in mm³, of solid `solid` as feature `f` leaves it.
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

/// A 20 x 20 mm sheet `w` in the plane z = 0 (a planar face of a square
/// sketch), and a box `b` 10 x 30 x 10 over its middle half in x.
fn sheet_and_box(s: &mut Session) {
    sketch(s, "s", json!("xy"), square(0.0, 0.0, 20.0));
    add(s, json!({"type": "planar", "id": "w", "sketch": "s"}));
    add(
        s,
        json!({"type": "box", "id": "b", "params": {"x": 10, "y": 30, "z": 10},
               "placement": {"origin": [-5, -15, -5]}}),
    );
}

/// An open arc of radius `r` mm from (15, y0) to (-15, y0) about
/// (0, 5 - r), bulging towards +y through (0, 5).
fn arc_across(r: f64) -> Value {
    let c = -r + 5.0;
    let y0 = c + ryukei::math::sqrt(r * r - 15.0 * 15.0);
    json!([
        {"op": "add", "id": "o", "type": "point", "at": [0, c]},
        {"op": "add", "id": "b", "type": "point", "at": [15, y0]},
        {"op": "add", "id": "e", "type": "point", "at": [-15, y0]},
        {"op": "add", "id": "a1", "type": "arc", "center": "o", "start": "b", "end": "e"}
    ])
}

/// The volume (mm³) of the part of a `w` x `d` x `h` box centred on the
/// origin in x and y lying beyond (in +y of) the arc of radius `r` about
/// (0, 5 - r): ∫ (d/2 - y(x)) dx over x in [-w/2, w/2], times `h`, where
/// the arc is y(x) = 5 - r + sqrt(r² - x²), which stays inside the box.
fn segment_volume(r: f64, w: f64, d: f64, h: f64) -> f64 {
    use ryukei::math::{asin, sqrt};
    let c = 5.0 - r;
    let half = w / 2.0;
    let f = |x: f64| x * sqrt(r * r - x * x) / 2.0 + r * r / 2.0 * asin(x / r);
    let under_arc = 2.0 * half * c + (f(half) - f(-half));
    (d / 2.0 * w - under_arc) * h
}

/// The text of `solid`'s face lying flat at `at` mm along `axis`.
fn face_at(s: &mut Session, solid: &str, axis: usize, at: f64) -> String {
    let b = body(s, solid);
    let f = b
        .faces()
        .find(|f| {
            let bx = ryukei::intersect::face_box(&b, *f);
            let c = |p: ryukei::geom::Point3| [p.x, p.y, p.z][axis];
            (c(bx.min()) - at * 1e-3).abs() < 1e-9 && (c(bx.max()) - at * 1e-3).abs() < 1e-9
        })
        .unwrap_or_else(|| panic!("{solid} has no face at {at} mm along {axis}"));
    format!("p/{solid}/{}", b.face(f).unwrap().name)
}

#[test]
fn a_sheet_trimmed_by_a_solid_both_ways_sums_to_the_sheet() {
    let mut areas = Vec::new();
    for flip in [false, true] {
        let (_dir, mut s) = project();
        sheet_and_box(&mut s);
        add(
            &mut s,
            json!({"type": "split", "id": "t", "target": "w", "tool": "b",
                   "flip": flip, "keep": "behind"}),
        );
        let r = reported(&mut s, "t");
        assert_eq!(r["ok"], true, "{r}");
        assert_eq!(r["split"]["trim"], true, "{r}");
        areas.push(area(&body(&mut s, "w")));
    }
    // Inside the box: 10 x 20; outside: 2 x 5 x 20.
    close(areas[0], 2e-4, 1e-9);
    close(areas[1], 2e-4, 1e-9);
    close(areas[0] + areas[1], 4e-4, 1e-9);
}

#[test]
fn a_solid_split_by_a_curved_sheet_sums_to_the_target() {
    let (_dir, mut s) = project();
    add(
        &mut s,
        json!({"type": "box", "id": "h", "params": {"x": 20, "y": 20, "z": 10},
               "placement": {"origin": [-10, -10, 0]}}),
    );
    // A parting surface: an open arc of radius 30 mm on the xy plane,
    // bulging towards +y through (0, 5), extruded through the box.
    sketch(&mut s, "a", json!("xy"), arc_across(30.0));
    add(
        &mut s,
        json!({"type": "extrude", "id": "parting", "sketch": "a", "surface": true,
               "extent": {"type": "symmetric", "length": "40 mm"}}),
    );
    add(
        &mut s,
        json!({"type": "split", "id": "sp", "target": "h", "tool": "parting"}),
    );
    let (behind, front) = (volume(&mut s, "sp", "h"), volume(&mut s, "sp", "sp"));
    close(behind + front, 4000.0, 1e-6);
    // The arc's segment of the box, beyond it: the smaller side.
    close(
        front.min(behind),
        segment_volume(30.0, 20.0, 20.0, 10.0),
        1e-4,
    );
    assert_eq!(reported(&mut s, "sp")["split"]["tool_kind"], "sheet");
}

#[test]
fn a_split_by_another_bodys_faces() {
    let (_dir, mut s) = project();
    add(
        &mut s,
        json!({"type": "box", "id": "h", "params": {"x": 10, "y": 10, "z": 10}}),
    );
    add(
        &mut s,
        json!({"type": "box", "id": "k", "params": {"x": 4, "y": 20, "z": 20},
               "placement": {"origin": [3, -5, -5]}}),
    );
    // k's face at x = 3, square to x, parts h at 3: 300 and 700 mm³.
    let face = face_at(&mut s, "k", 0, 3.0);
    add(
        &mut s,
        json!({"type": "split", "id": "sp", "target": "h", "tool_faces": [face]}),
    );
    let mut v = [volume(&mut s, "sp", "h"), volume(&mut s, "sp", "sp")];
    v.sort_by(f64::total_cmp);
    close(v[0], 300.0, 1e-9);
    close(v[1], 700.0, 1e-9);
    // k is left as it was.
    close(volume(&mut s, "k", "k"), 1600.0, 1e-9);
}

#[test]
fn moving_the_tool_sheet_renames_nothing() {
    let names_at = |x: f64| {
        let (_dir, mut s) = project();
        add(
            &mut s,
            json!({"type": "box", "id": "h", "params": {"x": 10, "y": 10, "z": 10}}),
        );
        sketch(&mut s, "a", json!("xy"), line([x, -5.0], [x, 15.0]));
        add(
            &mut s,
            json!({"type": "extrude", "id": "wl", "sketch": "a", "surface": true,
                   "extent": {"type": "symmetric", "length": "40 mm"}}),
        );
        add(
            &mut s,
            json!({"type": "split", "id": "sp", "target": "h", "tool": "wl"}),
        );
        (names(&mut s, "h"), names(&mut s, "sp"))
    };
    assert_eq!(names_at(4.0), names_at(6.0));
}

#[test]
fn a_tool_that_doesnt_divide_is_refused() {
    let (_dir, mut s) = project();
    add(
        &mut s,
        json!({"type": "box", "id": "h", "params": {"x": 10, "y": 10, "z": 10}}),
    );
    // A wall ending inside the box: it cuts, but doesn't divide.
    sketch(&mut s, "a", json!("xy"), line([5.0, -5.0], [5.0, 5.0]));
    add(
        &mut s,
        json!({"type": "extrude", "id": "wl", "sketch": "a", "surface": true,
               "extent": {"type": "symmetric", "length": "40 mm"}}),
    );
    add(
        &mut s,
        json!({"type": "split", "id": "sp", "target": "h", "tool": "wl"}),
    );
    let r = reported(&mut s, "sp");
    assert_eq!(r["error"]["code"], "boolean_failed", "{r}");
    assert_eq!(
        r["error"]["details"]["reason"], "tool_does_not_divide",
        "{r}"
    );
    // A tool that misses: a side is empty.
    let (_dir, mut s) = project();
    sheet_and_box(&mut s);
    add(
        &mut s,
        json!({"type": "box", "id": "far", "params": {"x": 1, "y": 1, "z": 1},
               "placement": {"origin": [100, 0, 0]}}),
    );
    add(
        &mut s,
        json!({"type": "split", "id": "sp", "target": "w", "tool": "far"}),
    );
    let r = reported(&mut s, "sp");
    assert_eq!(r["error"]["code"], "operation_no_effect", "{r}");
}

#[test]
fn a_sheet_tool_that_misses_is_refused_whatever_is_kept() {
    for keep in ["behind", "both"] {
        let (_dir, mut s) = project();
        add(
            &mut s,
            json!({"type": "box", "id": "h", "params": {"x": 10, "y": 10, "z": 10}}),
        );
        // A wall at x = 50 mm, far from the box: it divides nothing.
        sketch(&mut s, "a", json!("xy"), line([50.0, -5.0], [50.0, 15.0]));
        add(
            &mut s,
            json!({"type": "extrude", "id": "wl", "sketch": "a", "surface": true,
                   "extent": {"type": "symmetric", "length": "40 mm"}}),
        );
        add(
            &mut s,
            json!({"type": "split", "id": "sp", "target": "h", "tool": "wl", "keep": keep}),
        );
        let r = reported(&mut s, "sp");
        assert_eq!(r["error"]["code"], "operation_no_effect", "{keep}: {r}");
        assert_eq!(r["error"]["details"]["target"], "h", "{keep}: {r}");
    }
}
