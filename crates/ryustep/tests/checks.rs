//! Every body ryustep heals from a committed file passes the
//! self-intersection check: no faces crossing or overlapping, and no pair
//! it can't check. The files are the OCCT ones (`tests/data/occt`) and
//! the committed corpus (`tests/corpus`), real models whose fillets are
//! splines, tori and spheres. A body read twice from one file (a second
//! representation of the same solid) is checked once.
//!
//! Release only, in CI (`cargo test -p ryustep --release --test checks`):
//! every body checked on one thread, design 296's 30 s bar asserted with
//! `RYUKEI_BARS=1` and the times printed otherwise, the whole file about
//! half a minute on cox-worker-2's Ryzen 7 9700X since plan 15.C (it was about three, most of it
//! `nist_ctc_05_asme1_rd.stp`, whose spline fillets roll on cylinders,
//! tori and spheres split into several faces). Plan 8.F settled the 25
//! pairs it couldn't check, so none is left out.

use ryukei::check::{Fault, check_intersections};
use ryukei::name::OpKey;
use ryukei::topo::{Body, FaceId};
use ryustep::{ReadOptions, read};
use std::path::{Path, PathBuf};

/// Held by every test of this file, so a test timing its check on one
/// thread doesn't share the machine with its siblings' checks.
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Whether the time bars are asserted: only with `RYUKEI_BARS=1`, on the
/// machine they are recorded on. A check's time varies 15 to 160 times
/// between CPUs (`ftc_10` #6468 1.7 s on a Ryzen 7 9700X, about 280 s on
/// an Opteron 6380), so no wall-clock limit is safe on every machine
/// otherwise; the times are always printed.
fn bars() -> bool {
    std::env::var("RYUKEI_BARS").as_deref() == Ok("1")
}

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

/// Design 296's real-file bar (plan 15.C): every body's check under 30 s
/// on one thread, asserted with `RYUKEI_BARS=1`.
const BAR: f64 = 30.0;

/// The work units `nist_ctc_05`'s check may spend: it spends 1,721,702
/// (plan 15.C's review, cox-worker-4; units are the same on every
/// machine), and fitting its coincident pair onto the cylinder would add
/// 276,024 more.
const CTC_05_UNITS: u64 = 1_900_000;

/// Checks every body of `files`, which must number `count`, each on one
/// thread and under the bar.
fn check_clean(files: &[PathBuf], count: usize) {
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap();
    let mut checked = 0;
    for path in files {
        let file = path.file_name().unwrap().to_string_lossy().to_string();
        for (rep, body) in bodies(path) {
            let t = std::time::Instant::now();
            let faults = pool.install(|| check_intersections(&body));
            let s = t.elapsed().as_secs_f64();
            eprintln!("{file} #{rep}: {s:.1} s");
            assert_eq!(named(&body, &faults), [], "{file} #{rep}");
            assert!(!bars() || s < BAR, "{file} #{rep}: {s:.1} s");
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
    let _serial = serial();
    let all: Vec<PathBuf> = files("tests/data/occt")
        .into_iter()
        .chain(files("tests/corpus"))
        .collect();
    // The OCCT files' 15 bodies and the corpus files' 10.
    check_clean(&all, 25);
}

/// Faces tangent along a shared edge are certified apart on whichever
/// face keeps off the other's surface, without exhausting the other
/// first (#301): ftc_06's five cylinder pairs each spent 32,768 parts
/// on the face that runs on before the other passed in 9.
#[test]
#[cfg_attr(debug_assertions, ignore = "release only")]
fn tangent_pairs_pass_on_the_face_that_keeps_off() {
    let _serial = serial();
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/nist_ftc_06_asme1_rd.stp");
    let (_, body) = bodies(&path).remove(0);
    let work = ryukei::work::Work::unlimited();
    let faults = ryukei::check::check_intersections_with(&body, &work).unwrap();
    assert_eq!(named(&body, &faults), []);
    // 954,610 units on main; the ladder spends about 793,000.
    assert!(work.spent() < 850_000, "{}", work.spent());
}

/// ctc_05's NURBS fillets against the analytic faces they don't touch
/// traced for minutes to `Uncertain` before `faces_apart` certified them
/// apart in a unit (#302): 300 s on one thread, now under a minute.
#[test]
#[cfg_attr(debug_assertions, ignore = "release only")]
fn ctc_05_checks_in_under_a_minute() {
    let _serial = serial();
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/nist_ctc_05_asme1_rd.stp");
    let (_, body) = bodies(&path).remove(0);
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap();
    let t = std::time::Instant::now();
    let faults = pool.install(|| check_intersections(&body));
    assert_eq!(named(&body, &faults), []);
    eprintln!("ctc_05: {:?}", t.elapsed());
    assert!(
        !bars() || t.elapsed().as_secs_f64() < 60.0,
        "{:?}",
        t.elapsed()
    );
}

/// Two faces on one cylinder sharing two spline edges (ctc_05's `e538…`
/// and `b76b…`): their rims come from their own exact pcurves, not from
/// fits onto the cylinder (276,024 units, 23 to 35 s on one pair). The
/// body's whole check is held in work units, the same on every machine:
/// under [`CTC_05_UNITS`], which fitting that pair alone would pass.
#[test]
#[cfg_attr(debug_assertions, ignore = "release only")]
fn coincident_faces_use_their_own_pcurves() {
    let _serial = serial();
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/nist_ctc_05_asme1_rd.stp");
    let (_, body) = bodies(&path).remove(0);
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap();
    let w = ryukei::work::Work::unlimited();
    let faults = pool
        .install(|| ryukei::check::check_intersections_with(&body, &w))
        .unwrap();
    assert_eq!(named(&body, &faults), []);
    eprintln!("ctc_05's check: {} units", w.spent());
    assert!(w.spent() < CTC_05_UNITS, "{} units", w.spent());
}

/// The pinned corpus's folder (`scripts/fetch-corpus.sh`), if fetched;
/// `RYUCAD_REQUIRE_CORPUS=1` makes a missing one fail.
fn pinned(name: &str) -> Option<PathBuf> {
    let dir = std::env::var_os("RYUCAD_CORPUS")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/corpus"));
    let path = dir.join(name);
    if path.exists() {
        return Some(path);
    }
    assert!(
        std::env::var("RYUCAD_REQUIRE_CORPUS").as_deref() != Ok("1"),
        "{} missing: run scripts/fetch-corpus.sh",
        path.display()
    );
    None
}

/// #264: ftc_06's AP242 edition splits tori and spheres into halves on
/// one surface (turned copies a few ulps apart) sharing two edges along
/// meridians, each face's fin carrying its own fitted pcurve. The shared
/// edge was crossed with itself, two fits that can't be certified to
/// overlap: eight pairs `CannotCheck`. An edge both faces use cuts itself
/// only at its ends, so it isn't crossed with itself.
#[test]
#[ignore = "needs the pinned corpus (scripts/fetch-corpus.sh)"]
fn coincident_halves_sharing_their_meridians_check() {
    let _serial = serial();
    let Some(path) = pinned("NIST-PMI-STEP-Files/nist_ftc_06_asme1_ap242-e2.stp") else {
        return;
    };
    let (rep, body) = bodies(&path).remove(0);
    assert_eq!(rep, 2155);
    assert_eq!(named(&body, &check_intersections(&body)), []);
}

/// ftc_07's planes against its spline faces enclosed the same patches
/// again and again (61% of `jet_box` calls repeated): through the
/// operation's table its check takes under 8 s on one thread.
#[test]
#[cfg_attr(debug_assertions, ignore = "release only")]
fn ftc_07_encloses_each_patch_once() {
    let _serial = serial();
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/nist_ftc_07_asme1_rd.stp");
    let (_, body) = bodies(&path).remove(0);
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap();
    let t = std::time::Instant::now();
    let faults = pool.install(|| check_intersections(&body));
    assert_eq!(named(&body, &faults), []);
    eprintln!("ftc_07: {:?}", t.elapsed());
    assert!(
        !bars() || t.elapsed().as_secs_f64() < 8.0,
        "{:?}",
        t.elapsed()
    );
}
