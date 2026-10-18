//! Phase 4 benchmarks: validating profiles of 10 to 10,000 segments,
//! extruding and revolving them, and meshing swept NURBS faces.

use divan::black_box;
use ryukei::geom::{Axis, Point2, Point3, Vector3};
use ryukei::mesh::{MeshParams, tessellate};
use ryukei::name::OpKey;
use ryukei::sweep::{Loop, LoopKey, Profile, Segment, SegmentKey, ValidProfile, extrude, revolve};
use ryukei_bench::BenchProfile;
use ryukei_testkit::Lcg;
use ryukei_testkit::nurbs::blob2;
use std::sync::Arc;

fn main() {
    divan::main();
}

const SIZES: [usize; 4] = [10, 100, 1000, 10_000];

fn polygon(n: usize) -> ValidProfile {
    BenchProfile::polygon(n).to_ryukei().unwrap()
}

#[divan::bench(args = SIZES)]
fn validate(bencher: divan::Bencher, n: usize) {
    let p = BenchProfile::polygon(n);
    bencher.bench(|| black_box(p.to_ryukei().unwrap()));
}

#[divan::bench(args = SIZES)]
fn extrude_polygon(bencher: divan::Bencher, n: usize) {
    let p = polygon(n);
    let z = Vector3::new(0.0, 0.0, 1.0);
    bencher.bench(|| black_box(extrude(OpKey(1), &p, z, 0.0, 1.0).unwrap()));
}

#[divan::bench(args = SIZES)]
fn revolve_polygon(bencher: divan::Bencher, n: usize) {
    let p = polygon(n);
    let axis = Axis::new(Point3::ORIGIN, Vector3::new(0.0, 1.0, 0.0)).unwrap();
    bencher.bench(|| black_box(revolve(OpKey(1), &p, axis, 0.0, 2.0).unwrap()));
}

/// A NURBS blob in the world's xy plane, centred at `(2, 0)`.
fn blob() -> ValidProfile {
    let n = blob2(&mut Lcg::new(3), Point2::new(2.0, 0.0), 0.5);
    let [a, z] = n.domain();
    let loops = vec![Loop {
        key: LoopKey(1),
        segments: vec![Segment {
            key: SegmentKey(1),
            curve: ryukei::geom::Curve2::Nurbs2(Arc::new(n)),
            t0: a,
            t1: z,
        }],
    }];
    let frame = ryukei::geom::Frame::WORLD;
    ValidProfile::new(Profile { frame, loops }).unwrap()
}

/// Meshing a NURBS blob extruded (an extrusion side) and turned a quarter
/// turn (a revolution side), at coarse and display chords.
#[divan::bench(args = ["extrude", "revolve"], consts = [0, 1])]
fn mesh_swept_nurbs<const C: usize>(bencher: divan::Bencher, kind: &str) {
    let p = blob();
    let (b, _) = if kind == "extrude" {
        extrude(OpKey(1), &p, Vector3::new(0.0, 0.0, 1.0), 0.0, 1.0).unwrap()
    } else {
        let axis = Axis::new(Point3::ORIGIN, Vector3::new(0.0, 1.0, 0.0)).unwrap();
        revolve(OpKey(1), &p, axis, 0.0, 1.5).unwrap()
    };
    let params = MeshParams::new([1e-2, 1e-3][C], 0.5);
    bencher.bench(|| black_box(tessellate(&b, &params).unwrap()));
}
