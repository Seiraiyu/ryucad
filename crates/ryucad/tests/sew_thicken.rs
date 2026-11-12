//! Sews and thickens through the protocol (ryucad 18.B): a box's faces
//! extracted and sewn back to its volume as a solid, or as a closed sheet;
//! a gap past the tolerance refused as pieces; a square sheet thickened
//! forward and back against closed forms, and with a face of its own; a
//! box's face thickened and joined; names across thickness edits.

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

/// `got` within a part in `rel` of `want`.
fn near(got: f64, want: f64, rel: f64) {
    assert!(
        (got - want).abs() <= rel * want,
        "{got} mm³ against {want} mm³"
    );
}

/// Part `p` with a 10 x 20 x 5 mm box `b` and each of its faces extracted
/// as `x0`..`x5`, in `b.faces()` order; returns their IDs.
fn extracted(s: &mut Session) -> Vec<String> {
    add(
        s,
        json!({"type": "box", "id": "b", "params": {"x": 10, "y": 20, "z": 5}}),
    );
    let b = body(s, "b");
    let faces: Vec<String> = b
        .faces()
        .map(|f| {
            b.name_of(ryukei::topo::EntityId::Face(f))
                .unwrap()
                .to_string()
        })
        .collect();
    faces
        .iter()
        .enumerate()
        .map(|(k, n)| {
            let id = format!("x{k}");
            add(
                s,
                json!({"type": "extract", "id": id, "faces": [format!("p/b/{n}")]}),
            );
            id
        })
        .collect()
}

#[test]
fn a_boxs_faces_sew_back_into_its_solid_or_a_closed_sheet() {
    let (_dir, mut s) = project();
    let xs = extracted(&mut s);
    add(&mut s, json!({"type": "sew", "id": "sw", "sheets": xs}));
    near(volume(&mut s, "sw", "sw"), 1000.0, 1e-9);
    // Faces keep their names: the sewn solid has every face of the box.
    let b = names(&mut s, "b");
    let sw = names(&mut s, "sw");
    let faces_kept = body(&mut s, "b")
        .faces()
        .map(|f| {
            body(&mut s, "b")
                .name_of(ryukei::topo::EntityId::Face(f))
                .unwrap()
                .to_string()
        })
        .all(|n| sw.contains(&n));
    assert!(faces_kept, "{b:?} against {sw:?}");
    // Not a solid: a closed sheet.
    must(
        &mut s,
        "feature.edit",
        json!({"part": "p", "id": "sw", "make_solid": false}),
    );
    let sheet = body(&mut s, "sw");
    assert_eq!(
        (ryucad::kind::name(&sheet), ryucad::kind::boundaries(&sheet)),
        ("sheet", 0)
    );
    // The extracts are consumed: the report says so.
    let r = reported(&mut s, "sw");
    assert_eq!(r["consumes"].as_array().unwrap().len(), 6, "{r}");
}

#[test]
fn a_gap_past_the_tolerance_is_refused_as_pieces() {
    let (_dir, mut s) = project();
    // Two squares 1 mm apart, side by side in xy, as planar faces.
    sketch(&mut s, "a", json!("xy"), square(0.0, 0.0, 10.0));
    sketch(&mut s, "b", json!("xy"), square(11.0, 0.0, 10.0));
    add(&mut s, json!({"type": "planar", "id": "fa", "sketch": "a"}));
    add(&mut s, json!({"type": "planar", "id": "fb", "sketch": "b"}));
    add(
        &mut s,
        json!({"type": "sew", "id": "sw", "sheets": ["fa", "fb"], "make_solid": false}),
    );
    let r = reported(&mut s, "sw");
    assert_eq!(
        (
            r["error"]["code"].as_str(),
            r["error"]["details"]["reason"].as_str()
        ),
        (Some("sew_failed"), Some("pieces")),
        "{r}"
    );
}

#[test]
fn a_square_sheet_thickened_forward_and_back() {
    let (_dir, mut s) = project();
    sketch(&mut s, "a", json!("xy"), square(0.0, 0.0, 10.0));
    add(&mut s, json!({"type": "planar", "id": "f", "sketch": "a"}));
    add(
        &mut s,
        json!({"type": "thicken", "id": "t", "sheet": "f", "front": "2 mm"}),
    );
    near(volume(&mut s, "t", "t"), 200.0, 1e-9);
    let before = names(&mut s, "t");
    must(
        &mut s,
        "feature.edit",
        json!({"part": "p", "id": "t", "front": "3 mm"}),
    );
    near(volume(&mut s, "t", "t"), 300.0, 1e-9);
    // A new thickness renames nothing (a side at zero keeps the sheet's
    // names, so its back stays at zero here).
    assert_eq!(names(&mut s, "t"), before);
    must(
        &mut s,
        "feature.edit",
        json!({"part": "p", "id": "t", "back": "1 mm"}),
    );
    near(volume(&mut s, "t", "t"), 400.0, 1e-9);
    // The planar face is consumed: nothing may act on it after.
    let e = call(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "sew",
        "sheets": ["f", "t"]}),
    )
    .unwrap_err();
    assert_eq!(e.code, ryucad::error::Code::BodyConsumed, "{e}");
}

#[test]
fn a_boxs_face_thickened_outwards_joins_it() {
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
    let top = b.name_of(ryukei::topo::EntityId::Face(top)).unwrap();
    add(
        &mut s,
        json!({"type": "thicken", "id": "t", "from_faces": [format!("p/b/{top}")],
                       "front": "3 mm", "operation": "join", "targets": ["b"]}),
    );
    near(volume(&mut s, "t", "b"), 1000.0 + 600.0, 1e-9);
    // The box keeps its body's other faces' names.
    let r = reported(&mut s, "t");
    assert_eq!(r["operation"], "join", "{r}");
}
