//! Fills and extracts through the protocol (ryucad 18.B): a flat pentagon
//! filled as one plane face to its area, a quad tangent to a quarter
//! cylinder sheet, refusals (a gap, a side with two faces asked tangent),
//! a box's top extracted to its area and names, the whole box extracted as
//! a closed sheet, faces of two bodies refused.

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

/// A regular pentagon of circumradius `r` mm about the origin, lines
/// `l1`..`l5` between points `p1`..`p5`.
fn pentagon(r: f64) -> Value {
    let mut ops = Vec::new();
    for k in 0..5 {
        let (s, c) = ryukei::math::sin_cos((72.0 * k as f64).to_radians());
        ops.push(json!({"op": "add", "id": format!("p{}", k + 1), "type": "point", "at": [r * c, r * s]}));
    }
    for k in 0..5 {
        ops.push(
            json!({"op": "add", "id": format!("l{}", k + 1), "type": "line",
                        "start": format!("p{}", k + 1), "end": format!("p{}", (k + 1) % 5 + 1)}),
        );
    }
    json!(ops)
}

#[test]
fn a_flat_pentagon_fills_one_plane_face_of_its_area() {
    let (_dir, mut s) = project();
    sketch(&mut s, "s", json!("xy"), pentagon(10.0));
    let sides: Vec<Value> = (1..=5)
        .map(|k| json!({"curve": format!("p/s/l{k}")}))
        .collect();
    add(&mut s, json!({"type": "fill", "id": "f", "sides": sides}));
    let b = body(&mut s, "f");
    assert_eq!(b.faces().count(), 1);
    let (s72, _) = ryukei::math::sin_cos(72f64.to_radians());
    close(area(&b), 2.5 * 1e-4 * s72, 1e-9);
    let r = reported(&mut s, "f");
    assert_eq!(r["fill"]["sides"].as_array().unwrap().len(), 5, "{r}");
}

#[test]
fn a_quad_tangent_to_a_quarter_cylinder_sheet_meets_it_within_its_angle() {
    let (_dir, mut s) = project();
    // A quarter cylinder of radius 10 mm, 20 mm tall: an arc swept as a surface.
    sketch(
        &mut s,
        "arc",
        json!("xy"),
        json!([
        {"op": "add", "id": "o", "type": "point", "at": [0, 0]},
        {"op": "add", "id": "a", "type": "point", "at": [10, 0]},
        {"op": "add", "id": "b", "type": "point", "at": [0, 10]},
        {"op": "add", "id": "a1", "type": "arc", "center": "o", "start": "a", "end": "b"}]),
    );
    add(
        &mut s,
        json!({"type": "extrude", "id": "w", "sketch": "arc", "surface": true,
                       "extent": {"type": "blind", "length": "20 mm"}}),
    );
    // Its straight laminar edge at (0, 10, z): the one whose points all have x = 0.
    let w = body(&mut s, "w");
    let edge = w
        .edges()
        .filter(|e| w.is_laminar(*e))
        .find(|e| {
            let props = ryukei::measure::properties(&w, ryukei::measure::Entity::Edge(*e)).unwrap();
            matches!(props, ryukei::measure::Properties::Edge { start, end, .. }
                     if start.x.abs() < 1e-12 && end.x.abs() < 1e-12)
        })
        .unwrap();
    let name = w.name_of(ryukei::topo::EntityId::Edge(edge)).unwrap();
    // Three lines in the plane y = 10 mm closing the quad on -x, on a datum plane.
    add(
        &mut s,
        json!({"type": "plane", "id": "y10", "by": "offset", "from": "zx", "distance": "10 mm"}),
    );
    // In the zx plane's frame x runs along z and y along x (sketch.get gives
    // model-space positions: check them if the quad doesn't close).
    sketch(
        &mut s,
        "q",
        json!({"plane": "y10"}),
        json!([
        {"op": "add", "id": "p1", "type": "point", "at": [20, 0]},
        {"op": "add", "id": "p2", "type": "point", "at": [20, -10]},
        {"op": "add", "id": "p3", "type": "point", "at": [0, -10]},
        {"op": "add", "id": "p4", "type": "point", "at": [0, 0]},
        {"op": "add", "id": "l1", "type": "line", "start": "p1", "end": "p2"},
        {"op": "add", "id": "l2", "type": "line", "start": "p2", "end": "p3"},
        {"op": "add", "id": "l3", "type": "line", "start": "p3", "end": "p4"}]),
    );
    add(
        &mut s,
        json!({"type": "fill", "id": "f", "sides": [
        {"edge": format!("p/w/{name}"), "tangent": true},
        {"curve": "p/q/l1"}, {"curve": "p/q/l2"}, {"curve": "p/q/l3"}]}),
    );
    close(area(&body(&mut s, "f")), 2e-4, 1e-9);
    let r = reported(&mut s, "f");
    let angle = r["fill"]["sides"][0]["angle_deg"].as_f64().unwrap();
    assert!(angle <= 1.0, "{r}");
}

#[test]
fn a_gap_and_a_tangent_edge_with_two_faces_are_refused() {
    let (_dir, mut s) = project();
    add(
        &mut s,
        json!({"type": "box", "id": "b", "params": {"x": 10, "y": 10, "z": 10}}),
    );
    // A box edge along x from the origin, closed by three lines below it in xy.
    let b = body(&mut s, "b");
    let edge = b
        .edges()
        .find(|e| {
            let p = ryukei::measure::properties(&b, ryukei::measure::Entity::Edge(*e)).unwrap();
            matches!(p, ryukei::measure::Properties::Edge { start, end, .. }
                     if start.y.abs() < 1e-12 && end.y.abs() < 1e-12
                        && start.z.abs() < 1e-12 && end.z.abs() < 1e-12)
        })
        .unwrap();
    let name = b.name_of(ryukei::topo::EntityId::Edge(edge)).unwrap();
    sketch(
        &mut s,
        "s",
        json!("xy"),
        json!([
        {"op": "add", "id": "p1", "type": "point", "at": [10, 0]},
        {"op": "add", "id": "p2", "type": "point", "at": [10, -10]},
        {"op": "add", "id": "p3", "type": "point", "at": [0, -10]},
        {"op": "add", "id": "p4", "type": "point", "at": [0, 0]},
        {"op": "add", "id": "l1", "type": "line", "start": "p1", "end": "p2"},
        {"op": "add", "id": "l2", "type": "line", "start": "p2", "end": "p3"},
        {"op": "add", "id": "l3", "type": "line", "start": "p3", "end": "p4"}]),
    );
    let sides = |tangent: bool| {
        json!([{"edge": format!("p/b/{name}"), "tangent": tangent},
                                       {"curve": "p/s/l1"}, {"curve": "p/s/l2"}, {"curve": "p/s/l3"}])
    };
    add(
        &mut s,
        json!({"type": "fill", "id": "f", "sides": sides(true)}),
    );
    let r = reported(&mut s, "f");
    assert_eq!(
        (
            r["error"]["code"].as_str(),
            r["error"]["details"]["reason"].as_str()
        ),
        (Some("fill_failed"), Some("not_laminar")),
        "{r}"
    );
    assert_eq!(r["error"]["details"]["side"], 0);
    // Not tangent, it fills; with the last line moved 2 mm off, it gaps.
    must(
        &mut s,
        "feature.edit",
        json!({"part": "p", "id": "f", "sides": sides(false)}),
    );
    assert_eq!(reported(&mut s, "f")["ok"], true);
    must(
        &mut s,
        "sketch.edit",
        json!({"part": "p", "sketch": "s",
        "ops": [{"op": "move", "id": "p4", "at": [0, -2]}]}),
    );
    let r = reported(&mut s, "f");
    assert_eq!(r["error"]["details"]["reason"], "not_closed", "{r}");
    assert!(r["error"]["details"]["gap_m"].as_f64().unwrap() > 1e-3);
}

#[test]
fn a_boxs_top_extracted_keeps_its_name_and_area() {
    let (_dir, mut s) = project();
    add(
        &mut s,
        json!({"type": "box", "id": "b", "params": {"x": 10, "y": 20, "z": 5}}),
    );
    let b = body(&mut s, "b");
    let top = b
        .faces()
        .find(|f| {
            matches!(
                ryukei::measure::properties(&b, ryukei::measure::Entity::Face(*f)).unwrap(),
                ryukei::measure::Properties::Face {
                    shape: ryukei::measure::FaceShape::Plane { normal, .. }, ..
                } if normal.get().z > 0.5
            )
        })
        .unwrap();
    let name = b.name_of(ryukei::topo::EntityId::Face(top)).unwrap();
    add(
        &mut s,
        json!({"type": "extract", "id": "x", "faces": [format!("p/b/{name}")]}),
    );
    let x = body(&mut s, "x");
    close(area(&x), 2e-4, 1e-12);
    assert!(x.by_name(name).is_some());
    assert_eq!(ryucad::kind::boundaries(&x), 1);
    let inspected = must(&mut s, "inspect", json!({"part": "p", "feature": "x"}));
    let face = inspected["bodies"][0]["faces"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["name"] == name.to_string())
        .unwrap();
    assert_eq!(
        face["origin"]["extract_of"],
        name.to_string(),
        "{inspected}"
    );
    // Every face: a closed sheet.
    let all: Vec<String> = b
        .faces()
        .map(|f| {
            format!(
                "p/b/{}",
                b.name_of(ryukei::topo::EntityId::Face(f)).unwrap()
            )
        })
        .collect();
    add(
        &mut s,
        json!({"type": "extract", "id": "all", "faces": all}),
    );
    let all = body(&mut s, "all");
    assert_eq!(
        (ryucad::kind::name(&all), ryucad::kind::boundaries(&all)),
        ("sheet", 0)
    );
}
