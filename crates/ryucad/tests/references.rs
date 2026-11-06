//! Reference imports (design 364), through the protocol: every feature
//! that would change a reference is refused `reference_body`; features
//! that read one work.

use ryucad::doc::id::Id;
use ryucad::doc::project::{ImportFile, feature_path, part_path, write_atomic, write_json};
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

fn add(s: &mut Session, f: Value) -> ryucad::error::Result<Value> {
    let mut f = f;
    f["part"] = json!("p");
    call(s, "feature.add", f)
}

/// The bytes of OCCT's file `name` (ryustep's test data).
fn occt(name: &str) -> Vec<u8> {
    let path = format!(
        "{}/../ryustep/tests/data/occt/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read(path).unwrap()
}

/// Writes `bytes` as `imports/<feature>.step` and an import feature
/// `feature` of product `product`'s first body (none: the first no product
/// reaches) against it, in `mode` (none: a solid), listed last in `part`.
fn write_import(
    s: &mut Session,
    part: &str,
    feature: &str,
    (bytes, product): (&[u8], Option<&str>),
    mode: Option<&str>,
) {
    let root = s.root().to_owned();
    let path = format!("imports/{feature}.step");
    write_atomic(&root.join(&path), bytes).unwrap();
    let file = ImportFile::new(bytes.to_vec());
    let mut f = json!({"body": 0, "file": {"blake3": file.blake3, "path": path},
                       "format": 5, "kind": "ryucad.feature", "type": "import"});
    if let Some(p) = product {
        f["product"] = json!({"id": p, "name": p});
    }
    if let Some(m) = mode {
        f["mode"] = json!(m);
        f["format"] = json!(13);
    }
    write_feature(s, part, feature, f);
}

/// Writes feature `feature` of `part` as `v`, listed last.
fn write_feature(s: &mut Session, part: &str, feature: &str, v: Value) {
    let root = s.root().to_owned();
    let p = Id::parse(part, "part").unwrap();
    let fid = Id::parse(feature, "feature").unwrap();
    write_json(&feature_path(&root, &p, &fid), &v).unwrap();
    let text = std::fs::read_to_string(part_path(&root, &p)).unwrap();
    let mut list: Value = serde_json::from_str(&text).unwrap();
    let features = list["features"].as_array_mut().unwrap();
    if !features.iter().any(|f| f == feature) {
        features.push(json!(feature));
    }
    write_json(&part_path(&root, &p), &list).unwrap();
    s.reload().unwrap();
}

/// The faces of `solid` (an `inspect` of the part's model), by name.
fn faces(s: &mut Session, solid: &str) -> Vec<Value> {
    let v = must(s, "inspect", json!({"part": "p"}));
    v["bodies"]
        .as_array()
        .unwrap()
        .iter()
        .find(|b| b["solid"] == solid)
        .unwrap_or_else(|| panic!("no {solid} in {v}"))["faces"]
        .as_array()
        .unwrap()
        .clone()
}

/// A planar face of `solid`, by name.
fn plane_face(s: &mut Session, solid: &str) -> String {
    faces(s, solid)
        .into_iter()
        .find(|f| f["surface"]["type"] == "plane")
        .unwrap_or_else(|| panic!("no plane in {:#}", faces(s, solid)[0]))["name"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// An edge of `solid`, by name.
fn any_edge(s: &mut Session, solid: &str) -> String {
    let v = must(s, "inspect", json!({"part": "p"}));
    let b = v["bodies"]
        .as_array()
        .unwrap()
        .iter()
        .find(|b| b["solid"] == solid)
        .unwrap()
        .clone();
    b["edges"][0]["name"].as_str().unwrap().to_owned()
}

/// A project with `p`'s reference import `r` of OCCT's box
/// (`box_ap214.stp`, 6 faces) and a 10 mm box primitive `b` beside it.
fn project() -> (tempfile::TempDir, Session) {
    let dir = tempfile::tempdir().unwrap();
    ryucad::server::init(dir.path(), None).unwrap();
    let mut s = Session::open(dir.path()).unwrap();
    must(&mut s, "part.create", json!({"part": "p"}));
    let bytes = occt("box_ap214.stp");
    write_import(
        &mut s,
        "p",
        "r",
        (&bytes, Some("part 2")),
        Some("reference"),
    );
    add(
        &mut s,
        json!({"type": "box", "id": "b", "params": {"x": 10, "y": 10, "z": 10}}),
    )
    .unwrap();
    (dir, s)
}

#[test]
fn features_that_change_a_reference_are_refused() {
    let (_dir, mut s) = project();
    let r = call(&mut s, "rebuild", json!({"part": "p"}))
        .unwrap_or_else(|e| panic!("{}", serde_json::to_string_pretty(&e.details).unwrap()));
    assert_eq!(r["parts"][0]["solids"][0]["reference"], true, "{r}");
    let top = plane_face(&mut s, "r");
    let edge = any_edge(&mut s, "r");
    for (f, role) in [
        (
            json!({"type": "box", "id": "c", "operation": "cut", "targets": ["r"],
                   "params": {"x": 1, "y": 1, "z": 1}}),
            "targets",
        ),
        (
            json!({"type": "boolean", "id": "u", "op": "unite", "target": "b", "tools": ["r"]}),
            "tools",
        ),
        (
            json!({"type": "fillet", "id": "f", "radius": 0.1, "edges": [format!("p/r/{edge}")]}),
            "targets",
        ),
        (
            json!({"type": "shell", "id": "sh", "target": "r", "thickness": 0.1}),
            "targets",
        ),
        (
            json!({"type": "offset", "id": "o", "target": "r", "distance": 0.1}),
            "targets",
        ),
        (
            json!({"type": "split", "id": "sp", "target": "r", "plane": "xy"}),
            "targets",
        ),
        (
            json!({"type": "mirror", "id": "m", "solid": "r", "plane": "yz", "operation": "new"}),
            "solid",
        ),
    ] {
        let e = add(&mut s, f.clone()).unwrap_err();
        assert_eq!(e.code, ryucad::error::Code::ReferenceBody, "{f}: {e:?}");
        assert_eq!(e.details["solid"], "r");
        assert_eq!(e.details["hint"], "ryucad import convert p/r");
        assert_eq!(e.details["role"], role, "{f}");
    }
    // Reading its geometry works: a sketch on its face, a mirror of b in
    // its face's plane.
    add(
        &mut s,
        json!({"type": "sketch", "id": "s", "face": format!("p/r/{top}")}),
    )
    .unwrap();
    add(
        &mut s,
        json!({"type": "mirror", "id": "m", "solid": "b", "plane": format!("p/r/{top}"),
               "operation": "new"}),
    )
    .unwrap();
    // A file written by hand past the write's check fails at rebuild too:
    // b's box, made a cut of r.
    let root = s.root().to_owned();
    let b = feature_path(
        &root,
        &Id::parse("p", "part").unwrap(),
        &Id::parse("b", "feature").unwrap(),
    );
    let mut cut: Value = serde_json::from_str(&std::fs::read_to_string(b).unwrap()).unwrap();
    cut["operation"] = json!("cut");
    cut["targets"] = json!(["r"]);
    cut["format"] = json!(3);
    write_feature(&mut s, "p", "c", cut);
    let e = call(&mut s, "rebuild", json!({"part": "p"})).unwrap_err();
    let c = e.details["parts"][0]["features"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["id"] == "c")
        .unwrap()
        .clone();
    assert_eq!(c["error"]["code"], "reference_body", "{c}");
}

#[test]
fn a_mate_and_a_sketch_on_a_reference_survive_its_conversion() {
    let (_dir, mut s) = project();
    let top = plane_face(&mut s, "r");
    add(
        &mut s,
        json!({"type": "sketch", "id": "s", "face": format!("p/r/{top}")}),
    )
    .unwrap();
    must(&mut s, "part.create", json!({"part": "rig", "kind": "assembly"}));
    for (id, fixed) in [("a", true), ("c", false)] {
        must(
            &mut s,
            "feature.add",
            json!({"part": "rig", "type": "instance", "id": id, "of": "p", "fixed": fixed}),
        );
    }
    must(
        &mut s,
        "feature.add",
        json!({"part": "rig", "type": "mate", "form": "planar",
               "a": format!("rig/a/r/{top}"), "b": format!("rig/c/r/{top}")}),
    );
    let names = |s: &mut Session| -> std::collections::BTreeSet<String> {
        faces(s, "r")
            .iter()
            .map(|f| f["name"].as_str().unwrap().to_owned())
            .collect()
    };
    let before = names(&mut s);
    let v = must(&mut s, "import.convert", json!({"part": "p", "feature": "r"}));
    assert_eq!(v["converted"], json!(["r"]), "{v}");
    // Every face keeps its name; the sketch and the mate still build.
    assert_eq!(names(&mut s), before);
    let p = must(&mut s, "rebuild", json!({"part": "p"}));
    assert_eq!(p["parts"][0]["failed"], 0, "{p}");
    let rig = must(&mut s, "rebuild", json!({"part": "rig"}));
    assert_eq!(rig["parts"][0]["failed"], 0, "{rig}");
}
