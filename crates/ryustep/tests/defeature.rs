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
/// carried working faces meet uncertain (plan 16.C, question 517). Plan
/// 18.C's known seams don't reach them: their seams lie on the frontier
/// or are only partly smooth (#652, ryukei 19). Ryukei 19's held seams
/// and stops settle FTC 07's seams, but both still end at a pair of
/// working faces carried far along their straight directions that cross
/// nearly tangent with no edge between them, which no seam certificate
/// meets (plan 19.B, question 882: FTC 07's cone/cylinder past its
/// stopped run, FTC 10's cylinder continuing its torus's tube). Plan
/// 20.A's narrowed build gets FTC 07 to rung 2, where a cylinder and a
/// NURBS face (`18dae4b7`/`33b57350`) carried with no shared edge refuse
/// `Uncertain`; FTC 10's narrowed retry refuses at a plane paired with
/// itself. Plan 20.B's near boxes part both pairs (their cuts' tests), and
/// its related overlapping stretches settle the plane (its split tied two
/// stretches lying on each other). What stops each now (#652, plan 20.B):
/// FTC 07's narrowed walks clip at faces of the grown ring touching no
/// change, and rung 3 refuses at a torus and a plane touching none,
/// overlapping; FTC 10's walk is `HealAmbiguous` at a kept 1 mm ball slit
/// by a carried NURBS face's curve ending inside it, as its first builds
/// at rungs 0 and 1 are. Plan 21.A pins those faces touching no change in
/// narrowed builds, so FTC 07's ring clip and its torus/plane pair are
/// never met; it now refuses at rung 2, in half the time, where a carried
/// NURBS face touching a change (`33b57350`) meets a pinned thin plane
/// (`f19ffa9e`) `Uncertain`, with no edge between them (#652; moved to
/// ryukei 22, "Working faces against pins and near-box limits": working
/// NURBS `33b57350` growing into pin `f19ffa9e`). FTC 10's slit is left to
/// ryukei 21.C. Plan 21.B's certified miss changes neither: FTC 07 refuses
/// at the same rung-2 pair in the same time as on main, and FTC 10 as
/// before. Plan 21.C's seed retry doesn't settle FTC 10 either (census VI,
/// #652): the slit is the ball's held seam with carried NURBS `a497893e`,
/// the ball's one piece is its whole working sphere (the grown faces meet
/// it only at points), and no seed yields: the ball's shortcut seed holds
/// its old face and the NURBS quad's is rule 3's own (moved to ryukei 22,
/// "a kept ball bounded only by a held seam"). Plan 21.D changes neither
/// (FTC 07 and FTC 10 stop where 21.C left them). Plan 22.A leaves FTC
/// 07 rd (22.B) and FTC 10 rb (22.C) where they were; FTC 07 spends its
/// adjusted builds first (31 s to 56 s, inside the deadline). Plan 22.B
/// keeps a working face off a pin it shares nothing with, but working
/// NURBS `33b57350` and pin `f19ffa9e` share a vertex (census VII), and
/// the pin's box meets the NURBS' own box on every axis (they touch at
/// that vertex), so no one-sided cut keeps the own box and clears the
/// pin (census VII's INSIDE): FTC 07 rd still stops at that pair in 56 s
/// (#652; moved to ryukei 23, "a pin touching the working face at a
/// vertex, its box meeting the face's own box").
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
/// cancel, unpolled). Cancelled 5 s in, it answers within 5 s more. On a
/// pool of its own (#939): on the global pool it waited for another
/// test's whole check, on Windows.
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
    let own = rayon::ThreadPoolBuilder::new()
        .num_threads(2)
        .build()
        .unwrap();
    let flag = AtomicBool::new(false);
    let start = Instant::now();
    let (r, stopped) = std::thread::scope(|s| {
        s.spawn(|| {
            std::thread::sleep(Duration::from_secs(5));
            flag.store(true, Ordering::Relaxed);
        });
        let r = own.install(|| {
            delete_blends_with(
                OpKey(0x13d),
                &body,
                &spec,
                &Work::unlimited().with_cancel(&flag),
            )
        });
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
///
/// Plan 18.C's known seams settle none of them: every failing pair has a
/// face outside the region across a frontier edge, or an edge tangent
/// only partly (`PartlySmooth`) with NURBS pcurves (#652's census; ryukei
/// 19).
///
/// Plans 19.A and 19.B hold those seams and stop the working faces at
/// them (iso and plane stops, box-apart parts). What stops both now is
/// recorded on #652 (plan 19.B, question 882): FTC 07's stopped build
/// walks into its quarter runs carried a whole turn, and past that a
/// cone and a cylinder carried about 0.5 m cross nearly tangent with no
/// edge between them; FTC 10's working cylinder continues its torus's
/// tube straight on, tangent to it along an inner circle. The plane/plane
/// pair census I saw was an artefact of its skipped pairs.
///
/// Plan 20.B parts those pairs (near boxes; the tube contact in narrowed
/// builds) and recorded what stops both next; plan 21.A's pins move FTC
/// 07 on to its next cause (see [`KNOWN_REFUSED`]).
#[test]
#[ignore = "#652: FTC 10's kept ball bounded only by a held seam (ryukei 22.C); FTC 07's working NURBS 33b57350 meets pin f19ffa9e, which touches it at a vertex, its box meeting the NURBS' own box (ryukei 23)"]
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

/// Pinned editions that must defeature and check clean, by file and
/// representation: the 17 plan 18.C saw defeatured (16.C's), and the
/// seam-cause editions plan 19.B's held seams and stops settle: CTC 02's
/// geometry-only edition (#20754), its bends' `Uncertain` tangency now
/// held (question 917). A refusal or a deadline for any of them fails.
/// Still out (#652, #658): FTC 07 AP242 #13541 (a pair of working faces
/// with no shared edge refuses `Uncertain`, after its narrowed retry at
/// a cylinder/cone pair); STC 07 #11020 (the rung's ordered sum, plan
/// 20.A, bounds its builds: refused `Uncertain` in 106 s, inside the
/// deadline);
/// STC 10 #3044 (a torus/cylinder pair with no shared edge, then its
/// narrowed retry at a sphere paired with itself); CTC 02 with PMI #295
/// (carried near-tangent cylinders with no shared edge, recorded under
/// question 882); linkrods #11 (still past the deadline: its first build,
/// now bounded by the ordered sum, takes 65 s, one NURBS/torus pair
/// costing about 4 µs a unit, and the narrowed retry of its held NURBS
/// pair 52 s more).
///
/// Plan 20.B's near boxes part FTC 07 AP242's first pair, STC 10's and
/// CTC 02 with PMI's, and none of the five defeatures yet (#652, #658):
/// FTC 07 AP242's retry refuses at a cylinder and a cone carried with no
/// shared edge (`79b561f3`/`178d3260`, overlapping); STC 10's split can't
/// group two sphere faces holding a lone pole; CTC 02 with PMI's retry
/// refuses at a plane and a NURBS face touching no change, overlapping
/// (`74fca8b1`/`b150cb1a`); STC 07 still refuses inside the deadline and
/// linkrods still runs past it (122 s), as on main.
///
/// Plan 21.A pins narrowed builds' faces touching no change: the same 18
/// defeature, bit for bit as on main, and none more (#652, #658). CTC 02
/// with PMI's pair is never met, and its pinned retry clips a working
/// plane touching the change at its near box (`1294043e`, `GapNotHealed`
/// in 11 s; moved to ryukei 22, "Working faces against pins and near-box
/// limits": `1294043e` clipped at its near box in the pinned retry); STC
/// 07 now refuses in 64 s; FTC 07 AP242 (21.B), STC 10
/// (21.D) and FTC 10 (21.C) refuse as before; linkrods is still past the
/// deadline (out of scope, question 984).
///
/// Plan 21.B tries a certified miss after a failed trace at faces sharing
/// nothing, outside first builds: the same 18 defeature, bit for bit as
/// on main, and none more. FTC 07 AP242 #13541 still refuses at its retry
/// pair in 36 s: its cylinder and cone (`79b561f3`/`178d3260`) are each
/// certified to cross the other's surface, the pin grazing the bore's
/// wall, so neither keeps off the other; it needs an exact cylinder/cone
/// intersection (#658; moved to ryukei 22). STC 10 (21.D) and FTC 10
/// (21.C) refuse as before.
///
/// Plan 21.D places lone pole vertices by the cycles passing them, and
/// groups slits to a pole as slits: the same 18 defeature, bit for bit as
/// on main, and none more. STC 10 #3044's balls were never lone poles
/// (census VI): each holds a touch on its equator and a slit ending at a
/// tolerant vertex within reach of its pole, which lifted as a winding
/// cycle. They split now, and STC 10 now stops at its re-trim's walk,
/// clipped at working plane `a7b86e65` in the narrowed retry and at rung
/// 3 (`GapNotHealed` in 20 s; #658; moved to ryukei 22, "STC 10: working
/// plane clipped in the narrowed retry").
///
/// Plan 22.A gives room to a working face clipped in a narrowed build
/// (and to the faces whose clipped sides its piece reached): the same 18
/// defeature and none more. Room holds each clip, and the next cause is
/// behind it (#658; moved to ryukei 23). CTC 02 with PMI #295: with room
/// twice, `1294043e` runs past pinned cylinder `e153cd0e`, which should
/// cut it (a wrong pin; `GapNotHealed` in 12 s). STC 10 #3044: the clip
/// was coplanar plane `ccab7076`'s side; given room, the walk is
/// ambiguous at outside cylinder `e098ee50`, as rungs 0 and 1 are
/// (`GapNotHealed` in 28 s). Refusals spend their adjusted builds first:
/// FTC 07 rd 31 s to 55 s, FTC 09 e1 3 s to 10 s, FTC 10 e2 27 s to 37 s,
/// all inside the deadline.
///
/// Plan 22.B keeps a working face off a pin it shares nothing with (its
/// near box cut back short of the pin): the same 18 defeature and none
/// more, every answer and time as on main. No narrowed build in the
/// corpus refuses at such a pair (census VII); FTC 07 rd's pair shares a
/// vertex (#652; moved to ryukei 23).
const PINNED_DEFEATURED: &[(&str, u64)] = &[
    ("nist_ctc_01_asme1_ap203.stp", 148),
    ("nist_ctc_01_asme1_rd.stp", 13),
    ("nist_ctc_02_asme1_rc.stp", 20754),
    ("nist_ctc_03_asme1_ap203.stp", 154),
    ("nist_ctc_03_asme1_rc.stp", 13),
    ("nist_ctc_04_asme1_ap203.stp", 559),
    ("nist_ctc_04_asme1_rd.stp", 14149),
    ("nist_ctc_05_asme1_ap203.stp", 91),
    ("nist_ctc_05_asme1_ap242-e1.stp", 1177),
    ("nist_ctc_05_asme1_rd.stp", 6603),
    ("nist_ftc_06_asme1_ap242-e2.stp", 2155),
    ("nist_ftc_06_asme1_rd.stp", 13),
    ("nist_ftc_08_asme1_ap242-e2.stp", 1711),
    ("nist_ftc_08_asme1_rc.stp", 7969),
    ("nist_ftc_09_asme1_rd.stp", 5785),
    ("nist_stc_06_asme1_ap242-e3.stp", 5079),
    ("nist_stc_08_asme1_ap242-e3.stp", 1781),
    ("nist_stc_09_asme1_ap242-e3.stp", 4622),
];

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
            let must = PINNED_DEFEATURED
                .iter()
                .any(|(f, r)| name.ends_with(f) && *r == rep);
            if blends == 0 {
                assert!(!must, "{name} #{rep}: must defeature (plans 18.C, 19.A)");
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
                    assert!(
                        !must,
                        "{name} #{rep}: must defeature (plans 18.C, 19.A): {e:?}"
                    );
                    println!(
                        "{name} #{rep}: {blends} blends, refused {e:?} in {:.2} s",
                        t.elapsed().as_secs_f64()
                    );
                    refused += 1;
                }
                Err(Error::Cancelled) => {
                    assert!(
                        !must,
                        "{name} #{rep}: must defeature (plans 18.C, 19.A), past the deadline"
                    );
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
