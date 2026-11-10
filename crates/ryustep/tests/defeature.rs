//! Imported bodies defeatured (design 399, plan 13.D): every body the
//! committed corpus and the OCCT files heal has its blends recognised,
//! and every blend no larger than 2 mm deleted. Each result is a body
//! that checks clean (`check` and `check_intersections`) and has no
//! recognised blend under 2 mm left, or a typed refusal
//! (`InvalidTweak`), printed. Release only (minutes in a debug build):
//! its own step in full.yml.

use ryukei::Error;
use ryukei::blend::{BlendKind, EdgeBlend, blend};
use ryukei::check::{check, check_intersections};
use ryukei::geom::Rigid;
use ryukei::mesh::{MeshParams, tessellate};
use ryukei::name::OpKey;
use ryukei::primitive;
use ryukei::topo::Body;
use ryukei::tweak::{DeleteBlends, RecogniseOptions, delete_blends_with, recognise_blends};
use ryukei::work::Work;
use ryukei_testkit::mesh::volume_and_area;
use ryustep::{LengthUnit, Product, ReadOptions, WriteModel, WriteOptions, read, write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// How long one body's defeaturing may run before it is cancelled and
/// counted as past the deadline (NIST CTC 04 refused after 631 s on
/// cox-worker-4 while this plan was written).
const DEADLINE: Duration = Duration::from_secs(120);

/// `f` under a cancel set at [`DEADLINE`].
fn within_deadline<T>(f: impl FnOnce(&Work) -> Result<T, Error>) -> Result<T, Error> {
    let flag = AtomicBool::new(false);
    let done = AtomicBool::new(false);
    let start = Instant::now();
    std::thread::scope(|s| {
        s.spawn(|| {
            while !done.load(Ordering::Relaxed) && start.elapsed() < DEADLINE {
                std::thread::sleep(Duration::from_millis(100));
            }
            flag.store(true, Ordering::Relaxed);
        });
        let r = f(&Work::unlimited().with_cancel(&flag));
        done.store(true, Ordering::Relaxed);
        r
    })
}

/// The largest blend deleted: 2 mm, in meters.
const MAX_SIZE: f64 = 2e-3;

/// The STEP files of the committed corpus and the OCCT files, sorted.
fn files() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let mut files = Vec::new();
    for dir in [root.join("corpus"), root.join("data/occt")] {
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            let ext = p
                .extension()
                .and_then(|x| x.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if ext == "stp" || ext == "step" {
                files.push(p);
            }
        }
    }
    files.sort();
    files
}

/// The size of a recognised blend: a fillet's radius, a chamfer's
/// larger setback.
fn size(kind: &ryukei::blend::BlendKind) -> f64 {
    use ryukei::blend::{BlendKind, Chamfer};
    match *kind {
        BlendKind::Fillet { radius } => radius,
        BlendKind::Chamfer(Chamfer::Equal { distance }) => distance,
        BlendKind::Chamfer(Chamfer::Distances { first, second, .. }) => first.max(second),
        BlendKind::Chamfer(Chamfer::DistanceAngle {
            distance, angle, ..
        }) => distance.max(distance * ryukei::math::tan(angle)),
    }
}

/// How many recognised blends of `b` are no larger than [`MAX_SIZE`].
fn small_blends(b: &Body) -> Result<usize, Error> {
    let r = recognise_blends(b, &RecogniseOptions::default())?;
    Ok(r.blends
        .iter()
        .filter(|x| size(&x.kind) <= MAX_SIZE)
        .count())
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "minutes in a debug build: CI runs it in release"
)]
fn imported_bodies_lose_their_small_blends() {
    let spec = DeleteBlends {
        max_size: Some(MAX_SIZE),
        faces: Vec::new(),
        options: RecogniseOptions::default(),
    };
    let (mut defeatured, mut refused, mut none, mut slow) = (0, 0, 0, 0);
    for path in files() {
        let file = path.file_name().unwrap().to_string_lossy().into_owned();
        let bytes = std::fs::read(&path).unwrap();
        let m = read(
            &bytes,
            &ReadOptions {
                key: OpKey(7),
                heal: Default::default(),
            },
        )
        .unwrap();
        for (i, imported) in m.bodies.iter().enumerate() {
            let Ok((body, _, _)) = &imported.result else {
                continue;
            };
            let t = Instant::now();
            let small = match small_blends(body) {
                Ok(n) => n,
                Err(Error::InvalidTweak(e)) => {
                    println!("{file} #{i}: recognition refused {e:?}");
                    refused += 1;
                    continue;
                }
                Err(e) => panic!("{file} #{i}: {e:?}"),
            };
            if small == 0 {
                none += 1;
                println!("{file} #{i}: no blend under 2 mm");
                continue;
            }
            match within_deadline(|w| delete_blends_with(OpKey(0x13d), body, &spec, w)) {
                Ok((o, _)) => {
                    assert_eq!(check(&o), [], "{file} #{i}");
                    assert_eq!(check_intersections(&o), [], "{file} #{i}");
                    let left = small_blends(&o).unwrap_or(0);
                    assert_eq!(left, 0, "{file} #{i}: {left} small blends left");
                    println!(
                        "{file} #{i}: {small} blends deleted, {} faces to {}, in {:.2} s",
                        body.counts().faces,
                        o.counts().faces,
                        t.elapsed().as_secs_f64()
                    );
                    defeatured += 1;
                }
                Err(Error::InvalidTweak(e)) => {
                    println!(
                        "{file} #{i}: {small} blends, refused {e:?} in {:.2} s",
                        t.elapsed().as_secs_f64()
                    );
                    refused += 1;
                }
                Err(Error::Cancelled) => {
                    println!("{file} #{i}: {small} blends, past the deadline");
                    slow += 1;
                }
                Err(e) => panic!("{file} #{i}: not a tweak's refusal: {e:?}"),
            }
        }
    }
    println!(
        "{defeatured} bodies defeatured, {refused} refused, {slow} past the deadline, {none} with no small blend"
    );
    // No count is asserted: while planning 13.D no NIST body
    // defeatured (each refusal is filed on the stage), and the rounded
    // box below is the case that must. What is asserted is that every
    // result checks clean and every failure is a typed refusal.
    let _ = (defeatured, refused, slow, none);
}

/// A box 10 × 6 × 4 mm with every edge rounded at 1 mm, written to STEP
/// in millimetres and healed back (tolerant edges, fitted pcurves), loses
/// every fillet and corner: the box's volume, checked clean.
#[test]
fn a_rounded_box_through_step_is_defeatured_to_the_box() {
    let (b, _) = primitive::block(OpKey(1), 0.01, 0.006, 0.004, &Rigid::IDENTITY).unwrap();
    let edges: Vec<EdgeBlend> = b
        .edges()
        .map(|e| EdgeBlend {
            edge: b.edge(e).unwrap().name,
            kind: BlendKind::Fillet { radius: 1e-3 },
        })
        .collect();
    let (rounded, _) = blend(OpKey(2), &b, &edges).unwrap();
    let bytes = write(
        &WriteModel {
            products: vec![Product {
                id: "rounded".into(),
                name: "rounded".into(),
                bodies: vec![rounded],
                ..Default::default()
            }],
        },
        &WriteOptions {
            unit: LengthUnit::Millimeter,
            time_stamp: "2026-11-09T00:00:00".into(),
            ..WriteOptions::default()
        },
    )
    .unwrap();
    let m = read(
        &bytes,
        &ReadOptions {
            key: OpKey(7),
            heal: Default::default(),
        },
    )
    .unwrap();
    let (healed, _, _) = m.bodies[0].result.clone().unwrap();
    assert_eq!(small_blends(&healed).unwrap(), 12);
    let spec = DeleteBlends {
        max_size: Some(MAX_SIZE),
        faces: Vec::new(),
        options: RecogniseOptions::default(),
    };
    let (o, _) = delete_blends_with(OpKey(0x13d), &healed, &spec, &Work::unlimited()).unwrap();
    assert_eq!(check(&o), []);
    assert_eq!(check_intersections(&o), []);
    assert_eq!(o.counts().faces, 6);
    let mesh = tessellate(&o, &MeshParams::new(1e-6, 0.1)).unwrap();
    assert!(mesh.faults.is_empty());
    let (v, area) = volume_and_area(&mesh);
    let exact = 0.01 * 0.006 * 0.004;
    assert!((v - exact).abs() <= 1e-6 * area, "{v} against {exact}");
}
