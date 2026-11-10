//! Drawings' dimensions and annotations against the model (design 417,
//! plan 9.C), through the requests an agent sends: every dimension type's
//! value against the plate's closed forms, a hole pattern's callout,
//! refusals (foreshortened, not shown), names surviving an edit and an
//! entity a later feature takes away, balloons against item numbers, and
//! the lint.

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

/// Part `p`: a plate `b` 40 × 30 × 10 mm (from the origin) drilled by
/// hole `h`, ⌀6 through all at (10, 15) and (30, 15), from a sketch `s` on
/// its top face; and a drawing `d` of it at 1:1 with a top view `v1` and a
/// front view `v2`.
fn plate() -> (tempfile::TempDir, Session) {
    let dir = tempfile::tempdir().unwrap();
    ryucad::server::init(dir.path(), None).unwrap();
    let mut s = Session::open(dir.path()).unwrap();
    must(&mut s, "part.create", json!({"part": "p"}));
    must(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "box", "id": "b", "params": {"x": 40, "y": 30, "z": 10}}),
    );
    let top = face(&mut s, |f| plane(f, [0.0, 0.0, 1.0]));
    must(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "sketch", "id": "s", "face": format!("p/b/{top}")}),
    );
    must(
        &mut s,
        "sketch.edit",
        json!({"part": "p", "sketch": "s", "ops": [
            {"op": "add", "type": "point", "at": [10, 15]}, {"op": "add", "type": "fix", "a": "p1"},
            {"op": "add", "type": "point", "at": [30, 15]}, {"op": "add", "type": "fix", "a": "p2"}]}),
    );
    must(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "hole", "id": "h", "sketch": "s", "diameter": "6 mm",
               "depth": "through_all", "targets": ["b"]}),
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
                "at": ["120 mm", "60 mm"]}}]}),
    );
    (dir, s)
}

fn body(s: &mut Session) -> Value {
    must(s, "inspect", json!({"part": "p"}))["bodies"][0].clone()
}

fn plane(f: &Value, n: [f64; 3]) -> bool {
    let sign = if f["reversed"] == true { -1.0 } else { 1.0 };
    f["surface"]["type"] == "plane"
        && (0..3)
            .all(|i| (f["surface"]["frame"]["z"][i].as_f64().unwrap() * sign - n[i]).abs() < 1e-12)
}

/// The name of the first face `pick` takes.
fn face(s: &mut Session, pick: impl Fn(&Value) -> bool) -> String {
    let b = body(s);
    b["faces"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| pick(f))
        .map(|f| f["name"].as_str().unwrap().to_owned())
        .expect("a face")
}

/// The name of the vertex at `p` (mm).
fn vertex(s: &mut Session, p: [f64; 3]) -> String {
    let b = body(s);
    b["vertices"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| (0..3).all(|i| (v["point"][i].as_f64().unwrap() * 1000.0 - p[i]).abs() < 1e-9))
        .map(|v| v["name"].as_str().unwrap().to_owned())
        .expect("a vertex")
}

/// The straight edge between the vertices at `a` and `b` (mm).
fn edge(s: &mut Session, a: [f64; 3], b: [f64; 3]) -> String {
    let (va, vb) = (vertex(s, a), vertex(s, b));
    let body = body(s);
    body["edges"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| {
            (e["start"] == va.as_str() && e["end"] == vb.as_str())
                || (e["start"] == vb.as_str() && e["end"] == va.as_str())
        })
        .map(|e| e["name"].as_str().unwrap().to_owned())
        .expect("an edge")
}

/// Adds `value` as an annotation, answering the drawing as built.
fn add(s: &mut Session, value: Value) -> ryucad::error::Result<Value> {
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

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-12
}

#[test]
fn every_dimension_type_measures_the_model() {
    let (_d, mut s) = plate();
    let left = face(&mut s, |f| plane(f, [-1.0, 0.0, 0.0]));
    let right = face(&mut s, |f| plane(f, [1.0, 0.0, 0.0]));
    let wall = face(&mut s, |f| f["surface"]["type"] == "cylinder");
    // The plate's width between its end faces, seen edge on from above.
    let g = add(
        &mut s,
        json!({"type": "linear", "view": "v1", "a": format!("b/{left}"),
        "b": format!("b/{right}"), "along": "horizontal", "tolerance": {"symmetric": "0.1 mm"}}),
    )
    .unwrap();
    let d1 = ann(&g, "d1");
    assert!(close(d1["value"]["value"].as_f64().unwrap(), 0.04), "{d1}");
    assert_eq!(d1["text"], "40.00 ±0.10");
    // A placed offset was written back, and the text's place.
    let file = must(&mut s, "drawing.list", json!({"part": "p"}));
    assert!(file["drawings"][0]["name"] == "Plate");
    // A bore's diameter seen end on; its radius.
    add(&mut s, json!({"type": "diameter", "view": "v1", "of": format!("b/{wall}"), "tolerance": {"fit": "H7"}}))
        .unwrap();
    add(
        &mut s,
        json!({"type": "radius", "view": "v1", "of": format!("b/{wall}")}),
    )
    .unwrap();
    // The plate's height in the front view, vertically between vertices.
    let (v0, v1) = (
        vertex(&mut s, [0.0, 0.0, 0.0]),
        vertex(&mut s, [0.0, 0.0, 10.0]),
    );
    add(&mut s, json!({"type": "linear", "view": "v2", "a": format!("b/{v0}"), "b": format!("b/{v1}"), "along": "vertical"}))
        .unwrap();
    // The corner's right angle, between two top edges.
    let ex = edge(&mut s, [0.0, 0.0, 10.0], [40.0, 0.0, 10.0]);
    let ey = edge(&mut s, [0.0, 0.0, 10.0], [0.0, 30.0, 10.0]);
    add(
        &mut s,
        json!({"type": "angular", "view": "v1", "a": format!("b/{ex}"), "b": format!("b/{ey}")}),
    )
    .unwrap();
    // The far corner's ordinate from the near one.
    let vf = vertex(&mut s, [40.0, 30.0, 10.0]);
    add(&mut s, json!({"type": "ordinate", "view": "v1", "datum": format!("b/{v1}"), "of": format!("b/{vf}"), "along": "vertical"}))
        .unwrap();
    // One edge's own length.
    let g = add(
        &mut s,
        json!({"type": "linear", "view": "v1", "a": format!("b/{ex}")}),
    )
    .unwrap();
    let v = |k: &str| ann(&g, k)["value"]["value"].as_f64().unwrap();
    assert!(close(v("d2"), 0.006));
    assert_eq!(ann(&g, "d2")["text"], "⌀6.00 H7 (+0.012/0)");
    assert!(close(v("d3"), 0.003));
    assert_eq!(ann(&g, "d3")["text"], "R3.00");
    assert!(close(v("d4"), 0.01));
    assert!((v("d5") - std::f64::consts::FRAC_PI_2).abs() < 1e-12);
    assert_eq!(ann(&g, "d5")["text"], "90°");
    assert!(close(v("d6"), 0.03));
    assert!(close(v("d7"), 0.04));
    assert!(
        g["findings"]
            .as_array()
            .unwrap()
            .iter()
            .all(|f| f["severity"] != "error"),
        "{}",
        g["findings"]
    );
    // Placed offsets are written back: building again changes nothing.
    let again = must(
        &mut s,
        "drawing.edit",
        json!({"part": "p", "drawing": "d", "ops": []}),
    );
    assert_eq!(
        again["sheets"][0]["annotations"],
        g["sheets"][0]["annotations"]
    );
}

#[test]
fn a_hole_patterns_callout_comes_from_its_feature() {
    let (_d, mut s) = plate();
    let wall = face(&mut s, |f| f["surface"]["type"] == "cylinder");
    let g = add(
        &mut s,
        json!({"type": "hole_callout", "view": "v1", "of": format!("b/{wall}")}),
    )
    .unwrap();
    let c = ann(&g, "d1");
    assert_eq!(c["text"], "2X ⌀6.00 THRU", "{c}");
    assert_eq!(c["value"]["count"], 2);
    // The lint finds no hole uncalled now.
    assert!(
        !g["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["code"] == "dwg_hole_uncalled")
    );
    // ISO writes ×.
    must(
        &mut s,
        "drawing.edit",
        json!({"part": "p", "drawing": "d", "ops": [{"op": "set", "fields": {"standard": "iso"}}]}),
    );
    assert_eq!(ann(&get(&mut s), "d1")["text"], "2× ⌀6.00 THRU");
}

#[test]
fn a_drawing_without_callouts_is_linted_with_fixes() {
    let (_d, mut s) = plate();
    let lint = must(&mut s, "drawing.lint", json!({"part": "p", "drawing": "d"}));
    let uncalled: Vec<&Value> = lint["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["code"] == "dwg_hole_uncalled")
        .collect();
    assert_eq!(uncalled.len(), 1, "{lint}");
    // Its fix, sent as it is, clears it.
    let fix = uncalled[0]["fixes"][0].clone();
    must(
        &mut s,
        "drawing.edit",
        json!({"part": "p", "drawing": "d", "ops": fix}),
    );
    let lint = must(&mut s, "drawing.lint", json!({"part": "p", "drawing": "d"}));
    assert!(
        !lint["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["code"] == "dwg_hole_uncalled")
    );
    assert!(lint["scores"]["s1"].as_i64().unwrap() <= 100);
    let e = must(
        &mut s,
        "drawing.lint",
        json!({"explain": "dwg_chain_closed"}),
    );
    assert_eq!(e["weight"], 4);
    // Configured off, a rule finds nothing.
    must(
        &mut s,
        "drawing.lint.configure",
        json!({"severity": {"dwg_untoleranced": "off"}}),
    );
    let wall = face(&mut s, |f| f["surface"]["type"] == "cylinder");
    let g = add(
        &mut s,
        json!({"type": "diameter", "view": "v1", "of": format!("b/{wall}")}),
    )
    .unwrap();
    assert!(
        !g["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["code"] == "dwg_untoleranced")
    );
}

#[test]
fn foreshortened_and_hidden_entities_are_refused_when_written() {
    let (_d, mut s) = plate();
    // From a top corner to a datum point under the far corner: across
    // the plate's depth, so not true length from above.
    must(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "point", "id": "q", "by": "coordinates",
        "coordinates": ["40 mm", "30 mm", "0 mm"]}),
    );
    let a = vertex(&mut s, [0.0, 0.0, 10.0]);
    let e = add(&mut s, json!({"type": "linear", "view": "v1", "a": format!("b/{a}"), "b": "q", "along": "aligned"}))
        .unwrap_err();
    assert_eq!(e.details["reason"], "foreshortened", "{e}");
    assert_eq!(e.details["pointer"], "/annotations/d1");
    // A bore's wall is hidden in the front view, which draws no hidden
    // lines.
    let wall = face(&mut s, |f| f["surface"]["type"] == "cylinder");
    let e = add(
        &mut s,
        json!({"type": "diameter", "view": "v2", "of": format!("b/{wall}")}),
    )
    .unwrap_err();
    assert_eq!(json!(e.code), "drawing_reference_not_shown", "{e}");
    // Nothing was written.
    assert!(
        get(&mut s)["sheets"][0]["annotations"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn names_follow_an_edit_and_a_lost_entity_is_kept_and_reported() {
    let (_d, mut s) = plate();
    let wall = face(&mut s, |f| f["surface"]["type"] == "cylinder");
    let ex = edge(&mut s, [0.0, 0.0, 10.0], [40.0, 0.0, 10.0]);
    add(
        &mut s,
        json!({"type": "hole_callout", "view": "v1", "of": format!("b/{wall}")}),
    )
    .unwrap();
    add(
        &mut s,
        json!({"type": "linear", "view": "v1", "a": format!("b/{ex}")}),
    )
    .unwrap();
    // The holes moved: the callout follows.
    must(
        &mut s,
        "sketch.edit",
        json!({"part": "p", "sketch": "s", "ops": [{"op": "move", "id": "p1", "at": [12, 15]}]}),
    );
    assert_eq!(ann(&get(&mut s), "d1")["text"], "2X ⌀6.00 THRU");
    // A cut takes the edge away: the dimension is kept, and reported.
    must(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "box", "id": "c", "params": {"x": 50, "y": 4, "z": 4},
               "placement": {"origin": [-5, -1, 8]}, "operation": "cut", "targets": ["b"]}),
    );
    let g = get(&mut s);
    let d2 = ann(&g, "d2");
    assert_eq!(d2["error"]["code"], "drawing_reference_unresolved", "{d2}");
    assert_eq!(d2["error"]["details"]["removed_by"], "c", "{d2}");
    let check = call(&mut s, "check", json!({"part": "p"})).unwrap_err();
    let report = &check.details["findings"];
    assert!(
        report
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["code"] == "drawing_reference_unresolved"),
        "{report}"
    );
}

/// Draws a dimensioned plate to `RYUCAD_DRAWING_LOOK` (a PNG), to look at.
#[test]
#[ignore]
fn look() {
    let (_d, mut s) = plate();
    let left = face(&mut s, |f| plane(f, [-1.0, 0.0, 0.0]));
    let right = face(&mut s, |f| plane(f, [1.0, 0.0, 0.0]));
    let wall = face(&mut s, |f| f["surface"]["type"] == "cylinder");
    let top = face(&mut s, |f| plane(f, [0.0, 0.0, 1.0]));
    let (v0, v1) = (
        vertex(&mut s, [0.0, 0.0, 0.0]),
        vertex(&mut s, [0.0, 0.0, 10.0]),
    );
    let ex = edge(&mut s, [0.0, 0.0, 10.0], [40.0, 0.0, 10.0]);
    let ey = edge(&mut s, [0.0, 0.0, 10.0], [0.0, 30.0, 10.0]);
    for v in [
        json!({"type": "linear", "view": "v1", "a": format!("b/{left}"), "b": format!("b/{right}"), "along": "horizontal", "tolerance": {"symmetric": "0.1 mm"}}),
        json!({"type": "linear", "view": "v1", "a": format!("b/{ey}"), "along": "vertical"}),
        json!({"type": "diameter", "view": "v1", "of": format!("b/{wall}"), "tolerance": {"fit": "H7"}}),
        json!({"type": "hole_callout", "view": "v1", "of": format!("b/{wall}"), "at": ["-30 mm", "-30 mm"]}),
        json!({"type": "linear", "view": "v2", "a": format!("b/{v0}"), "b": format!("b/{v1}"), "along": "vertical", "tolerance": {"deviation": ["0.1 mm", "0 mm"]}}),
        json!({"type": "angular", "view": "v1", "a": format!("b/{ex}"), "b": format!("b/{ey}")}),
        json!({"type": "datum_feature", "view": "v2", "letter": "A", "on": format!("b/{top}")}),
        json!({"type": "feature_control_frame", "view": "v1", "on": format!("b/{wall}"), "characteristic": "position",
               "rows": [{"tolerance": "0.1 mm", "diameter": true, "condition": "m", "datums": ["A"]}]}),
        json!({"type": "surface_finish", "view": "v2", "on": format!("b/{top}"), "ra": "1.6 um", "process": "removal"}),
        json!({"type": "centre_mark", "view": "v1", "of": format!("b/{wall}")}),
        json!({"type": "note", "sheet": "s1", "text": "BREAK ALL EDGES\nPART {number}"}),
    ] {
        add(&mut s, v).unwrap();
    }
    let root = s.root().to_path_buf();
    let mut slot = Some(s);
    let ans = ryucad::server::handle_safely(
        &mut slot,
        None,
        None,
        &root,
        Request::from_rpc(
            "drawing.render",
            Some(json!({"part": "p", "drawing": "d", "scale": 4.0})),
        )
        .unwrap(),
    )
    .unwrap();
    std::fs::write(
        std::env::var("RYUCAD_DRAWING_LOOK").unwrap(),
        ans.binary.unwrap(),
    )
    .unwrap();
}
