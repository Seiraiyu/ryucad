//! Every body ryustep heals from a committed file passes the
//! self-intersection check: no faces crossing or overlapping, and no pair
//! it can't check. The files are the OCCT ones (`tests/data/occt`) and
//! the committed corpus (`tests/corpus`), real models whose fillets are
//! splines, tori and spheres. A body read twice from one file (a second
//! representation of the same solid) is checked once.
//!
//! The OCCT files and the quick corpus files run in release only, in CI
//! (`cargo test -p ryustep --release --test checks`), in seconds. The
//! files in `SLOW` take minutes each in release (ftc_07's about twenty), so
//! that test is ignored and run by name (`-- --ignored slow`). So is
//! `nist_ctc_05_asme1_rd.stp`'s, which still leaves the pairs
//! `CTC_05_UNCHECKED` lists (`-- --ignored ctc_05`).

use ryukei::check::{Fault, check_intersections};
use ryukei::name::OpKey;
use ryukei::topo::{Body, FaceId};
use ryustep::{ReadOptions, read};
use std::path::{Path, PathBuf};

const CTC_05: &str = "nist_ctc_05_asme1_rd.stp";

/// The corpus files whose check takes minutes in release: many spline
/// fillets traced against their neighbours' surfaces.
const SLOW: &[&str] = &[
    "nist_ctc_04_asme1_rd.stp",
    "nist_ftc_07_asme1_rd.stp",
    "nist_ftc_10_asme1_rb.stp",
];

/// The pairs of `nist_ctc_05_asme1_rd.stp`'s solid the check can't
/// decide, by the faces' import names in hex, each pair in name order. Its
/// spline fillets roll on cylinders, tori and spheres split into several
/// faces, so a fillet is tangent along its rail to the surface of faces
/// it doesn't touch; the trace can't follow a tangency along a curve
/// between analytic surfaces, and the general path, which can, gives up
/// on these after minutes. The coincident tori are two copies of one torus
/// placed differently, onto whose other copy a tolerant edge read from
/// the file can't be fitted. Plan 8.F (hardening) takes them on.
const CTC_05_UNCHECKED: &[&str] = &[
    "05610de553f266b59acae16858d310f7 927cdb48311b4606abfd1bb3f61d04dc",
    "05610de553f266b59acae16858d310f7 e538ff676f4ef8ed9275457e3f515e30",
    "062424c02eeffa34e93958ca3f7735fa d304286099a868bec2b37c04da1cc985",
    "0a07136f5cebf8d3228030b51963a3d6 8cca2f3541f42c1f30f28701e7b028fd",
    "0a07136f5cebf8d3228030b51963a3d6 b76b5c9730a020ffdb7d8833a4056496",
    "1382cfc3cb535bafdefd4963e9862a82 b2bc4bc8f68a50f6a4f96b459b3a216c",
    "1382cfc3cb535bafdefd4963e9862a82 b76b5c9730a020ffdb7d8833a4056496",
    "1fb1ae39b8cebb27d2c129a5f250e4d4 5ea001351912051b09d2f323efd4a28d",
    "1fb1ae39b8cebb27d2c129a5f250e4d4 e538ff676f4ef8ed9275457e3f515e30",
    "20d4d2c40333a63b90ab0c62891453bb eddbb8d82024b9c2f1d53f296d5b6c1b",
    "2eaaf60a1bd1cc3c8c98e1c854ef373c 57ba94795f85c200c554dc002e64a0c2",
    "2efc1c54d43cc4b283f4bec94f47b50e e20df7e56b2bc2daebb5652f2f3ca4ed",
    "337da19c2af877d7f1899ec889db9f81 d304286099a868bec2b37c04da1cc985",
    "57ba94795f85c200c554dc002e64a0c2 d304286099a868bec2b37c04da1cc985",
    "5ea001351912051b09d2f323efd4a28d e29923a6946ffcd8174a625796a3e32d",
    "630577ce153bd3e615f0c20687340ab2 eddbb8d82024b9c2f1d53f296d5b6c1b",
    "8cca2f3541f42c1f30f28701e7b028fd 9d4e74cc6b4b27b493b68eaab94365df",
    "927cdb48311b4606abfd1bb3f61d04dc 9ac43101d0caec8370aa6c1ef4c88baf",
    "927cdb48311b4606abfd1bb3f61d04dc d304286099a868bec2b37c04da1cc985",
    "9ac43101d0caec8370aa6c1ef4c88baf e538ff676f4ef8ed9275457e3f515e30",
    "9d4e74cc6b4b27b493b68eaab94365df b76b5c9730a020ffdb7d8833a4056496",
    "b2bc4bc8f68a50f6a4f96b459b3a216c f9a218217d535bc72fddcc063842ee3e",
    "b76b5c9730a020ffdb7d8833a4056496 f9a218217d535bc72fddcc063842ee3e",
    "e041564a6bc9eafbd604daf1698033da e20df7e56b2bc2daebb5652f2f3ca4ed",
    "e29923a6946ffcd8174a625796a3e32d e538ff676f4ef8ed9275457e3f515e30",
];

fn files(dir: &str) -> Vec<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join(dir);
    let mut out: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "stp"))
        .collect();
    out.sort();
    out
}

/// Each distinct body healed from the file, with its representation.
fn bodies(path: &Path) -> Vec<(u64, Body)> {
    let opts = ReadOptions {
        key: OpKey(7),
        heal: Default::default(),
    };
    let model = read(&std::fs::read(path).unwrap(), &opts).unwrap();
    let mut seen: Vec<String> = Vec::new();
    let mut out: Vec<(u64, Body)> = Vec::new();
    for b in model.bodies {
        if let Ok((body, _, _)) = b.result {
            let dump = body.dump_unnamed();
            if !seen.contains(&dump) {
                seen.push(dump);
                out.push((b.representation, body));
            }
        }
    }
    out
}

/// The faults' faces by name, each pair in name order.
fn named(body: &Body, faults: &[Fault]) -> Vec<(String, u128, u128)> {
    let name = |f: FaceId| body.face(f).unwrap().name.to_u128();
    let mut out: Vec<(String, u128, u128)> = faults
        .iter()
        .map(|f| {
            let (kind, a, b) = match *f {
                Fault::CannotCheck { a, b } => ("cannot_check", a, b),
                Fault::FacesCross { a, b } => ("faces_cross", a, b),
                Fault::FacesOverlap { a, b } => ("faces_overlap", a, b),
                ref f => panic!("{f:?}"),
            };
            let (a, b) = (name(a), name(b));
            (kind.to_string(), a.min(b), a.max(b))
        })
        .collect();
    out.sort();
    out
}

/// Checks every body of `files`, which must number `count`.
fn check_clean(files: &[PathBuf], count: usize) {
    let mut checked = 0;
    for path in files {
        let file = path.file_name().unwrap().to_string_lossy().to_string();
        for (rep, body) in bodies(path) {
            let t = std::time::Instant::now();
            let faults = check_intersections(&body);
            eprintln!("{file} #{rep}: {:.1} s", t.elapsed().as_secs_f64());
            assert_eq!(named(&body, &faults), [], "{file} #{rep}");
            checked += 1;
        }
    }
    assert_eq!(checked, count);
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "minutes in a debug build: CI runs it in release"
)]
fn imported_bodies_check_clean() {
    let quick: Vec<PathBuf> = files("tests/data/occt")
        .into_iter()
        .chain(files("tests/corpus"))
        .filter(|p| {
            let file = p.file_name().unwrap().to_string_lossy();
            file != CTC_05 && !SLOW.contains(&&*file)
        })
        .collect();
    // The OCCT files' 15 bodies and the quick corpus files' 6.
    check_clean(&quick, 21);
}

#[test]
#[ignore = "about half an hour in release; run it by name"]
fn slow_corpus_bodies_check_clean() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus");
    let slow: Vec<PathBuf> = SLOW.iter().map(|f| dir.join(f)).collect();
    check_clean(&slow, 3);
}

#[test]
#[ignore = "about twenty minutes in release; run it by name"]
fn ctc_05_checks_but_for_known_pairs() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/corpus")
        .join(CTC_05);
    let found = bodies(&path);
    assert_eq!(found.len(), 1);
    let (_, body) = &found[0];
    let t = std::time::Instant::now();
    let got = named(body, &check_intersections(body));
    eprintln!("{CTC_05}: {:.1} s", t.elapsed().as_secs_f64());
    for (kind, a, b) in &got {
        eprintln!("{kind} {a:#034x} {b:#034x}");
    }
    let hex = |x: &str| u128::from_str_radix(x, 16).unwrap();
    let want: Vec<(String, u128, u128)> = CTC_05_UNCHECKED
        .iter()
        .map(|pair| {
            let (a, b) = pair.split_once(' ').unwrap();
            ("cannot_check".to_string(), hex(a), hex(b))
        })
        .collect();
    assert_eq!(got, want);
}
