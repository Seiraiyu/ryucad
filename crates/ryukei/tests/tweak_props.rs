//! Seeded tweaks and drafts (design 384, plan 9.C): the `tweak` fuzz
//! target's cases ([`ryukei_testkit::tweak_bytes`]). Every result keeps
//! the original's names, checks clean and meshes; its volume lies within
//! mass's bound of its mesh's (plus the mesh's own error); a draft's
//! faces' normals lie within its report's angles; one thread and eight
//! give the same bits. Every refusal is typed (`InvalidTweak`).
//!
//! Release only (minutes in a debug build): its own step in full.yml.

use ryukei::Error;
use ryukei::check::{check, check_intersections};
use ryukei::measure::{MassOptions, mass};
use ryukei::mesh::{BodyMesh, MeshParams, check_mesh, tessellate};
use ryukei::name::{Journal, Name, OpKey};
use ryukei::topo::Body;
use ryukei::tweak::{
    DraftReport, DraftSpec, draft_with, move_faces_with, offset_faces_with, replace_faces_with,
};
use ryukei::work::Work;
use ryukei_testkit::Lcg;
use ryukei_testkit::mesh::volume_and_area;
use ryukei_testkit::tweak_bytes::{Op, TweakCase, tweak_case};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// How many seeds are tried.
const SEEDS: u64 = 64;

/// How many of them must build (the rest refused, typed).
const MIN_BUILT: usize = 16;

/// Seeds known to run past the deadline, each with its issue. Empty
/// until Task 4 of plan 9.C measures them.
const KNOWN_SLOW: &[u64] = &[];

/// Seeds whose result the mesher refuses at a thousandth of its size,
/// each with its issue: their volume and angles aren't judged, the rest
/// is. Empty since plan 17.A bounded offsets' enclosures by their
/// distance and halved their derivative bounds where loose (#599).
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

type Made = (Body, Journal, Option<DraftReport>);

fn run(case: &TweakCase, key: OpKey, w: &Work) -> Result<Made, Error> {
    let body = &case.body;
    match &case.op {
        Op::Move(s) => move_faces_with(key, body, s, w).map(|(b, j)| (b, j, None)),
        Op::Offset(s) => offset_faces_with(key, body, s, w).map(|(b, j)| (b, j, None)),
        Op::Replace(s) => replace_faces_with(key, body, s, w).map(|(b, j)| (b, j, None)),
        Op::Draft(s) => draft_with(key, body, s, w).map(|(b, j, r)| (b, j, Some(r))),
    }
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

/// Every mesh corner of each drafted face has its normal at an angle from
/// the pull within the report's least and greatest (either way round: a
/// face drafted against the pull reads negative).
fn angles_within(o: &Body, m: &BodyMesh, spec: &DraftSpec, r: &DraftReport, seed: u64) {
    let pull = spec.pull.get();
    for a in &r.faces {
        let f = o
            .faces()
            .find(|&f| o.face(f).unwrap().name == a.face)
            .expect("a drafted face");
        let fm = m.faces.iter().find(|x| x.face == f).expect("meshed");
        let inside = |x: f64| x >= a.least - 1e-6 && x <= a.greatest + 1e-6;
        for c in &m.corners[fm.corners.start as usize..fm.corners.end as usize] {
            let t = ryukei::math::asin(c.normal.get().dot(pull).clamp(-1.0, 1.0));
            assert!(
                inside(t) || inside(-t),
                "seed {seed}: a corner at {t} rad, the report [{}, {}]",
                a.least,
                a.greatest
            );
        }
    }
}

#[test]
#[cfg_attr(debug_assertions, ignore = "minutes in a debug build")]
fn seeded_tweaks_check_clean_and_keep_their_names() {
    let one = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap();
    let eight = rayon::ThreadPoolBuilder::new()
        .num_threads(8)
        .build()
        .unwrap();
    let mut rng = Lcg::new(0x9c);
    let (mut built, mut refused) = (0, 0);
    let mut slow = Vec::new();
    for seed in 0..SEEDS {
        let bytes: Vec<u8> = (0..96).map(|_| (rng.next_u64() >> 56) as u8).collect();
        let Some(case) = tweak_case(&bytes) else {
            continue;
        };
        let key = OpKey(0x9c00 + u128::from(seed));
        let started = Instant::now();
        let got = one.install(|| within_deadline(|w| run(&case, key, w)));
        println!(
            "seed {seed}: {} on {} faces: {:.2} s",
            case.op.kind(),
            case.body.counts().faces,
            started.elapsed().as_secs_f64()
        );
        let (o, _, report) = match got {
            Ok(made) => made,
            Err(Error::Cancelled) => {
                println!("seed {seed}: past the deadline");
                slow.push(seed);
                continue;
            }
            Err(Error::InvalidTweak(e)) => {
                println!("seed {seed}: refused {e:?}");
                refused += 1;
                continue;
            }
            Err(e) => panic!("seed {seed}: not a tweak's refusal: {e:?}"),
        };
        built += 1;
        assert_eq!(names(&o), names(&case.body), "seed {seed}");
        assert_eq!(check(&o), [], "seed {seed}");
        assert_eq!(check_intersections(&o), [], "seed {seed}");
        if KNOWN_UNMESHED.contains(&seed) {
            let (again, _, _) = eight
                .install(|| run(&case, key, &Work::unlimited()))
                .unwrap();
            assert_eq!(again.dump(), o.dump(), "seed {seed}: eight threads differ");
            continue;
        }
        let chord = size(&o) * 1e-3;
        let m = tessellate(&o, &MeshParams::new(chord, 0.2)).unwrap();
        assert_eq!(m.faults, [], "seed {seed}");
        check_mesh(&o, &m).unwrap();
        let (meshed, area) = volume_and_area(&m);
        let p = mass(
            &o,
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
        if let (Some(r), Op::Draft(spec)) = (&report, &case.op) {
            angles_within(&o, &m, spec, r, seed);
        }
        let (again, _, _) = eight
            .install(|| run(&case, key, &Work::unlimited()))
            .unwrap();
        assert_eq!(again.dump(), o.dump(), "seed {seed}: eight threads differ");
    }
    println!("{built} built, {refused} refused, past the deadline {slow:?}");
    assert!(built >= MIN_BUILT, "{built} built");
    for s in &slow {
        assert!(KNOWN_SLOW.contains(s), "seed {s} past the deadline");
    }
}
