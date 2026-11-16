//! Inputs the `feature` fuzz target found slow, replayed in release
//! through the calls the target makes (design 600, plan 30.D). The base
//! part is the target's own, `base()` in `fuzz/fuzz_targets/feature.rs`:
//! keep the two alike.

use ryucad::assembly::{Rebuild, Store};
use ryucad::doc::canonical;
use ryucad::doc::files::{Feature, PartFile};
use ryucad::doc::id::Id;
use ryucad::error::{Code, Error};
use ryucad::expr::Env;
use ryucad::units::Unit;
use serde_json::json;
use std::collections::BTreeMap;

/// What a release run may take before it counts as the runaway it was.
const RUNAWAY: f64 = 300.0;

/// The target's base part `fz`, in its order: boxes `b` and `c`, the
/// sketches `s`, `t` and `u`, the pocket `k` and the bent section `w`.
const BASE: [(&str, &str); 7] = [
    (
        "b",
        r#"{"format": 1, "kind": "ryucad.feature", "type": "box",
            "params": {"x": "20 mm", "y": "10 mm", "z": "5 mm"},
            "placement": {"origin": ["0 mm", "0 mm", "0 mm"],
                          "x": [1, 0, 0], "z": [0, 0, 1]}}"#,
    ),
    (
        "c",
        r#"{"format": 1, "kind": "ryucad.feature", "type": "box",
            "params": {"x": "8 mm", "y": "8 mm", "z": "8 mm"},
            "placement": {"origin": ["16 mm", "4 mm", "2 mm"],
                          "x": [1, 0, 0], "z": [0, 0, 1]}}"#,
    ),
    (
        "s",
        r#"{"format": 2, "kind": "ryucad.feature", "type": "sketch", "plane": "zx",
            "constraints": {},
            "entities": {
                "p1": {"type": "point", "m": [0.002, 0.002]},
                "p2": {"type": "point", "m": [0.008, 0.002]},
                "p3": {"type": "point", "m": [0.008, 0.006]},
                "p4": {"type": "point", "m": [0.002, 0.006]},
                "l1": {"type": "line", "start": "p1", "end": "p2"},
                "l2": {"type": "line", "start": "p2", "end": "p3"},
                "l3": {"type": "line", "start": "p3", "end": "p4"},
                "l4": {"type": "line", "start": "p4", "end": "p1"}}}"#,
    ),
    (
        "t",
        r#"{"format": 2, "kind": "ryucad.feature", "type": "sketch",
            "placement": {"origin": ["0 mm", "0 mm", "5 mm"],
                          "x": [1, 0, 0], "z": [0, 0, 1]},
            "constraints": {},
            "entities": {"p1": {"type": "point", "m": [0.005, 0.005]}}}"#,
    ),
    (
        "u",
        r#"{"format": 2, "kind": "ryucad.feature", "type": "sketch", "plane": "xy",
            "constraints": {},
            "entities": {
                "p1": {"type": "point", "m": [0.04, 0.0]},
                "p2": {"type": "point", "m": [0.03, 0.0]},
                "p3": {"type": "point", "m": [0.03, 0.01]},
                "l1": {"type": "line", "start": "p1", "end": "p2"},
                "l2": {"type": "line", "start": "p2", "end": "p3"}}}"#,
    ),
    (
        "k",
        r#"{"format": 3, "kind": "ryucad.feature", "type": "box",
            "params": {"x": "2 mm", "y": "2 mm", "z": "2 mm"},
            "placement": {"origin": ["9 mm", "4 mm", "4 mm"],
                          "x": [1, 0, 0], "z": [0, 0, 1]},
            "operation": "cut", "targets": ["b"]}"#,
    ),
    (
        "w",
        r#"{"format": 11, "kind": "ryucad.feature", "type": "base_flange",
            "sketch": "u", "thickness": "1.5 mm", "radius": "1 mm",
            "depth": "20 mm", "allowance": {"k_factor": "0.44"}}"#,
    ),
];

/// The project the target builds for a mate or a pattern `written`: part
/// `fz`, assembly `asm` placing `i1` (free, 20 mm up) and `i2` (fixed)
/// and then `f`, and assembly `sub` placing `asm` 10 mm up.
fn project_files(written: &str) -> BTreeMap<String, Vec<u8>> {
    let mut files = BTreeMap::new();
    let mut put = |path: &str, v: serde_json::Value| {
        files.insert(path.to_owned(), canonical::to_string(&v).into_bytes());
    };
    put(
        "ryucad.json",
        json!({"default_unit": "mm", "format": 1, "kind": "ryucad.project"}),
    );
    put(
        "parameters.json",
        json!({"format": 1, "kind": "ryucad.parameters",
               "parameters": {"wall": "3 mm", "gap": "wall / 2"}}),
    );
    put(
        "parts/fz/part.json",
        PartFile::new(
            BASE.iter()
                .map(|(i, _)| Id::parse(i, "feature").unwrap())
                .collect(),
        )
        .to_json(),
    );
    for (i, text) in BASE {
        let f =
            Feature::from_json(&canonical::parse(text).unwrap(), Unit::Mm, &Env::empty()).unwrap();
        put(&format!("parts/fz/features/{i}.json"), f.to_json());
    }
    let assembly = |list: &[&str]| json!({"features": list, "format": 2, "kind": "ryucad.part", "part_kind": "assembly"});
    put("parts/asm/part.json", assembly(&["i1", "i2", "f"]));
    for (i, z, fixed) in [("i1", 0.02, false), ("i2", 0.0, true)] {
        put(
            &format!("parts/asm/features/{i}.json"),
            json!({"format": 4, "kind": "ryucad.feature", "type": "instance",
                   "part": "fz", "rev": "working", "fixed": fixed,
                   "pose": {"origin_m": [0, 0, z], "z": [0, 0, 1], "x": [1, 0, 0]}}),
        );
    }
    put(
        "parts/asm/features/f.json",
        canonical::parse(written).unwrap(),
    );
    put("parts/sub/part.json", assembly(&["a"]));
    put(
        "parts/sub/features/a.json",
        json!({"format": 4, "kind": "ryucad.feature", "type": "instance",
               "part": "asm", "rev": "working",
               "pose": {"origin_m": [0, 0, 0.01], "z": [0, 0, 1], "x": [1, 0, 0]}}),
    );
    files
}

/// #769: the committed seed `bffe36ee…` (ryucad 5.D), a revolute mate
/// of `i2` 5 mm along x to `i1` at 12°. Replayed in the fuzz build on
/// cox-worker-4 under load it ran 1,756 s, all of it in
/// `interference::find_for`'s pairs. Both assemblies must rebuild, and
/// their interference must be found the same from what it cached and
/// from nothing, well inside the runaway it was.
#[test]
#[cfg_attr(debug_assertions, ignore = "release only")]
fn the_revolute_mate_769_finds_its_interference() {
    let seed = r#"{"a": {"at": "origin", "instance": ["i2"],
                         "offset": {"origin": ["5 mm", "0 mm", "0 mm"]}},
                   "angle": "12 deg",
                   "b": {"at": "origin", "instance": ["i1"]},
                   "form": "revolute", "format": 4, "kind": "ryucad.feature", "type": "mate"}"#;
    // As the target writes it: read, then written canonically.
    let f = Feature::from_json(&canonical::parse(seed).unwrap(), Unit::Mm, &Env::empty())
        .expect("a mate");
    let files = project_files(&canonical::to_string(&f.to_json()));
    let project = ryucad::doc::project::Project::load_from(std::path::Path::new("/fz"), &files)
        .expect("the project loads");
    let pins = ryucad::assembly::pins(&project, |part, rev, _within| {
        Err(Error::new(
            Code::LabelNotFound,
            format!("{part} has no {}", rev.text()),
        ))
    });
    let mut r = Rebuild::new(&project, &pins, Store::default());
    let start = std::time::Instant::now();
    for asm in ["asm", "sub"] {
        let part = Id::parse(asm, "part").unwrap();
        r.result(&part).expect("assemblies rebuild");
        let model = r
            .model(&part, &ryucad::doc::assembly::Rev::Working)
            .expect("its model");
        let find = |cache: &mut ryucad::interference::Cache| {
            ryucad::interference::find(&part, &model, &[], cache).expect("interference")
        };
        let mut cache = ryucad::interference::Cache::default();
        let v = find(&mut cache);
        assert_eq!(find(&mut cache), v, "from what it cached");
        assert_eq!(find(&mut Default::default()), v, "from nothing");
        eprintln!(
            "{asm}: {} interferences, {} unchecked, at {:.1} s",
            v["interferences"].as_array().map_or(0, Vec::len),
            v["unchecked"].as_array().map_or(0, Vec::len),
            start.elapsed().as_secs_f64()
        );
    }
    let s = start.elapsed().as_secs_f64();
    assert!(s < RUNAWAY, "{s} s");
}
