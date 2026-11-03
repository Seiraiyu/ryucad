//! Scratch for planning 15.C (never merged): times #260's sweep finds'
//! outcomes and their checks, with per-pair lines under RYUKEI_CHECK_PROBE.
use ryukei::check::check_intersections;
use std::time::Instant;

fn bytes(hex: &str) -> Vec<u8> {
    (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap()).collect()
}

#[test]
fn finds_260() {
    let sweeps = [
        concat!(
            "0adfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdf0000000000000008dfdfdfdfdfdfdfdfdfdfdf",
            "dfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdf",
            "dfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdf",
        ),
        concat!(
            "8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8caf8c8c8c8c8c8c8c8c8c8c8c8c",
            "8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c",
            "8c8c8c8c8c8c8c8c8c8c0a0a",
        ),
    ];
    for hex in sweeps {
        let data = bytes(hex);
        let t = Instant::now();
        let out = ryukei_testkit::along_bytes::outcome(&data);
        eprintln!("{}: outcome in {:.1} s", &hex[..8], t.elapsed().as_secs_f64());
        if let Some((made, _, _)) = out {
            match made {
                Ok(body) => {
                    let t = Instant::now();
                    let f = check_intersections(&body);
                    eprintln!("{}: built, {} faces, check {:.1} s, {:?}", &hex[..8], body.faces().count(), t.elapsed().as_secs_f64(), f);
                }
                Err(e) => eprintln!("{}: refused {e:?}", &hex[..8]),
            }
        }
    }
}

use ryukei::geom::{Curve, Curve2, Frame, KnotVector, Line2, NurbsCurve, Point2, Point3, Vector3};
use ryukei::name::OpKey;
use ryukei::offset::{OffsetSpec, offset_body_with};
use ryukei::sweep::{AlongOptions, Loop, LoopKey, Path, PathSegment, Profile, Segment, SegmentKey, ValidProfile, along};
use ryukei::work::Work;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

fn cancel_after<T>(secs: u64, f: impl FnOnce(&Work) -> T) -> T {
    let flag = AtomicBool::new(false);
    let done = AtomicBool::new(false);
    let start = Instant::now();
    std::thread::scope(|s| {
        s.spawn(|| {
            while !done.load(Ordering::Relaxed) && start.elapsed().as_secs() < secs {
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            flag.store(true, Ordering::Relaxed);
        });
        let w = Work::unlimited().with_cancel(&flag);
        let r = f(&w);
        eprintln!("units {}", w.spent());
        done.store(true, Ordering::Relaxed);
        r
    })
}

#[test]
fn shrunk_sweep_223() {
    let k = KnotVector::clamped(3, &[0.0, 1.0, 2.0], &[4, 1, 4]).unwrap();
    let pts = vec![
        Point3::new(0.0, 0.0, 0.0), Point3::new(1.0, 0.0, 0.0), Point3::new(2.0, 1.0, 0.5),
        Point3::new(3.0, 0.5, 1.5), Point3::new(4.0, 0.0, 2.0),
    ];
    let path = Path { segments: vec![PathSegment { key: SegmentKey(1), curve: Curve::Nurbs(Arc::new(NurbsCurve::new(k, pts, None).unwrap())), range: [0.0, 2.0] }] };
    let frame = Frame::new(Point3::ORIGIN, Vector3::new(1.0, 0.0, 0.0), Vector3::new(0.0, 1.0, 0.0)).unwrap();
    let h = 0.15;
    let q = [(-h, -h), (h, -h), (h, h), (-h, h)];
    let segments = (0..4).map(|i| {
        let (p, r) = (Point2::new(q[i].0, q[i].1), Point2::new(q[(i + 1) % 4].0, q[(i + 1) % 4].1));
        Segment { key: SegmentKey(10 + i as u128), curve: Curve2::Line2(Line2::through(p, r).unwrap()), t0: 0.0, t1: p.distance(r) }
    }).collect();
    let profile = ValidProfile::new(Profile { frame, loops: vec![Loop { key: LoopKey(1), segments }] }).unwrap();
    let (b, _) = along(OpKey(0xd0), &profile, &path, &AlongOptions::default()).unwrap();
    let t = Instant::now();
    let r = cancel_after(900, |w| offset_body_with(OpKey(0xd1), &b, &OffsetSpec { distance: -0.03, faces: Vec::new() }, w));
    eprintln!("223 shrink: {:.1} s {:?}", t.elapsed().as_secs_f64(), r.map(|_| ()));
}

#[test]
fn seed_408() {
    let mut rng = ryukei_testkit::Lcg::new(0x14d);
    for seed in 0..48u64 {
        let bytes: Vec<u8> = (0..64).map(|_| (rng.next_u64() >> 56) as u8).collect();
        if seed != 3 { continue; }
        let case = ryukei_testkit::offset_bytes::offset_case(&bytes).unwrap();
        let d = match &case.op {
            ryukei_testkit::offset_bytes::Op::Offset(s) => s.distance,
            ryukei_testkit::offset_bytes::Op::Shell(s) => -s.thickness,
        };
        let t = Instant::now();
        let r = cancel_after(600, |w| offset_body_with(OpKey(0x14d0 + seed as u128), &case.body, &OffsetSpec { distance: d, faces: Vec::new() }, w));
        eprintln!("408 seed 3 by {d}: {} faces, {:.1} s {:?}", case.body.faces().count(), t.elapsed().as_secs_f64(), r.map(|_| ()));
    }
}
