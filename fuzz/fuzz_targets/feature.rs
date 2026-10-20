//! Feature files from bytes. Any text must either load to a feature that
//! writes back canonically and reads back equal, or give a structured
//! error. Loaded, it is built as feature `f` of a part after a box `b`
//! and a rectangle sketch `s`, so its references resolve against real
//! faces, edges and vertices: building must succeed or give a structured
//! error, a body must pass the checks, and a sketch that solves must solve
//! to finite positions that are current: solving them again, in the same
//! context, changes nothing. Any panic is a bug.

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
use std::collections::BTreeMap;
use std::sync::OnceLock;

fn id(s: &str) -> Id {
    Id::parse(s, "feature").unwrap()
}

/// The box `b` and the sketch `s` every fuzzed feature follows.
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
        let read = |t: &str| {
            Feature::from_json(&canonical::parse(t).unwrap(), Unit::Mm, &Env::empty()).unwrap()
        };
        vec![(id("b"), read(b)), (id("s"), read(s))]
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
        file: Ok(PartFile {
            features: vec![id("b"), id("s"), id("f")],
        }),
        parameters: None,
        env: Env::empty(),
        features,
        uses: BTreeMap::new(),
        revision: String::new(),
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
    let part = part(f);
    let r = rebuild_part(&part);
    let me = id("f");
    match r.output(&me) {
        Some(Output::Body(b)) => assert!(ryukei::check::check(&b.body).is_empty()),
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
        None => {}
    }
});
