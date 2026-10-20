//! Feature files from bytes. Any text must either load to a feature that
//! writes back canonically and reads back equal, or give a structured
//! error; building it must succeed or give a structured error. A sketch
//! that solves must solve to finite positions that are current: solving
//! them again changes nothing. Any panic is a bug.

#![no_main]

use libfuzzer_sys::fuzz_target;
use ryucad::doc::canonical;
use ryucad::doc::files::{Feature, Manifest, PartFile};
use ryucad::doc::id::Id;
use ryucad::doc::sketch::Geometry;
use ryucad::expr::Env;
use ryucad::rebuild::build_feature;
use ryucad::units::Unit;

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
    let id = Id::parse("f", "feature").unwrap();
    if let Some(s) = f.sketch() {
        if let Ok(solved) = ryucad::sketch::solve(&id, s) {
            for e in solved.sketch.entities.values() {
                match e.geometry {
                    Geometry::Point { m } => assert!(m.iter().all(|x| x.is_finite())),
                    Geometry::Circle { r_m, .. } => assert!(r_m.is_finite() && r_m >= 0.0),
                    _ => {}
                }
            }
            let again = ryucad::sketch::solve(&id, &solved.sketch).unwrap();
            assert!(!again.stale);
        }
    } else if let Ok(b) = build_feature(&id, &id, &f) {
        assert!(ryukei::check::check(&b.body).is_empty());
    }
});
