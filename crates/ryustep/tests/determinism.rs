//! Bit-exact determinism of reading and writing. Every CI platform and
//! CPU level must reproduce `tests/golden/step.txt`: the bodies read from
//! the OCCT files, the bytes written for seeded bodies in meters and
//! millimetres, and those bodies read back. After a deliberate change,
//! regenerate with `UPDATE_GOLDEN=1 cargo test -p ryustep --test
//! determinism` and review the diff like any other change.

use ryukei::blend::blend;
use ryukei::boolean::boolean;
use ryukei::geom::{Rigid, Vector3};
use ryukei::name::OpKey;
use ryukei::primitive;
use ryukei::topo::Body;
use ryukei_testkit::blend_bytes::blend_case;
use ryukei_testkit::boolean_bytes::boolean_case;
use ryukei_testkit::sweep_bytes::sweep;
use ryukei_testkit::{Golden, Lcg, Section};
use ryustep::{LengthUnit, Model, Product, ReadOptions, WriteModel, WriteOptions, read, write};

fn opts() -> ReadOptions {
    ReadOptions {
        key: OpKey(7),
        heal: Default::default(),
    }
}

/// Every body of a model as its dump, journal and report, or its error.
fn record(s: &mut Section, m: &Model) {
    for b in &m.bodies {
        match &b.result {
            Ok((body, journal, report)) => {
                s.str(&body.dump());
                s.str(&format!("{journal:?} {report:?}"));
            }
            Err(e) => s.str(&format!("{e:?}")),
        }
    }
}

fn occt(golden: &mut Golden) {
    let mut s = golden.section("read");
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/occt");
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|n| n.ends_with(".stp"))
        .collect();
    names.sort();
    for n in names {
        let bytes = std::fs::read(format!("{dir}/{n}")).unwrap();
        record(&mut s, &read(&bytes, &opts()).unwrap());
    }
}

/// The primitives, and seeded sweeps, booleans and blends.
fn bodies() -> Vec<Body> {
    let t = Rigid::rotation(Vector3::new(1.0, 2.0, 0.5).normalize().unwrap(), 0.7);
    let k = OpKey(1);
    let mut out = vec![
        primitive::block(k, 1.0, 2.0, 3.0, &t).unwrap().0,
        primitive::cylinder(k, 1.0, 2.0, &t).unwrap().0,
        primitive::cone(k, 1.0, 2.0, &t).unwrap().0,
        primitive::sphere(k, 1.0, &t).unwrap().0,
        primitive::torus(k, 2.0, 0.5, &t).unwrap().0,
    ];
    let bytes = |seed: u64| {
        let mut g = Lcg::new(seed);
        (0..96)
            .map(|_| (g.next_u64() >> 56) as u8)
            .collect::<Vec<u8>>()
    };
    for seed in 0..12 {
        if let Some((b, _)) = sweep(&bytes(seed)) {
            out.push(b);
        }
        if let Some(case) = boolean_case(&bytes(seed))
            && let Ok((b, _)) = boolean(OpKey(9), case.op, &case.target, &case.tools())
        {
            out.push(b);
        }
        if let Some(case) = blend_case(&bytes(seed))
            && let Ok((b, _)) = blend(OpKey(9), &case.body, &case.blends)
        {
            out.push(b);
        }
    }
    out.retain(|b| b.faces().next().is_some());
    out
}

fn written(golden: &mut Golden) {
    let bodies = bodies();
    for (section, unit) in [
        ("write_m", LengthUnit::Meter),
        ("write_mm", LengthUnit::Millimeter),
    ] {
        let mut s = golden.section(section);
        for b in &bodies {
            let bytes = write(
                &WriteModel {
                    products: vec![Product {
                        id: "part".into(),
                        name: "part".into(),
                        bodies: vec![b.clone()],
                        children: vec![],
                        ..Default::default()
                    }],
                },
                &WriteOptions {
                    unit,
                    time_stamp: "2026-10-22T00:00:00".into(),
                    ..WriteOptions::default()
                },
            )
            .unwrap();
            s.str(&ryukei_testkit::unversioned(
                std::str::from_utf8(&bytes).unwrap(),
            ));
            record(&mut s, &read(&bytes, &opts()).unwrap());
        }
    }
}

#[test]
fn determinism() {
    let mut golden = Golden::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/golden/step.txt"
    ));
    occt(&mut golden);
    written(&mut golden);
    golden.finish();
}
