//! Feature files from bytes. Any text must either load to a feature that
//! writes back canonically and reads back equal, or give a structured
//! error. Loaded, it is built as feature `f` of a part after boxes `b`
//! and `c`, a rectangle sketch `s`, a sketch `t` on `b`'s top with a lone
//! point and a pocket `k` cut from `b`'s top, so its references resolve
//! against real faces, edges and vertices, its operations act on real
//! solids, its fillets and chamfers (seeded with `b`'s edges) round real
//! edges, its holes drill at `t`'s point and its patterns repeat `k`:
//! building must succeed or give a structured error, every body it leaves
//! must pass the checks, cross or overlap itself nowhere and mesh with no
//! faults, and a sketch that solves must solve to finite
//! positions that are current: solving them again, in the same context,
//! changes nothing. An instance is built instead as feature `f` of an
//! assembly `asm` in a project holding that part as `fz` and an assembly
//! `sub` placing `asm`: it must place its part or give a structured error,
//! never an internal one, and every occurrence must be finite and sound.
//! A mate or an instance pattern is built the same way, after instances
//! `i1` (free) and `i2` (fixed) of `fz`: what it solves and places must be
//! finite and sound too. Each assembly's interference is found, and
//! found again with nothing cached: the same report, every volume and
//! box finite. Any panic is a bug.

#![no_main]

use libfuzzer_sys::fuzz_target;
use ryucad::doc::canonical;
use ryucad::doc::files::{Feature, Manifest, PartFile};
use ryucad::doc::id::Id;
use ryucad::doc::project::Part;
use ryucad::doc::sketch::Geometry;
use ryucad::expr::Env;
use ryucad::rebuild::{Output, rebuild_part};
use ryucad::units::Unit;
use ryukei::check::{Fault, check, check_intersections};
use ryukei::mesh::{MeshParams, check_mesh, tessellate};
use std::collections::BTreeMap;
use std::sync::OnceLock;

fn id(s: &str) -> Id {
    Id::parse(s, "feature").unwrap()
}

/// The boxes `b` and `c`, the sketches `s` and `t` and the pocket `k`
/// every fuzzed feature follows.
fn base() -> &'static [(Id, Feature)] {
    static BASE: OnceLock<Vec<(Id, Feature)>> = OnceLock::new();
    BASE.get_or_init(|| {
        let b = r#"{"format": 1, "kind": "ryucad.feature", "type": "box",
                    "params": {"x": "20 mm", "y": "10 mm", "z": "5 mm"},
                    "placement": {"origin": ["0 mm", "0 mm", "0 mm"],
                                  "x": [1, 0, 0], "z": [0, 0, 1]}}"#;
        let s = r#"{"format": 2, "kind": "ryucad.feature", "type": "sketch", "plane": "zx",
                    "constraints": {},
                    "entities": {
                        "p1": {"type": "point", "m": [0.002, 0.002]},
                        "p2": {"type": "point", "m": [0.008, 0.002]},
                        "p3": {"type": "point", "m": [0.008, 0.006]},
                        "p4": {"type": "point", "m": [0.002, 0.006]},
                        "l1": {"type": "line", "start": "p1", "end": "p2"},
                        "l2": {"type": "line", "start": "p2", "end": "p3"},
                        "l3": {"type": "line", "start": "p3", "end": "p4"},
                        "l4": {"type": "line", "start": "p4", "end": "p1"}}}"#;
        let c = r#"{"format": 1, "kind": "ryucad.feature", "type": "box",
                    "params": {"x": "8 mm", "y": "8 mm", "z": "8 mm"},
                    "placement": {"origin": ["16 mm", "4 mm", "2 mm"],
                                  "x": [1, 0, 0], "z": [0, 0, 1]}}"#;
        let t = r#"{"format": 2, "kind": "ryucad.feature", "type": "sketch",
                    "placement": {"origin": ["0 mm", "0 mm", "5 mm"],
                                  "x": [1, 0, 0], "z": [0, 0, 1]},
                    "constraints": {},
                    "entities": {"p1": {"type": "point", "m": [0.005, 0.005]}}}"#;
        let k = r#"{"format": 3, "kind": "ryucad.feature", "type": "box",
                    "params": {"x": "2 mm", "y": "2 mm", "z": "2 mm"},
                    "placement": {"origin": ["9 mm", "4 mm", "4 mm"],
                                  "x": [1, 0, 0], "z": [0, 0, 1]},
                    "operation": "cut", "targets": ["b"]}"#;
        let read = |t: &str| {
            Feature::from_json(&canonical::parse(t).unwrap(), Unit::Mm, &Env::empty()).unwrap()
        };
        vec![
            (id("b"), read(b)),
            (id("c"), read(c)),
            (id("s"), read(s)),
            (id("t"), read(t)),
            (id("k"), read(k)),
        ]
    })
}

/// Part `fz`: the base, then `f`.
fn part(f: Feature) -> Part {
    let mut features: BTreeMap<Id, ryucad::error::Result<Feature>> = base()
        .iter()
        .map(|(i, x)| (i.clone(), Ok(x.clone())))
        .collect();
    features.insert(id("f"), Ok(f));
    Part {
        id: Id::parse("fz", "part").unwrap(),
        file: Ok(PartFile::new(vec![
            id("b"),
            id("c"),
            id("s"),
            id("t"),
            id("k"),
            id("f"),
        ])),
        parameters: None,
        env: Env::empty(),
        features,
        uses: BTreeMap::new(),
        acts: BTreeMap::new(),
        revision: String::new(),
    }
}

/// Builds instance, mate or instance pattern `f` (as written) in assembly
/// `asm` of a project with part `fz` (the base) and an assembly `sub`
/// placing `asm`; a mate or a pattern follows instances `i1` (free, 20 mm
/// up) and `i2` (fixed) of `fz`.
fn assemble(written: &str, after_instances: bool) {
    use ryucad::assembly::{Rebuild, Store};
    use ryucad::error::{Code, Error};
    let mut files: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    let mut put = |path: &str, v: serde_json::Value| {
        files.insert(path.to_owned(), canonical::to_string(&v).into_bytes());
    };
    put(
        "ryucad.json",
        serde_json::json!({"default_unit": "mm", "format": 1, "kind": "ryucad.project"}),
    );
    put(
        "parts/fz/part.json",
        PartFile::new(base().iter().map(|(i, _)| i.clone()).collect()).to_json(),
    );
    for (i, f) in base() {
        put(&format!("parts/fz/features/{i}.json"), f.to_json());
    }
    let assembly = |list: &[&str]| {
        serde_json::json!({"features": list, "format": 2, "kind": "ryucad.part",
                           "part_kind": "assembly"})
    };
    if after_instances {
        put("parts/asm/part.json", assembly(&["i1", "i2", "f"]));
        for (i, z, fixed) in [("i1", 0.02, false), ("i2", 0.0, true)] {
            put(
                &format!("parts/asm/features/{i}.json"),
                serde_json::json!({"format": 4, "kind": "ryucad.feature", "type": "instance",
                                   "part": "fz", "rev": "working", "fixed": fixed,
                                   "pose": {"origin_m": [0, 0, z], "z": [0, 0, 1], "x": [1, 0, 0]}}),
            );
        }
    } else {
        put("parts/asm/part.json", assembly(&["f"]));
    }
    put(
        "parts/asm/features/f.json",
        canonical::parse(written).unwrap(),
    );
    put("parts/sub/part.json", assembly(&["a"]));
    put(
        "parts/sub/features/a.json",
        serde_json::json!({"format": 4, "kind": "ryucad.feature", "type": "instance",
                           "part": "asm", "rev": "working",
                           "pose": {"origin_m": [0, 0, 0.01], "z": [0, 0, 1], "x": [1, 0, 0]}}),
    );
    let project = ryucad::doc::project::Project::load_from(std::path::Path::new("/fz"), &files)
        .expect("the project loads");
    let pins = ryucad::assembly::pins(&project, |part, label| {
        Err(Error::new(
            Code::LabelNotFound,
            format!("{part} has no {label}"),
        ))
    });
    let mut r = Rebuild::new(&project, &pins, Store::default());
    for asm in ["asm", "sub"] {
        let result = r
            .result(&Id::parse(asm, "part").unwrap())
            .expect("assemblies rebuild");
        let sound = |p: &ryucad::assembly::Placed| {
            for o in &p.model.occurrences {
                let pose = ryucad::assembly::rigid_pose(&o.pose);
                assert!(
                    pose.origin_m
                        .iter()
                        .chain(&pose.z)
                        .chain(&pose.x)
                        .all(|x| x.is_finite())
                );
                for sd in &o.solids {
                    assert!(ryukei::check::check(&sd.built.body).is_empty());
                }
            }
        };
        for (_, out) in &result.features {
            match out {
                Ok(Output::Instance(p)) => sound(p),
                Ok(Output::Copies(c)) => c.copies.iter().for_each(|c| sound(&c.placed)),
                Ok(Output::Mate(m)) => {
                    for f in [&m.a, &m.b] {
                        assert!(
                            f.rotation
                                .iter()
                                .flatten()
                                .chain(&f.origin)
                                .all(|x| x.is_finite())
                        );
                    }
                }
                Ok(_) => panic!("an assembly makes only instances, mates and copies"),
                Err(e) => assert_ne!(e.code, Code::Internal, "{e:?}"),
            }
        }
        let part = Id::parse(asm, "part").unwrap();
        let model = r
            .model(&part, &ryucad::doc::assembly::Rev::Working)
            .expect("its model");
        let found = |cache: &mut ryucad::interference::Cache| {
            ryucad::interference::find(&part, &model, &[], cache).expect("interference")
        };
        let mut cache = ryucad::interference::Cache::default();
        let v = found(&mut cache);
        assert_eq!(found(&mut cache), v, "from what it cached");
        assert_eq!(found(&mut Default::default()), v, "from nothing");
        for i in v["interferences"].as_array().unwrap() {
            assert!(i["volume_m3"].as_f64().is_some_and(f64::is_finite), "{i}");
            for k in ["min_m", "max_m"] {
                assert!(
                    i["box"][k]
                        .as_array()
                        .unwrap()
                        .iter()
                        .all(|x| x.as_f64().is_some_and(f64::is_finite)),
                    "{i}"
                );
            }
        }
    }
}

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let Ok(v) = canonical::parse(text) else {
        return;
    };
    let _ = Manifest::from_json(&v);
    let _ = PartFile::from_json(&v);
    let Ok(f) = Feature::from_json(&v, Unit::Mm, &Env::empty()) else {
        return;
    };
    let written = canonical::to_string(&f.to_json());
    let back = Feature::from_json(
        &canonical::parse(&written).unwrap(),
        Unit::In,
        &Env::empty(),
    )
    .unwrap();
    assert_eq!(back, f);
    assert_eq!(canonical::to_string(&back.to_json()), written);
    if let Feature::Instance(_) = &f {
        assemble(&written, false);
        return;
    }
    if matches!(f, Feature::Mate(_) | Feature::InstancePattern(_)) {
        assemble(&written, true);
        return;
    }
    let part = part(f);
    let r = rebuild_part(&part);
    let me = id("f");
    match r.output(&me) {
        Some(Output::Solids(m)) => {
            let params = MeshParams {
                max_triangles: 200_000,
                ..MeshParams::new(2e-4, 0.5)
            };
            for (_, b) in &m.solids {
                assert_eq!(check(&b.body), []);
                for fault in check_intersections(&b.body) {
                    assert!(matches!(fault, Fault::CannotCheck { .. }), "{fault:?}");
                }
                match tessellate(&b.body, &params) {
                    Ok(mesh) => {
                        assert_eq!(mesh.faults, []);
                        assert_eq!(check_mesh(&b.body, &mesh), Ok(()));
                    }
                    Err(ryukei::Error::MeshTooLarge { .. }) => {}
                    Err(e) => panic!("unexpected {e:?}"),
                }
            }
        }
        Some(Output::Sketch(solved)) => {
            for e in solved.sketch.entities.values() {
                match e.geometry {
                    Geometry::Point { m } => assert!(m.iter().all(|x| x.is_finite())),
                    Geometry::Circle { r_m, .. } => assert!(r_m.is_finite() && r_m >= 0.0),
                    _ => {}
                }
            }
            let ctx = ryucad::sketch::context(&part, part.order(), &me, &solved.sketch, &r)
                .expect("what resolved once resolves again");
            let again = ryucad::sketch::solve_in(&me, &solved.sketch, &ctx).unwrap();
            assert!(!again.stale);
        }
        // An assembly's feature in a part fails before it builds.
        Some(Output::Instance(_) | Output::Mate(_) | Output::Copies(_)) => {
            panic!("an assembly's feature built in a part")
        }
        None => {}
    }
});
