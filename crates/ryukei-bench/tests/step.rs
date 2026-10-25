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
//! - The pinned corpus `scripts/fetch-corpus.sh` downloads: no file heals
//!   fewer bodies than `crates/ryustep/tests/corpus/pinned.txt` records.
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
use ryustep::{LengthUnit, Product, ReadOptions, WriteModel, WriteOptions, read, write};
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
