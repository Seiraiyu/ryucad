//! Stable names across edits of solids, through the protocol: edits that
//! must rename nothing (moving and resizing cuts, fillets, chamfers and
//! holes, a pattern's spacing), edits that must rename exactly what they
//! say (a hole's form, a pattern's count, skips and second direction),
//! and the freecad-spike's two cases: a notch in the sketch under a
//! fillet keeps the fillet on its corner, and the sketch drawn again from
//! scratch fails the fillet loudly, naming what it lost.

use ryucad::doc::id::Id;
use ryucad::proto::Request;
use ryucad::server::handle;
use ryucad::session::Session;
use serde_json::{Value, json};
use std::collections::BTreeMap;

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

fn edit(s: &mut Session, id: &str, f: Value) -> Value {
    let mut f = f;
    f["part"] = json!("p");
    f["id"] = json!(id);
    must(s, "feature.edit", f)
}

/// A project with part `p`, in millimetres.
fn project() -> (tempfile::TempDir, Session) {
    let dir = tempfile::tempdir().unwrap();
    ryucad::server::init(dir.path(), None).unwrap();
    let mut s = Session::open(dir.path()).unwrap();
    must(&mut s, "part.create", json!({"part": "p"}));
    (dir, s)
}

/// Part `p` with a plate `b` 40 × 30 × 10 mm and a sketch `t` on its top
/// with a fixed point at each of `points` (mm).
fn plate(points: &[[f64; 2]]) -> (tempfile::TempDir, Session) {
    let (dir, mut s) = project();
    add(
        &mut s,
        json!({"type": "box", "id": "b", "params": {"x": 40, "y": 30, "z": 10}}),
    );
    add(
        &mut s,
        json!({"type": "sketch", "id": "t", "placement": {"origin": [0, 0, 10]}}),
    );
    let mut ops = Vec::new();
    for (i, at) in points.iter().enumerate() {
        ops.push(json!({"op": "add", "type": "point", "at": at}));
        ops.push(json!({"op": "add", "type": "fix", "a": format!("p{}", i + 1)}));
    }
    must(
        &mut s,
        "sketch.edit",
        json!({"part": "p", "sketch": "t", "ops": ops}),
    );
    (dir, s)
}

/// The body of `inspect`'s answer (`feature`'s state, or the model's one
/// solid).
fn body(s: &mut Session, feature: Option<&str>) -> Value {
    let mut q = json!({"part": "p"});
    if let Some(f) = feature {
        q["feature"] = json!(f);
    }
    let v = must(s, "inspect", q);
    assert_eq!(v["bodies"].as_array().unwrap().len(), 1, "{v}");
    v["bodies"][0].clone()
}

/// Every face, edge and vertex name of the model's one solid, with its
/// origin.
fn names(s: &mut Session) -> BTreeMap<String, Value> {
    let b = body(s, None);
    let mut out = BTreeMap::new();
    for list in ["faces", "edges", "vertices"] {
        for e in b[list].as_array().unwrap() {
            out.insert(e["name"].as_str().unwrap().to_owned(), e["origin"].clone());
        }
    }
    out
}

/// The names `after` has that `before` hasn't, and those it lost, each
/// with its origin.
fn changed(
    before: &BTreeMap<String, Value>,
    after: &BTreeMap<String, Value>,
) -> (Vec<Value>, Vec<Value>) {
    let only = |a: &BTreeMap<String, Value>, b: &BTreeMap<String, Value>| {
        a.iter()
            .filter(|(n, _)| !b.contains_key(*n))
            .map(|(_, o)| o.clone())
            .collect()
    };
    (only(after, before), only(before, after))
}

/// The name of the vertex of `body` at `p` (meters).
fn vertex(body: &Value, p: [f64; 3]) -> String {
    body["vertices"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| (0..3).all(|k| (v["point"][k].as_f64().unwrap() - p[k]).abs() < 1e-9))
        .map(|v| v["name"].as_str().unwrap().to_owned())
        .unwrap_or_else(|| panic!("no vertex at {p:?}"))
}

/// The name of the edge of `body` from `a` to `b` (meters).
fn edge(body: &Value, a: [f64; 3], b: [f64; 3]) -> String {
    let (a, b) = (vertex(body, a), vertex(body, b));
    body["edges"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| (e["start"] == a && e["end"] == b) || (e["start"] == b && e["end"] == a))
        .map(|e| e["name"].as_str().unwrap().to_owned())
        .expect("an edge from a to b")
}

/// The part's rebuild has no failures.
fn builds(s: &mut Session) {
    let part = Id::parse("p", "part").unwrap();
    let r = s.rebuilt(&part).unwrap();
    assert_eq!(r.failed(), 0, "{:?}", r.features);
}

#[test]
fn moving_and_resizing_a_cut_renames_nothing() {
    let (_dir, mut s) = plate(&[]);
    add(
        &mut s,
        json!({"type": "box", "id": "c", "operation": "cut", "targets": ["b"],
               "params": {"x": 6, "y": 4, "z": 3}, "placement": {"origin": [5, 5, 8]}}),
    );
    let before = names(&mut s);
    edit(
        &mut s,
        "c",
        json!({"params": {"x": 9, "y": 5, "z": 4}, "placement": {"origin": [7, 4, 7]}}),
    );
    builds(&mut s);
    assert_eq!(names(&mut s), before);
}

#[test]
fn resizing_a_fillet_or_a_chamfer_renames_nothing() {
    let (_dir, mut s) = plate(&[]);
    let b = body(&mut s, Some("b"));
    let front = edge(&b, [0.0, 0.0, 0.01], [0.04, 0.0, 0.01]);
    let side = edge(&b, [0.0, 0.03, 0.0], [0.0, 0.03, 0.01]);
    add(
        &mut s,
        json!({"type": "fillet", "id": "f", "radius": 2, "edges": [format!("p/b/{front}")],
               "propagate": false}),
    );
    add(
        &mut s,
        json!({"type": "chamfer", "id": "k", "form": "equal", "d": 1,
               "edges": [format!("p/b/{side}")], "propagate": false}),
    );
    let before = names(&mut s);
    edit(&mut s, "f", json!({"radius": 3}));
    edit(&mut s, "k", json!({"d": "1.5 mm"}));
    builds(&mut s);
    assert_eq!(names(&mut s), before);
}

#[test]
fn moving_and_resizing_a_hole_renames_nothing() {
    let (_dir, mut s) = plate(&[[10.0, 10.0]]);
    add(
        &mut s,
        json!({"type": "hole", "id": "h", "sketch": "t", "diameter": 4, "depth": {"blind": 6},
               "bottom": {"drill": 118}}),
    );
    let before = names(&mut s);
    must(
        &mut s,
        "sketch.edit",
        json!({"part": "p", "sketch": "t", "ops": [
            {"op": "remove", "id": "c1"},
            {"op": "move", "id": "p1", "at": [25, 18]},
            {"op": "add", "type": "fix", "a": "p1"}
        ]}),
    );
    edit(
        &mut s,
        "h",
        json!({"diameter": 5, "depth": {"blind": 7}, "bottom": {"drill": 120}}),
    );
    builds(&mut s);
    assert_eq!(names(&mut s), before);
}

#[test]
fn a_holes_new_form_renames_only_its_new_segments() {
    let (_dir, mut s) = plate(&[[10.0, 10.0]]);
    add(
        &mut s,
        json!({"type": "hole", "id": "h", "sketch": "t", "diameter": 4, "depth": "through_all"}),
    );
    let before = names(&mut s);
    edit(
        &mut s,
        "h",
        json!({"form": "counterbore", "counterbore": {"diameter": 8, "depth": 3}}),
    );
    builds(&mut s);
    let after = names(&mut s);
    let (new, gone) = changed(&before, &after);
    // The counterbore's wall and floor are new faces; what is gone or new
    // besides is where the bore met the plate's top, now its floor.
    assert!(!new.is_empty());
    for o in new.iter().chain(&gone) {
        assert_eq!(o["feature"], "h", "{o}");
    }
    let faces = |b: &Value| -> Vec<Value> {
        b["faces"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|f| f["origin"]["feature"] == "h")
            .map(|f| f["origin"]["segment"].clone())
            .collect()
    };
    let mut segments = faces(&body(&mut s, None));
    segments.sort_by_key(|v| v.to_string());
    assert_eq!(
        segments,
        [
            json!("counterbore_floor"),
            json!("counterbore_wall"),
            json!("wall")
        ]
    );
    // The bore's own wall kept its name.
    let wall = |m: &BTreeMap<String, Value>| {
        m.iter()
            .find(|(_, o)| o["segment"] == "wall" && o.get("point").is_some())
            .map(|(n, _)| n.clone())
    };
    assert_eq!(wall(&before), wall(&after));
    assert!(wall(&before).is_some());
}

/// Instances that `origins` came from, sorted.
fn instances(origins: &[Value]) -> Vec<Value> {
    let mut out: Vec<Value> = origins.iter().map(|o| o["instance"].clone()).collect();
    out.sort_by_key(|v| v.to_string());
    out.dedup();
    out
}

#[test]
fn a_patterns_spacing_renames_nothing_and_its_count_and_skips_only_their_instances() {
    let (_dir, mut s) = plate(&[[5.0, 5.0]]);
    add(
        &mut s,
        json!({"type": "hole", "id": "h", "sketch": "t", "diameter": 2, "depth": {"blind": 4}}),
    );
    add(
        &mut s,
        json!({"type": "pattern", "id": "row", "features": ["h"], "direction": "x",
               "spacing": 6, "count": 3}),
    );
    let first = names(&mut s);
    // A wider spacing: nothing renamed.
    edit(&mut s, "row", json!({"spacing": 7}));
    builds(&mut s);
    assert_eq!(names(&mut s), first);
    // A fourth instance: only its own names are new.
    edit(&mut s, "row", json!({"count": 4}));
    builds(&mut s);
    let four = names(&mut s);
    let (new, gone) = changed(&first, &four);
    assert!(gone.is_empty(), "{gone:?}");
    assert_eq!(instances(&new), [json!([3])]);
    // Skipping the second: only its names go.
    edit(&mut s, "row", json!({"skip": [1]}));
    builds(&mut s);
    let skipped = names(&mut s);
    let (new, gone) = changed(&four, &skipped);
    assert!(new.is_empty(), "{new:?}");
    assert_eq!(instances(&gone), [json!([1])]);
    // A second direction, the same instance skipped as `[1, 0]`: the
    // first row keeps every name.
    edit(
        &mut s,
        "row",
        json!({"direction2": "y", "spacing2": 8, "count2": 2, "skip": [[1, 0]]}),
    );
    builds(&mut s);
    let grid = names(&mut s);
    let (new, gone) = changed(&skipped, &grid);
    assert!(gone.is_empty(), "{gone:?}");
    assert!(!new.is_empty());
    for o in &new {
        assert_eq!(o["instance"][1], 1, "{o}");
    }
}

/// The ops for a 20 × 10 mm rectangle `p1`..`p4`, `l1`..`l4` with its
/// corner at the origin, every point fixed.
fn rectangle() -> Value {
    rectangle_with(["p1", "p2", "p3", "p4"], ["l1", "l2", "l3", "l4"])
}

fn rectangle_with(p: [&str; 4], l: [&str; 4]) -> Value {
    let at = [[0, 0], [20, 0], [20, 10], [0, 10]];
    let mut ops = Vec::new();
    for (id, at) in p.iter().zip(at) {
        ops.push(json!({"op": "add", "id": id, "type": "point", "at": at}));
        ops.push(json!({"op": "add", "type": "fix", "a": id}));
    }
    for k in 0..4 {
        ops.push(json!({"op": "add", "id": l[k], "type": "line",
                        "start": p[k], "end": p[(k + 1) % 4]}));
    }
    json!(ops)
}

/// The spike's part: the rectangle `s` extruded 5 mm as `e`, a hole `h`
/// through it from a sketch `t` on its top, and a fillet `f` of 1 mm on
/// the vertical edge at the rectangle's corner `p3`. Returns that edge's
/// name.
fn spike(s: &mut Session) -> String {
    add(s, json!({"type": "sketch", "id": "s"}));
    must(
        s,
        "sketch.edit",
        json!({"part": "p", "sketch": "s", "ops": rectangle()}),
    );
    add(
        s,
        json!({"type": "extrude", "id": "e", "sketch": "s", "extent": {"length": 5}}),
    );
    add(
        s,
        json!({"type": "sketch", "id": "t", "placement": {"origin": [0, 0, 5]}}),
    );
    must(
        s,
        "sketch.edit",
        json!({"part": "p", "sketch": "t", "ops": [
            {"op": "add", "type": "point", "at": [4, 6]},
            {"op": "add", "type": "fix", "a": "p1"}
        ]}),
    );
    add(
        s,
        json!({"type": "hole", "id": "h", "sketch": "t", "diameter": 3, "depth": "through_all"}),
    );
    let corner = edge(&body(s, Some("e")), [0.02, 0.01, 0.0], [0.02, 0.01, 0.005]);
    add(
        s,
        json!({"type": "fillet", "id": "f", "radius": 1, "edges": [format!("p/e/{corner}")]}),
    );
    corner
}

/// The ops that notch the rectangle's bottom: `l1` now runs from `p1` to
/// `p5`, and `l5` to `l8` go up, across and down to `p2`.
fn notch() -> Value {
    let mut ops = vec![json!({"op": "remove", "id": "l1"})];
    for (id, at) in [
        ("p5", [6, 0]),
        ("p6", [6, 3]),
        ("p7", [12, 3]),
        ("p8", [12, 0]),
    ] {
        ops.push(json!({"op": "add", "id": id, "type": "point", "at": at}));
        ops.push(json!({"op": "add", "type": "fix", "a": id}));
    }
    for (id, a, b) in [
        ("l1", "p1", "p5"),
        ("l5", "p5", "p6"),
        ("l6", "p6", "p7"),
        ("l7", "p7", "p8"),
        ("l8", "p8", "p2"),
    ] {
        ops.push(json!({"op": "add", "id": id, "type": "line", "start": a, "end": b}));
    }
    json!(ops)
}

/// The ops that draw the rectangle again from scratch, as `p5`..`p8` and
/// `l5`..`l8`, where it was.
fn redraw() -> Value {
    let mut ops: Vec<Value> = ["l1", "l2", "l3", "l4", "p1", "p2", "p3", "p4"]
        .iter()
        .map(|id| json!({"op": "remove", "id": id}))
        .collect();
    ops.extend(
        rectangle_with(["p5", "p6", "p7", "p8"], ["l5", "l6", "l7", "l8"])
            .as_array()
            .unwrap()
            .iter()
            .cloned(),
    );
    json!(ops)
}

/// The fillet's face in the model, and the edges it replaces.
fn fillet_face(s: &mut Session) -> (Value, Value) {
    let b = body(s, None);
    let f = b["faces"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["origin"]["feature"] == "f")
        .cloned()
        .expect("the fillet's face");
    let replaces = f["origin"]["replaces"].clone();
    (f, replaces)
}

#[test]
fn a_notch_keeps_the_fillet_on_its_corner() {
    let (_dir, mut s) = project();
    let corner = spike(&mut s);
    let (face, replaces) = fillet_face(&mut s);
    // The edge, and the corner vertices the fillet's ends took away.
    assert!(replaces.as_array().unwrap().contains(&json!(corner)));
    must(
        &mut s,
        "sketch.edit",
        json!({"part": "p", "sketch": "s", "ops": notch()}),
    );
    builds(&mut s);
    // The edge at the corner keeps its name, and the fillet is still on it:
    // the same face, a cylinder about the corner's axis 1 mm in.
    let e = body(&mut s, Some("e"));
    assert_eq!(edge(&e, [0.02, 0.01, 0.0], [0.02, 0.01, 0.005]), corner);
    let (after, again) = fillet_face(&mut s);
    assert_eq!(again, replaces);
    assert_eq!(after["name"], face["name"]);
    let axis = &after["surface"]["frame"]["origin"];
    assert!((axis[0].as_f64().unwrap() - 0.019).abs() < 1e-9, "{after}");
    assert!((axis[1].as_f64().unwrap() - 0.009).abs() < 1e-9, "{after}");
}

#[test]
fn a_redrawn_sketch_fails_the_fillet_naming_what_it_lost() {
    let (_dir, mut s) = project();
    let corner = spike(&mut s);
    must(
        &mut s,
        "sketch.edit",
        json!({"part": "p", "sketch": "s", "ops": redraw()}),
    );
    let part = Id::parse("p", "part").unwrap();
    let r = s.rebuilt(&part).unwrap();
    // The extrude and the hole build; the fillet fails, and nothing lands
    // on another edge.
    let by: BTreeMap<&str, &ryucad::error::Result<_>> =
        r.features.iter().map(|(i, o)| (i.as_str(), o)).collect();
    assert!(by["e"].is_ok() && by["h"].is_ok());
    let e = by["f"].as_ref().unwrap_err();
    assert_eq!(e.code, ryucad::error::Code::ReferenceUnresolved, "{e}");
    assert_eq!(e.details["role"], "groups.0.edges.0");
    assert_eq!(e.details["reference"], format!("p/e/{corner}"));
    assert_eq!(
        e.details["was"],
        json!({"kind": "edge",
               "from": {"feature": "e", "sketch": "s", "entities": ["l2", "l3"]}})
    );
    assert_eq!(e.details["removed_by"], "e");
}
