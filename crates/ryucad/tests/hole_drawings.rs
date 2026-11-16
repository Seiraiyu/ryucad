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

/// The plate drilled by a fill of M3 holes 10 mm apart from (5, 5), less
/// [1, 1], an H7 dowel at (15, 15) where the skipped one was, and datum
/// point `o` at the plate's top corner.
fn filled() -> (tempfile::TempDir, Session) {
    let (d, mut s) = plate(&[[5.0, 5.0]]);
    drill(&mut s, "h", &["p1"], "M3x0.5-6H THRU");
    let top = top_face(&mut s);
    add(
        &mut s,
        json!({"type": "pattern", "id": "f", "features": ["h"], "form": "fill",
               "face": top, "direction": "x", "spacing": 10, "skip": [[1, 1]]}),
    );
    for (id, at) in [
        ("dp", ["15 mm", "15 mm", "10 mm"]),
        ("o", ["0 mm", "0 mm", "10 mm"]),
    ] {
        add(
            &mut s,
            json!({"type": "point", "id": id, "by": "coordinates", "coordinates": at}),
        );
    }
    add(
        &mut s,
        json!({"type": "hole", "id": "dowel", "spec": "DIA 3 H7 THRU",
               "places": {"a": {"at": "p/dp"}}}),
    );
    (d, s)
}

#[test]
fn a_hole_table_groups_by_spec_and_orders_by_x_then_y() {
    let (_d, mut s) = filled();
    let g = annotate(
        &mut s,
        json!({"type": "hole_table", "view": "v1", "origin": "o"}),
    )
    .unwrap();
    let t = ann(&g, "t1");
    assert_eq!(t["text"], "hole table of p, 12 holes", "{t}");
    let v = &t["value"];
    assert_eq!(
        v["groups"],
        json!([{"tag": "A", "spec": "DIA 3 H7 THRU", "count": 1},
               {"tag": "B", "spec": "M3x0.5-6H THRU", "count": 11}])
    );
    let rows: Vec<(String, f64, f64)> = v["holes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            (
                r["tag"].as_str().unwrap().to_owned(),
                (r["x"].as_f64().unwrap() * 1e3).round(),
                (r["y"].as_f64().unwrap() * 1e3).round(),
            )
        })
        .collect();
    let want: Vec<(String, f64, f64)> = [
        ("A1", 15, 15),
        ("B1", 5, 5),
        ("B2", 5, 15),
        ("B3", 5, 25),
        ("B4", 15, 5),
        ("B5", 15, 25),
        ("B6", 25, 5),
        ("B7", 25, 15),
        ("B8", 25, 25),
        ("B9", 35, 5),
        ("B10", 35, 15),
        ("B11", 35, 25),
    ]
    .iter()
    .map(|(t, x, y)| ((*t).to_owned(), f64::from(*x), f64::from(*y)))
    .collect();
    assert_eq!(rows, want);
    assert_eq!(v["holes"][0]["ref"], "p/dowel/a");
    // Only what it names: the dowel by its reference, the fill by its ID.
    let g = annotate(
        &mut s,
        json!({"type": "hole_table", "view": "v1", "origin": "o", "holes": ["p/dowel/a"]}),
    )
    .unwrap();
    assert_eq!(ann(&g, "t2")["value"]["rows"], 1);
    let g = annotate(
        &mut s,
        json!({"type": "hole_table", "view": "v1", "origin": "o", "holes": ["f"]}),
    )
    .unwrap();
    assert_eq!(
        ann(&g, "t3")["value"]["rows"],
        10,
        "the fill's instances, not its seed"
    );
}

#[test]
fn a_hole_table_naming_no_hole_is_refused_and_one_on_an_assembly_view_too() {
    let (_d, mut s) = filled();
    let e = annotate(
        &mut s,
        json!({"type": "hole_table", "view": "v1", "origin": "o", "holes": ["nope"]}),
    )
    .unwrap_err();
    assert_eq!(json!(e.code), "not_found", "{e}");
    assert_eq!(e.details["reason"], "no_hole", "{e}");
    assert_eq!(e.details["pointer"], "/annotations/t1");
    must(
        &mut s,
        "part.create",
        json!({"part": "a", "kind": "assembly"}),
    );
    must(
        &mut s,
        "feature.add",
        json!({"part": "a", "type": "instance", "id": "i", "of": "p", "fixed": true}),
    );
    must(
        &mut s,
        "drawing.create",
        json!({"part": "a", "drawing": "d", "name": "A", "views": "standard"}),
    );
    let e = call(
        &mut s,
        "drawing.edit",
        json!({"part": "a", "drawing": "d", "ops": [{"op": "add", "kind": "annotation",
               "value": {"type": "hole_table", "view": "v1", "origin": "i/b/0123456789abcdef0123456789abcdef"}}]}),
    )
    .unwrap_err();
    assert_eq!(json!(e.code), "invalid_param", "{e}");
    assert_eq!(e.details["reason"], "assembly_view", "{e}");
}

#[test]
fn a_tapped_hole_sized_only_by_a_diameter_wants_its_thread_called_out() {
    let (_d, mut s) = plate(&[[10.0, 15.0], [30.0, 15.0]]);
    drill(&mut s, "t", &["p1"], "M6x1.0-6H THRU");
    drill(&mut s, "h", &["p2"], "DIA 6 THRU");
    // Nothing called out: each hole once, as uncalled; no thread finding.
    let g = get(&mut s);
    assert_eq!(codes(&g, "dwg_hole_uncalled"), 2, "{}", g["findings"]);
    assert_eq!(codes(&g, "dwg_thread_uncalled"), 0);
    // A diameter on each wall: the plain hole is called, the tapped one's
    // thread isn't.
    for r in ["p/t/p1", "p/h/p2"] {
        let w = wall(&mut s, r);
        annotate(&mut s, json!({"type": "diameter", "view": "v1", "of": w})).unwrap();
    }
    let g = get(&mut s);
    assert_eq!(codes(&g, "dwg_hole_uncalled"), 0);
    assert_eq!(codes(&g, "dwg_thread_uncalled"), 1, "{}", g["findings"]);
    let f = g["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["code"] == "dwg_thread_uncalled")
        .unwrap()
        .clone();
    assert_eq!(f["details"]["hole"], "t");
    assert_eq!(f["fixes"][0][0]["value"]["type"], "hole_callout");
    // A hole table calls both out, threads and all.
    must(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "point", "id": "o",
        "by": "coordinates", "coordinates": ["0 mm", "0 mm", "10 mm"]}),
    );
    let g = annotate(
        &mut s,
        json!({"type": "hole_table", "view": "v1", "origin": "o"}),
    )
    .unwrap();
    assert_eq!(codes(&g, "dwg_thread_uncalled"), 0, "{}", g["findings"]);
    assert_eq!(
        ryucad::drawing::lint::rule("dwg_thread_uncalled").map(|r| r.weight),
        Some(3)
    );
}

#[test]
fn a_hole_listed_in_a_table_is_called_out() {
    let (_d, mut s) = filled();
    assert!(codes(&get(&mut s), "dwg_hole_uncalled") > 0);
    let g = annotate(
        &mut s,
        json!({"type": "hole_table", "view": "v1", "origin": "o"}),
    )
    .unwrap();
    assert_eq!(codes(&g, "dwg_hole_uncalled"), 0, "{}", g["findings"]);
}

/// Every path item whose model entity is `entity`: (start, its segments).
fn paths_of(g: &Value, entity: &str) -> Vec<Value> {
    g["sheets"][0]["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["entity"] == entity && i.get("path").is_some())
        .cloned()
        .collect()
}

#[test]
fn a_tapped_holes_thread_is_drawn_end_on_side_on_and_in_section() {
    let (_d, mut s) = plate(&[[10.0, 15.0]]);
    drill(&mut s, "t", &["p1"], "M6x1.0-6H THRU");
    let w = wall(&mut s, "p/t/p1");
    let entity = format!("p/{w}");
    let full = |s: &mut Session| {
        must(
            s,
            "drawing.get",
            json!({"part": "p", "drawing": "d", "items": true}),
        )
    };
    let g = full(&mut s);
    let drawn = paths_of(&g, &entity);
    // End on in the top view: a three-quarter circle of the major
    // diameter's radius, 3 mm at 1:1.
    let arcs: Vec<&Value> = drawn
        .iter()
        .flat_map(|i| i["path"]["segments"].as_array().unwrap())
        .filter_map(|x| x.get("arc"))
        .filter(|a| (a["sweep"].as_f64().unwrap() - 1.5 * std::f64::consts::PI).abs() < 1e-12)
        .collect();
    assert_eq!(arcs.len(), 1, "{drawn:?}");
    assert!((arcs[0]["radius"].as_f64().unwrap() - 3.0).abs() < 1e-9);
    // Side on in the front view, hidden: the bore's own silhouettes 5 mm
    // apart (the tap drill) and the thread's lines 6 mm apart outside
    // them, all dashed, all the wall's.
    let straight = |i: &&Value| {
        i["path"]["segments"].as_array().unwrap().len() == 1
            && i["path"]["segments"][0].get("line").is_some()
    };
    // A solid line's stroke has no dashes (`"dash": []`).
    let dashed = |i: &&Value| {
        i["stroke"]["dash"]
            .as_array()
            .is_some_and(|d| !d.is_empty())
    };
    let mut xs: Vec<f64> = drawn
        .iter()
        .filter(straight)
        .filter(dashed)
        .map(|i| i["path"]["start"][0].as_f64().unwrap())
        .collect();
    xs.sort_by(f64::total_cmp);
    assert!(xs.len() >= 4, "{drawn:?}");
    assert!((xs[xs.len() - 1] - xs[0] - 6.0).abs() < 1e-9, "{xs:?}");
    // Without hidden lines, nothing of the wall in the front view.
    must(
        &mut s,
        "drawing.edit",
        json!({"part": "p", "drawing": "d", "ops": [{"op": "set", "id": "v2", "fields": {"hidden": "off"}}]}),
    );
    let g = full(&mut s);
    assert!(paths_of(&g, &entity).iter().filter(straight).count() == 0);
    // In a section through the hole: the thread's two thin solid lines,
    // 6 mm apart, under the section's thick outline (the cut wall's lines
    // are named by the cut, so the thread's are the wall's alone).
    must(
        &mut s,
        "drawing.edit",
        json!({"part": "p", "drawing": "d", "ops": [{"op": "add", "kind": "view",
            "value": {"type": "section", "sheet": "s1", "from": "v1", "label": "A",
                      "cut": {"plane": "zx", "offset": "15 mm"}}}]}),
    );
    let g = full(&mut s);
    let v3 = g["sheets"][0]["views"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["view"] == "v3")
        .cloned()
        .unwrap();
    let range =
        v3["items"][0].as_u64().unwrap() as usize..v3["items"][1].as_u64().unwrap() as usize;
    let items = &g["sheets"][0]["items"].as_array().unwrap()[range];
    let thick = items
        .iter()
        .filter(|i| i.get("path").is_some() && !dashed(i))
        .filter_map(|i| i["stroke"]["width"].as_f64())
        .fold(0.0, f64::max);
    let mut xs: Vec<f64> = items
        .iter()
        .filter(|i| i["entity"] == entity.as_str() && i.get("path").is_some())
        .filter(straight)
        .filter(|i| !dashed(i) && i["stroke"]["width"].as_f64().unwrap() < thick)
        .map(|i| i["path"]["start"][0].as_f64().unwrap())
        .collect();
    xs.sort_by(f64::total_cmp);
    assert_eq!(xs.len(), 2, "{xs:?} in {v3}");
    assert!((xs[1] - xs[0] - 6.0).abs() < 1e-9, "{xs:?}");
}

#[test]
fn a_plain_holes_drawing_draws_no_threads() {
    let (_d, mut s) = plate(&[[10.0, 15.0]]);
    drill(&mut s, "h", &["p1"], "DIA 5 THRU");
    let g = must(
        &mut s,
        "drawing.get",
        json!({"part": "p", "drawing": "d", "items": true}),
    );
    let quarter = g["sheets"][0]["items"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|i| {
            i["path"]["segments"]
                .as_array()
                .cloned()
                .unwrap_or_default()
        })
        .filter_map(|x| x.get("arc").cloned())
        .filter(|a| (a["sweep"].as_f64().unwrap() - 1.5 * std::f64::consts::PI).abs() < 1e-12)
        .count();
    assert_eq!(quarter, 0);
}
