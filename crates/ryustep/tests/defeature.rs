//! Imported bodies defeatured (design 399, plans 13.D and 16.C): every
//! body the committed corpus and the OCCT files heal has its blends
//! recognised, and every blend no larger than 2 mm deleted on one
//! thread. Question 425's bar: each result is a body that checks clean
//! (`check` and `check_intersections`) with no recognised blend under
//! 2 mm left, within the deadline; the bodies `KNOWN_REFUSED` lists,
//! each with its issue, refuse typed (`InvalidTweak`) or run past the
//! deadline. The deadline is asserted only with `RYUKEI_BARS` (where a
//! known body past it fails too), else printed. Release only (minutes in
//! a debug build): its own step in full.yml.

use ryukei::Error;
use ryukei::blend::{BlendKind, EdgeBlend, blend};
use ryukei::check::{check, check_intersections, check_intersections_with};
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

/// Committed bodies whose small blends are still refused, each with its
/// issue: NIST FTC 07 and FTC 10, kept faces tangent along a seam whose
/// carried working faces meet uncertain (plan 16.C, question 517).
const KNOWN_REFUSED: &[(&str, u32)] = &[("nist_ftc_07", 652), ("nist_ftc_10", 652)];

/// How a body's delete ended, for [`Tally::count`].
#[derive(Clone, Copy, Debug)]
enum End {
    /// Refused typed (`InvalidTweak`).
    Refused,
    /// Cancelled at [`DEADLINE`].
    Cancelled,
}

/// The bodies refused and those past the deadline.
#[derive(Default, Debug, PartialEq)]
struct Tally {
    refused: usize,
    slow: usize,
}

impl Tally {
    /// Counts a body's refusal or cancel. A refusal must be `known`
    /// (in [`KNOWN_REFUSED`]). A known body's cancel is its known
    /// outcome, counted as refused, except with `bars`, where the
    /// deadline holds for every body: there, and for any other body, a
    /// cancel is past the deadline.
    fn count(&mut self, known: bool, end: End, bars: bool) {
        match end {
            End::Refused => {
                assert!(known, "refused, and not in KNOWN_REFUSED");
                self.refused += 1;
            }
            End::Cancelled if known && !bars => self.refused += 1,
            End::Cancelled => self.slow += 1,
        }
    }

    /// Question 425's bar: exactly the `KNOWN_REFUSED` bodies refused
    /// (or, without `bars`, past the deadline), and with `bars` none past
    /// it. Without `bars` a slow body is printed, not asserted: full.yml
    /// runs this on hosted runners about twice as slow as the bar machine.
    fn assert_bar(&self, bars: bool) {
        assert_eq!(
            self.refused,
            KNOWN_REFUSED.len(),
            "{} refused",
            self.refused
        );
        if bars {
            assert_eq!(self.slow, 0, "{} past the deadline", self.slow);
        }
    }
}

/// Whether `RYUKEI_BARS` is set: the deadline asserted for every body.
fn bars() -> bool {
    std::env::var_os("RYUKEI_BARS").is_some()
}

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
    let bars = bars();
    let (mut defeatured, mut none) = (0, 0);
    let mut tally = Tally::default();
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
        let known = KNOWN_REFUSED.iter().any(|(f, _)| file.contains(f));
        for (i, imported) in m.bodies.iter().enumerate() {
            let Ok((body, _, _)) = &imported.result else {
                continue;
            };
            let t = Instant::now();
            let small = match small_blends(body) {
                Ok(n) => n,
                Err(Error::InvalidTweak(e)) => {
                    println!("{file} #{i}: recognition refused {e:?}");
                    tally.count(known, End::Refused, bars);
                    continue;
                }
                Err(e) => panic!("{file} #{i}: {e:?}"),
            };
            if small == 0 {
                none += 1;
                println!("{file} #{i}: no blend under 2 mm");
                continue;
            }
            let one = rayon::ThreadPoolBuilder::new()
                .num_threads(1)
                .build()
                .unwrap();
            match one
                .install(|| within_deadline(|w| delete_blends_with(OpKey(0x13d), body, &spec, w)))
            {
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
                    assert!(known, "{file} #{i}: refused {e:?}");
                    tally.count(known, End::Refused, bars);
                }
                Err(Error::Cancelled) => {
                    println!("{file} #{i}: {small} blends, past the deadline");
                    tally.count(known, End::Cancelled, bars);
                }
                Err(e) => panic!("{file} #{i}: not a tweak's refusal: {e:?}"),
            }
        }
    }
    println!(
        "{defeatured} bodies defeatured, {} refused, {} past the deadline, {none} with no small blend",
        tally.refused, tally.slow
    );
    // Question 425's bar: every committed body with blends under 2 mm
    // defeatured, checked clean; those `KNOWN_REFUSED` names, with their
    // issues, refused or past the deadline. Within the deadline on one
    // thread is asserted only with `RYUKEI_BARS` on the bar machine, else
    // printed.
    tally.assert_bar(bars);
}

/// The tally's rules: a known body cancelled is its known outcome
/// without `RYUKEI_BARS` and past the deadline with it; any other body
/// cancelled is past the deadline, asserted only with it.
#[test]
fn a_known_bodys_cancel_counts_as_refused_except_under_the_bars() {
    let ends = |bars: bool, ends: &[(bool, End)]| {
        let mut t = Tally::default();
        for &(known, end) in ends {
            t.count(known, end, bars);
        }
        t
    };
    let known = [(true, End::Refused), (true, End::Cancelled)];
    let t = ends(false, &known);
    assert_eq!(
        t,
        Tally {
            refused: 2,
            slow: 0
        }
    );
    t.assert_bar(false);
    t.assert_bar(true);
    let t = ends(true, &known);
    assert_eq!(
        t,
        Tally {
            refused: 1,
            slow: 1
        }
    );
    // An unknown body past the deadline: printed, asserted only with bars.
    let other = [
        (true, End::Refused),
        (true, End::Refused),
        (false, End::Cancelled),
    ];
    let t = ends(false, &other);
    assert_eq!(
        t,
        Tally {
            refused: 2,
            slow: 1
        }
    );
    t.assert_bar(false);
    let bar = std::panic::catch_unwind(|| t.assert_bar(true));
    assert!(bar.is_err(), "a slow body passed the bars");
    // A refusal not in KNOWN_REFUSED fails either way.
    for bars in [false, true] {
        let r = std::panic::catch_unwind(|| Tally::default().count(false, End::Refused, bars));
        assert!(r.is_err(), "an unknown refusal passed");
    }
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

/// A cancel stops deleting NIST FTC 10's blends at once (plan 13.D: its
/// re-trim fitted a carried spline's working sheet for 17 minutes past a
/// cancel, unpolled). Cancelled 5 s in, it answers within 5 s more.
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "minutes in a debug build: CI runs it in release"
)]
fn a_cancel_stops_deleting_ftc_10s_blends() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/nist_ftc_10_asme1_rb.stp");
    let m = read(
        &std::fs::read(path).unwrap(),
        &ReadOptions {
            key: OpKey(7),
            heal: Default::default(),
        },
    )
    .unwrap();
    let (body, _, _) = m.bodies[0].result.clone().unwrap();
    let spec = DeleteBlends {
        max_size: Some(MAX_SIZE),
        faces: Vec::new(),
        options: RecogniseOptions::default(),
    };
    let flag = AtomicBool::new(false);
    let start = Instant::now();
    let (r, stopped) = std::thread::scope(|s| {
        s.spawn(|| {
            std::thread::sleep(Duration::from_secs(5));
            flag.store(true, Ordering::Relaxed);
        });
        let r = delete_blends_with(
            OpKey(0x13d),
            &body,
            &spec,
            &Work::unlimited().with_cancel(&flag),
        );
        (r, start.elapsed())
    });
    assert!(matches!(r, Err(Error::Cancelled)), "{:?}", r.err());
    assert!(
        stopped < Duration::from_secs(10),
        "stopped after {stopped:?}"
    );
}

/// NIST FTC 09's countersinks: each hole's wall is two half cylinders on
/// one surface and its chamfer two half cones. Deleting one hole's
/// chamfer leaves the hole a plain cylinder, checked clean (#651: the
/// halves' working faces coincided and the walk closed two ways).
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "seconds in release, minutes in a debug build: CI runs it in release"
)]
fn ftc_09s_split_holes_lose_their_countersinks() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/nist_ftc_09_asme1_rd.stp");
    let m = read(
        &std::fs::read(path).unwrap(),
        &ReadOptions {
            key: OpKey(7),
            heal: Default::default(),
        },
    )
    .unwrap();
    let (body, _, _) = m.bodies[0].result.clone().unwrap();
    let r = recognise_blends(&body, &RecogniseOptions::default()).unwrap();
    // The first chamfer chain: one hole's two half cones.
    let chain = r
        .blends
        .iter()
        .find(|b| matches!(b.kind, BlendKind::Chamfer(_)))
        .unwrap()
        .chain;
    let faces: Vec<_> = r
        .blends
        .iter()
        .filter(|b| b.chain == chain)
        .flat_map(|b| b.faces.clone())
        .collect();
    assert_eq!(faces.len(), 2);
    let spec = DeleteBlends {
        max_size: None,
        faces,
        options: RecogniseOptions::default(),
    };
    let (o, _) = delete_blends_with(OpKey(0x16c), &body, &spec, &Work::unlimited()).unwrap();
    assert_eq!(check(&o), []);
    assert_eq!(check_intersections(&o), []);
    // The two cones went and the two half cylinders became one face.
    assert_eq!(o.counts().faces, body.counts().faces - 3);
}

/// Deleting one of NIST CTC 04's fillets (517 faces) spends fewer units
/// than checking the whole result once: the re-trim checks only the pairs
/// with a face it made. Every rung checked every pair, 4 s of the
/// delete's 8.2 s on cox-worker-2.
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "seconds in release, minutes in a debug build: CI runs it in release"
)]
fn a_delete_checks_only_its_own_pairs() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/nist_ctc_04_asme1_rd.stp");
    let m = read(
        &std::fs::read(path).unwrap(),
        &ReadOptions {
            key: OpKey(7),
            heal: Default::default(),
        },
    )
    .unwrap();
    let (body, _, _) = m.bodies[0].result.clone().unwrap();
    let r = recognise_blends(&body, &RecogniseOptions::default()).unwrap();
    // The least-named fillet of 0.5 to 0.6 mm.
    let pick = r
        .blends
        .iter()
        .filter(
            |b| matches!(b.kind, BlendKind::Fillet { radius } if (5e-4..6e-4).contains(&radius)),
        )
        .min_by_key(|b| b.faces[0])
        .unwrap();
    let spec = DeleteBlends {
        max_size: None,
        faces: pick.faces.clone(),
        options: RecogniseOptions::default(),
    };
    let w = Work::unlimited();
    let (o, _) = delete_blends_with(OpKey(0x16c), &body, &spec, &w).unwrap();
    let c = Work::unlimited();
    assert_eq!(check_intersections_with(&o, &c).unwrap(), []);
    println!(
        "delete {} units, one whole check {} units",
        w.spent(),
        c.spent()
    );
    assert!(w.spent() < c.spent(), "{} against {}", w.spent(), c.spent());
}

/// NIST FTC 08's pocket floor: 16 fillets (8 cylinders, 8 tori) round
/// its floor, one set, deleted and checked clean (#653).
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "seconds in release, minutes in a debug build: CI runs it in release"
)]
fn ftc_08s_pocket_floor_loses_its_fillets() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/nist_ftc_08_asme1_rc.stp");
    let m = read(
        &std::fs::read(path).unwrap(),
        &ReadOptions {
            key: OpKey(7),
            heal: Default::default(),
        },
    )
    .unwrap();
    let (body, _, _) = m.bodies[0].result.clone().unwrap();
    let r = recognise_blends(&body, &RecogniseOptions::default()).unwrap();
    // The chain of 16 blends, all 1.524 mm.
    let mut sizes: std::collections::BTreeMap<usize, usize> = Default::default();
    for b in &r.blends {
        *sizes.entry(b.chain).or_default() += 1;
    }
    let chain = sizes
        .iter()
        .find(|(_, n)| **n == 16)
        .map(|(c, _)| *c)
        .unwrap();
    let faces: Vec<_> = r
        .blends
        .iter()
        .filter(|b| b.chain == chain)
        .flat_map(|b| b.faces.clone())
        .collect();
    let spec = DeleteBlends {
        max_size: None,
        faces,
        options: RecogniseOptions::default(),
    };
    let (o, _) = delete_blends_with(OpKey(0x16c), &body, &spec, &Work::unlimited()).unwrap();
    assert_eq!(check(&o), []);
    assert_eq!(check_intersections(&o), []);
    assert_eq!(o.counts().faces, body.counts().faces - 16);
}

/// NIST FTC 07's and FTC 10's blends under 2 mm (87 and 86) deleted and
/// checked clean within the bar's 120 s on one thread (#652). The time is
/// asserted only with `RYUKEI_BARS=1`, on the bar machine.
///
/// Still refused (plan 16.C): each refusal is a pair of kept region faces
/// sharing one edge along which they are tangent (a pipe-like round's
/// straight run against its corner torus, cylinders and spheres against a
/// NURBS face), whose working faces, carried past that seam, meet
/// tangentially along it; the trace can't certify it within its cap
/// (3 to 34 s a pair) and refuses `Uncertain`. FTC 07 in 85 s, FTC 10 in
/// 65 s, one thread on cox-worker-2 (AMD Ryzen 7 9700X).
#[test]
#[ignore = "#652: kept faces tangent along a seam, carried past it, meet uncertain"]
fn ftc_07s_and_ftc_10s_blends_are_deleted() {
    let one = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap();
    for name in ["nist_ftc_07_asme1_rd.stp", "nist_ftc_10_asme1_rb.stp"] {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/corpus")
            .join(name);
        let m = read(
            &std::fs::read(path).unwrap(),
            &ReadOptions {
                key: OpKey(7),
                heal: Default::default(),
            },
        )
        .unwrap();
        let (body, _, _) = m.bodies[0].result.clone().unwrap();
        let spec = DeleteBlends {
            max_size: Some(MAX_SIZE),
            faces: Vec::new(),
            options: RecogniseOptions::default(),
        };
        let t = Instant::now();
        let (o, _) = one
            .install(|| within_deadline(|w| delete_blends_with(OpKey(0x13d), &body, &spec, w)))
            .unwrap();
        let s = t.elapsed().as_secs_f64();
        println!(
            "{name}: {} faces to {} in {s:.1} s",
            body.counts().faces,
            o.counts().faces
        );
        assert_eq!(check(&o), []);
        assert_eq!(check_intersections(&o), []);
        assert_eq!(small_blends(&o).unwrap(), 0);
        if std::env::var("RYUKEI_BARS").as_deref() == Ok("1") {
            assert!(s < 120.0, "{name}: {s} s");
        }
    }
}

/// Where `scripts/fetch-corpus.sh` keeps the pinned corpus.
fn pinned_dir() -> PathBuf {
    match std::env::var_os("RYUCAD_CORPUS") {
        Some(d) => PathBuf::from(d),
        None => Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/corpus"),
    }
}

/// Each `heals` line of `pinned.txt`: the file's path in the cache.
fn pinned_files() -> Vec<String> {
    let listed = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/pinned.txt"),
    )
    .unwrap();
    listed
        .lines()
        .filter_map(|l| l.trim().strip_prefix("heals "))
        .map(|rest| rest.split_once(' ').unwrap().1.to_string())
        .collect()
}

/// The bodies a pinned file heals with tightening on, each once (a second
/// representation of the same solid is defeatured once).
fn pinned_bodies(name: &str) -> Vec<(u64, Body)> {
    let path = pinned_dir().join(name);
    let Ok(bytes) = std::fs::read(&path) else {
        panic!("{} is missing: run scripts/fetch-corpus.sh", path.display());
    };
    let mut seen: Vec<String> = Vec::new();
    let mut out = Vec::new();
    let options = ReadOptions {
        key: OpKey(7),
        heal: ryukei::heal::HealOptions {
            tighten: true,
            ..Default::default()
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

/// Every body the pinned corpus heals (tightening on), defeatured on one
/// thread under the deadline (design 399, plans 13.D and 16.C): its
/// recognised blends no larger than 2 mm deleted. Each result checks
/// clean (`check` and `check_intersections`), or is a typed refusal
/// (`InvalidTweak`) or past the deadline, printed with the time it took.
/// The design records the pinned editions and doesn't hold them to the
/// bar, so only soundness is asserted. One narrowing of "checks clean":
/// a `CannotCheck` pair the body already had before the delete, by its
/// faces' names, may stay (NIST CTC 02's AP242 edition, #264). That
/// uncertainty is the import's, not the delete's; a crossing, an
/// overlap, or a pair that could be checked before still fails. Moved
/// from ryukei-bench, where it needed the `occt` feature though it uses
/// no OCCT. Run after `scripts/fetch-corpus.sh`.
#[test]
#[ignore = "needs scripts/fetch-corpus.sh: run with --ignored"]
fn pinned_bodies_defeature() {
    let spec = DeleteBlends {
        max_size: Some(MAX_SIZE),
        faces: Vec::new(),
        options: RecogniseOptions::default(),
    };
    let (mut defeatured, mut refused, mut none, mut slow) = (0, 0, 0, 0);
    let start = Instant::now();
    let one = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap();
    for name in pinned_files() {
        for (rep, body) in pinned_bodies(&name) {
            let t = Instant::now();
            let blends = match small_blends(&body) {
                Ok(n) => n,
                Err(Error::InvalidTweak(e)) => {
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
            match one
                .install(|| within_deadline(|w| delete_blends_with(OpKey(0x13d), &body, &spec, w)))
            {
                Ok((o, _)) => {
                    assert_eq!(check(&o), [], "{name} #{rep}");
                    // Pairs the body couldn't be checked at before the
                    // delete may stay unchecked (#264); anything else is
                    // the delete's.
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
                Err(Error::InvalidTweak(e)) => {
                    println!(
                        "{name} #{rep}: {blends} blends, refused {e:?} in {:.2} s",
                        t.elapsed().as_secs_f64()
                    );
                    refused += 1;
                }
                Err(Error::Cancelled) => {
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
