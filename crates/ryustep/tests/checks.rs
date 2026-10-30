//! Every body ryustep heals from a committed file passes the
//! self-intersection check: no faces crossing or overlapping, and no pair
//! it can't check. The files are the OCCT ones (`tests/data/occt`) and
//! the committed corpus (`tests/corpus`), real models whose fillets are
//! splines, tori and spheres. A body read twice from one file (a second
//! representation of the same solid) is checked once.
//!
//! Release only, in CI (`cargo test -p ryustep --release --test checks`):
//! about three minutes, most of it `nist_ctc_05_asme1_rd.stp`, whose spline
//! fillets roll on cylinders, tori and spheres split into several faces.
//! Plan 8.F settled the 25 pairs it couldn't check (and the minutes each
//! of `nist_ctc_04`, `nist_ftc_07` and `nist_ftc_10` took), so none is
//! left out.

use ryukei::check::{Fault, check_intersections};
use ryukei::name::OpKey;
use ryukei::topo::{Body, FaceId};
use ryustep::{ReadOptions, read};
use std::path::{Path, PathBuf};

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
    let all: Vec<PathBuf> = files("tests/data/occt")
        .into_iter()
        .chain(files("tests/corpus"))
        .collect();
    // The OCCT files' 15 bodies and the corpus files' 10.
    check_clean(&all, 25);
}
