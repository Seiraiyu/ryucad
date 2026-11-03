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

/// Faces tangent along a shared edge are certified apart on whichever
/// face keeps off the other's surface, without exhausting the other
/// first (#301): ftc_06's five cylinder pairs each spent 32,768 parts
/// on the face that runs on before the other passed in 9.
#[test]
#[cfg_attr(debug_assertions, ignore = "release only")]
fn tangent_pairs_pass_on_the_face_that_keeps_off() {
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
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/nist_ctc_05_asme1_rd.stp");
    let (_, body) = bodies(&path).remove(0);
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap();
    let t = std::time::Instant::now();
    let faults = pool.install(|| check_intersections(&body));
    assert_eq!(named(&body, &faults), []);
    assert!(t.elapsed().as_secs_f64() < 60.0, "{:?}", t.elapsed());
}

/// Two faces on one cylinder sharing two spline edges (ctc_05's `e538…`
/// and `b76b…`): their rims come from their own exact pcurves, not from
/// fits onto the cylinder (276,024 units, 23 to 35 s on one pair).
#[test]
#[cfg_attr(debug_assertions, ignore = "release only")]
fn coincident_faces_use_their_own_pcurves() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/nist_ctc_05_asme1_rd.stp");
    let (_, body) = bodies(&path).remove(0);
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap();
    let t = std::time::Instant::now();
    let faults = pool.install(|| check_intersections(&body));
    assert_eq!(named(&body, &faults), []);
    assert!(t.elapsed().as_secs_f64() < 15.0, "{:?}", t.elapsed());
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
    let Some(path) = pinned("NIST-PMI-STEP-Files/nist_ftc_06_asme1_ap242-e2.stp") else {
        return;
    };
    let (rep, body) = bodies(&path).remove(0);
    assert_eq!(rep, 2155);
    assert_eq!(named(&body, &check_intersections(&body)), []);
}
