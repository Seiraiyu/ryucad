//! Design 120's bars for STEP at scale: the race's 1,600 seeded parts
//! (`ryukei_testkit::race`), each placed three times under one root, as
//! AP242 in millimetres, imported into a project. A daemon started cold
//! with an empty disk cache rebuilds the root within 1.25 times ryustep's
//! own `read` of the file; a daemon started cold over the warm cache
//! answers `rebuild` in under 2 s; and the root exports within 1.25 times
//! ryustep's `write` of the same products. Design 193's bars follow on
//! the same project: the root exported as usdc and as 3MF, each against
//! the STEP export, at a chord of 1 mm and 15 degrees: the parts are a
//! metre or so across, so 1 mm is to them what the default 0.01 mm is to
//! a 10 mm part (at 0.01 mm the race would be about 5 × 10⁸ triangles).
//! The usdc holds meshes alone: 170 of the race's 1,598 bodies have no
//! BrepArray yet (#247), and one refuses the whole export. Release builds
//! only.
//!
//! The race in `ryukei-bench` lays its parts along x 10 m apart, which
//! runs 16 km past ryucad's ±500 m size box; here they stand on a 40 × 40
//! grid 10 m apart, three high. Two seeded bodies don't come back through
//! STEP (design 55's limit): they are left out, so every part builds and
//! the root exports. The file is made once per target folder.
//!
//! With `RYUCAD_BARS=1` the bars are asserted as they are; elsewhere (CI)
//! ten times looser, which still catches a cost that grows out of hand.

mod bar_project;

use assert_cmd::Command;
use bar_project::machine;
use ryucad::proto::Request;
use ryucad::server::{handle, init};
use ryucad::session::Session;
use ryukei::geom::{Rigid, Vector3};
use ryustep::{Child, LengthUnit, Product, ReadOptions, WriteModel, WriteOptions};
use serde_json::{Value, json};
use std::time::Instant;

const PARTS: usize = 1600;
const COPIES: usize = 3;
/// Design 193's bars, as multiples of the root's STEP export: usdc (each
/// part's mesh, the meshes made) and 3MF (the same meshes, from the
/// session's cache). About one and a half times the slowest of plan 8.D's
/// recorded runs (43 to 52 and 28 to 36 times, on cox-worker-4's Opteron
/// 6380; see `docs/claude/ryucad-formats.md`): the STEP export writes each
/// part once, the mesh formats mesh it and write every triangle as text.
const USDC_BAR: f64 = 80.0;
const THREE_MF_BAR: f64 = 55.0;

fn slack() -> f64 {
    if std::env::var_os("RYUCAD_BARS").is_some() {
        1.0
    } else {
        10.0
    }
}

/// Each body its own product `p<i>`, and a root placing each [`COPIES`]
/// times on the grid.
fn model(bodies: Vec<ryukei::topo::Body>) -> WriteModel {
    let n = bodies.len();
    let mut products: Vec<Product> = bodies
        .into_iter()
        .enumerate()
        .map(|(i, b)| Product {
            id: format!("p{i}"),
            name: format!("p{i}"),
            bodies: vec![b],
            ..Default::default()
        })
        .collect();
    let children = (0..n)
        .flat_map(|i| {
            (0..COPIES).map(move |c| Child {
                product: i,
                place: Rigid::translation(Vector3::new(
                    10.0 * (i % 40) as f64,
                    10.0 * (i / 40) as f64,
                    10.0 * c as f64,
                )),
                id: String::new(),
            })
        })
        .collect();
    products.push(Product {
        id: "root".into(),
        name: "root".into(),
        children,
        ..Default::default()
    });
    WriteModel { products }
}

fn write_options() -> WriteOptions {
    WriteOptions {
        unit: LengthUnit::Millimeter,
        time_stamp: "2026-10-27T00:00:00".into(),
        ..WriteOptions::default()
    }
}

fn read_options() -> ReadOptions {
    ReadOptions {
        key: ryukei::name::OpKey(7),
        heal: Default::default(),
    }
}

/// The race file: made from the seeded bodies, those that don't come
/// back through STEP left out; kept in the target folder after the first
/// run.
fn race() -> Vec<u8> {
    let kept = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("step-bar-race.step");
    if let Ok(b) = std::fs::read(&kept) {
        return b;
    }
    let mut bodies = ryukei_testkit::race::writable(PARTS);
    let first = ryustep::write(&model(bodies.clone()), &write_options()).unwrap();
    let read = ryustep::read(&first, &read_options()).unwrap();
    let refused: Vec<String> = read
        .bodies
        .iter()
        .filter(|b| b.result.is_err())
        .filter_map(|b| b.definition)
        .map(|d| read.products.definitions[d].product_id.to_string())
        .collect();
    let mut i = 0;
    bodies.retain(|_| {
        i += 1;
        !refused.contains(&format!("p{}", i - 1))
    });
    let bytes = ryustep::write(&model(bodies), &write_options()).unwrap();
    std::fs::write(&kept, &bytes).unwrap();
    bytes
}

fn call(s: &mut Session, method: &str, params: Value) -> Value {
    handle(s, Request::from_rpc(method, Some(params)).unwrap())
        .unwrap_or_else(|e| panic!("{method}: {e}"))
}

#[test]
#[cfg_attr(debug_assertions, ignore = "minutes in a debug build")]
fn the_race_imports_rebuilds_and_exports_within_its_bars() {
    let bytes = race();
    // ryustep alone: read and heal every body, then write them all again.
    let t = Instant::now();
    let read = ryustep::read(&bytes, &read_options()).unwrap();
    let read_s = t.elapsed().as_secs_f64();
    let healed: Vec<ryukei::topo::Body> = read
        .bodies
        .iter()
        .map(|b| b.result.as_ref().expect("every body heals").0.clone())
        .collect();
    let parts = healed.len();
    let t = Instant::now();
    ryustep::write(&model(healed), &write_options()).unwrap();
    let write_s = t.elapsed().as_secs_f64();
    // The project: the file imported, every part and the root.
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("p");
    std::fs::create_dir(&root).unwrap();
    init(&root, None).unwrap();
    let file = dir.path().join("race.step");
    std::fs::write(&file, &bytes).unwrap();
    let t = Instant::now();
    let mut s = Session::open(&root).unwrap();
    let v = call(&mut s, "import", json!({"path": file}));
    let import_s = t.elapsed().as_secs_f64();
    assert_eq!(v["parts"].as_array().unwrap().len(), parts + 1);
    drop(s);
    let (run, cache) = (dir.path().join("run"), dir.path().join("cache"));
    let ryucad = |args: &[&str]| -> (f64, Value) {
        let t = Instant::now();
        let out = Command::cargo_bin("ryucad")
            .unwrap()
            .current_dir(&root)
            .args(args)
            .env("RYUCAD_RUNTIME_DIR", &run)
            .env("RYUCAD_CONFIG_DIR", dir.path().join("config"))
            .env("RYUCAD_CACHE_DIR", &cache)
            .env("RYUCAD_IDLE_TIMEOUT", "600")
            .output()
            .unwrap();
        let v: Value = serde_json::from_slice(&out.stdout)
            .unwrap_or_else(|_| panic!("{args:?}: {}", String::from_utf8_lossy(&out.stderr)));
        assert_eq!(v["ok"], true, "{args:?}: {v}");
        (t.elapsed().as_secs_f64(), v)
    };
    // A cold daemon heals every part, filling the cache; another, started
    // over it, heals nothing.
    let (cold, first) = ryucad(&["rebuild", "root"]);
    ryucad(&["daemon", "stop"]);
    let (warm, again) = ryucad(&["rebuild", "root"]);
    assert_eq!(
        again["result"]["parts"][0]["hash"],
        first["result"]["parts"][0]["hash"]
    );
    let out = dir.path().join("out.step");
    let (export, _) = ryucad(&[
        "export",
        "root",
        "--out",
        out.to_str().unwrap(),
        "--time-stamp",
        "2026-10-27T00:00:00",
    ]);
    // The other formats, from the same warm daemon. usdc meshes every
    // part (meshes alone, #247); 3MF asks for the same meshes and finds
    // them in the session's mesh cache.
    let formats = |ext: &str, more: &[&str]| -> (f64, Value) {
        let out = dir.path().join(format!("out.{ext}"));
        let mut args = vec![
            "export",
            "root",
            "--out",
            out.to_str().unwrap(),
            "--chord",
            "1 mm",
            "--angle",
            "15 deg",
            "--time-stamp",
            "2026-10-27T00:00:00",
        ];
        args.extend(more);
        ryucad(&args)
    };
    let (usdc, usdc_v) = formats("usdc", &["--no-brep"]);
    let (three_mf, three_mf_v) = formats("3mf", &[]);
    ryucad(&["daemon", "stop"]);
    let triangles = |v: &Value| -> u64 {
        v["result"]["products"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["triangles"].as_u64().unwrap_or(0))
            .sum()
    };
    let mb = |v: &Value| v["result"]["bytes"].as_u64().unwrap() as f64 / 1e6;
    println!(
        "{parts} parts placed {COPIES} times ({:.1} MB), on {}: ryustep read {read_s:.1} s, \
         wrote {write_s:.1} s; import {import_s:.1} s; a cold daemon's rebuild {cold:.1} s \
         ({:.2} times the read), over the warm cache {warm:.2} s; export {export:.1} s ({:.2} \
         times the write)",
        bytes.len() as f64 / 1e6,
        machine(),
        cold / read_s,
        export / write_s,
    );
    println!(
        "at 1 mm and 15 deg, {} triangles: usdc {usdc:.1} s ({:.2} times the STEP export, \
         {:.1} MB); 3MF {three_mf:.1} s from the meshes usdc made ({:.2} times, {:.1} MB)",
        triangles(&three_mf_v),
        usdc / export,
        mb(&usdc_v),
        three_mf / export,
        mb(&three_mf_v),
    );
    // Every number is printed before any bar is judged.
    assert!(cold < 1.25 * read_s * slack(), "cold: {cold:.1} s");
    assert!(warm < 2.0 * slack(), "warm: {warm:.2} s");
    assert!(export < 1.25 * write_s * slack(), "export: {export:.1} s");
    assert!(usdc < USDC_BAR * export * slack(), "usdc: {usdc:.1} s");
    assert!(
        three_mf < THREE_MF_BAR * export * slack(),
        "3mf: {three_mf:.1} s"
    );
}
