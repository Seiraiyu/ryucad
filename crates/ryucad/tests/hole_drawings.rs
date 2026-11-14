//! Holes in drawings (design 533, plan 28.D), through the requests an
//! agent sends: callouts of tapped, fitted and mirrored holes and of a
//! fill, hole tables, the thread lint and cosmetic threads in views.

mod common;

use common::{call, must};
use ryucad::session::Session;
use serde_json::{Value, json};

/// Part `p`: a plate `b` 40 × 30 × 10 mm from the origin, a sketch `s` on
/// its top with fixed points at `points` (mm), and drawing `d` at 1:1
/// with a top view `v1` and a front view `v2` (hidden lines dashed).
fn plate(points: &[[f64; 2]]) -> (tempfile::TempDir, Session) {
    let dir = tempfile::tempdir().unwrap();
    ryucad::server::init(dir.path(), None).unwrap();
    let mut s = Session::open(dir.path()).unwrap();
    must(&mut s, "part.create", json!({"part": "p"}));
    add(
        &mut s,
        json!({"type": "box", "id": "b", "params": {"x": 40, "y": 30, "z": 10}}),
    );
    add(
        &mut s,
        json!({"type": "sketch", "id": "s", "placement": {"origin": [0, 0, 10]}}),
    );
    let mut ops = Vec::new();
    for (i, at) in points.iter().enumerate() {
        ops.push(json!({"op": "add", "type": "point", "at": at}));
        ops.push(json!({"op": "add", "type": "fix", "a": format!("p{}", i + 1)}));
    }
    must(
        &mut s,
        "sketch.edit",
        json!({"part": "p", "sketch": "s", "ops": ops}),
    );
    must(
        &mut s,
        "drawing.create",
        json!({"part": "p", "drawing": "d", "name": "Plate"}),
    );
    must(
        &mut s,
        "drawing.edit",
        json!({"part": "p", "drawing": "d", "ops": [
            {"op": "set", "id": "s1", "fields": {"scale": "1:1"}},
            {"op": "add", "kind": "view", "value": {"type": "base", "sheet": "s1", "orient": "top"}},
            {"op": "add", "kind": "view", "value": {"type": "base", "sheet": "s1", "orient": "front",
                "at": ["120 mm", "60 mm"], "hidden": "dashed"}}]}),
    );
    (dir, s)
}

fn add(s: &mut Session, mut f: Value) -> Value {
    f["part"] = json!("p");
    must(s, "feature.add", f)
}

/// Hole `id` at the sketch's `points` by `spec`.
fn drill(s: &mut Session, id: &str, points: &[&str], spec: &str) {
    add(
        s,
        json!({"type": "hole", "id": id, "sketch": "s", "points": points, "spec": spec}),
    );
}

/// The `wall` face of hole `reference` as an annotation names it
/// (`b/<name>`).
fn wall(s: &mut Session, reference: &str) -> String {
    let v = must(s, "holes.list", json!({"part": "p", "ref": reference}));
    let f = &v["holes"][0]["faces"]["wall"];
    let full = f.as_str().or(f[0].as_str()).unwrap();
    full.split_once('/').unwrap().1.to_owned()
}

/// Adds `value` as an annotation: the drawing as built, or the refusal.
fn annotate(s: &mut Session, value: Value) -> ryucad::error::Result<Value> {
    call(
        s,
        "drawing.edit",
        json!({"part": "p", "drawing": "d", "ops": [{"op": "add", "kind": "annotation", "value": value}]}),
    )
}

fn get(s: &mut Session) -> Value {
    must(s, "drawing.get", json!({"part": "p", "drawing": "d"}))
}

/// The built annotation `key`.
fn ann(get: &Value, key: &str) -> Value {
    get["sheets"][0]["annotations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["annotation"] == key)
        .cloned()
        .unwrap_or_else(|| panic!("no {key} in {get}"))
}

fn codes(get: &Value, code: &str) -> usize {
    get["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["code"] == code)
        .count()
}

fn set_standard(s: &mut Session, standard: &str) {
    must(
        s,
        "drawing.edit",
        json!({"part": "p", "drawing": "d", "ops": [{"op": "set", "fields": {"standard": standard}}]}),
    );
}

#[test]
fn a_tapped_patterns_callout_gives_its_thread_then_its_drill() {
    let (_d, mut s) = plate(&[[10.0, 15.0], [30.0, 15.0]]);
    drill(&mut s, "h", &["p1", "p2"], "M6x1.0-6H THRU");
    let w = wall(&mut s, "p/h/p1");
    let g = annotate(
        &mut s,
        json!({"type": "hole_callout", "view": "v1", "of": w}),
    )
    .unwrap();
    let c = ann(&g, "d1");
    assert_eq!(c["text"], "2X M6X1-6H THRU ⌀5.00 THRU", "{c}");
    assert_eq!(c["value"]["count"], 2);
    assert_eq!(c["value"]["thread"], "M6x1.0-6H");
    set_standard(&mut s, "iso");
    assert_eq!(
        ann(&get(&mut s), "d1")["text"],
        "2× M6×1-6H THRU ⌀5.00 THRU"
    );
}

#[test]
fn a_fitted_hole_writes_its_class_and_a_plain_ones_report_keeps_its_keys() {
    let (_d, mut s) = plate(&[[10.0, 15.0], [30.0, 15.0]]);
    drill(&mut s, "f", &["p1"], "DIA 6 H7 THRU");
    drill(&mut s, "h", &["p2"], "DIA 6 THRU");
    let (wf, wh) = (wall(&mut s, "p/f/p1"), wall(&mut s, "p/h/p2"));
    annotate(
        &mut s,
        json!({"type": "hole_callout", "view": "v1", "of": wf}),
    )
    .unwrap();
    let g = annotate(
        &mut s,
        json!({"type": "hole_callout", "view": "v1", "of": wh}),
    )
    .unwrap();
    let f = ann(&g, "d1");
    assert_eq!(f["text"], "⌀6.00 H7 THRU", "{f}");
    assert_eq!(f["value"]["fit"], "H7");
    let h = ann(&g, "d2");
    let keys: Vec<&String> = h["value"].as_object().unwrap().keys().collect();
    assert_eq!(
        keys,
        ["count", "depth", "diameter", "form", "hole", "part", "text"],
        "{h}"
    );
}

#[test]
fn a_mirror_and_a_fill_count_their_instances() {
    // #808's mirror, joined into its own solid: two bores of one feature.
    let (_d, mut s) = plate(&[[8.0, 8.0]]);
    drill(&mut s, "h", &["p1"], "DIA 4 THRU");
    add(
        &mut s,
        json!({"type": "mirror", "id": "m", "solid": "b", "plane": "yz"}),
    );
    let w = wall(&mut s, "p/m/h/p1");
    let g = annotate(
        &mut s,
        json!({"type": "hole_callout", "view": "v1", "of": w}),
    )
    .unwrap();
    assert_eq!(ann(&g, "d1")["text"], "2X ⌀4.00 THRU");
    // A fill of 12 places, less a skip.
    let (_d2, mut t) = plate(&[[5.0, 5.0]]);
    drill(&mut t, "h", &["p1"], "M3x0.5-6H THRU");
    let top = top_face(&mut t);
    add(
        &mut t,
        json!({"type": "pattern", "id": "f", "features": ["h"], "form": "fill",
               "face": top, "direction": "x", "spacing": 10, "skip": [[1, 1]]}),
    );
    let w = wall(&mut t, "p/f/h/p1/1,0");
    let g = annotate(
        &mut t,
        json!({"type": "hole_callout", "view": "v1", "of": w}),
    )
    .unwrap();
    assert_eq!(ann(&g, "d1")["text"], "11X M3X0.5-6H THRU ⌀2.50 THRU");
}

/// The plate's top face as a reference.
fn top_face(s: &mut Session) -> String {
    let b = must(s, "inspect", json!({"part": "p", "feature": "b"}))["bodies"][0].clone();
    let name = b["faces"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| {
            let sign = if f["reversed"] == true { -1.0 } else { 1.0 };
            f["surface"]["type"] == "plane"
                && f["surface"]["frame"]["z"][2].as_f64().unwrap() * sign == 1.0
        })
        .map(|f| f["name"].as_str().unwrap().to_owned())
        .unwrap();
    format!("p/b/{name}")
}
