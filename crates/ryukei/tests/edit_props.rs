//! Seeded direct edits (design 399, plan 13.D): the `edit` fuzz target's
//! cases ([`ryukei_testkit::edit_bytes`]): faces and features deleted,
//! topology-changing tweaks, blends deleted and resized. Every result
//! checks clean and meshes; its volume lies within mass's bound of its
//! mesh's (plus the mesh's own error); its journal accounts for every
//! face the body had; a blend resized to its own size gives the body's
//! names back and its volume within both bounds; one thread and eight
//! give the same bits. Every refusal is typed (`InvalidTweak`). And
//! seeded blends ([`ryukei_testkit::blend_bytes`]) are recognised back
//! to their kinds and sizes.
//!
//! Release only (minutes in a debug build): its own step in full.yml.

use ryukei::blend::{BlendKind, Chamfer, blend};
use ryukei::check::{check, check_intersections};
use ryukei::measure::{MassOptions, Value, mass};
use ryukei::mesh::{MeshParams, check_mesh, tessellate};
use ryukei::name::{Event, Journal, Name, OpKey};
use ryukei::topo::Body;
use ryukei::tweak::{RecogniseOptions, recognise_blends};
use ryukei::work::Work;
use ryukei::{Error, TweakError};
use ryukei_testkit::Lcg;
use ryukei_testkit::blend_bytes::blend_case;
use ryukei_testkit::edit_bytes::{Op, edit_case, run};
use ryukei_testkit::mesh::volume_and_area;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// How many seeds are tried.
const SEEDS: u64 = 128;

/// How many of them must build (the rest refused, typed). Set by Task 3
/// of plan 13.D from what the seeds give.
const MIN_BUILT: usize = 32;

/// Seeds known to run past the deadline, each with its issue.
///
/// - 76: an offset of a face of a 19-face body traces past two minutes
///   (#659).
///
/// The `edit` fuzz target's first find is slow the same way, and isn't a
/// seed here: an offset with `Topology::Change` of a face of a 16-face
/// body refuses `Uncertain` after about 200 s (#647; replayed, cancelled,
/// by `fuzz_regressions.rs`' `a_cancel_stops_an_edits_working_sheets`).
const KNOWN_SLOW: &[u64] = &[76];

/// Seeds of `seeded_blends_are_recognised_back` whose blend isn't read
/// back, each with why:
///
/// - 32 and 92: chamfers on a cone setting back more than
///   `CHAMFER_SHARE` (0.4) of a support's width, which recognition leaves
///   out by design (plan 13.C);
/// - 39, 100 and 102: a chamfer along a line of three edges a cylinder's
///   hole splits, a chamfer ending on a torus, and a chamfer between a
///   disc and a cone coaxial with another (#660).
const KNOWN_UNRECOGNISED: &[u64] = &[32, 39, 92, 100, 102];

/// Seeds whose result the mesher refuses at a thousandth of its size,
/// each with its issue: their volume isn't judged, the rest is.
const KNOWN_UNMESHED: &[u64] = &[];

/// How long a case may run before it is cancelled.
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

fn names(b: &Body) -> Vec<Name> {
    let mut n: Vec<Name> = b
        .faces()
        .map(|f| b.face(f).unwrap().name)
        .chain(b.edges().map(|e| b.edge(e).unwrap().name))
        .chain(b.vertices().map(|v| b.vertex(v).unwrap().name))
        .collect();
    n.sort();
    n
}

/// The length of the body's box's diagonal.
fn size(b: &Body) -> f64 {
    let bx = b
        .faces()
        .map(|f| ryukei::intersect::face_box(b, f))
        .reduce(|a, c| a.union(&c))
        .unwrap();
    (bx.max() - bx.min()).length()
}

/// Every face the body had is in the result, or the journal says what
/// became of it.
fn journal_accounts_for_every_face(before: &Body, after: &Body, j: &Journal, seed: u64) {
    let named = |n: Name| {
        j.events().iter().any(|e| match e {
            Event::Deleted(x) | Event::Modified(x) => *x == n,
            Event::Split { parent, .. } => *parent == n,
            Event::Merged { parents, .. } => parents.contains(&n),
            Event::Replaced { old, .. } => *old == n,
            _ => false,
        })
    };
    for f in before.faces() {
        let n = before.face(f).unwrap().name;
        assert!(
            after.by_name(n).is_some() || named(n),
            "seed {seed}: face {n:?} is gone and the journal doesn't say so"
        );
    }
}

/// The volume and its bound at a thousandth of the body's size, after
/// checking its mesh there against mass's value.
fn measured(o: &Body, seed: u64) -> Value {
    let chord = size(o) * 1e-3;
    let m = tessellate(o, &MeshParams::new(chord, 0.2)).unwrap();
    assert_eq!(m.faults, [], "seed {seed}");
    check_mesh(o, &m).unwrap();
    let (meshed, area) = volume_and_area(&m);
    let p = mass(
        o,
        1.0,
        &MassOptions {
            chord: Some(chord),
            ..MassOptions::default()
        },
    )
    .unwrap();
    let gap = (p.volume.value() - meshed).abs();
    assert!(
        gap <= p.volume.bound() + 2.0 * chord * area,
        "seed {seed}: mass {:?} against the mesh's {meshed}",
        p.volume
    );
    p.volume
}

fn bytes(rng: &mut Lcg) -> Vec<u8> {
    (0..96).map(|_| (rng.next_u64() >> 56) as u8).collect()
}

#[test]
#[cfg_attr(debug_assertions, ignore = "minutes in a debug build")]
fn seeded_edits_check_clean() {
    let one = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap();
    let eight = rayon::ThreadPoolBuilder::new()
        .num_threads(8)
        .build()
        .unwrap();
    let mut rng = Lcg::new(0x13d);
    let (mut built, mut refused) = (0, 0);
    let mut kinds = std::collections::BTreeMap::<&str, [usize; 2]>::new();
    let mut slow = Vec::new();
    for seed in 0..SEEDS {
        let data = bytes(&mut rng);
        let Some(case) = edit_case(&data) else {
            continue;
        };
        let key = OpKey(0x13d00 + u128::from(seed));
        let started = Instant::now();
        let got = one.install(|| within_deadline(|w| run(&case, key, w)));
        println!(
            "seed {seed}: {} on {} faces: {:.2} s",
            case.op.kind(),
            case.body.counts().faces,
            started.elapsed().as_secs_f64()
        );
        let tally = kinds.entry(case.op.kind()).or_default();
        let (o, j) = match got {
            Ok(made) => made,
            Err(Error::Cancelled) => {
                println!("seed {seed}: past the deadline");
                slow.push(seed);
                continue;
            }
            Err(Error::InvalidTweak(e)) => {
                println!("seed {seed}: refused {e:?}");
                refused += 1;
                tally[1] += 1;
                continue;
            }
            Err(e) => panic!("seed {seed}: not a tweak's refusal: {e:?}"),
        };
        built += 1;
        tally[0] += 1;
        assert_eq!(check(&o), [], "seed {seed}");
        assert_eq!(check_intersections(&o), [], "seed {seed}");
        journal_accounts_for_every_face(&case.body, &o, &j, seed);
        let (again, _) = eight
            .install(|| run(&case, key, &Work::unlimited()))
            .unwrap();
        assert_eq!(again.dump(), o.dump(), "seed {seed}: eight threads differ");
        if KNOWN_UNMESHED.contains(&seed) {
            continue;
        }
        let v = measured(&o, seed);
        if let Op::Resize { factor, .. } = case.op
            && factor == 1.0
        {
            assert_eq!(names(&o), names(&case.body), "seed {seed}: names moved");
            let w = measured(&case.body, seed);
            assert!(
                (v.value() - w.value()).abs() <= v.bound() + w.bound(),
                "seed {seed}: resized to its own size, {v:?} against {w:?}"
            );
        }
    }
    println!("{built} built, {refused} refused, past the deadline {slow:?}");
    for (k, [b, r]) in &kinds {
        println!("{k}: {b} built, {r} refused");
    }
    assert!(built >= MIN_BUILT, "{built} built");
    for s in &slow {
        assert!(KNOWN_SLOW.contains(s), "seed {s} past the deadline");
    }
}

/// Two kinds alike within `tol`: a fillet's radius, a chamfer's
/// distances either way round (a `Distances` chamfer is recognised with
/// its supports in name order), a `DistanceAngle` chamfer by the distance
/// it gives.
fn alike(a: &BlendKind, b: &BlendKind, tol: f64) -> bool {
    let near = |x: f64, y: f64| (x - y).abs() <= tol;
    let distances = |k: &BlendKind| match *k {
        BlendKind::Chamfer(Chamfer::Equal { distance }) => Some([distance, distance]),
        BlendKind::Chamfer(Chamfer::Distances { first, second, .. }) => Some([first, second]),
        _ => None,
    };
    match (a, b) {
        (BlendKind::Fillet { radius: x }, BlendKind::Fillet { radius: y }) => near(*x, *y),
        (BlendKind::Chamfer(Chamfer::DistanceAngle { distance: d, .. }), other)
        | (other, BlendKind::Chamfer(Chamfer::DistanceAngle { distance: d, .. })) => {
            // Recognition gives a DistanceAngle chamfer as its distances;
            // the other setback depends on the wedge, so only the one
            // given is compared.
            distances(other).is_some_and(|[p, q]| near(p, *d) || near(q, *d))
        }
        _ => match (distances(a), distances(b)) {
            (Some([p, q]), Some([r, s])) => {
                (near(p, r) && near(q, s)) || (near(p, s) && near(q, r))
            }
            _ => false,
        },
    }
}

/// A surface's kind, for reports.
fn kind_of(s: &ryukei::geom::Surface) -> &'static str {
    use ryukei::geom::Surface;
    match s {
        Surface::Plane(_) => "a plane",
        Surface::Cylinder(_) => "a cylinder",
        Surface::Cone(_) => "a cone",
        Surface::Sphere(_) => "a sphere",
        Surface::Torus(_) => "a torus",
        Surface::Nurbs(_) => "a spline",
        Surface::Pipe(_) => "a pipe",
        _ => "another surface",
    }
}

/// Seeded blends recognised back: each blended edge's blend faces (what
/// the journal replaced it with) are one recognised blend of the kind
/// and size it was made with, or a ball corner's. Chamfers on curved
/// rails (ruled splines) aren't blends to recognition (plan 13.C), and
/// are counted, not judged.
#[test]
#[cfg_attr(debug_assertions, ignore = "minutes in a debug build")]
fn seeded_blends_are_recognised_back() {
    let mut rng = Lcg::new(0x13e);
    let (mut judged, mut ruled, mut missed) = (0, 0, Vec::new());
    for seed in 0..SEEDS {
        let data = bytes(&mut rng);
        let Some(case) = blend_case(&data) else {
            continue;
        };
        let Ok((b, j)) = blend(OpKey(0x13e00 + u128::from(seed)), &case.body, &case.blends) else {
            continue;
        };
        let r = match recognise_blends(&b, &RecogniseOptions::default()) {
            Ok(r) => r,
            Err(Error::InvalidTweak(TweakError::Uncertain { .. })) => {
                println!("seed {seed}: recognition uncertain");
                continue;
            }
            Err(e) => panic!("seed {seed}: {e:?}"),
        };
        for eb in &case.blends {
            let Some(new) = j.events().iter().find_map(|e| match e {
                Event::Replaced { old, new } if *old == eb.edge => Some(new.clone()),
                _ => None,
            }) else {
                continue;
            };
            let surface = |n: Name| {
                let f = b.faces().find(|&f| b.face(f).unwrap().name == n).unwrap();
                b.surface(b.face(f).unwrap().surface).unwrap().clone()
            };
            if matches!(eb.kind, BlendKind::Chamfer(_))
                && matches!(surface(new[0]), ryukei::geom::Surface::Nurbs(_))
            {
                ruled += 1;
                continue;
            }
            judged += 1;
            let ok = match r.holding(new[0]) {
                Some(Ok(i)) => alike(&r.blends[i].kind, &eb.kind, 1e-9 * size(&b).max(1.0)),
                Some(Err(_)) => true,
                None => false,
            };
            if !ok {
                let read = r
                    .holding(new[0])
                    .and_then(|h| h.ok())
                    .map(|i| r.blends[i].kind);
                println!(
                    "seed {seed}: {:?} on {} not recognised back as {:?}: read {read:?}",
                    new,
                    kind_of(&surface(new[0])),
                    eb.kind
                );
                missed.push(seed);
            }
        }
    }
    println!("{judged} blends judged, {ruled} ruled chamfers counted, missed in {missed:?}");
    assert!(judged > 0);
    for s in &missed {
        assert!(
            KNOWN_UNRECOGNISED.contains(s),
            "seed {s} not recognised back"
        );
    }
}
