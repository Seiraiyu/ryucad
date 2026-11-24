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

/// Feature seed `written` built as the `feature` target builds it, on
/// one thread, and its solids checked as the target checks them: the
/// seconds it took.
fn rebuilt_and_checked(written: &str) -> f64 {
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap();
    pool.install(|| {
        let files = part_files(written);
        let project = ryucad::doc::project::Project::load_from(std::path::Path::new("/fz"), &files)
            .expect("the project loads");
        let pins = ryucad::assembly::pins(&project, |part, rev, _within| {
            Err(Error::new(
                Code::LabelNotFound,
                format!("{part} has no {}", rev.text()),
            ))
        });
        let start = std::time::Instant::now();
        let mut r = Rebuild::new(&project, &pins, Store::default());
        let result = r
            .result(&Id::parse("fz", "part").unwrap())
            .expect("the part rebuilds");
        let f = Id::parse("f", "feature").unwrap();
        if let Some((_, Ok(ryucad::rebuild::Output::Solids(m)))) =
            result.features.iter().find(|(i, _)| *i == f)
        {
            for (_, b) in m.solids.iter().filter(|(_, b)| !b.reference) {
                assert_eq!(ryukei::check::check(&b.body), []);
                for fault in ryukei::check::check_intersections(&b.body) {
                    assert!(
                        matches!(fault, ryukei::check::Fault::CannotCheck { .. }),
                        "{fault:?}"
                    );
                }
            }
        }
        start.elapsed().as_secs_f64()
    })
}

/// Whether the bars hold (`RYUCAD_BARS=1`, a quiet machine).
fn bars() -> bool {
    std::env::var_os("RYUCAD_BARS").is_some_and(|v| v == "1")
}

/// #919: the feature corpus seed `277051ee…`, a loft of `s` to a face of
/// `c`, took 8-10 s plain release (cox-worker-4, about 4 s of it the
/// target's one-time import), in general intersections. Built and
/// checked under 5 s on one thread under the bars.
#[test]
#[cfg_attr(debug_assertions, ignore = "release only")]
fn a_feature_seed_277051ee_rebuilds_quickly() {
    let seed =
        include_str!("../../../fuzz/corpus/feature/277051ee3f367b107acd9c3e6fc11a041bffb37d");
    let s = rebuilt_and_checked(seed);
    eprintln!("277051ee: {s:.1} s");
    assert!(!bars() || s < 5.0, "{s} s");
}

/// #919: the slow feature seed `e5a86bea…` (#851's loft cut) took 10-11
/// s plain release (cox-worker-4), in `general::Face::search` and the
/// trace's subdivision. Built and checked under 5 s on one thread under
/// the bars.
#[test]
#[cfg_attr(debug_assertions, ignore = "release only")]
fn a_feature_seed_e5a86bea_rebuilds_quickly() {
    let seed = include_str!("../../../fuzz/slow/feature/e5a86bea8e5ad78aa5eaf61942a195a74aa75f56");
    let s = rebuilt_and_checked(seed);
    eprintln!("e5a86bea: {s:.1} s");
    assert!(!bars() || s < 5.0, "{s} s");
}

/// #1071: the `feature` target's find in release 10.A's dispatch (run
/// 45392447398, `crash-46ffea2a…`): a spotface hole whose near chamfer
/// is as wide as its spotface. The chamfer's corner fell on the
/// spotface floor's, and the tool's profile panicked on the empty
/// segment between them (`hole.rs`, "a hole's corners are apart"). A
/// near chamfer must sit inside the spotface: refused typed, at
/// `/near/diameter`.
#[test]
fn a_spotface_as_wide_as_its_near_chamfer_is_refused() {
    let seed = r#"{"format":12,"kind":"ryucad.feature","type":"hole","sketch":"t","form":"spotface","diameter":"2 mm","spotface":{"diameter":"3 mm","depth":"0.5 mm"},"near":{"diameter":"3 mm"},"depth":"through_all","targets":["b"]}"#;
    let files = part_files(seed);
    let project = ryucad::doc::project::Project::load_from(std::path::Path::new("/fz"), &files)
        .expect("the project loads");
    let pins = ryucad::assembly::pins(&project, |part, rev, _within| {
        Err(Error::new(
            Code::LabelNotFound,
            format!("{part} has no {}", rev.text()),
        ))
    });
    let mut r = Rebuild::new(&project, &pins, Store::default());
    let result = r
        .result(&Id::parse("fz", "part").unwrap())
        .expect("the part rebuilds");
    let f = Id::parse("f", "feature").unwrap();
    let (_, out) = result
        .features
        .iter()
        .find(|(i, _)| *i == f)
        .expect("feature f");
    let Err(e) = out else {
        panic!("a spotface as wide as its near chamfer built");
    };
    assert_eq!(e.code, Code::InvalidParam, "{e:?}");
    assert_eq!(e.details["pointer"], "/near/diameter", "{e:?}");
}

/// Part `fz`, the base, then `written` as its feature `f`.
fn part_files(written: &str) -> BTreeMap<String, Vec<u8>> {
    let mut files = project_files(written);
    files.retain(|k, _| !k.starts_with("parts/asm/") && !k.starts_with("parts/sub/"));
    let mut list: Vec<Id> = BASE
        .iter()
        .map(|(i, _)| Id::parse(i, "feature").unwrap())
        .collect();
    list.push(Id::parse("f", "feature").unwrap());
    files.insert(
        "parts/fz/part.json".to_owned(),
        canonical::to_string(&PartFile::new(list).to_json()).into_bytes(),
    );
    files.insert(
        "parts/fz/features/f.json".to_owned(),
        canonical::to_string(&canonical::parse(written).unwrap()).into_bytes(),
    );
    files
}

/// #851's loft `e5a86bea…`: a cut lofted from sketch `s` to a face of
/// `b`. Under ASan its rebuild ran 806 s, 356 s past the `feature`
/// target's 450 s deadline, in the loft's self-check and the cut's
/// boolean: face pairs along rims compared every pair of their edges
/// with nothing to stop them (plan 30.D). A cancel at any point of the
/// rebuild must end it within `LATENCY` in release.
#[test]
#[cfg_attr(debug_assertions, ignore = "release only")]
fn the_loft_851_stops_soon_after_a_cancel() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    const LATENCY: f64 = 0.3;
    let seed = r#"{"format":8,"kind":"ryucad.feature","type":"loft","sections":[{"sketch":"s"},{"face":{"feature":"b","name":"4dea6ff41f82679b674048865c452961"}}],"end_tangent":{"direction":"y"},"operation":"cut","targets":["b"]}"#;
    let files = part_files(seed);
    let project = ryucad::doc::project::Project::load_from(std::path::Path::new("/fz"), &files)
        .expect("the project loads");
    let part = Id::parse("fz", "part").unwrap();
    let pins = ryucad::assembly::pins(&project, |part, rev, _within| {
        Err(Error::new(
            Code::LabelNotFound,
            format!("{part} has no {}", rev.text()),
        ))
    });
    // Uncancelled: how long the whole rebuild takes.
    let start = std::time::Instant::now();
    let mut r = Rebuild::new(&project, &pins, Store::default());
    r.result(&part).expect("the part rebuilds");
    let whole = start.elapsed().as_secs_f64();
    eprintln!("whole rebuild {whole:.2} s");
    let mut worst: f64 = 0.0;
    for k in 1..=16 {
        let at = whole * f64::from(k) / 17.0;
        let flag = Arc::new(AtomicBool::new(false));
        let mut r = Rebuild::new(&project, &pins, Store::default());
        r.job = ryucad::motion::Job::new("t", "t", "rebuild", Arc::clone(&flag));
        let fired = std::thread::scope(|s| {
            let f = Arc::clone(&flag);
            let timer = s.spawn(move || {
                std::thread::sleep(std::time::Duration::from_secs_f64(at));
                f.store(true, Ordering::SeqCst);
                std::time::Instant::now()
            });
            let _ = r.result(&part);
            let end = std::time::Instant::now();
            let fired = timer.join().unwrap();
            end.checked_duration_since(fired)
        });
        if let Some(late) = fired {
            let late = late.as_secs_f64();
            eprintln!("cancel at {at:.2} s: stopped {late:.3} s later");
            worst = worst.max(late);
        }
    }
    assert!(
        worst <= LATENCY,
        "a cancel took {worst:.3} s to stop the rebuild"
    );
}
