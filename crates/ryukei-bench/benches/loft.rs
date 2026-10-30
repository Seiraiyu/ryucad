//! Phase 8 benchmarks (design 94, plan 8.F): lofts and sweeps along
//! paths. A frustum of squares, the NURBS race's pin (four wavy closed
//! cubics) and block (a bicubic top), and a square along a planar spline.

use divan::black_box;
use ryukei::geom::{Curve, Curve2, Frame, KnotVector, Line2, NurbsCurve, Point2, Point3, Vector3};
use ryukei::name::OpKey;
use ryukei::sweep::{
    AlongOptions, LoftOptions, Loop, LoopKey, Path, PathSegment, Profile, Segment, SegmentKey,
    ValidProfile, along, loft,
};
use ryukei_testkit::nurbs_race;
use std::sync::Arc;

fn main() {
    divan::main();
}

fn square(first: u128, s: f64) -> Loop {
    let h = s / 2.0;
    let pts = [(-h, -h), (h, -h), (h, h), (-h, h)].map(|(x, y)| Point2::new(x, y));
    Loop {
        key: LoopKey(1),
        segments: (0..4)
            .map(|i| {
                let (p, q) = (pts[i], pts[(i + 1) % 4]);
                Segment {
                    key: SegmentKey(first + i as u128),
                    curve: Curve2::Line2(Line2::through(p, q).unwrap()),
                    t0: 0.0,
                    t1: p.distance(q),
                }
            })
            .collect(),
    }
}

fn at(z: f64, l: Loop) -> ValidProfile {
    ValidProfile::new(Profile {
        frame: Frame::new(
            Point3::new(0.0, 0.0, z),
            Vector3::new(0.0, 0.0, 1.0),
            Vector3::new(1.0, 0.0, 0.0),
        )
        .unwrap(),
        loops: vec![l],
    })
    .unwrap()
}

#[divan::bench]
fn frustum_of_squares(bencher: divan::Bencher) {
    let sections = [at(0.0, square(10, 2.0)), at(1.0, square(20, 1.0))];
    bencher.bench(|| black_box(loft(OpKey(1), &sections, &LoftOptions::default()).unwrap()));
}

#[divan::bench(sample_count = 10)]
fn race_pin(bencher: divan::Bencher) {
    bencher.bench(|| black_box(nurbs_race::clear_pin()));
}

#[divan::bench(sample_count = 10)]
fn race_block(bencher: divan::Bencher) {
    bencher.bench(|| black_box(nurbs_race::block()));
}

#[divan::bench(sample_count = 10)]
fn square_along_a_planar_spline(bencher: divan::Bencher) {
    let k = KnotVector::clamped(3, &[0.0, 1.0, 2.0], &[4, 1, 4]).unwrap();
    let pts = vec![
        Point3::new(0.0, 0.0, 0.0),
        Point3::new(1.0, 0.0, 0.0),
        Point3::new(2.0, 0.0, 1.0),
        Point3::new(3.0, 0.0, 1.5),
        Point3::new(4.0, 0.0, 2.0),
    ];
    let path = Path {
        segments: vec![PathSegment {
            key: SegmentKey(1),
            curve: Curve::Nurbs(Arc::new(NurbsCurve::new(k, pts, None).unwrap())),
            range: [0.0, 2.0],
        }],
    };
    let profile = ValidProfile::new(Profile {
        frame: Frame::new(
            Point3::ORIGIN,
            Vector3::new(1.0, 0.0, 0.0),
            Vector3::new(0.0, 1.0, 0.0),
        )
        .unwrap(),
        loops: vec![square(10, 0.3)],
    })
    .unwrap();
    bencher
        .bench(|| black_box(along(OpKey(2), &profile, &path, &AlongOptions::default()).unwrap()));
}
