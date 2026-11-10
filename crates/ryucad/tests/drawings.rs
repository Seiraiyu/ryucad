//! Drawings' closed forms (design 417, plan 9.B): sections against their
//! caps' areas, a detail inside its circle, a flat pattern view against
//! `flat.get`, an exploded view's parts apart, and names surviving an edit,
//! through the requests an agent sends.

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

fn project() -> (tempfile::TempDir, Session) {
    let dir = tempfile::tempdir().unwrap();
    ryucad::server::init(dir.path(), None).unwrap();
    let s = Session::open(dir.path()).unwrap();
    (dir, s)
}

/// A drawing `d` of `part` with `views` on one ANSI B sheet at 1:1.
fn drawing(s: &mut Session, part: &str, views: Value) -> Value {
    must(
        s,
        "drawing.create",
        json!({"part": part, "drawing": "d", "name": "D"}),
    );
    let mut ops = vec![json!({"op": "set", "id": "s1", "fields": {"scale": "1:1"}})];
    for (k, v) in views.as_object().unwrap() {
        ops.push(json!({"op": "add", "kind": "view", "id": k, "value": v}));
    }
    must(
        s,
        "drawing.edit",
        json!({"part": part, "drawing": "d", "ops": ops}),
    );
    must(
        s,
        "drawing.get",
        json!({"part": part, "drawing": "d", "items": true}),
    )
}

/// The items of `get`'s first sheet whose stroke is `width` wide, or
/// text.
fn items(get: &Value) -> &Vec<Value> {
    get["sheets"][0]["items"].as_array().unwrap()
}

fn view<'a>(get: &'a Value, key: &str) -> &'a Value {
    get["sheets"][0]["views"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["view"] == key)
        .unwrap_or_else(|| panic!("no view {key}"))
}

/// The sheet's hatch lines' total length (thin, continuous, in the hatch
/// layer's width and nothing else: every two-point path at 45° or 135°).
fn hatched(get: &Value) -> f64 {
    items(get)
        .iter()
        .filter_map(|it| {
            let p = it.get("path")?;
            let segs = p["segments"].as_array()?;
            let [s0] = segs.as_slice() else { return None };
            let a = p["start"].as_array()?;
            let b = s0["line"].as_array()?;
            let (dx, dy) = (
                b[0].as_f64()? - a[0].as_f64()?,
                b[1].as_f64()? - a[1].as_f64()?,
            );
            let thin = it["stroke"]["width"].as_f64()? < 0.5;
            (thin && (dx.abs() - dy.abs()).abs() < 1e-9 && dx != 0.0)
                .then(|| (dx * dx + dy * dy).sqrt())
        })
        .sum()
}

/// A 60 × 40 × 10 mm plate with a 10 mm hole through it at (20, 20).
fn plate(s: &mut Session) {
    must(s, "part.create", json!({"part": "p"}));
    must(
        s,
        "feature.add",
        json!({"part": "p", "type": "box", "id": "b", "params": {"x": 60, "y": 40, "z": 10}}),
    );
    must(
        s,
        "feature.add",
        json!({"part": "p", "type": "cylinder", "id": "h", "params": {"radius": 5, "height": 30},
               "placement": {"origin": [20, 20, -5]}, "operation": "cut", "targets": ["b"]}),
    );
}

#[test]
fn a_full_section_through_the_hole_hatches_the_plate_less_it() {
    let (_d, mut s) = project();
    plate(&mut s);
    let get = drawing(
        &mut s,
        "p",
        json!({
            "v1": {"type": "base", "sheet": "s1", "orient": "top"},
            "v2": {"type": "section", "sheet": "s1", "from": "v1", "cut": {"plane": "yz", "offset": "20 mm"}}
        }),
    );
    assert_eq!(get["findings"], json!([]));
    // The letter it was given, and its caption.
    let texts: Vec<&str> = items(&get)
        .iter()
        .filter_map(|i| i["text"].as_str())
        .collect();
    assert!(texts.contains(&"SECTION A-A"), "{texts:?}");
    // At x = 20 the cap is 40 × 10 less the hole's 10 × 10: 300 mm².
    let area = hatched(&get) * 2.5;
    assert!((area - 300.0).abs() < 300.0 * 0.05, "{area}");
}

#[test]
fn an_offset_section_through_two_holes_hatches_both_steps() {
    let (_d, mut s) = project();
    plate(&mut s);
    must(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "cylinder", "id": "h2", "params": {"radius": 5, "height": 30},
               "placement": {"origin": [45, 30, -5]}, "operation": "cut", "targets": ["b"]}),
    );
    let get = drawing(
        &mut s,
        "p",
        json!({
            "v1": {"type": "base", "sheet": "s1", "orient": "top"},
            "v2": {"type": "section", "sheet": "s1", "from": "v1", "label": "B", "flip": true,
                   "cut": {"through": [[-1, 20, 0], [32, 20, 0], [32, 30, 0], [61, 30, 0]]}}
        }),
    );
    assert_eq!(get["findings"], json!([]));
    // Caps 33 + 29 = 60 mm long and 10 thick, less both holes' 10 × 10.
    let area = hatched(&get) * 2.5;
    assert!((area - 400.0).abs() < 400.0 * 0.05, "{area}");
}

#[test]
fn a_flat_view_draws_the_pattern_flat_get_gives() {
    let (_d, mut s) = project();
    must(&mut s, "part.create", json!({"part": "p"}));
    must(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "sketch", "id": "l", "plane": "xy"}),
    );
    let pts = [[40.0, 0.0], [0.0, 0.0], [0.0, 30.0]];
    let mut ops = Vec::new();
    for (i, at) in pts.iter().enumerate() {
        ops.push(json!({"op": "add", "id": format!("p{}", i + 1), "type": "point", "at": at}));
        ops.push(json!({"op": "add", "type": "fix", "a": format!("p{}", i + 1)}));
    }
    for i in 0..2 {
        ops.push(
            json!({"op": "add", "id": format!("l{}", i + 1), "type": "line",
                        "start": format!("p{}", i + 1), "end": format!("p{}", i + 2)}),
        );
    }
    must(
        &mut s,
        "sketch.edit",
        json!({"part": "p", "sketch": "l", "ops": ops}),
    );
    must(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "base_flange", "id": "bl", "sketch": "l", "thickness": "1.5 mm",
               "radius": "1 mm", "allowance": {"k_factor": "0.44"}, "depth": "20 mm"}),
    );
    let flat = must(&mut s, "flat.get", json!({"part": "p"}));
    let get = drawing(&mut s, "p", json!({"v1": {"type": "flat", "sheet": "s1"}}));
    assert_eq!(get["findings"], json!([]));
    // The view's box is the pattern's, at 1:1.
    let b = &view(&get, "v1")["box_mm"];
    let w = b[1][0].as_f64().unwrap() - b[0][0].as_f64().unwrap();
    let h = b[1][1].as_f64().unwrap() - b[0][1].as_f64().unwrap();
    let fb = &flat["box_mm"];
    let fw = fb[1][0].as_f64().unwrap() - fb[0][0].as_f64().unwrap();
    let fh = fb[1][1].as_f64().unwrap() - fb[0][1].as_f64().unwrap();
    assert!(
        (w - fw).abs() < 1e-6 && (h - fh).abs() < 1e-6,
        "{w} × {h} against {fw} × {fh}"
    );
    // One bend: its note.
    let texts: Vec<&str> = items(&get)
        .iter()
        .filter_map(|i| i["text"].as_str())
        .collect();
    assert!(
        texts
            .iter()
            .any(|t| t.starts_with("UP 90°") || t.starts_with("DOWN 90°")),
        "{texts:?}"
    );
}

#[test]
fn an_exploded_view_draws_its_parts_apart() {
    let (_d, mut s) = project();
    must(&mut s, "part.create", json!({"part": "blk"}));
    must(
        &mut s,
        "feature.add",
        json!({"part": "blk", "type": "box", "id": "b", "params": {"x": 10, "y": 10, "z": 10}}),
    );
    must(
        &mut s,
        "part.create",
        json!({"part": "asm", "kind": "assembly"}),
    );
    must(
        &mut s,
        "feature.add",
        json!({"part": "asm", "type": "instance", "id": "a", "of": "blk", "fixed": true}),
    );
    must(
        &mut s,
        "feature.add",
        json!({"part": "asm", "type": "instance", "id": "b", "of": "blk", "fixed": true,
               "placement": {"origin": [0, 0, 10]}}),
    );
    must(
        &mut s,
        "explode.save",
        json!({"part": "asm", "explode": "up", "steps": [{"instances": ["b"], "translate": [0, 0, 30]}]}),
    );
    let get = drawing(
        &mut s,
        "asm",
        json!({
            "v1": {"type": "base", "sheet": "s1", "orient": "front"},
            "v2": {"type": "exploded", "sheet": "s1", "explode": "up", "orient": "front", "trails": true}
        }),
    );
    assert_eq!(get["findings"], json!([]));
    let height =
        |v: &Value| v["box_mm"][1][1].as_f64().unwrap() - v["box_mm"][0][1].as_f64().unwrap();
    // Stacked, 20 mm tall; exploded, 50.
    assert!((height(view(&get, "v1")) - 20.0).abs() < 1e-9);
    assert!((height(view(&get, "v2")) - 50.0).abs() < 1e-9);
}

#[test]
fn a_section_on_a_face_follows_it_by_name_across_an_edit() {
    let (_d, mut s) = project();
    plate(&mut s);
    let face = |s: &mut Session| -> String {
        let b = must(s, "inspect", json!({"part": "p", "feature": "b"}));
        // The plate's front face (y = 0), facing -y.
        b["bodies"][0]["faces"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| {
                let z = &f["surface"]["frame"]["z"];
                let s = if f["reversed"] == json!(true) {
                    -1.0
                } else {
                    1.0
                };
                f["surface"]["type"] == "plane"
                    && z[1].as_f64() == Some(-s)
                    && z[0].as_f64() == Some(0.0)
            })
            .map(|f| f["name"].as_str().unwrap().to_owned())
            .expect("a front face")
    };
    let name = face(&mut s);
    drawing(
        &mut s,
        "p",
        json!({
            "v1": {"type": "base", "sheet": "s1", "orient": "top"},
            "v2": {"type": "section", "sheet": "s1", "from": "v1", "label": "A",
                   "cut": {"plane": {"feature": "b", "name": name}, "offset": "-20 mm"}}
        }),
    );
    // The hole moved: the section on the plate's own face stays.
    must(
        &mut s,
        "feature.edit",
        json!({"part": "p", "id": "h", "placement": {"origin": [30, 20, -5]}}),
    );
    let get = must(
        &mut s,
        "drawing.get",
        json!({"part": "p", "drawing": "d", "items": true}),
    );
    assert_eq!(get["findings"], json!([]));
    // At y = 20 now through the moved hole: 60 × 10 less 10 × 10.
    let area = hatched(&get) * 2.5;
    assert!((area - 500.0).abs() < 500.0 * 0.05, "{area}");
}
