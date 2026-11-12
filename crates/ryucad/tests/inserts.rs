//! Inserts (design 521, issue #41) through the protocol, in one session.

use ryucad::proto::Request;
use ryucad::server::handle;
use ryucad::session::Session;
use serde_json::{Value, json};

fn call(s: &mut Session, method: &str, params: Value) -> ryucad::error::Result<Value> {
    handle(s, Request::from_rpc(method, Some(params)).unwrap())
}

fn must(s: &mut Session, method: &str, params: Value) -> Value {
    call(s, method, params.clone()).unwrap_or_else(|e| panic!("{method} {params}: {e:?}"))
}

fn add(s: &mut Session, part: &str, mut f: Value) -> Value {
    f["part"] = json!(part);
    must(s, "feature.add", f)
}

/// A project in a temp dir: pcb, a 40 × 30 × 1.6 mm board below xy with
/// four 3 mm holes 4 mm in from its corners.
pub fn board() -> (tempfile::TempDir, Session) {
    let dir = tempfile::tempdir().unwrap();
    ryucad::server::init(dir.path(), None).unwrap();
    let mut s = Session::open(dir.path()).unwrap();
    must(&mut s, "part.create", json!({"part": "pcb"}));
    // The board's top is flush with xy, so a hole sketched there drills
    // down through it.
    add(
        &mut s,
        "pcb",
        json!({"type": "box", "id": "board", "params": {"x": "40 mm", "y": "30 mm", "z": "1.6 mm"},
               "placement": {"origin": ["0 mm", "0 mm", "-1.6 mm"]}}),
    );
    add(&mut s, "pcb", json!({"type": "sketch", "id": "drill"}));
    let mut ops = Vec::new();
    for (i, (x, y)) in [(4, 4), (36, 4), (4, 26), (36, 26)].into_iter().enumerate() {
        ops.push(json!({"op": "add", "type": "point", "at": [x, y]}));
        ops.push(json!({"op": "add", "type": "fix", "a": format!("p{}", i + 1)}));
    }
    must(
        &mut s,
        "sketch.edit",
        json!({"part": "pcb", "sketch": "drill", "ops": ops}),
    );
    add(
        &mut s,
        "pcb",
        json!({"type": "hole", "id": "holes", "sketch": "drill", "diameter": "3 mm",
               "depth": "through_all", "targets": ["board"]}),
    );
    (dir, s)
}

/// The body `inspect --feature <insert>` gives for inserted solid `key`.
fn inserted<'a>(inspect: &'a Value, key: &str) -> &'a Value {
    inspect["bodies"]
        .as_array()
        .unwrap()
        .iter()
        .find(|b| b["solid"] == key)
        .unwrap_or_else(|| panic!("no body {key} in {inspect}"))
}

/// The name of `body`'s planar face whose outward normal is `z` along the
/// z axis.
fn face(body: &Value, z: f64) -> String {
    body["faces"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| {
            let sign = if f["reversed"] == true { -1.0 } else { 1.0 };
            f["surface"]["type"] == "plane"
                && f["surface"]["frame"]["z"][2].as_f64().unwrap() * sign == z
        })
        .map(|f| f["name"].as_str().unwrap().to_owned())
        .unwrap()
}

/// The reference text of inserted solid `key`'s face whose outward normal
/// is `-z` (`bottom`) or `+z` (`top`), in part `part`.
fn face_named(inspect: &Value, part: &str, key: &str, which: &str) -> String {
    let z = if which == "bottom" { -1.0 } else { 1.0 };
    format!("{part}/{key}/{}", face(inserted(inspect, key), z))
}

/// The reference texts of the circle edges on inserted solid `key`'s
/// bottom face.
fn hole_edges(inspect: &Value, part: &str, key: &str) -> Vec<String> {
    let body = inserted(inspect, key);
    let bottom = face(body, -1.0);
    body["edges"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["curve"]["type"] == "circle")
        .filter(|e| e["faces"].as_array().unwrap().iter().any(|f| *f == bottom))
        .map(|e| format!("{part}/{key}/{}", e["name"].as_str().unwrap()))
        .collect()
}

#[test]
fn issue_41_an_enclosure_is_modelled_against_an_inserted_board() {
    let (_dir, mut s) = board();
    must(&mut s, "part.create", json!({"part": "enclosure"}));
    add(
        &mut s,
        "enclosure",
        json!({"type": "box", "id": "shell", "params": {"x": "44 mm", "y": "34 mm", "z": "10 mm"},
               "placement": {"origin": ["-2 mm", "-2 mm", "-2 mm"]}}),
    );
    add(
        &mut s,
        "enclosure",
        json!({"type": "insert", "id": "board", "of": "pcb"}),
    );
    // The board's bottom face, by the source's own name, and its four
    // hole edges projected into a sketch on it.
    let inspect = must(
        &mut s,
        "inspect",
        json!({"part": "enclosure", "feature": "board"}),
    );
    let bottom = face_named(&inspect, "enclosure", "board/board", "bottom");
    let holes = hole_edges(&inspect, "enclosure", "board/board");
    assert_eq!(holes.len(), 4, "{holes:?}");
    add(
        &mut s,
        "enclosure",
        json!({"type": "sketch", "id": "standoffs", "face": bottom}),
    );
    let ops: Vec<Value> = holes
        .iter()
        .map(|h| json!({"op": "add", "type": "circle", "projected": h}))
        .collect();
    must(
        &mut s,
        "sketch.edit",
        json!({"part": "enclosure", "sketch": "standoffs", "ops": ops}),
    );
    add(
        &mut s,
        "enclosure",
        json!({"type": "extrude", "id": "bores", "sketch": "standoffs",
               "extent": {"type": "through_all", "both": true}, "operation": "cut",
               "targets": ["shell"]}),
    );
    add(
        &mut s,
        "enclosure",
        json!({"type": "delete_body", "id": "drop-board", "solids": ["board"]}),
    );
    let r = must(&mut s, "rebuild", json!({"part": "enclosure"}));
    assert_eq!(r["parts"][0]["failed"], 0, "{r}");
    // One solid left: the shell, less four 3 mm bores through 10 mm.
    let solids = r["parts"][0]["solids"].as_array().unwrap();
    assert_eq!(solids.len(), 1, "{solids:?}");
    assert_eq!(solids[0]["solid"], "shell");
    let m = must(&mut s, "mass", json!({"of": "enclosure"}));
    let want = 0.044 * 0.034 * 0.010 - 4.0 * core::f64::consts::PI * 0.0015_f64.powi(2) * 0.010;
    let got = m["volume"]["m3"].as_f64().unwrap();
    assert!((got - want).abs() < 1e-6 * want, "{got} vs {want}");
    // check is clean.
    must(&mut s, "check", json!({"part": "enclosure"}));
}
