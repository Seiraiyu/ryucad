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
//!   fewer bodies than `crates/ryustep/tests/corpus/pinned.txt` records,
//!   and every body builds as a reference (design 364).
//! - A large assembly (1,600 seeded parts placed 3 times each, about
//!   100 MB) read, healed and written on all cores, against OCCT reading,
//!   fixing and writing it: ryukei must be faster.
//!
//! Prints a report (run with `--nocapture`).

use ryukei::geom::{Rigid, Vector3};
use ryukei::heal::HealOptions;
use ryukei::mesh::{MeshParams, tessellate};
use ryukei::name::OpKey;
use ryukei::topo::Body;
use ryukei_bench::occt;
use ryukei_testkit::mesh::volume_and_area;
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

/// The race's bodies (design 55's): the primitives, then the sweeps,
/// booleans and blends seeds make (`ryukei_testkit::race`).
fn seeded(seeds: u64) -> Vec<Body> {
    ryukei_testkit::race::bodies(seeds)
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

/// The bodies of a file refused as open shells, read again with sheets
/// asked for (design 295, plan 12.C): how many heal into sheets that
/// check clean.
fn sheets_of(bytes: &[u8], model: &ryustep::Model) -> usize {
    let open: Vec<u64> = model
        .bodies
        .iter()
        .filter(|b| {
            matches!(
                b.result,
                Err(ryustep::ImportError::Heal(
                    ryukei::HealError::OpenShell { .. }
                ))
            )
        })
        .map(|b| b.representation)
        .collect();
    if open.is_empty() {
        return 0;
    }
    let sheets = ReadOptions {
        heal: ryukei::heal::HealOptions {
            open: ryukei::heal::Open::Sheet,
            ..Default::default()
        },
        ..opts()
    };
    let opened = ryustep::open(bytes).unwrap();
    opened
        .held
        .iter()
        .enumerate()
        .filter(|(_, h)| open.contains(&h.representation))
        .filter(|&(k, _)| {
            let b = opened.body(k, &sheets);
            b.result
                .as_ref()
                .is_ok_and(|(body, _, _)| ryukei::check::check(body).is_empty())
        })
        .count()
}

#[test]
fn pinned_corpus_heals_no_fewer_bodies() {
    let listed = std::fs::read_to_string(tests_dir().join("corpus/pinned.txt")).unwrap();
    let dir = pinned_dir();
    // `sheets <healed> <path>`: open shells that heal as sheets.
    let sheet_counts: std::collections::BTreeMap<&str, usize> = listed
        .lines()
        .filter_map(|l| l.trim().strip_prefix("sheets "))
        .map(|rest| {
            let (n, name) = rest.split_once(' ').unwrap();
            (name, n.parse().unwrap())
        })
        .collect();
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
        let sheets = sheets_of(&bytes, &model);
        let recorded = sheet_counts.get(name).copied().unwrap_or(0);
        if sheets > 0 || recorded > 0 {
            println!("sheets {sheets} {name}    (recorded {recorded})");
        }
        if sheets < recorded {
            fell.push(format!("{name}: {sheets} sheets, {recorded} recorded"));
        }
    }
    println!("healed {healed} of {total} bodies");
    assert!(fell.is_empty(), "{fell:#?}");
}

/// A reference's size: the box of points along its edges.
fn reference_size(b: &Body) -> f64 {
    // Nine points along each edge (a face bounded by one closed edge has
    // one vertex, so its vertices' box is a point), else the faces' boxes
    // for a body with no edge (a whole sphere): a face's box can be the
    // whole size box.
    let along = b.edges().flat_map(|e| {
        let edge = b.edge(e).unwrap();
        let curve = b.curve(edge.curve).unwrap();
        let [t0, t1] = edge.interval;
        (0..=8).map(move |i| curve.point(t0 + (t1 - t0) * f64::from(i) / 8.0))
    });
    ryukei::geom::Box3::from_points(along)
        .or_else(|| {
            b.faces()
                .map(|f| ryukei::intersect::face_box(b, f))
                .reduce(|x, y| x.union(&y))
        })
        .map(|x| (x.max() - x.min()).length())
        .unwrap()
}

/// A mesh's chord for a body: a hundredth of its size, at least what
/// `MeshParams` takes.
fn reference_chord(b: &Body) -> f64 {
    (reference_size(b) * 1e-2).max(10.0 * ryukei::tol::LINEAR_RESOLUTION)
}

/// Every body of the pinned corpus builds as a reference (design 364,
/// plan 21.A): ryustep reads each, and the reference is a sheet whose
/// topology holds. Prints each file's faces, work units, faces split,
/// dropped and unmeshed, the time on one thread, and the largest
/// representation's units beside ryucad's default `heal.budget`: design 364
/// says the budget never binds.
#[test]
fn pinned_corpus_builds_as_references() {
    use ryukei::check::check_topology;
    use ryukei::work::Work;
    let mut wrong = Vec::new();
    let mut seconds = 0.0;
    // Design 364: the budget never binds. Each representation's units under
    // no budget, the largest beside ryucad's default `heal.budget`.
    let mut largest: (u64, String) = (0, String::new());
    let one = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap();
    for name in pinned_files() {
        let path = pinned_dir().join(&name);
        let Ok(bytes) = std::fs::read(&path) else {
            panic!("{} is missing: run scripts/fetch-corpus.sh", path.display());
        };
        let opened = ryustep::open(&bytes).unwrap();
        let (mut split, mut dropped, mut unmeshed) = (0, 0, 0);
        let (mut faces, mut units) = (0, 0u64);
        for k in 0..opened.held.len() {
            let work = Work::unlimited();
            let start = std::time::Instant::now();
            let r = one.install(|| opened.reference_with(k, &opts(), &work));
            seconds += start.elapsed().as_secs_f64();
            let spent = work.spent();
            units += spent;
            if spent > largest.0 {
                largest = (spent, format!("{name} #{}", r.representation));
            }
            match &r.result {
                Ok((body, report)) => {
                    if !check_topology(body).is_empty() {
                        wrong.push(format!("{name} #{}: topology", r.representation));
                    }
                    faces += body.faces().count();
                    split += report.split_faces.len();
                    dropped += report.dropped_faces.len();
                    let m = ryukei::mesh::tessellate(
                        body,
                        &ryukei::mesh::MeshParams::new(reference_chord(body), 0.5),
                    )
                    .unwrap();
                    unmeshed += m.faults.len();
                }
                // A shape the file doesn't let ryustep read as geometry at
                // all is refused as healing refuses it, and only where it is
                // listed.
                Err(e) => {
                    let refused = PINNED_REFUSED.iter().any(|x| x.0 == name);
                    let healed = opened.body(k, &opts()).result.is_ok();
                    if !refused || healed {
                        wrong.push(format!("{name} #{}: {e:?}", r.representation));
                    }
                }
            }
        }
        println!(
            "references {} {name}    ({faces} faces, {units} units; {split} faces split, {dropped} dropped, {unmeshed} unmeshed)",
            opened.held.len()
        );
    }
    println!("references built in {seconds:.1} s on one thread");
    println!(
        "largest reference {} units ({}), against ryucad's default heal.budget of {}",
        largest.0,
        largest.1,
        1u64 << 25
    );
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// Pinned files whose bodies are refused before a reference is built, with
/// why: what the file gives can't be read as geometry at all, as healing
/// reads it too (`pinned.txt` heals none of their bodies).
const PINNED_REFUSED: &[(&str, &str)] = &[(
    "occt/screw.step",
    "a spindle torus (InvalidGeometry Degenerate; step.md's Limits)",
)];

/// Each `heals` line of `pinned.txt`: the file's path in the cache.
fn pinned_files() -> Vec<String> {
    let listed = std::fs::read_to_string(tests_dir().join("corpus/pinned.txt")).unwrap();
    listed
        .lines()
        .filter_map(|l| l.trim().strip_prefix("heals "))
        .map(|rest| rest.split_once(' ').unwrap().1.to_string())
        .collect()
}

/// The bodies a pinned file heals, each once (a second representation of
/// the same solid is checked once), with tightening on or off.
fn pinned_bodies(name: &str, tighten: bool) -> Vec<(u64, Body)> {
    let path = pinned_dir().join(name);
    let Ok(bytes) = std::fs::read(&path) else {
        panic!("{} is missing: run scripts/fetch-corpus.sh", path.display());
    };
    let mut seen: Vec<String> = Vec::new();
    let mut out = Vec::new();
    let options = ReadOptions {
        key: OpKey(7),
        heal: HealOptions {
            tighten,
            ..HealOptions::default()
        },
    };
    for b in read(&bytes, &options).unwrap().bodies {
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

/// The pairs the check can't decide in the pinned corpus, by file and the
/// faces' import names in hex, each pair in name order: the AP242 twins
/// of NIST cases whose AP203 files check clean (issue #264). A pair not
/// listed fails the check; a listed pair that clears is printed, to be
/// taken off the list. `nist_ftc_06`'s eight cleared in plan 15.C (an
/// edge both faces use isn't crossed with itself); `nist_ftc_07`'s two
/// are each a plane tangent to a fillet torus at a saddle of its tube, at
/// the vertex the faces share, left for a later plan (#264).
const PINNED_UNCHECKED: &[(&str, &str)] = &[
    (
        "NIST-PMI-STEP-Files/nist_ctc_02_asme1_ap242-e2.stp",
        "33f0b149492e8adf6bb148cb901e976e a999ec9c2b833a43b6160fedc6ecc825",
    ),
    (
        "NIST-PMI-STEP-Files/nist_ftc_07_asme1_ap242-e2.stp",
        "0f4e65eba4be4862e9ba1b5cf4bf1812 813cab6f011e62375e3ca126f73bab46",
    ),
    (
        "NIST-PMI-STEP-Files/nist_ftc_07_asme1_ap242-e2.stp",
        "cfb33267bca2ec53f6613331fc8f2503 f8334885217d122514169dbf2d9a7f84",
    ),
];

/// The faults' faces by name, each pair in name order.
fn named(body: &Body, faults: &[ryukei::check::Fault]) -> Vec<(String, String)> {
    use ryukei::check::Fault;
    let name = |f: ryukei::topo::FaceId| body.face(f).unwrap().name.to_u128();
    let mut out: Vec<(String, String)> = faults
        .iter()
        .map(|f| {
            let (kind, a, b) = match *f {
                Fault::CannotCheck { a, b } => ("cannot_check", a, b),
                Fault::FacesCross { a, b } => ("faces_cross", a, b),
                Fault::FacesOverlap { a, b } => ("faces_overlap", a, b),
                ref f => return (format!("{f:?}"), String::new()),
            };
            let (a, b) = (name(a), name(b));
            (
                kind.to_string(),
                format!("{:032x} {:032x}", a.min(b), a.max(b)),
            )
        })
        .collect();
    out.sort();
    out
}

/// Checks every body `files` heals: no faces crossing or overlapping, and
/// nothing left undecided but [`PINNED_UNCHECKED`]'s pairs.
fn check_pinned(files: &[String]) {
    let (mut checked, mut faults, mut seen) = (0, Vec::new(), Vec::new());
    for name in files {
        for (rep, body) in pinned_bodies(name, true) {
            let t = Instant::now();
            let found = named(&body, &ryukei::check::check_intersections(&body));
            println!(
                "{name} #{rep}: {} faces, {:.1} s, {} faults",
                body.counts().faces,
                t.elapsed().as_secs_f64(),
                found.len()
            );
            checked += 1;
            for (kind, pair) in found {
                let listed = kind == "cannot_check"
                    && PINNED_UNCHECKED.contains(&(name.as_str(), pair.as_str()));
                if listed {
                    seen.push((name.clone(), pair));
                } else {
                    faults.push(format!("{name} #{rep}: {kind} {pair}"));
                }
            }
        }
    }
    for (file, pair) in PINNED_UNCHECKED {
        if files.iter().any(|f| f == file) && !seen.iter().any(|(f, p)| f == file && p == pair) {
            println!("cleared, take it off PINNED_UNCHECKED: {file} {pair}");
        }
    }
    println!("{checked} bodies checked");
    assert!(faults.is_empty(), "{faults:#?}");
}

/// Every body the pinned corpus heals passes the self-intersection check
/// (design 94: `CannotCheck` counted, and zero but for the listed pairs).
/// Since plan 15.C no file's check is slow: the five that took
/// over a minute (`linkrods.step` 44 minutes, `nist_ctc_02`'s AP242
/// edition 20, on cox-worker-2 in plan 8.F) were checked by name and are
/// now among the rest. On cox-worker-4 (AMD Opteron 6380, 8 threads,
/// shared at a load of 9 to 12) at 198515ab, the slowest body was
/// `nist_ftc_07`'s AP203 #7830 at 71.8 s, the five once slow 2.5 to 57.9 s
/// (`nist_ctc_05`'s AP203 #6603); on one thread on cox-worker-2 (Ryzen 7
/// 9700X) every NIST body is under 30 s and `linkrods.step` 2.3 s.
#[test]
fn pinned_corpus_checks_clean() {
    check_pinned(&pinned_files());
}

/// A fixed set of the pinned corpus's bodies, each drilled through along
/// z by a rod a twentieth of its size across at the middle of its box,
/// and its longest straight edge between two planes filleted at a
/// hundredth of its size with its tangent chain (halved where the faces
/// round it can't take it, the next longest where none will do): what the
/// cut takes and what it leaves make up the body, and every result checks
/// clean.
#[test]
fn pinned_bodies_drill_and_fillet() {
    use ryukei::BlendError;
    use ryukei::blend::{BlendKind, EdgeBlend, blend};
    use ryukei::boolean::{BooleanOp, Tool, ToolKey, boolean};
    use ryukei::geom::{Curve, Rigid, Surface, Vector3};
    use ryukei::measure::{Entity, Properties, properties};
    let volume = |b: &Body| {
        let Properties::Body { volume, .. } = properties(b, Entity::Body).unwrap() else {
            unreachable!()
        };
        (volume.value(), volume.bound())
    };
    let mut unclean = Vec::new();
    let mut clean = |what: &str, b: &Body| {
        let faults = ryukei::check::check(b);
        let crossing = ryukei::check::check_intersections(b);
        if !faults.is_empty() || !crossing.is_empty() {
            unclean.push(format!("{what}: {faults:?} {crossing:?}"));
        }
    };
    let mut unfilleted = Vec::new();
    for name in PINNED_SET {
        let (_, body) = pinned_bodies(name, true)
            .into_iter()
            .next()
            .expect("a body");
        let bx = body
            .faces()
            .map(|f| ryukei::intersect::face_box(&body, f))
            .reduce(|x, y| x.union(&y))
            .unwrap();
        let size = (bx.max() - bx.min()).length();
        let mid = bx.min() + (bx.max() - bx.min()) * 0.5;
        let h = bx.max().z - bx.min().z + size;
        let place = Rigid::translation(Vector3::new(mid.x, mid.y, bx.min().z - size / 2.0));
        let (rod, _) = ryukei::primitive::cylinder(OpKey(40), size / 40.0, h, &place).unwrap();
        let tools = [Tool {
            key: ToolKey(1),
            body: &rod,
            place: Rigid::IDENTITY,
        }];
        let (cut, _) = boolean(OpKey(41), BooleanOp::Subtract, &body, &tools).unwrap();
        let (kept, _) = boolean(OpKey(42), BooleanOp::Intersect, &body, &tools).unwrap();
        clean(&format!("{name} drilled"), &cut);
        let ((v, vb), (c, cb), (k, kb)) = (volume(&body), volume(&cut), volume(&kept));
        println!("{name}: {v:.9} m³ = {c:.9} drilled + {k:.9} taken");
        assert!((c + k - v).abs() <= vb + cb + kb, "{name}: volumes");
        // The straight edges between two planes, longest first.
        let planar = |f| {
            matches!(
                body.surface(body.face(f).unwrap().surface),
                Some(Surface::Plane(_))
            )
        };
        let len = |e| {
            let x = body.edge(e).unwrap();
            x.interval[1] - x.interval[0]
        };
        let mut edges: Vec<_> = body
            .edges()
            .filter(|&e| {
                let x = body.edge(e).unwrap();
                matches!(body.curve(x.curve), Some(Curve::Line(_)))
                    && body.edge_fins(e).all(|fin| {
                        let lp = body.fin(fin).unwrap().loop_;
                        planar(body.loop_(lp).unwrap().face)
                    })
            })
            .collect();
        edges.sort_by(|&a, &b| len(b).total_cmp(&len(a)));
        assert!(!edges.is_empty(), "{name}: a straight edge between planes");
        // Each with its tangent chain, as ryucad's propagation blends it: an
        // edge tangent to one left sharp is `OpenTangentChain`. A radius the
        // faces round it can't take (`Overflow`, `RadiusTooLarge`) is the
        // model's, so it is halved, up to four times, and an edge refused at
        // every radius gives way to the next longest (five at most).
        let mut refused = Vec::new();
        let found = 'edges: {
            for &edge in edges.iter().take(5) {
                let chain = ryukei::blend::tangent_chain(&body, edge).unwrap();
                let mut radius = size / 100.0;
                loop {
                    let blends: Vec<EdgeBlend> = chain
                        .iter()
                        .map(|&e| EdgeBlend {
                            edge: body.edge(e).unwrap().name,
                            kind: BlendKind::Fillet { radius },
                        })
                        .collect();
                    match blend(OpKey(43), &body, &blends) {
                        Ok((rounded, _)) => break 'edges Some((rounded, radius, chain)),
                        Err(ryukei::Error::InvalidBlend(
                            e @ (BlendError::Overflow { .. } | BlendError::RadiusTooLarge { .. }),
                        )) => {
                            if radius > size / 1600.0 {
                                radius /= 2.0;
                            } else {
                                refused.push(format!("{e:?}"));
                                break;
                            }
                        }
                        Err(e) => panic!("{name}: filleting at {radius} m: {e:?}"),
                    }
                }
            }
            None
        };
        for r in &refused {
            println!("{name}: passed over at every radius: {r}");
        }
        let listed = FILLET_REFUSED.contains(name);
        match found {
            Some((rounded, radius, chain)) => {
                println!("{name}: filleted at {radius:.6} m ({} edges)", chain.len());
                if listed {
                    println!("{name} fillets now: take it off FILLET_REFUSED");
                }
                clean(&format!("{name} filleted"), &rounded);
            }
            None if listed => println!("{name}: no edge filleted (listed)"),
            None => unfilleted.push(name.to_string()),
        }
    }
    assert!(unclean.is_empty(), "{unclean:#?}");
    assert!(unfilleted.is_empty(), "no edge filleted: {unfilleted:?}");
}

/// [`PINNED_SET`]'s bodies healed with tightening off (design 299, plan
/// 14.A), drilled as [`pinned_bodies_drill_and_fillet`] drills them: each
/// must drill, check clean and make up its whole, as its tightened twin
/// does. Fillets on them are plan 14.B's, shells 14.D's.
#[test]
fn pinned_bodies_drill_untightened() {
    use ryukei::boolean::{BooleanOp, Tool, ToolKey, boolean};
    use ryukei::geom::{Rigid, Vector3};
    use ryukei::measure::{Entity, Properties, properties};
    let volume = |b: &Body| {
        let Properties::Body { volume, .. } = properties(b, Entity::Body).unwrap() else {
            unreachable!()
        };
        (volume.value(), volume.bound())
    };
    let mut unclean = Vec::new();
    for name in PINNED_SET {
        let (_, body) = pinned_bodies(name, false)
            .into_iter()
            .next()
            .expect("a body");
        let bx = body
            .faces()
            .map(|f| ryukei::intersect::face_box(&body, f))
            .reduce(|x, y| x.union(&y))
            .unwrap();
        let size = (bx.max() - bx.min()).length();
        let mid = bx.min() + (bx.max() - bx.min()) * 0.5;
        let h = bx.max().z - bx.min().z + size;
        let place = Rigid::translation(Vector3::new(mid.x, mid.y, bx.min().z - size / 2.0));
        let (rod, _) = ryukei::primitive::cylinder(OpKey(40), size / 40.0, h, &place).unwrap();
        let tools = [Tool {
            key: ToolKey(1),
            body: &rod,
            place: Rigid::IDENTITY,
        }];
        let (cut, _) = boolean(OpKey(41), BooleanOp::Subtract, &body, &tools)
            .unwrap_or_else(|e| panic!("{name} untightened, drilled: {e:?}"));
        let (kept, _) = boolean(OpKey(42), BooleanOp::Intersect, &body, &tools)
            .unwrap_or_else(|e| panic!("{name} untightened, taken: {e:?}"));
        for (what, b) in [("drilled", &cut), ("taken", &kept)] {
            let faults = ryukei::check::check(b);
            let crossing = ryukei::check::check_intersections(b);
            if !faults.is_empty() || !crossing.is_empty() {
                unclean.push(format!(
                    "{name} untightened, {what}: {faults:?} {crossing:?}"
                ));
            }
        }
        let ((v, vb), (c, cb), (k, kb)) = (volume(&body), volume(&cut), volume(&kept));
        println!("{name} untightened: {v:.9} m³ = {c:.9} drilled + {k:.9} taken");
        assert!((c + k - v).abs() <= vb + cb + kb, "{name}: volumes");
    }
    assert!(unclean.is_empty(), "{unclean:#?}");
}

/// [`PINNED_SET`]'s bodies healed with and without tightening (design
/// 299, plan 14.B), filleted as [`pinned_bodies_drill_and_fillet`]
/// fillets them: the tightened body's edge and radius, then the edge
/// whose middle is nearest in the untightened body at that radius, which
/// must fillet, check clean and cross nothing, its volume within both
/// bounds of the tightened result's and its tolerances' strip.
/// [`FILLET_REFUSED`]'s bodies fillet neither way.
#[test]
fn pinned_bodies_fillet_untightened() {
    use ryukei::BlendError;
    use ryukei::blend::{BlendKind, EdgeBlend, blend};
    use ryukei::geom::{Curve, Surface};
    use ryukei::measure::{Entity, Properties, properties};
    use ryukei::topo::EdgeId;
    let volume = |b: &Body| {
        let Properties::Body { volume, area } = properties(b, Entity::Body).unwrap() else {
            unreachable!()
        };
        (volume.value(), volume.bound(), area.value())
    };
    let mid = |b: &Body, e: EdgeId| {
        let x = b.edge(e).unwrap();
        b.curve(x.curve)
            .unwrap()
            .point(0.5 * (x.interval[0] + x.interval[1]))
    };
    let fillet = |b: &Body, edge: EdgeId, radius: f64| {
        let chain = ryukei::blend::tangent_chain(b, edge).unwrap();
        let blends: Vec<EdgeBlend> = chain
            .iter()
            .map(|&e| EdgeBlend {
                edge: b.edge(e).unwrap().name,
                kind: BlendKind::Fillet { radius },
            })
            .collect();
        blend(OpKey(43), b, &blends)
    };
    let mut failures = Vec::new();
    for name in PINNED_SET {
        if FILLET_REFUSED.contains(name) {
            continue;
        }
        let (_, tight) = pinned_bodies(name, true)
            .into_iter()
            .next()
            .expect("a body");
        let (_, loose) = pinned_bodies(name, false)
            .into_iter()
            .next()
            .expect("a body");
        let bx = tight
            .faces()
            .map(|f| ryukei::intersect::face_box(&tight, f))
            .reduce(|x, y| x.union(&y))
            .unwrap();
        let size = (bx.max() - bx.min()).length();
        let planar = |f| {
            matches!(
                tight.surface(tight.face(f).unwrap().surface),
                Some(Surface::Plane(_))
            )
        };
        let len = |e: EdgeId| {
            let x = tight.edge(e).unwrap();
            x.interval[1] - x.interval[0]
        };
        let mut edges: Vec<EdgeId> = tight
            .edges()
            .filter(|&e| {
                let x = tight.edge(e).unwrap();
                matches!(tight.curve(x.curve), Some(Curve::Line(_)))
                    && tight.edge_fins(e).all(|fin| {
                        let lp = tight.fin(fin).unwrap().loop_;
                        planar(tight.loop_(lp).unwrap().face)
                    })
            })
            .collect();
        edges.sort_by(|&x, &y| len(y).total_cmp(&len(x)));
        let found = edges.iter().take(5).find_map(|&e| {
            let mut radius = size / 100.0;
            loop {
                match fillet(&tight, e, radius) {
                    Ok((r, _)) => return Some((e, radius, r)),
                    Err(ryukei::Error::InvalidBlend(
                        BlendError::Overflow { .. } | BlendError::RadiusTooLarge { .. },
                    )) if radius > size / 1600.0 => radius /= 2.0,
                    Err(_) => return None,
                }
            }
        });
        let (e, radius, rounded) = found.expect("the tightened body fillets");
        let m = mid(&tight, e);
        let twin = loose
            .edges()
            .min_by(|&x, &y| {
                mid(&loose, x)
                    .distance(m)
                    .total_cmp(&mid(&loose, y).distance(m))
            })
            .unwrap();
        let r = match fillet(&loose, twin, radius) {
            Ok((r, _)) => r,
            Err(err) => {
                failures.push(format!("{name} untightened, at {radius} m: {err:?}"));
                continue;
            }
        };
        let faults = ryukei::check::check(&r);
        let crossing = ryukei::check::check_intersections(&r);
        if !faults.is_empty() || !crossing.is_empty() {
            failures.push(format!("{name} untightened: {faults:?} {crossing:?}"));
            continue;
        }
        let worst = r
            .edges()
            .map(|e| r.edge(e).unwrap().tolerance.get())
            .fold(0.0, f64::max);
        let ((ve, be, ae), (vt, bt, _)) = (volume(&rounded), volume(&r));
        println!("{name} filleted at {radius:.6} m: {ve:.9} m³ tightened, {vt:.9} untightened");
        if (vt - ve).abs() > be + bt + 2.0 * worst * ae {
            failures.push(format!("{name}: volume {vt} against {ve}"));
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

/// The pinned bodies [`pinned_bodies_drill_and_fillet`] drills and
/// fillets: NIST's test cases with a straight edge between planes, AP242
/// but for `nist_ftc_06`, whose AP242 edition leaves pairs unchecked
/// before anything is done to it (issue #264) and whose AP203 file checks
/// clean.
const PINNED_SET: &[&str] = &[
    "NIST-PMI-STEP-Files/nist_ctc_01_asme1_ap242-e1.stp",
    "NIST-PMI-STEP-Files/nist_ctc_03_asme1_ap242-e2.stp",
    "NIST-PMI-STEP-Files/nist_ctc_04_asme1_ap242-e1.stp",
    "NIST-PMI-STEP-Files/AP203 geometry only/nist_ftc_06_asme1_rd.stp",
    "NIST-PMI-STEP-Files/nist_ftc_09_asme1_ap242-e1.stp",
    "NIST-PMI-STEP-Files/nist_stc_06_asme1_ap242-e3.stp",
];

/// The bodies of [`PINNED_SET`] whose five longest straight edges between
/// planes all refuse a fillet at every radius tried. None since issue
/// #265 (NIST CTC 04's AP242 edition, whose arcs and the lines running on
/// from them met 1.2e-10 rad apart, was judged not tangent there).
const FILLET_REFUSED: &[&str] = &[];

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

/// The committed bodies [`pinned_bodies_shell_untightened`] shells
/// besides [`PINNED_SET`]'s (whose tightened shells all refuse, at every
/// thickness tried, on 8.E's `PartlySmooth`, `Fill`, `FaceCollapses` or
/// an uncertain envelope): NIST's FTC 11 in AP203 and the OCCT files
/// ryustep reads, each of which must be compared.
const SHELL_SET: &[&str] = &[
    "NIST-PMI-STEP-Files/AP203 geometry only/nist_ftc_11_asme1_rb.stp",
    "occt/box_ap214.stp",
    "occt/box_inch.stp",
    "occt/cavity.stp",
    "occt/cone.stp",
    "occt/cylinder.stp",
    "occt/elliptic_prism.stp",
    "occt/nurbs_cylinder.stp",
    "occt/plate_hole.stp",
    "occt/sphere.stp",
    "occt/spline_revolve.stp",
    "occt/torus.stp",
];

/// [`PINNED_SET`]'s and [`SHELL_SET`]'s bodies shelled inwards to a
/// cavity (design 299, plan 14.D) a fortieth of their least side deep,
/// else a hundredth or a four-hundredth where the tightened shell
/// refuses (or runs past 90 s): untightened, each must shell wherever its tightened twin
/// does, check clean, and enclose the tightened shell's volume within
/// their tolerance bound. Every body of [`SHELL_SET`] must be compared,
/// so the test never passes by refusing everything.
#[test]
fn pinned_bodies_shell_untightened() {
    use ryukei::measure::{Entity, Properties, properties};
    use ryukei::offset::{ShellSpec, shell, shell_with};
    use ryukei::work::Work;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};
    // A tightened shell that runs past this is counted refused: some of
    // the pinned bodies' shells (NIST CTC 04 at a four-hundredth) run on
    // for many minutes before refusing.
    let within = |b: &Body, spec: &ShellSpec| {
        let (flag, done, start) = (
            AtomicBool::new(false),
            AtomicBool::new(false),
            Instant::now(),
        );
        std::thread::scope(|sc| {
            sc.spawn(|| {
                while !done.load(Ordering::Relaxed) && start.elapsed() < Duration::from_secs(90) {
                    std::thread::sleep(Duration::from_millis(100));
                }
                flag.store(true, Ordering::Relaxed);
            });
            let r = shell_with(OpKey(44), b, spec, &Work::unlimited().with_cancel(&flag));
            done.store(true, Ordering::Relaxed);
            r
        })
    };
    // Volumes at the measure's default chord, else (a shell of the OCCT
    // spline revolution, whose inner face is an offset of the revolution
    // that doesn't mesh at a hundred-thousandth of its size) at a
    // ten-thousandth, the bound widening with it.
    let volume = |b: &Body| match properties(b, Entity::Body) {
        Ok(Properties::Body { volume, area }) => (volume.value(), volume.bound(), area.value()),
        _ => {
            let bx = b
                .faces()
                .map(|f| ryukei::intersect::face_box(b, f))
                .reduce(|x, y| x.union(&y))
                .unwrap();
            let size = (bx.max() - bx.min()).length();
            let opts = ryukei::measure::MassOptions {
                chord: Some(size * 1e-4),
                ..Default::default()
            };
            let m = ryukei::measure::mass(b, 1.0, &opts).unwrap();
            (m.volume.value(), m.volume.bound(), m.area.value())
        }
    };
    let occt = Path::new(env!("CARGO_MANIFEST_DIR")).join("../ryustep/tests/data");
    let mut failures = Vec::new();
    let mut compared = Vec::new();
    for &name in PINNED_SET.iter().chain(SHELL_SET) {
        let path = if name.starts_with("occt/") {
            occt.join(name).display().to_string()
        } else {
            name.to_string()
        };
        let (_, tight) = pinned_bodies(&path, true)
            .into_iter()
            .next()
            .expect("a body");
        let (_, loose) = pinned_bodies(&path, false)
            .into_iter()
            .next()
            .expect("a body");
        let bx = tight
            .faces()
            .map(|f| ryukei::intersect::face_box(&tight, f))
            .reduce(|x, y| x.union(&y))
            .unwrap();
        let ext = bx.max() - bx.min();
        let least = ext.x.min(ext.y).min(ext.z);
        let mut tried = None;
        for div in [40.0, 100.0, 400.0] {
            let spec = ShellSpec {
                thickness: least / div,
                outward: false,
                open: vec![],
                faces: vec![],
            };
            match within(&tight, &spec) {
                Ok((s, _)) => {
                    tried = Some((spec, s));
                    break;
                }
                Err(e) => println!("{name}: tightened, by {} m, refused {e:?}", spec.thickness),
            }
        }
        let Some((spec, shelled)) = tried else {
            continue;
        };
        let t = spec.thickness;
        let r = match shell(OpKey(44), &loose, &spec) {
            Ok((r, _)) => r,
            Err(e) => {
                failures.push(format!("{name} untightened, by {t} m: {e:?}"));
                continue;
            }
        };
        let faults = ryukei::check::check(&r);
        let crossing = ryukei::check::check_intersections(&r);
        if !faults.is_empty() || !crossing.is_empty() {
            failures.push(format!("{name} untightened: {faults:?} {crossing:?}"));
            continue;
        }
        let worst = r
            .edges()
            .map(|e| r.edge(e).unwrap().tolerance.get())
            .fold(0.0, f64::max);
        let ((ve, be, ae), (vt, bt, _)) = (volume(&shelled), volume(&r));
        println!("{name} shelled by {t:.6} m: {ve:.9} m³ tightened, {vt:.9} untightened");
        if (vt - ve).abs() > be + bt + 2.0 * worst * ae {
            failures.push(format!("{name}: volume {vt} against {ve}"));
        }
        compared.push(name);
    }
    for name in SHELL_SET {
        if !compared.contains(name) {
            failures.push(format!(
                "{name}: not compared (its tightened shell refused)"
            ));
        }
    }
    println!(
        "{} of {} bodies compared",
        compared.len(),
        PINNED_SET.len() + SHELL_SET.len()
    );
    assert!(failures.is_empty(), "{failures:#?}");
}

/// How long one pinned body's defeaturing may run before it is
/// cancelled and counted as past the deadline: ryustep's `defeature.rs`'s.
const DEFEATURE_DEADLINE: std::time::Duration = std::time::Duration::from_secs(120);

/// Every body the pinned corpus heals, defeatured (design 399, plan
/// 13.D): its recognised blends no larger than 2 mm deleted. Each result
/// checks clean (`check` and `check_intersections`), or is a typed
/// refusal (`InvalidTweak`), printed with the time it took. One
/// narrowing of plan 13.D's "checks clean": a `CannotCheck` pair the
/// body already had before the delete, by its faces' names, may stay
/// (NIST CTC 02's AP242 edition, #264's `PINNED_UNCHECKED`). That
/// uncertainty is the import's, not the delete's; a crossing, an overlap,
/// or a pair that could be checked before still fails. Each delete is
/// cancelled at [`DEFEATURE_DEADLINE`], as ryustep's `defeature.rs` does
/// (uncancelled, NIST FTC 07's, FTC 10's and STC 07's took 17 to 24
/// minutes each to refuse `ExtensionUnsupported` on cox-worker-4). Run
/// after `scripts/fetch-corpus.sh`, in the devcontainer.
#[test]
fn pinned_bodies_defeature() {
    use ryukei::blend::{BlendKind, Chamfer};
    use ryukei::check::{check, check_intersections};
    use ryukei::tweak::{DeleteBlends, RecogniseOptions, delete_blends_with, recognise_blends};
    use ryukei::work::Work;
    use std::sync::atomic::{AtomicBool, Ordering};
    // A fillet's radius, a chamfer's larger setback.
    let size = |kind: &BlendKind| match *kind {
        BlendKind::Fillet { radius } => radius,
        BlendKind::Chamfer(Chamfer::Equal { distance }) => distance,
        BlendKind::Chamfer(Chamfer::Distances { first, second, .. }) => first.max(second),
        BlendKind::Chamfer(Chamfer::DistanceAngle {
            distance, angle, ..
        }) => distance.max(distance * ryukei::math::tan(angle)),
    };
    let spec = DeleteBlends {
        max_size: Some(2e-3),
        faces: Vec::new(),
        options: RecogniseOptions::default(),
    };
    let (mut defeatured, mut refused, mut none, mut slow) = (0, 0, 0, 0);
    let start = Instant::now();
    for name in pinned_files() {
        for (rep, body) in pinned_bodies(&name, true) {
            let t = Instant::now();
            let blends = match recognise_blends(&body, &spec.options) {
                Ok(r) => r.blends.iter().filter(|b| size(&b.kind) <= 2e-3).count(),
                Err(ryukei::Error::InvalidTweak(e)) => {
                    println!("{name} #{rep}: recognition refused {e:?}");
                    refused += 1;
                    continue;
                }
                Err(e) => panic!("{name} #{rep}: {e:?}"),
            };
            if blends == 0 {
                none += 1;
                continue;
            }
            let flag = AtomicBool::new(false);
            let done = AtomicBool::new(false);
            let r = std::thread::scope(|sc| {
                sc.spawn(|| {
                    while !done.load(Ordering::Relaxed) && t.elapsed() < DEFEATURE_DEADLINE {
                        std::thread::sleep(std::time::Duration::from_millis(100));
                    }
                    flag.store(true, Ordering::Relaxed);
                });
                let r = delete_blends_with(
                    OpKey(0x13d),
                    &body,
                    &spec,
                    &Work::unlimited().with_cancel(&flag),
                );
                done.store(true, Ordering::Relaxed);
                r
            });
            match r {
                Ok((o, _)) => {
                    assert_eq!(check(&o), [], "{name} #{rep}");
                    // Pairs the body couldn't be checked at before the
                    // delete may stay unchecked (`PINNED_UNCHECKED`'s,
                    // #264); anything else is the delete's.
                    let after = named(&o, &check_intersections(&o));
                    if !after.is_empty() {
                        let before = named(&body, &check_intersections(&body));
                        let new: Vec<_> = after
                            .iter()
                            .filter(|f| f.0 != "cannot_check" || !before.contains(f))
                            .collect();
                        assert!(new.is_empty(), "{name} #{rep}: {new:?}");
                        println!("{name} #{rep}: unchecked as before: {after:?}");
                    }
                    println!(
                        "{name} #{rep}: {blends} blends deleted, {} faces to {} in {:.2} s",
                        body.counts().faces,
                        o.counts().faces,
                        t.elapsed().as_secs_f64()
                    );
                    defeatured += 1;
                }
                Err(ryukei::Error::InvalidTweak(e)) => {
                    println!(
                        "{name} #{rep}: {blends} blends, refused {e:?} in {:.2} s",
                        t.elapsed().as_secs_f64()
                    );
                    refused += 1;
                }
                Err(ryukei::Error::Cancelled) => {
                    println!(
                        "{name} #{rep}: {blends} blends, past the deadline ({:.2} s)",
                        t.elapsed().as_secs_f64()
                    );
                    slow += 1;
                }
                Err(e) => panic!("{name} #{rep}: not a tweak's refusal: {e:?}"),
            }
        }
    }
    println!(
        "pinned corpus: {defeatured} defeatured, {refused} refused, {slow} past the deadline, {none} with no blend under 2 mm, in {:.1} s",
        start.elapsed().as_secs_f64()
    );
}
