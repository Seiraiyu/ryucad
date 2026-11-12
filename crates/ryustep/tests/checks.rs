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
    bodies_keyed(path, OpKey(7))
}

/// [`bodies`] healed under `key`.
fn bodies_keyed(path: &Path, key: OpKey) -> Vec<(u64, Body)> {
    let opts = ReadOptions {
        key,
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

/// The work units `nist_ctc_05`'s check may spend: about 1,444,000
/// since its tangent pairs are traced under a cap before their largest
/// searches apart (#302; units are the same on every machine). Searching
/// first spent 1,721,702 (plan 15.C's review), and fitting its coincident
/// pair onto the cylinder would add 276,024.
const CTC_05_UNITS: u64 = 1_600_000;

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

/// Faces touching at vertices they share but no edge are traced in the
/// wider region, under a cap, before the search apart outside the
/// vertices' balls (#303): ftc_10's ten fillet tori and the cylinders
/// they round failed that search after its whole budget, 65,536 units
/// and 2.5 s a pair on a Xeon E5-2650, then the trace settled each in
/// about 19,000.
#[test]
#[cfg_attr(debug_assertions, ignore = "release only")]
fn vertex_touching_pairs_are_traced_before_searched_apart() {
    let _serial = serial();
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/nist_ftc_10_asme1_rb.stp");
    let (_, body) = bodies(&path).remove(0);
    let work = ryukei::work::Work::unlimited();
    let faults = ryukei::check::check_intersections_with(&body, &work).unwrap();
    assert_eq!(named(&body, &faults), []);
    // About 2,340,000 units on main; tracing first spends about 1,680,000.
    eprintln!("ftc_10: {} units", work.spent());
    assert!(work.spent() < 1_800_000, "{}", work.spent());
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

/// KiCad's 3D models, fetched by `scripts/fetch-kicad.sh` (CC-BY-SA-4.0
/// with KiCad's exception: never committed). `None` (or a panic with
/// `RYUEDA_REQUIRE_KICAD=1`) when they aren't there.
fn kicad(name: &str) -> Option<PathBuf> {
    let dir = std::env::var_os("RYUEDA_KICAD_CORPUS")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/kicad"));
    let path = dir.join("kicad-packages3D-9.0.9").join(name);
    if path.exists() {
        return Some(path);
    }
    assert!(
        std::env::var("RYUEDA_REQUIRE_KICAD").as_deref() != Ok("1"),
        "{} missing: run scripts/fetch-kicad.sh",
        path.display()
    );
    None
}

/// #302: KiCad's CP_Radial_D12.5mm can. Its rim spline is tangent to the
/// wall's cylinder along the rim's edge with the wall's other sector,
/// sharing one vertex with the wall (four pairs), and its bottom rounds
/// are sectors of one torus written as revolutions turned and mirrored
/// about the axis (three pairs): seven `CannotCheck` pairs on main, none
/// now, in under 30 s on one thread with `RYUKEI_BARS=1`.
#[test]
#[ignore = "needs the KiCad corpus (scripts/fetch-kicad.sh)"]
fn cp_radial_rims_and_rounds_check() {
    let _serial = serial();
    let Some(path) = kicad("Capacitor_THT.3dshapes/CP_Radial_D12.5mm_P5.00mm.step") else {
        return;
    };
    // Healed as ryucad imports it (part `body`, feature `body`), as
    // step-hunt found it; it heals alike under `OpKey(7)` (#693,
    // `cp_radial_heals_alike_under_both_keys`).
    let key = OpKey::from_bytes(b"ryucad.feature\0body\0body");
    let (_, body) = bodies_keyed(&path, key).remove(0);
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap();
    let t = std::time::Instant::now();
    let faults = pool.install(|| check_intersections(&body));
    eprintln!("CP_Radial_D12.5mm: {:?}", t.elapsed());
    assert_eq!(named(&body, &faults), []);
    assert!(
        !bars() || t.elapsed().as_secs_f64() < 30.0,
        "{:?}",
        t.elapsed()
    );
}

/// #693: CP_Radial_D12.5mm healed under `OpKey(7)` refused
/// `FacesOverlap` where ryucad's key healed it. Since ryukei 16.A to 16.C
/// both heal the can to the same 53 faces, checking clean: the outcome
/// doesn't hang on the key that names it. (The faces are stored in
/// another order, some with their surfaces the other way round, so the
/// surfaces are compared by kind.)
#[test]
#[ignore = "needs the KiCad corpus (scripts/fetch-kicad.sh)"]
fn cp_radial_heals_alike_under_both_keys() {
    let _serial = serial();
    let Some(path) = kicad("Capacitor_THT.3dshapes/CP_Radial_D12.5mm_P5.00mm.step") else {
        return;
    };
    let ryucad = OpKey::from_bytes(b"ryucad.feature\0body\0body");
    let mut kinds = Vec::new();
    for key in [OpKey(7), ryucad] {
        let bodies = bodies_keyed(&path, key);
        assert_eq!(bodies.len(), 1, "{key:?}");
        let body = &bodies[0].1;
        assert_eq!(body.faces().count(), 53, "{key:?}");
        assert_eq!(named(body, &check_intersections(body)), [], "{key:?}");
        let mut k: Vec<String> = body
            .faces()
            .map(|f| {
                let s = body.surface(body.face(f).unwrap().surface).unwrap();
                format!("{s:?}").split('(').next().unwrap().to_owned()
            })
            .collect();
        k.sort();
        kinds.push(k);
    }
    assert_eq!(kinds[0], kinds[1]);
}

/// #322: KiCad's two CP_Radial_D8.0mm cans. Their top is a ring of
/// torus sectors (`SURFACE_OF_REVOLUTION`s of one circle) whose healed
/// corners leave gaps of a few nanometres between pcurves; a ray from a
/// neighbour's rim slipped through one, so healing read sectors that only
/// abut as overlapping and its self-union failed (`FacesOverlap`). Now
/// each heals as one solid, without a repair, and checks clean in under
/// 30 s on one thread with `RYUKEI_BARS=1`.
#[test]
#[ignore = "needs the KiCad corpus (scripts/fetch-kicad.sh)"]
fn cp_radial_d8_cans_heal_and_check() {
    let _serial = serial();
    let key = OpKey::from_bytes(b"ryucad.feature\0body\0body");
    for name in [
        "Capacitor_THT.3dshapes/CP_Radial_D8.0mm_P2.50mm.step",
        "Capacitor_THT.3dshapes/CP_Radial_D8.0mm_P5.00mm.step",
    ] {
        let Some(path) = kicad(name) else {
            return;
        };
        let bodies = bodies_keyed(&path, key);
        assert_eq!(bodies.len(), 1, "{name}");
        let body = &bodies[0].1;
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(1)
            .build()
            .unwrap();
        let t = std::time::Instant::now();
        let faults = pool.install(|| check_intersections(body));
        eprintln!("{name}: {:?}", t.elapsed());
        assert_eq!(named(body, &faults), [], "{name}");
        assert!(
            !bars() || t.elapsed().as_secs_f64() < 30.0,
            "{name}: {:?}",
            t.elapsed()
        );
    }
}

/// #598: KiCad's Coilcraft SQ coils, as references (ryucad's import key),
/// meshed at ryucad's normal quality: each wire end is two plane slivers
/// 0.2 to 0.6 µm wide whose sides cross, which never meshed, so every
/// round meshed the coil's 1,288-pole face again (146 s to a 300 s
/// timeout on an Opteron 6380). Drawn as strips, each meshes whole, in
/// under 30 s on one thread with `RYUKEI_BARS=1`.
#[test]
#[ignore = "needs the KiCad corpus (scripts/fetch-kicad.sh)"]
fn coilcraft_coils_mesh_whole_as_references() {
    let _serial = serial();
    let Some(dir) = kicad("Inductor_SMD.3dshapes") else {
        return;
    };
    let mut coils: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("L_Coilcraft_") && n.contains("SQ-"))
        })
        .collect();
    coils.sort();
    assert_eq!(coils.len(), 16, "{coils:?}");
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap();
    let opts = ReadOptions {
        key: OpKey::from_bytes(b"ryucad.feature\0ref\0ref"),
        heal: Default::default(),
    };
    for path in coils {
        let opened = ryustep::open(&std::fs::read(&path).unwrap()).unwrap();
        let (body, _) = pool.install(|| opened.reference(0, &opts)).result.unwrap();
        let diagonal = body
            .faces()
            .map(|f| ryukei::intersect::face_box(&body, f))
            .reduce(|a, b| a.union(&b))
            .map(|b| (b.max() - b.min()).length())
            .unwrap();
        let params = ryukei::mesh::MeshParams::new(diagonal * 5e-4, core::f64::consts::PI / 12.0);
        let t = std::time::Instant::now();
        let mesh = pool
            .install(|| ryukei::mesh::tessellate(&body, &params))
            .unwrap();
        let secs = t.elapsed().as_secs_f64();
        eprintln!(
            "{}: {} triangles in {secs:.1} s",
            path.file_name().unwrap().to_string_lossy(),
            mesh.triangles.len()
        );
        assert_eq!(mesh.faults, [], "{}", path.display());
        assert_eq!(
            ryukei::mesh::check_mesh(&body, &mesh),
            Ok(()),
            "{}",
            path.display()
        );
        assert!(!bars() || secs < 30.0, "{}: {secs:.1} s", path.display());
    }
}

/// #303: KiCad's MSOP-8-1EP. Four NURBS–NURBS pairs sharing an edge, not
/// tangent, each spend 1.8M to 2M units as the general trace's face
/// search recurses along the shared edge, which is one surface's isoline
/// and so lies in the face plane where that surface's parameter is held
/// (#529's case): 45 to 46 s, 8.2M units. Trying the pair apart beside the
/// edge first would settle each in under 1,300 units, but for faces
/// crossing at an angle it would accept a crossing inside the strip, so
/// it isn't done; the cause is on #303. Under 60 s on one thread with
/// `RYUKEI_BARS=1`, not step-hunt's 30 s.
#[test]
#[ignore = "needs the KiCad corpus (scripts/fetch-kicad.sh)"]
fn msop_8_1ep_checks_in_time() {
    let _serial = serial();
    let Some(path) =
        kicad("Package_SO.3dshapes/MSOP-8-1EP_3x3mm_P0.65mm_EP2.5x3mm_Mask1.73x2.36mm.step")
    else {
        return;
    };
    let key = OpKey::from_bytes(b"ryucad.feature\0body\0body");
    let (_, body) = bodies_keyed(&path, key).remove(0);
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap();
    let work = ryukei::work::Work::unlimited();
    let t = std::time::Instant::now();
    let faults = pool
        .install(|| ryukei::check::check_intersections_with(&body, &work))
        .unwrap();
    let secs = t.elapsed().as_secs_f64();
    eprintln!("MSOP-8-1EP: {secs:.1} s");
    assert_eq!(named(&body, &faults), []);
    // #303: the check bar of 30 s is unmet for this one case.
    assert!(!bars() || secs < 60.0, "{secs:.1} s");
}

/// #550: KiCad's L_Wurth_WE-HCFT-2012 inductor. Its four solids overlap,
/// so ryustep heals each alone and unites them; one face pair (the coil's
/// spline face against a lead's cylinder) traced 23.5M units in 25 s on
/// main before the union refused (26 to 31 s in all). Under the union
/// scope's cap (`1 << 24` units a pair) the import answers in 19.9 s on
/// one thread on cox-worker-2 (AMD Ryzen 7 9700X, load 2): the file's own
/// `FacesOverlap`, or a solid that checks clean. With `RYUKEI_BARS=1` it
/// is asserted under 25 s; design 444's 10 s bar is left to 16.D's
/// record, the cap keeping the caps' rule (plan 16.B, question 495).
#[test]
#[ignore = "needs the KiCad corpus (scripts/fetch-kicad.sh)"]
fn the_wurth_inductors_union_refuses_in_bounded_time() {
    let _serial = serial();
    let Some(path) =
        kicad("Inductor_THT_Wurth.3dshapes/L_Wurth_WE-HCFT-2012_LeadDiameter1.5mm.step")
    else {
        return;
    };
    let opts = ReadOptions {
        key: OpKey::from_bytes(b"ryucad.feature\0body\0body"),
        heal: Default::default(),
    };
    let bytes = std::fs::read(&path).unwrap();
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap();
    let t = std::time::Instant::now();
    let model = pool.install(|| read(&bytes, &opts)).unwrap();
    let secs = t.elapsed().as_secs_f64();
    eprintln!("L_Wurth_WE-HCFT-2012: {secs:.1} s");
    assert_eq!(model.bodies.len(), 1);
    match &model.bodies[0].result {
        Ok((body, _, _)) => assert_eq!(named(body, &check_intersections(body)), []),
        Err(ryustep::ImportError::Heal(ryukei::HealError::FacesOverlap { .. })) => {}
        Err(e) => panic!("{e:?}"),
    }
    assert!(!bars() || secs < 25.0, "{secs:.1} s");
}

/// #264: ftc_07's AP242 edition. Four planes, each tangent to a fillet
/// torus at a saddle of its tube (a critical value of 2.6e-13 m), at the
/// one vertex they share: the trace fails on the cells there. Traced
/// outside a cube round the vertex within the touch reach, they settle.
#[test]
#[ignore = "needs the pinned corpus (scripts/fetch-corpus.sh)"]
fn saddle_tangencies_at_shared_vertices_check() {
    let _serial = serial();
    let Some(path) = pinned("NIST-PMI-STEP-Files/nist_ftc_07_asme1_ap242-e2.stp") else {
        return;
    };
    let (rep, body) = bodies(&path).remove(0);
    assert_eq!(rep, 13541);
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
