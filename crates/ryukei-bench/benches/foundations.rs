//! Phase 0 benchmarks: vector math, transforms, and each predicate on easy
//! input (the filter decides) and near-degenerate input (the exact stage
//! decides), so the cost of the exact stages stays visible.

use divan::black_box;
use ryukei::geom::{Point2, Point3, Rigid, UnitVector3, Vector3};
use ryukei::predicates::{incircle, insphere, orient2d, orient3d};

fn main() {
    divan::main();
}

#[divan::bench]
fn vector_cross_normalize() -> Option<UnitVector3> {
    let a = black_box(Vector3::new(1.0, 2.0, 3.0));
    let b = black_box(Vector3::new(-4.0, 0.5, 2.0));
    a.cross(b).normalize()
}

#[divan::bench]
fn rigid_apply_point(bencher: divan::Bencher) {
    let axis = Vector3::new(1.0, 1.0, 1.0).normalize().unwrap();
    let r = Rigid::translation(Vector3::new(1.0, 2.0, 3.0)).then_after(&Rigid::rotation(axis, 0.3));
    bencher.bench(|| black_box(r).apply_point(black_box(Point3::new(4.0, 5.0, 6.0))));
}

#[divan::bench]
fn rigid_compose(bencher: divan::Bencher) {
    let r = Rigid::rotation(UnitVector3::Z, 0.3);
    let t = Rigid::translation(Vector3::new(1.0, 2.0, 3.0));
    bencher.bench(|| black_box(t).then_after(&black_box(r)));
}

const EASY2: [Point2; 4] = [
    Point2::new(0.0, 0.0),
    Point2::new(1.0, 0.0),
    Point2::new(0.0, 1.0),
    Point2::new(0.2, 0.3),
];
// Exactly collinear / cocircular: the filter cannot decide.
const HARD2: [Point2; 4] = [
    Point2::new(0.5, 0.5),
    Point2::new(12.0, 12.0),
    Point2::new(24.0, 24.0),
    Point2::new(0.0, -1.0),
];
const CIRCLE: [Point2; 3] = [
    Point2::new(1.0, 0.0),
    Point2::new(0.0, 1.0),
    Point2::new(-1.0, 0.0),
];

const EASY3: [Point3; 5] = [
    Point3::new(0.0, 0.0, 0.0),
    Point3::new(1.0, 0.0, 0.0),
    Point3::new(0.0, 1.0, 0.0),
    Point3::new(0.0, 0.0, -1.0),
    Point3::new(0.1, 0.2, -0.3),
];
const SPHERE: [Point3; 5] = [
    Point3::new(1.0, 0.0, 0.0),
    Point3::new(0.0, 1.0, 0.0),
    Point3::new(-1.0, 0.0, 0.0),
    Point3::new(0.0, 0.0, -1.0),
    Point3::new(0.0, 0.0, 1.0),
];

#[divan::bench(args = ["easy", "hard"])]
fn bench_orient2d(kind: &str) -> ryukei::predicates::Sign {
    let p = if kind == "easy" { EASY2 } else { HARD2 };
    orient2d(black_box(p[0]), black_box(p[1]), black_box(p[2]))
}

#[divan::bench(args = ["easy", "hard"])]
fn bench_orient3d(kind: &str) -> ryukei::predicates::Sign {
    let p = EASY3;
    let d = if kind == "easy" {
        p[3]
    } else {
        Point3::new(0.3, 0.7, 0.0)
    };
    orient3d(
        black_box(p[0]),
        black_box(p[1]),
        black_box(p[2]),
        black_box(d),
    )
}

#[divan::bench(args = ["easy", "hard"])]
fn bench_incircle(kind: &str) -> ryukei::predicates::Sign {
    let d = if kind == "easy" { EASY2[3] } else { HARD2[3] };
    incircle(
        black_box(CIRCLE[0]),
        black_box(CIRCLE[1]),
        black_box(CIRCLE[2]),
        black_box(d),
    )
}

#[divan::bench(args = ["easy", "hard"])]
fn bench_insphere(kind: &str) -> ryukei::predicates::Sign {
    let e = if kind == "easy" { EASY3[4] } else { SPHERE[4] };
    let s = SPHERE;
    insphere(
        black_box(s[0]),
        black_box(s[1]),
        black_box(s[2]),
        black_box(s[3]),
        black_box(e),
    )
}
