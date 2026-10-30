//! ryukei on every surface against OCCT and SMLib (design 94, plan 8.F).
//!
//! Bodies go to OCCT through STEP (ryustep, in millimetres, which OCCT
//! reads as such) and to SMLib through USD (ryuformats' BrepArrays, in
//! meters), so every kernel works on the same exact NURBS; curves go to
//! both as exact clamped NURBS. SMLib runs in Python
//! (`tests/nurbs/reference.py`, `RYUCAD_USD_PYTHON`, else
//! `/opt/usd/bin/python`), each job in a process of its own.
//!
//! - The race: a block whose top is a lofted bicubic face, minus 1,000
//!   lofted pins poking out of both faces, on one thread, against OCCT's
//!   `BRepAlgoAPI_Cut`: ryukei within twice OCCT's time (design 94 asked
//!   it to be faster; that is left to phase 9). Its flush variant (pins standing on the
//!   block's bottom, issue #77) is measured on the first row of 40 pins
//!   and printed with its ratio to the clear pins' time per pin; SMLib's
//!   times are printed beside them.
//!
//! Devcontainer only (`--features occt,usd`); prints a report (run with
//! `--nocapture`).

use ryukei::boolean::{BooleanOp, Tool, ToolKey, boolean};
use ryukei::geom::{Rigid, Vector3};
use ryukei::measure::{Entity, Properties, properties};
use ryukei::name::OpKey;
use ryukei::topo::Body;
use ryukei_bench::occt;
use ryukei_testkit::nurbs_race;
use serde_json::{Value, json};
use std::path::{Path as FilePath, PathBuf};
use std::sync::Arc;
use std::time::Instant;

/// A body's volume, measured by ryukei, and its bound.
fn measured(b: &Body) -> (f64, f64) {
    let Properties::Body { volume, .. } = properties(b, Entity::Body).unwrap() else {
        unreachable!()
    };
    (volume.value(), volume.bound())
}

/// `b` written as STEP in millimetres and read by OCCT, which keeps
/// millimetres: its volumes are in mm³.
fn to_occt(dir: &FilePath, name: &str, b: &Body) -> occt::Solid {
    use ryustep::{LengthUnit, Product, WriteModel, WriteOptions, write};
    let bytes = write(
        &WriteModel {
            products: vec![Product {
                id: name.into(),
                name: name.into(),
                bodies: vec![b.clone()],
                children: vec![],
                ..Default::default()
            }],
        },
        &WriteOptions {
            unit: LengthUnit::Millimeter,
            time_stamp: "2026-10-30T00:00:00".into(),
            ..WriteOptions::default()
        },
    )
    .unwrap();
    let path = dir.join(format!("{name}.stp"));
    std::fs::write(&path, bytes).unwrap();
    occt::Solid::read_step(&path, false).unwrap()
}

/// `b` written as USD (BrepArrays only, in meters) for SMLib.
fn to_usd(dir: &FilePath, name: &str, b: &Body) -> PathBuf {
    use ryuformats::Unit;
    use ryuformats::usd::stage::{Model, Options, Product, Solid, layer};
    let model = Model {
        products: vec![Product {
            id: name.into(),
            name: name.into(),
            part: name.into(),
            label: None,
            solids: vec![Solid {
                name: "s".into(),
                body: Arc::new(b.clone()),
                mesh: None,
            }],
            children: vec![],
        }],
    };
    let opts = Options {
        unit: Unit::Meter,
        brep: true,
        mesh: false,
        pipe_tolerance: 1e-7,
        layer_data: vec![],
    };
    let path = dir.join(format!("{name}.usda"));
    let text = ryuformats::usd::usda::to_string(&layer(&model, &opts).unwrap()).unwrap();
    std::fs::write(&path, text).unwrap();
    path
}

/// SMLib's results for `jobs`, one each (see `tests/nurbs/reference.py`).
fn smlib(dir: &FilePath, jobs: &[Value]) -> Vec<Value> {
    let python =
        std::env::var("RYUCAD_USD_PYTHON").unwrap_or_else(|_| "/opt/usd/bin/python".into());
    let manifest = dir.join("jobs.json");
    std::fs::write(&manifest, json!({ "jobs": jobs }).to_string()).unwrap();
    let script = FilePath::new(env!("CARGO_MANIFEST_DIR")).join("tests/nurbs/reference.py");
    let out = std::process::Command::new(&python)
        .arg(&script)
        .arg(&manifest)
        .output()
        .unwrap_or_else(|e| panic!("{python}: {e}"));
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

/// Millimetres for OCCT: `place` with its translation scaled.
fn in_mm(place: &Rigid) -> Rigid {
    let t = place.translation_part() * 1000.0;
    Rigid::translation(Vector3::new(t.x, t.y, t.z))
}

fn one_thread<T: Send>(f: impl FnOnce() -> T + Send) -> T {
    rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap()
        .install(f)
}

/// ryukei's cut of `pin` at `places` from `block`, and its seconds.
fn ryukei_cut(block: &Body, pin: &Body, places: &[Rigid]) -> (Body, f64) {
    let tools: Vec<Tool> = places
        .iter()
        .enumerate()
        .map(|(i, p)| Tool {
            key: ToolKey(i as u128 + 1),
            body: pin,
            place: *p,
        })
        .collect();
    let t = Instant::now();
    let (cut, _) = one_thread(|| boolean(OpKey(3), BooleanOp::Subtract, block, &tools))
        .expect("ryukei's boolean");
    (cut, t.elapsed().as_secs_f64())
}

/// OCCT's cut of the same, and its seconds; its failure as text.
fn occt_cut(
    block: &occt::Solid,
    pin: &occt::Solid,
    places: &[Rigid],
) -> (Result<occt::Solid, String>, f64) {
    let pins: Vec<occt::Solid> = places.iter().map(|p| pin.placed(&in_mm(p))).collect();
    let t = Instant::now();
    let cut = block.boolean(BooleanOp::Subtract, &pins, false);
    (cut, t.elapsed().as_secs_f64())
}

#[test]
fn the_nurbs_race_against_occt_on_one_thread() {
    let dir = tempfile::tempdir().unwrap();
    let (block, clear, flush) = (
        nurbs_race::block(),
        nurbs_race::clear_pin(),
        nurbs_race::flush_pin(),
    );
    let places = nurbs_race::places(nurbs_race::COLS * nurbs_race::ROWS);
    let row = &places[..nurbs_race::COLS];

    let (cut, ryukei_s) = ryukei_cut(&block, &clear, &places);
    assert_eq!(cut.counts().faces, 6 + places.len());
    let (row_cut, row_s) = ryukei_cut(&block, &flush, row);
    assert_eq!(row_cut.counts().faces, 6 + row.len());
    let clear_row_s = ryukei_cut(&block, &clear, row).1;

    let (ob, oc, of) = (
        to_occt(dir.path(), "block", &block),
        to_occt(dir.path(), "clear", &clear),
        to_occt(dir.path(), "flush", &flush),
    );
    let (occt_result, occt_s) = occt_cut(&ob, &oc, &places);
    let occt_result = occt_result.expect("OCCT cuts");
    assert_eq!(occt_result.faces() as usize, 6 + places.len());
    let (occt_flush, occt_flush_s) = occt_cut(&ob, &of, row);

    let usd = |name: &str, b: &Body| to_usd(dir.path(), name, b);
    let xyz = |ps: &[Rigid]| -> Vec<[f64; 3]> {
        ps.iter()
            .map(|p| {
                let t = p.translation_part();
                [t.x, t.y, t.z]
            })
            .collect()
    };
    let (ub, uc, uf) = (
        usd("block", &block),
        usd("clear", &clear),
        usd("flush", &flush),
    );
    let s = smlib(
        dir.path(),
        &[
            json!({"kind": "race", "block": ub, "pin": uc, "places": xyz(&places)}),
            json!({"kind": "race", "block": ub, "pin": uf, "places": xyz(row)}),
        ],
    );

    let (v, bound) = measured(&cut);
    println!("NURBS race, one thread, {} pins:", places.len());
    println!("  ryukei {ryukei_s:.1} s, {v:.6} m³ ± {bound:.1e}");
    println!(
        "  OCCT   {occt_s:.1} s, {:.6} m³ (OCCT's own volume)",
        occt_result.volume() * 1e-9
    );
    println!("  SMLib  {}", s[0]);
    println!(
        "Flush, the first row of {}: ryukei {row_s:.1} s ({:.1}× the clear pins' time per pin, {clear_row_s:.1} s); OCCT {}; SMLib {}",
        row.len(),
        row_s / clear_row_s,
        match &occt_flush {
            Ok(_) => format!("{occt_flush_s:.1} s"),
            Err(e) => format!("refused after {occt_flush_s:.1} s ({e})"),
        },
        s[1],
    );
    if let Some(sv) = s[0]["volume"].as_f64() {
        assert!(
            (v - sv).abs() <= bound + 1e-6 * sv,
            "ryukei {v} against SMLib {sv}"
        );
    }
    // Design 94's bar was to beat OCCT; plan 8.F measured ryukei at about
    // twice OCCT's time once OCCT read the pins right (it had read them as
    // other shapes, and lost), and keeps the gap from growing past that.
    println!("ryukei / OCCT: {:.2}", ryukei_s / occt_s);
    assert!(
        ryukei_s < 2.0 * occt_s,
        "ryukei {ryukei_s:.1} s vs OCCT {occt_s:.1} s"
    );
}
