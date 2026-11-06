//! Every body of the committed corpus and the OCCT files read as a
//! reference (design 364, plan 21.A): a sheet whose topology holds, every
//! face meshed but those `UNMESHED` lists, built in under a second on one
//! thread in release; and, in release, named as the healed body is.

use ryukei::check::check_topology;
use ryukei::geom::Box3;
use ryukei::mesh::{MeshParams, tessellate};
use ryukei::name::{Name, OpKey};
use ryukei::topo::{Body, BodyKind};
use ryustep::{ReadOptions, open};
use std::collections::BTreeSet;
use std::path::PathBuf;

/// Bodies refused before a reference is built, with why: what the file
/// gives can't be read as geometry at all, as healing reads it too.
const REFUSED: &[(&str, &str)] = &[(
    "offset_sheet.stp",
    "its offset surface folds (InvalidGeometry Degenerate; step-hunt's ledger: file)",
)];

/// Faces a reference leaves unmeshed, by file and representation, with
/// how many and the issue for each. Empty until a body needs a line.
const UNMESHED: &[(&str, u64, usize)] = &[];

fn files() -> Vec<PathBuf> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests");
    let mut out = Vec::new();
    for dir in [root.join("corpus"), root.join("data/occt")] {
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            let ext = p.extension().and_then(|x| x.to_str()).unwrap_or("");
            if ext.eq_ignore_ascii_case("stp") || ext.eq_ignore_ascii_case("step") {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

fn opts() -> ReadOptions {
    ReadOptions {
        key: OpKey(7),
        heal: Default::default(),
    }
}

/// A body's size: the box of its vertices (a face's box can be the
/// whole size box), else of its faces for a body with no vertex.
fn size(b: &Body) -> f64 {
    Box3::from_points(b.vertices().map(|v| b.vertex(v).unwrap().point))
        .or_else(|| {
            b.faces()
                .map(|f| ryukei::intersect::face_box(b, f))
                .reduce(|x, y| x.union(&y))
        })
        .map(|x| (x.max() - x.min()).length())
        .unwrap()
}

fn faces(b: &Body) -> BTreeSet<Name> {
    b.faces().map(|f| b.face(f).unwrap().name).collect()
}

#[test]
fn every_body_builds_and_meshes_as_a_reference() {
    let mut wrong = Vec::new();
    let mut seconds = 0.0;
    for path in files() {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let bytes = std::fs::read(&path).unwrap();
        let opened = open(&bytes).unwrap();
        for k in 0..opened.held.len() {
            let start = std::time::Instant::now();
            let r = rayon::ThreadPoolBuilder::new()
                .num_threads(1)
                .build()
                .unwrap()
                .install(|| opened.reference(k, &opts()));
            seconds += start.elapsed().as_secs_f64();
            let rep = r.representation;
            let (body, report) = match r.result {
                Ok(x) => x,
                // A shape the file doesn't let ryustep read at all, or that
                // isn't a solid's (a PMI sheet is a reference too).
                Err(e) => {
                    let refused = REFUSED.iter().any(|x| x.0 == name);
                    let healed = opened.body(k, &opts()).result.is_ok();
                    if !refused || healed {
                        wrong.push(format!("{name} #{rep}: {e:?}"));
                    }
                    continue;
                }
            };
            if !check_topology(&body).is_empty() {
                wrong.push(format!("{name} #{rep}: topology"));
            }
            if body.kind() != BodyKind::Sheet {
                wrong.push(format!("{name} #{rep}: not a sheet"));
            }
            let m = tessellate(&body, &MeshParams::new(size(&body) * 1e-2, 0.5)).unwrap();
            let allowed = UNMESHED
                .iter()
                .find(|x| x.0 == name && x.1 == rep)
                .map_or(0, |x| x.2);
            if m.faults.len() != allowed {
                wrong.push(format!(
                    "{name} #{rep}: {} faces unmeshed, {allowed} listed",
                    m.faults.len()
                ));
            }
            println!(
                "{name} #{rep}: {} faces, {} split, {} dropped, {} unmeshed",
                body.faces().count(),
                report.split_faces.len(),
                report.dropped_faces.len(),
                m.faults.len()
            );
        }
    }
    println!("references built in {seconds:.2} s on one thread");
    assert!(wrong.is_empty(), "{wrong:#?}");
    // The design's bar: every committed body in under a second, one
    // thread, release (ten times looser without RYUCAD_BARS).
    if !cfg!(debug_assertions) {
        let bar = if std::env::var_os("RYUCAD_BARS").is_some() {
            1.0
        } else {
            10.0
        };
        assert!(seconds < bar, "{seconds:.2} s, bar {bar} s");
    }
}

/// Healing the corpus takes minutes in a debug build.
#[test]
#[cfg_attr(debug_assertions, ignore)]
fn references_keep_the_face_names_healing_gives() {
    let mut wrong = Vec::new();
    for path in files() {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let bytes = std::fs::read(&path).unwrap();
        let opened = open(&bytes).unwrap();
        for k in 0..opened.held.len() {
            let healed = opened.body(k, &opts());
            let Ok((h, _, report)) = &healed.result else {
                continue;
            };
            let Ok((r, _)) = &opened.reference(k, &opts()).result else {
                wrong.push(format!("{name} #{}: refused", healed.representation));
                continue;
            };
            let (hf, rf) = (faces(h), faces(r));
            // A body united from its solids, or repaired, is named by the
            // boolean that made it: only bodies healed face for face compare.
            if !report.repaired.is_empty() || !hf.iter().all(|n| healed.instances.contains_key(n)) {
                continue;
            }
            let collapsed: BTreeSet<Name> = report.collapsed.iter().copied().collect();
            if !hf.is_subset(&rf) || !rf.difference(&hf).all(|n| collapsed.contains(n)) {
                wrong.push(format!(
                    "{name} #{}: face names differ",
                    healed.representation
                ));
            }
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}
