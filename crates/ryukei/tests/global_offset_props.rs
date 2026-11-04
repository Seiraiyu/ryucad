//! Seeded offsets against their definition (design 299, plan 14.D): the
//! `offset` fuzz target's bodies ([`ryukei_testkit::offset_bytes`]),
//! each offset by one distance either way, local or global as the kernel
//! chooses. Every result checks clean, and every vertex of it lies at the
//! distance from the original's boundary (its faces, edges and vertices),
//! within the result's tolerances and a micron of rounding; a shrunk body
//! lies inside the original and a grown one holds it.
//!
//! Release only (minutes in a debug build): its own step in full.yml.

use ryukei::Error;
use ryukei::check::{check, check_intersections};
use ryukei::geom::{Point2, Point3};
use ryukei::intersect::{Inclusion, Place, point_in_body, point_in_face};
use ryukei::name::OpKey;
use ryukei::offset::{OffsetSpec, offset_body_with};
use ryukei::topo::Body;
use ryukei::work::Work;
use ryukei_testkit::Lcg;
use ryukei_testkit::offset_bytes::{Op, offset_case};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// The distance from `q` to `b`'s boundary: each face's nearest point
/// where its surface's projection lies inside it, and each edge's
/// nearest point (its ends included).
fn boundary_distance(b: &Body, q: Point3) -> f64 {
    let mut best = f64::INFINITY;
    for f in b.faces() {
        let face = b.face(f).unwrap();
        let s = b.surface(face.surface).unwrap();
        let foot = s.project(q);
        let uv = Point2::new(foot.param.0, foot.param.1);
        if matches!(point_in_face(b, f, uv), Ok(Place::Inside)) {
            best = best.min(foot.point.distance(q));
        }
    }
    for e in b.edges() {
        let edge = b.edge(e).unwrap();
        let c = b.curve(edge.curve).unwrap();
        let [t0, t1] = edge.interval;
        let t = c.project(q).param;
        let mut cands = vec![c.point(t0), c.point(t1)];
        if (t0..=t1).contains(&t) {
            cands.push(c.point(t));
        }
        for x in cands {
            best = best.min(x.distance(q));
        }
    }
    best
}

/// Seeds known to run past the deadline: 14 (since the 0.8.x burn-down
/// caps the check's last grown trace, about 390 s on one thread on a Xeon
/// E5-2650, from 616), whose envelope's
/// self-intersection check runs about 11 minutes to end `CannotCheck` on a
/// shared Xeon E5-2650 (inside the deadline on an Apple M4), issue #416:
/// three pipe–torus pairs touching at shared vertices, each trace ending
/// `Uncertain` after about 24 s on one thread (plan 15.C's measurement).
/// Seed 3 (#408): since plan 15.C traces its closing edge from the fins'
/// own parameters it ends, refused `Uncertain`, in 19 to 26 s on
/// cox-worker-2's Ryzen 7 9700X, but past the deadline on an Intel Xeon
/// E5-2650 and an AMD Opteron 6380 (120.6 and 138.6 s), so it stays
/// listed; `offsets.rs`' `a_closing_edge_on_a_spline_offset_traces_quickly`
/// bounds it in work units instead.
const KNOWN_SLOW: &[u64] = &[3, 14];

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

#[test]
#[cfg_attr(debug_assertions, ignore = "minutes in a debug build")]
fn offsets_lie_at_their_distance() {
    let mut rng = Lcg::new(0x14d);
    let (mut built, mut refused) = (0, 0);
    let mut slow = Vec::new();
    for seed in 0..48 {
        let bytes: Vec<u8> = (0..64).map(|_| (rng.next_u64() >> 56) as u8).collect();
        let Some(case) = offset_case(&bytes) else {
            continue;
        };
        let d = match &case.op {
            Op::Offset(spec) => spec.distance,
            Op::Shell(spec) => -spec.thickness,
        };
        let spec = OffsetSpec {
            distance: d,
            faces: Vec::new(),
        };
        let started = std::time::Instant::now();
        let got = within_deadline(|w| {
            offset_body_with(OpKey(0x14d0 + seed as u128), &case.body, &spec, w)
        });
        println!(
            "seed {seed}: {} faces by {d:.4}: {:.2} s",
            case.body.counts().faces,
            started.elapsed().as_secs_f64()
        );
        if matches!(got, Err(Error::Cancelled)) {
            println!("seed {seed}: past the deadline");
            slow.push(seed);
            continue;
        }
        let o = match got {
            Ok((o, _)) => o,
            Err(e) => {
                println!("seed {seed}: refused {e:?}");
                refused += 1;
                continue;
            }
        };
        built += 1;
        assert_eq!(check(&o), [], "seed {seed}");
        assert_eq!(check_intersections(&o), [], "seed {seed}");
        for v in o.vertices() {
            let x = o.vertex(v).unwrap();
            let gap = (boundary_distance(&case.body, x.point) - d.abs()).abs();
            assert!(
                gap <= x.tolerance.get() + 1e-6,
                "seed {seed}: a vertex {gap} m off the distance"
            );
            let want = if d < 0.0 {
                Inclusion::Inside
            } else {
                Inclusion::Outside
            };
            assert_eq!(point_in_body(&case.body, x.point), Ok(want), "seed {seed}");
        }
    }
    println!("{built} built, {refused} refused, past the deadline {slow:?}");
    assert!(built >= 20, "{built} built");
    // Each past the deadline is a known slow local offset (its closing
    // edges traced on a spline's offset, seconds a projection), with its
    // issue; a new one fails.
    for s in &slow {
        assert!(KNOWN_SLOW.contains(s), "seed {s} past the deadline");
    }
}
