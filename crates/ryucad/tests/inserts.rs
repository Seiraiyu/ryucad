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
    let want = 0.044 * 0.034 * 0.010 - 4.0 * core::f64::consts::PI * 0.0015 * 0.0015 * 0.010;
    let got = m["volume"]["m3"].as_f64().unwrap();
    assert!((got - want).abs() < 1e-6 * want, "{got} vs {want}");
    // check is clean.
    must(&mut s, "check", json!({"part": "enclosure"}));
}

/// Runs git in `dir` with a fixed identity.
fn git(dir: &std::path::Path, args: &[&str]) {
    let st = std::process::Command::new("git")
        .args([
            "-c",
            "user.name=Test",
            "-c",
            "user.email=t@example.com",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        st.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&st.stderr)
    );
}

/// [`board`] in a Git repository with pcb released at A and committed.
fn released() -> (tempfile::TempDir, Session) {
    let (dir, mut s) = board();
    let r = dir.path();
    git(r, &["init", "-q", "-b", "main"]);
    must(
        &mut s,
        "props.set",
        json!({"part": "pcb", "properties": {"number": "100-0001"}}),
    );
    must(&mut s, "release.propose", json!({"part": "pcb"}));
    git(r, &["add", "."]);
    git(r, &["commit", "-q", "-m", "pcb A"]);
    s.reload().unwrap();
    (dir, s)
}

/// The hash of inserted solid `key` of `part` as its insert made it.
fn solid_hash(s: &mut Session, part: &str, key: &str) -> String {
    let (insert, _) = key.split_once('/').unwrap();
    let r = must(s, "rebuild", json!({"part": part}));
    let f = r["parts"][0]["features"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["id"] == insert)
        .unwrap_or_else(|| panic!("no {insert} in {r}"));
    f["insert"]["solids"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["key"] == key)
        .unwrap_or_else(|| panic!("no {key} in {f}"))["hash"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// The keys insert `insert` of `part` founded.
fn inserted_keys(s: &mut Session, part: &str, insert: &str) -> Vec<String> {
    let r = must(s, "rebuild", json!({"part": part}));
    let f = r["parts"][0]["features"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["id"] == insert)
        .unwrap_or_else(|| panic!("no {insert} in {r}"))
        .clone();
    f["insert"]["solids"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["key"].as_str().unwrap().to_owned())
        .collect()
}

/// The reference text of some edge of inserted solid `key` of `part`.
fn any_edge(s: &mut Session, part: &str, key: &str) -> String {
    let (insert, _) = key.split_once('/').unwrap();
    let inspect = must(s, "inspect", json!({"part": part, "feature": insert}));
    let body = inserted(&inspect, key);
    format!(
        "{part}/{key}/{}",
        body["edges"][0]["name"].as_str().unwrap()
    )
}

#[test]
fn a_working_insert_follows_its_source_and_a_label_doesnt_until_updated() {
    let (_dir, mut s) = released();
    must(&mut s, "part.create", json!({"part": "enc"}));
    let v = add(
        &mut s,
        "enc",
        json!({"type": "insert", "id": "at-a", "of": "pcb"}),
    );
    // Question 624: the newest official label.
    assert_eq!(v["took"]["rev"], json!({"label": "A"}), "{v}");
    add(
        &mut s,
        "enc",
        json!({"type": "insert", "id": "now", "of": "pcb", "rev": "working",
               "placement": {"origin": ["0 mm", "0 mm", "20 mm"]}}),
    );
    let before = solid_hash(&mut s, "enc", "at-a/board");
    let now_before = solid_hash(&mut s, "enc", "now/board");
    must(
        &mut s,
        "feature.edit",
        json!({"part": "pcb", "id": "board", "params": {"x": "50 mm"}}),
    );
    assert_eq!(solid_hash(&mut s, "enc", "at-a/board"), before);
    assert_ne!(solid_hash(&mut s, "enc", "now/board"), now_before);
    // The answer's pin is official at A's commit.
    let r = must(&mut s, "rebuild", json!({"part": "enc"}));
    assert_eq!(
        r["parts"][0]["features"][0]["insert"]["pin"]["official"], true,
        "{r}"
    );
}

#[test]
fn a_face_the_source_splits_is_followed_as_its_carrier() {
    let (_dir, mut s) = board();
    must(&mut s, "part.create", json!({"part": "enc"}));
    add(
        &mut s,
        "enc",
        json!({"type": "insert", "id": "b", "of": "pcb"}),
    );
    let inspect = must(&mut s, "inspect", json!({"part": "enc", "feature": "b"}));
    let top = face_named(&inspect, "enc", "b/board", "top");
    add(
        &mut s,
        "enc",
        json!({"type": "sketch", "id": "on-top", "face": top}),
    );
    // Split the board's top in the source: a slot across it.
    add(
        &mut s,
        "pcb",
        json!({"type": "box", "id": "slot", "operation": "cut", "targets": ["board"],
               "params": {"x": "2 mm", "y": "40 mm", "z": "1 mm"},
               "placement": {"origin": ["19 mm", "-5 mm", "-0.5 mm"]}}),
    );
    let r = must(&mut s, "rebuild", json!({"part": "enc"}));
    let sk = &r["parts"][0]["features"][1];
    assert_eq!(sk["ok"], true, "{sk}");
    assert_eq!(sk["warnings"][0]["code"], "reference_followed", "{sk}");
}

#[test]
fn a_flattened_reference_refuses_a_fillet_and_converted_is_a_boolean_tool() {
    // The OCCT assembly: several bodies, flattened as references.
    let (_dir, mut s) = board();
    let step = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../ryustep/tests/data/occt/assembly.stp");
    must(
        &mut s,
        "import",
        json!({"path": step.display().to_string(), "part": "vendor", "flatten": true}),
    );
    must(&mut s, "part.create", json!({"part": "enc"}));
    add(
        &mut s,
        "enc",
        json!({"type": "insert", "id": "v", "of": "vendor"}),
    );
    let keys = inserted_keys(&mut s, "enc", "v");
    assert!(keys.len() > 1, "{keys:?}");
    let edge = any_edge(&mut s, "enc", &keys[0]);
    let e = call(
        &mut s,
        "feature.add",
        json!({"part": "enc", "type": "fillet", "id": "f",
               "groups": [{"radius": "0.1 mm", "edges": [edge]}]}),
    )
    .unwrap_err();
    assert_eq!(e.code, ryucad::error::Code::ReferenceBody, "{e:?}");
    let source = keys[0].split_once('/').unwrap().1;
    assert_eq!(
        e.details["hint"],
        format!("ryucad import convert vendor/{source}"),
        "{e:?}"
    );
    must(
        &mut s,
        "import.convert",
        json!({"part": "vendor", "feature": source}),
    );
    add(
        &mut s,
        "enc",
        json!({"type": "box", "id": "base", "params": {"x": "500 mm", "y": "500 mm", "z": "500 mm"},
               "placement": {"origin": ["-250 mm", "-250 mm", "-250 mm"]}}),
    );
    add(
        &mut s,
        "enc",
        json!({"type": "boolean", "id": "b", "op": "subtract", "target": "base",
               "tools": [keys[0]]}),
    );
    let r = must(&mut s, "rebuild", json!({"part": "enc"}));
    assert_eq!(r["parts"][0]["failed"], 0, "{r}");
}

#[test]
fn two_inserts_of_one_part_and_nested_inserts_never_collide() {
    let (_dir, mut s) = board();
    must(&mut s, "part.create", json!({"part": "mid"}));
    add(
        &mut s,
        "mid",
        json!({"type": "insert", "id": "b", "of": "pcb"}),
    );
    must(&mut s, "part.create", json!({"part": "top"}));
    add(
        &mut s,
        "top",
        json!({"type": "insert", "id": "left", "of": "pcb"}),
    );
    add(
        &mut s,
        "top",
        json!({"type": "insert", "id": "right", "of": "pcb",
               "placement": {"origin": ["50 mm", "0 mm", "0 mm"]}}),
    );
    add(
        &mut s,
        "top",
        json!({"type": "insert", "id": "m", "of": "mid"}),
    );
    assert_eq!(inserted_keys(&mut s, "top", "m"), ["m/b/board"]);
    let r = must(&mut s, "rebuild", json!({"part": "top"}));
    let shown: Vec<&str> = r["parts"][0]["solids"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["solid"].as_str().unwrap())
        .collect();
    assert_eq!(shown, ["left/board", "right/board", "m/b/board"], "{r}");
}

#[test]
fn cycles_through_inserts_are_refused_at_write_and_rebuild() {
    let (_dir, mut s) = board();
    must(&mut s, "part.create", json!({"part": "a"}));
    add(
        &mut s,
        "a",
        json!({"type": "insert", "id": "p", "of": "pcb"}),
    );
    let e = call(
        &mut s,
        "feature.add",
        json!({"part": "pcb", "type": "insert", "of": "a"}),
    )
    .unwrap_err();
    assert_eq!(e.code, ryucad::error::Code::InsertCycle, "{e:?}");
    assert_eq!(e.details["path"], json!(["pcb", "a", "pcb"]), "{e:?}");
}

#[test]
fn a_failing_source_inserts_what_it_shows_and_warns() {
    let (dir, mut s) = board();
    must(&mut s, "part.create", json!({"part": "enc"}));
    add(
        &mut s,
        "enc",
        json!({"type": "insert", "id": "b", "of": "pcb"}),
    );
    // Break the hole by hand: it drills from a point that isn't there.
    let f = dir.path().join("parts/pcb/features/holes.json");
    let mut v: Value = serde_json::from_slice(&std::fs::read(&f).unwrap()).unwrap();
    v["points"] = json!(["p9"]);
    std::fs::write(&f, ryucad::doc::canonical::to_string(&v)).unwrap();
    s.reload().unwrap();
    let r = must(&mut s, "rebuild", json!({"part": "enc"}));
    let ins = &r["parts"][0]["features"][0];
    assert_eq!(ins["ok"], true, "{r}");
    assert_eq!(ins["warnings"][0]["code"], "insert_source_failed", "{ins}");
    assert_eq!(ins["warnings"][0]["failing"], json!(["holes"]), "{ins}");
}
