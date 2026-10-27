//! STEP head to head (design 55, plan 7.D): ryustep and OCCT read the
//! same files, and OCCT reads the files ryustep writes. SMLib has no
//! STEP reader in usd-brep's sources, so it takes no part.
//!
//! - The committed corpus (`crates/ryustep/tests/corpus`) and the OCCT
//!   files: every body ryustep heals must enclose the volume of one of
//!   OCCT's solids (`STEPControl_Reader` then `ShapeFix_Shape`, its exact
//!   B-rep volume) to within its mesh's error, `4 × chord × area`.
//! - Files ryustep writes from seeded bodies: OCCT reads a solid per
//!   lump, of the body's volume.
//! - Files ryucad exports (design 120, plan 6.C): parts and an assembly
//!   of a project, each read by OCCT as a solid per lump and occurrence,
//!   of the volumes ryucad's solids have.
//! - The pinned corpus `scripts/fetch-corpus.sh` downloads: no file heals
//!   fewer bodies than `crates/ryustep/tests/corpus/pinned.txt` records.
//! - A large assembly (1,600 seeded parts placed 3 times each, about
//!   100 MB) read, healed and written on all cores, against OCCT reading,
//!   fixing and writing it: ryukei must be faster.
//!
//! Prints a report (run with `--nocapture`).

use rayon::prelude::*;
use ryukei::blend::blend;
use ryukei::boolean::boolean;
use ryukei::geom::{Rigid, Vector3};
use ryukei::mesh::{MeshParams, tessellate};
use ryukei::name::OpKey;
use ryukei::primitive;
use ryukei::topo::Body;
use ryukei_bench::occt;
use ryukei_testkit::Lcg;
use ryukei_testkit::blend_bytes::blend_case;
use ryukei_testkit::boolean_bytes::boolean_case;
use ryukei_testkit::mesh::volume_and_area;
use ryukei_testkit::sweep_bytes::sweep;
use ryustep::{Child, LengthUnit, Product, ReadOptions, WriteModel, WriteOptions, read, write};
use std::path::{Path, PathBuf};
use std::time::Instant;

fn opts() -> ReadOptions {
    ReadOptions {
        key: OpKey(7),
        heal: Default::default(),
    }
}

/// The ryustep crate's test folder.
fn tests_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../ryustep/tests")
}

/// The STEP files in `dir`, sorted.
fn step_files(dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|r| r.map(|e| e.unwrap().path()).collect())
        .unwrap_or_default();
    out.retain(|p| {
        let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("");
        ext.eq_ignore_ascii_case("stp") || ext.eq_ignore_ascii_case("step")
    });
    out.sort();
    out
}

/// A body's volume and area in mm³ and mm², and the bound on its mesh's
/// volume error, `4 × chord × area`, with the chord a thousandth of its
/// size.
fn measure(b: &Body) -> Option<(f64, f64, f64)> {
    let size = b
        .faces()
        .map(|f| ryukei::intersect::face_box(b, f))
        .reduce(|x, y| x.union(&y))
        .map(|x| (x.max() - x.min()).length())?;
    let chord = (size * 1e-3).max(1e-7);
    let m = tessellate(b, &MeshParams::new(chord, 0.3)).ok()?;
    if !m.faults.is_empty() {
        return None;
    }
    let (v, a) = volume_and_area(&m);
    Some((v * 1e9, a * 1e6, 4.0 * chord * a * 1e9))
}

/// Whether ryustep's volume `v` (within `bound`) is OCCT's `w`.
fn agree(v: f64, bound: f64, w: f64) -> bool {
    (v - w).abs() <= bound + 1e-6 * w.abs()
}

#[test]
fn corpus_agrees_with_occt() {
    println!(
        "{:<44} {:>7} {:>7} {:>7} {:>9} {:>9}",
        "file", "bodies", "healed", "occt", "ryustep s", "occt s"
    );
    let mut files = step_files(&tests_dir().join("corpus"));
    files.extend(step_files(&tests_dir().join("data/occt")));
    let mut wrong = Vec::new();
    for path in files {
        let bytes = std::fs::read(&path).unwrap();
        let t = Instant::now();
        let model = read(&bytes, &opts()).unwrap();
        let ours = t.elapsed().as_secs_f64();
        let t = Instant::now();
        let theirs = occt::Solid::read_step(&path, true).map(|s| s.solids());
        let occt_s = t.elapsed().as_secs_f64();
        let solids = theirs.clone().unwrap_or_default();
        let mut healed = 0;
        for b in &model.bodies {
            let Ok((body, _, _)) = &b.result else {
                continue;
            };
            healed += 1;
            let Some((v, _, bound)) = measure(body) else {
                wrong.push(format!(
                    "{}: #{} doesn't mesh",
                    path.display(),
                    b.representation
                ));
                continue;
            };
            // A body of several solids is their sum.
            let total: f64 = solids.iter().map(|s| s.volume).sum();
            if !solids.is_empty()
                && !solids.iter().any(|s| agree(v, bound, s.volume))
                && !agree(v, bound, total)
            {
                wrong.push(format!(
                    "{}: #{} encloses {v:.6} mm³, OCCT's solids {:?}",
                    path.display(),
                    b.representation,
                    solids.iter().map(|s| s.volume).collect::<Vec<_>>()
                ));
            }
        }
        println!(
            "{:<44} {:>7} {:>7} {:>7} {:>9.3} {:>9.3}",
            path.file_name().unwrap().to_string_lossy(),
            model.bodies.len(),
            healed,
            theirs.map_or("fails".to_string(), |s| s.len().to_string()),
            ours,
            occt_s
        );
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// The primitives and seeded sweeps, booleans and blends, as the
/// determinism tests make them.
fn seeded(seeds: u64) -> Vec<Body> {
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
    // Each seed's bodies made in parallel, kept in seed order.
    let made: Vec<Vec<Body>> = (0..seeds)
        .into_par_iter()
        .map(|seed| {
            let mut here = Vec::new();
            if let Some((b, _)) = sweep(&bytes(seed)) {
                here.push(b);
            }
            if let Some(case) = boolean_case(&bytes(seed))
                && let Ok((b, _)) = boolean(OpKey(9), case.op, &case.target, &case.tools())
            {
                here.push(b);
            }
            if let Some(case) = blend_case(&bytes(seed))
                && let Ok((b, _)) = blend(OpKey(9), &case.body, &case.blends)
            {
                here.push(b);
            }
            here
        })
        .collect();
    out.extend(made.into_iter().flatten());
    out.retain(|b| b.faces().next().is_some());
    out
}

fn part(id: &str, bodies: Vec<Body>) -> Product {
    Product {
        id: id.into(),
        name: id.into(),
        bodies,
        children: vec![],
        ..Default::default()
    }
}

fn write_opts(unit: LengthUnit) -> WriteOptions {
    WriteOptions {
        unit,
        time_stamp: "2026-10-25T00:00:00".into(),
        ..WriteOptions::default()
    }
}

#[test]
fn occt_reads_what_ryustep_writes() {
    let dir = std::env::temp_dir().join("ryukei-bench-step");
    std::fs::create_dir_all(&dir).unwrap();
    let mut wrong = Vec::new();
    let mut read_back = 0;
    let bodies = seeded(24);
    for (i, b) in bodies.iter().enumerate() {
        let Some((v, _, bound)) = measure(b) else {
            continue;
        };
        let bytes = write(
            &WriteModel {
                products: vec![part("part", vec![b.clone()])],
            },
            &write_opts(LengthUnit::Millimeter),
        )
        .unwrap();
        let path = dir.join(format!("body{i}.stp"));
        std::fs::write(&path, &bytes).unwrap();
        match occt::Solid::read_step(&path, false).map(|s| s.solids()) {
            // A body of several lumps is as many solids.
            Ok(s) if !s.is_empty() && agree(v, bound, s.iter().map(|x| x.volume).sum()) => {
                read_back += 1
            }
            other => wrong.push(format!("body {i}: {v:.6} mm³, OCCT {other:?}")),
        }
    }
    println!(
        "OCCT read {read_back} of {} bodies ryustep wrote",
        bodies.len()
    );
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// A project of a plate drilled through, a pin, a part of a block and a
/// ball (two solids), and an assembly placing the plate, two pins and the
/// two-solid part, one pin turned.
fn exported_project() -> (tempfile::TempDir, ryucad::session::Session) {
    use serde_json::json;
    let dir = tempfile::tempdir().unwrap();
    ryucad::server::init(dir.path(), None).unwrap();
    let mut s = ryucad::session::Session::open(dir.path()).unwrap();
    let mut call = |m: &str, p: serde_json::Value| {
        let req = ryucad::proto::Request::from_rpc(m, Some(p)).unwrap();
        ryucad::server::handle(&mut s, req).unwrap_or_else(|e| panic!("{m}: {e}"));
    };
    call("part.create", json!({"part": "plate"}));
    call(
        "feature.add",
        json!({"part": "plate", "type": "box", "id": "slab",
               "params": {"x": 60, "y": 40, "z": 8}}),
    );
    call(
        "feature.add",
        json!({"part": "plate", "type": "cylinder", "id": "bore", "operation": "cut",
               "targets": ["slab"], "params": {"radius": 4, "height": 20},
               "placement": {"origin": [20, 20, -5]}}),
    );
    call("part.create", json!({"part": "pin"}));
    call(
        "feature.add",
        json!({"part": "pin", "type": "cylinder", "id": "shank",
               "params": {"radius": 3.9, "height": 30}}),
    );
    call("part.create", json!({"part": "pair"}));
    call(
        "feature.add",
        json!({"part": "pair", "type": "box", "id": "cube", "params": {"x": 10, "y": 10, "z": 10}}),
    );
    call(
        "feature.add",
        json!({"part": "pair", "type": "sphere", "id": "ball", "params": {"radius": 5},
               "placement": {"origin": [30, 0, 0]}}),
    );
    call("part.create", json!({"part": "frame", "kind": "assembly"}));
    for (id, of, origin) in [
        ("base", "plate", json!([0, 0, 0])),
        ("p1", "pin", json!([20, 20, -10])),
        ("p2", "pin", json!([45, 20, 8])),
        ("two", "pair", json!([0, 60, 0])),
    ] {
        call(
            "feature.add",
            json!({"part": "frame", "type": "instance", "id": id, "of": of,
                   "placement": {"origin": origin}}),
        );
    }
    call(
        "assembly.move",
        json!({"part": "frame", "instance": "p2", "axis": [1, 0, 0], "angle": "30 deg"}),
    );
    (dir, s)
}

#[test]
fn occt_reads_what_ryucad_exports() {
    use ryucad::doc::assembly::Rev;
    let (dir, mut s) = exported_project();
    let mut wrong = Vec::new();
    for (part, schema, unit, solids) in [
        ("plate", "ap242", "mm", 1),
        ("pin", "ap214", "m", 1),
        ("pair", "ap242", "m", 2),
        ("frame", "ap242", "mm", 5),
        ("frame", "ap214", "m", 5),
    ] {
        let id = ryucad::doc::id::Id::parse(part, "part").unwrap();
        // What ryucad's solids hold, each occurrence once.
        let model = s.model(&id, &Rev::Working).unwrap();
        let (mut v, mut bound) = (0.0, 0.0);
        for o in &model.occurrences {
            for sd in &o.solids {
                let (bv, _, bb) = measure(&sd.built.body).unwrap();
                v += bv;
                bound += bb;
            }
        }
        let params = serde_json::json!({"part": part, "schema": schema, "unit": unit,
                                        "time_stamp": "2026-10-27T00:00:00"});
        let req = ryucad::proto::Request::from_rpc("export", Some(params)).unwrap();
        let a = ryucad::server::answer_live(&mut s, None, None, req).unwrap();
        let path = dir.path().join(format!("{part}-{schema}-{unit}.stp"));
        std::fs::write(&path, a.binary.unwrap()).unwrap();
        match occt::Solid::read_step(&path, false).map(|x| x.solids()) {
            Ok(x) if x.len() == solids && agree(v, bound, x.iter().map(|y| y.volume).sum()) => {
                println!("{part} {schema} {unit}: {} solids, {v:.3} mm³", x.len())
            }
            other => wrong.push(format!(
                "{part} {schema} {unit}: {v:.3} mm³, OCCT {other:?}"
            )),
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// Where `scripts/fetch-corpus.sh` keeps the pinned corpus.
fn pinned_dir() -> PathBuf {
    match std::env::var_os("RYUCAD_CORPUS") {
        Some(d) => PathBuf::from(d),
        None => Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/corpus"),
    }
}

#[test]
fn pinned_corpus_heals_no_fewer_bodies() {
    let listed = std::fs::read_to_string(tests_dir().join("corpus/pinned.txt")).unwrap();
    let dir = pinned_dir();
    let mut fell = Vec::new();
    let (mut total, mut healed) = (0, 0);
    for line in listed.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // `heals <healed> <path>`, the path the rest of the line.
        let Some(rest) = line.strip_prefix("heals ") else {
            continue;
        };
        let (want, name) = rest.split_once(' ').unwrap();
        let want: usize = want.parse().unwrap();
        let path = dir.join(name);
        let Ok(bytes) = std::fs::read(&path) else {
            panic!("{} is missing: run scripts/fetch-corpus.sh", path.display());
        };
        let model = read(&bytes, &opts()).unwrap();
        let got = model.bodies.iter().filter(|b| b.result.is_ok()).count();
        total += model.bodies.len();
        healed += got;
        println!(
            "heals {got} {name}    ({} bodies, recorded {want})",
            model.bodies.len()
        );
        if got < want {
            fell.push(format!("{name}: {got} healed, {want} recorded"));
        }
    }
    println!("healed {healed} of {total} bodies");
    assert!(fell.is_empty(), "{fell:#?}");
}

/// Bodies in the large assembly, and placements of each.
const PARTS: usize = 1600;
const COPIES: usize = 3;

/// A large assembly: [`PARTS`] seeded bodies that can be written, each its own part, placed
/// [`COPIES`] times on a grid under one root, as AP242 in millimetres.
fn assembly() -> Vec<u8> {
    // A torus face whose loop spirals can't be written (7.B's limit).
    let mut bodies = seeded(PARTS as u64);
    bodies.retain(|b| ryukei::heal::seamed(b, 1e-7).is_ok());
    bodies.truncate(PARTS);
    let n = bodies.len();
    let mut products: Vec<Product> = bodies
        .into_iter()
        .enumerate()
        .map(|(i, b)| part(&format!("p{i}"), vec![b]))
        .collect();
    let children = (0..n)
        .flat_map(|i| {
            (0..COPIES).map(move |c| Child {
                product: i,
                place: Rigid::translation(Vector3::new(10.0 * i as f64, 10.0 * c as f64, 0.0)),
                id: String::new(),
            })
        })
        .collect();
    products.push(Product {
        id: "root".into(),
        name: "root".into(),
        bodies: vec![],
        children,
        ..Default::default()
    });
    write(
        &WriteModel { products },
        &write_opts(LengthUnit::Millimeter),
    )
    .unwrap()
}

#[test]
fn large_assembly_beats_occt() {
    let dir = std::env::temp_dir().join("ryukei-bench-step");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("assembly.stp");
    let t = Instant::now();
    let bytes = assembly();
    std::fs::write(&path, &bytes).unwrap();
    println!(
        "assembly: {:.1} MB, made in {:.1} s",
        bytes.len() as f64 / 1e6,
        t.elapsed().as_secs_f64()
    );
    // ryustep: read and heal every body, then write them all again.
    let t = Instant::now();
    let model = read(&bytes, &opts()).unwrap();
    let healed: Vec<Body> = model
        .bodies
        .iter()
        .filter_map(|b| b.result.as_ref().ok().map(|r| r.0.clone()))
        .collect();
    let read_s = t.elapsed().as_secs_f64();
    let t = Instant::now();
    let out = write(
        &WriteModel {
            products: vec![part("all", healed.clone())],
        },
        &write_opts(LengthUnit::Millimeter),
    )
    .unwrap();
    let write_s = t.elapsed().as_secs_f64();
    // OCCT: read, fix and write.
    let t = Instant::now();
    let shape = occt::Solid::read_step(&path, true).unwrap();
    let occt_read = t.elapsed().as_secs_f64();
    let t = Instant::now();
    shape
        .write_step(&dir.join("assembly-occt.stp"), true)
        .unwrap();
    let occt_write = t.elapsed().as_secs_f64();
    let ours = read_s + write_s;
    let theirs = occt_read + occt_write;
    println!(
        "ryustep: {} of {} bodies, read and healed {read_s:.2} s, wrote {:.1} MB in {write_s:.2} s; total {ours:.2} s",
        healed.len(),
        model.bodies.len(),
        out.len() as f64 / 1e6
    );
    println!(
        "OCCT: {} solids, read and fixed {occt_read:.2} s, wrote {occt_write:.2} s; total {theirs:.2} s",
        shape.solids().len()
    );
    // Seeded bodies that don't come back through STEP are refused by
    // name, never built wrong: counted, not failed.
    for b in &model.bodies {
        if let Err(e) = &b.result {
            println!("refused: #{} {e:?}", b.representation);
        }
    }
    assert!(ours < theirs, "ryustep {ours:.2} s, OCCT {theirs:.2} s");
}
