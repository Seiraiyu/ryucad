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
    must(
        &mut s,
        "part.create",
        json!({"part": "rig", "kind": "assembly"}),
    );
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
    let v = must(
        &mut s,
        "import.convert",
        json!({"part": "p", "feature": "r"}),
    );
    assert_eq!(v["converted"], json!(["r"]), "{v}");
    // Every face keeps its name; the sketch and the mate still build.
    assert_eq!(names(&mut s), before);
    let p = must(&mut s, "rebuild", json!({"part": "p"}));
    assert_eq!(p["parts"][0]["failed"], 0, "{p}");
    let rig = must(&mut s, "rebuild", json!({"part": "rig"}));
    assert_eq!(rig["parts"][0]["failed"], 0, "{rig}");
}

/// The path of the committed corpus's file `name` (ryustep's).
fn corpus(name: &str) -> String {
    format!(
        "{}/../ryustep/tests/corpus/{name}",
        env!("CARGO_MANIFEST_DIR")
    )
}

#[test]
fn check_warns_of_a_reference_and_never_runs_the_intersection_check_on_it() {
    let dir = tempfile::tempdir().unwrap();
    ryucad::server::init(dir.path(), None).unwrap();
    let mut s = Session::open(dir.path()).unwrap();
    // NIST CTC 01's body heals and checks clean as a solid; as a reference
    // it must check without findings that fail.
    let v = must(
        &mut s,
        "import",
        json!({"path": corpus("nist_ctc_01_asme1_rd.stp"), "part": "q"}),
    );
    assert_eq!(v["parts"][0]["features"][0]["mode"], "reference", "{v}");
    let v = must(&mut s, "check", json!({"part": "q"}));
    assert_eq!(v["passed"], true, "{v}");
    for f in v["findings"].as_array().unwrap() {
        assert_eq!(f["severity"], "warning", "{f}");
        assert!(f["code"].as_str().unwrap().starts_with("reference_"), "{f}");
    }
}

#[test]
fn references_interfere_on_their_meshes() {
    let (_dir, mut s) = project(); // r: OCCT's box as a reference; b: a 10 mm box
    must(
        &mut s,
        "part.create",
        json!({"part": "rig", "kind": "assembly"}),
    );
    for id in ["a", "c"] {
        must(
            &mut s,
            "feature.add",
            json!({"part": "rig", "type": "instance", "id": id, "of": "p", "fixed": true}),
        );
    }
    // Two copies of one part standing in each other: every pair overlaps,
    // and those with a reference side are judged on their meshes.
    let v = must(&mut s, "interference", json!({"part": "rig"}));
    assert_eq!(v["unchecked"], json!([]), "{v}");
    let approximate: Vec<&serde_json::Value> = v["interferences"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["approximate"] == true)
        .collect();
    assert!(!approximate.is_empty(), "{v}");
    for i in &approximate {
        assert!(i["volume_m3"].is_null(), "{v}");
        assert!(!i["faces"]["a"].as_array().unwrap().is_empty(), "{v}");
        assert!(!i["faces"]["b"].as_array().unwrap().is_empty(), "{v}");
    }
    // The reference against its own copy, its surfaces on the copy's: the
    // whole volume is shared.
    assert!(
        approximate.iter().any(|i| {
            let (a, b) = (i["a"].as_str().unwrap(), i["b"].as_str().unwrap());
            a.ends_with("/r") && b.ends_with("/r")
        }),
        "{v}"
    );
    // The solid pair is still a boolean's, with its volume.
    assert!(
        v["interferences"]
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["volume_m3"].is_f64()),
        "{v}"
    );
    let v = must(&mut s, "check", json!({"part": "rig"}));
    assert!(
        v["findings"]
            .as_array()
            .unwrap()
            .iter()
            .all(|f| f["code"] != "cannot_check"),
        "{v}"
    );
    // Moved a metre off, nothing is paired.
    must(
        &mut s,
        "feature.edit",
        json!({"part": "rig", "id": "c", "placement": {"origin": [1000, 0, 0]}}),
    );
    let v = must(&mut s, "interference", json!({"part": "rig"}));
    assert_eq!(v["interferences"], json!([]), "{v}");
    assert_eq!(v["unchecked"], json!([]), "{v}");
}

#[test]
fn check_and_clearance_read_reference_pairs() {
    let (_dir, mut s) = project();
    must(
        &mut s,
        "part.create",
        json!({"part": "rig", "kind": "assembly"}),
    );
    for id in ["a", "c"] {
        must(
            &mut s,
            "feature.add",
            json!({"part": "rig", "type": "instance", "id": id, "of": "p", "fixed": true}),
        );
    }
    let v = must(&mut s, "check", json!({"part": "rig"}));
    let f = v["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["code"] == "instances_interfere" && f["approximate"] == true)
        .cloned()
        .unwrap_or_else(|| panic!("{v}"));
    assert!(f["volume_m3"].is_null(), "{f}");
    assert!(
        f["message"]
            .as_str()
            .unwrap()
            .contains("judged on their meshes"),
        "{f}"
    );
    let v = must(&mut s, "clearance", json!({"part": "rig"}));
    assert_eq!(v["unchecked"], json!([]), "{v}");
}

#[test]
fn a_reference_measures_as_its_faces_and_its_area() {
    let (_dir, mut s) = project(); // r: OCCT's box as a reference; b: a 10 mm box
    let v = must(&mut s, "measure", json!({"entities": ["p/r"]}));
    assert_eq!(v["reference"], true, "{v}");
    let area = v["area"]["m2"].as_f64().unwrap();
    assert!(area > 0.0, "{v}");
    assert!(v.get("volume").is_none(), "{v}");
    // Two solids of an occurrence, one a reference: areas summed, the
    // volume the solid's alone, said so.
    must(
        &mut s,
        "part.create",
        json!({"part": "rig", "kind": "assembly"}),
    );
    must(
        &mut s,
        "feature.add",
        json!({"part": "rig", "type": "instance", "id": "a", "of": "p", "fixed": true}),
    );
    let both = must(&mut s, "measure", json!({"entities": ["rig/a"]}));
    assert!(both["area"]["m2"].as_f64().unwrap() > area, "{both}");
    assert!(
        (both["volume"]["m3"].as_f64().unwrap() - 1e-6).abs() < 1e-15,
        "{both}"
    );
    assert_eq!(both["references"], 1, "{both}");
    // A face's shape, and a distance to the box.
    let top = plane_face(&mut s, "r");
    let f = must(
        &mut s,
        "measure",
        json!({"entities": [format!("p/r/{top}")]}),
    );
    assert_eq!(f["shape"]["type"], "plane", "{f}");
    must(
        &mut s,
        "measure",
        json!({"entities": [format!("p/r/{top}"), "p/b"]}),
    );
}

#[test]
fn a_closed_reference_weighs_from_its_mesh_and_an_open_one_is_refused() {
    let (_dir, mut s) = project();
    must(
        &mut s,
        "materials.set",
        json!({"name": "steel", "density": "7850 kg/m^3", "default": true}),
    );
    let m = must(&mut s, "mass", json!({"of": "p/r"}));
    assert_eq!(m["reference"], true, "{m}");
    // Within the reference's bound of the healed solid's volume.
    let healed = must(
        &mut s,
        "import.convert",
        json!({"part": "p", "feature": "r"}),
    );
    assert_eq!(healed["converted"], json!(["r"]));
    let h = must(&mut s, "mass", json!({"of": "p/r"}));
    assert!(h.get("reference").is_none(), "{h}");
    let (rv, rb) = (
        m["volume"]["m3"].as_f64().unwrap(),
        m["volume"]["bound_m3"].as_f64().unwrap(),
    );
    let hv = h["volume"]["m3"].as_f64().unwrap();
    assert!((rv - hv).abs() <= rb, "{m} {h}");
    // An open reference: a NIST PMI surface model's sheet.
    let dir = tempfile::tempdir().unwrap();
    ryucad::server::init(dir.path(), None).unwrap();
    let mut o = Session::open(dir.path()).unwrap();
    let v = must(
        &mut o,
        "import",
        json!({"path": corpus("nist_ctc_04_asme1_rd.stp"), "part": "o"}),
    );
    let sheet = v["parts"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|p| {
            let part = p["part"].as_str().unwrap().to_owned();
            p["features"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|f| f["reference"]["laminar"].as_u64().unwrap_or(0) > 0)
                .map(move |f| format!("{part}/{}", f["id"].as_str().unwrap()))
                .collect::<Vec<_>>()
        })
        .next()
        .unwrap_or_else(|| panic!("no open reference in {v}"));
    let e = call(&mut o, "mass", json!({"of": sheet})).unwrap_err();
    assert_eq!(e.code, ryucad::error::Code::ReferenceNotSolid, "{e:?}");
    assert_eq!(e.details["reason"], "open");
}

/// The box of a binary STL's points.
fn stl_box(bytes: &[u8]) -> ([f32; 3], [f32; 3]) {
    let n = u32::from_le_bytes(bytes[80..84].try_into().unwrap()) as usize;
    let (mut lo, mut hi) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
    for t in 0..n {
        let at = 84 + 50 * t + 12;
        for v in 0..9 {
            let o = at + 4 * v;
            let x = f32::from_le_bytes(bytes[o..o + 4].try_into().unwrap());
            lo[v % 3] = lo[v % 3].min(x);
            hi[v % 3] = hi[v % 3].max(x);
        }
    }
    (lo, hi)
}

#[test]
fn a_flattened_part_exports_and_reads_back_where_it_stands() {
    let dir = tempfile::tempdir().unwrap();
    ryucad::server::init(dir.path(), None).unwrap();
    let mut s = Session::open(dir.path()).unwrap();
    let src = tempfile::tempdir().unwrap();
    let path = src.path().join("asm.step");
    std::fs::write(&path, occt("assembly.stp")).unwrap();
    let path = path.display().to_string();
    must(
        &mut s,
        "import",
        json!({"path": path, "flatten": true, "part": "board"}),
    );
    // As imported whole: the assembly's occurrences, where the file puts them.
    must(&mut s, "import", json!({"path": path, "part": "whole"}));
    let stl = |s: &mut Session, part: &str| {
        let out = src.path().join(format!("{part}.stl"));
        must(
            s,
            "export",
            json!({"part": part, "out": out.display().to_string(), "unit": "m"}),
        );
        stl_box(&std::fs::read(&out).unwrap())
    };
    let near = |a: ([f32; 3], [f32; 3]), b: ([f32; 3], [f32; 3])| {
        (0..3).all(|k| (a.0[k] - b.0[k]).abs() < 1e-6 && (a.1[k] - b.1[k]).abs() < 1e-6)
    };
    let board = stl(&mut s, "board");
    let whole = stl(&mut s, "whole");
    assert!(near(board, whole), "{board:?} {whole:?}");
    // Through STEP and back: each placed reference is a child product at
    // its place, so what is read back stands where the part does.
    let out = src.path().join("board.step");
    must(
        &mut s,
        "export",
        json!({"part": "board", "out": out.display().to_string()}),
    );
    must(
        &mut s,
        "import",
        json!({"path": out.display().to_string(), "part": "again"}),
    );
    let again = stl(&mut s, "again");
    assert!(near(board, again), "{board:?} {again:?}");
}
