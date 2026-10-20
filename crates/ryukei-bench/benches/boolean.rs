//! Phase 5 benchmarks: a block minus 1, 100 and 1,000 through-holes (the
//! spike's case) on one thread and on all of them, a union of up to 1,000
//! overlapping spheres, intersection-heavy pairs, and the
//! self-intersection check on a holed block. `boolean_kernels` times OCCT
//! and SMLib on the same cases.

use divan::black_box;
use ryukei::boolean::{BooleanOp, Tool, ToolKey, boolean};
use ryukei::check::check_intersections;
use ryukei::geom::{Rigid, Vector3};
use ryukei::name::OpKey;
use ryukei::primitive;
use ryukei::topo::Body;
use std::f64::consts::FRAC_PI_2;

fn main() {
    divan::main();
}

fn at(x: f64, y: f64, z: f64) -> Rigid {
    Rigid::translation(Vector3::new(x, y, z))
}

/// The spike's block, `cols` by `rows`, and a rod through each cell.
fn grid(cols: usize, rows: usize) -> (Body, Body, Vec<Rigid>) {
    let block = primitive::block(OpKey(1), cols as f64, rows as f64, 0.5, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let rod = primitive::cylinder(OpKey(2), 0.3, 1.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let places = (0..rows)
        .flat_map(|j| (0..cols).map(move |i| at(i as f64 + 0.5, j as f64 + 0.5, -0.25)))
        .collect();
    (block, rod, places)
}

fn tools<'a>(body: &'a Body, places: &[Rigid]) -> Vec<Tool<'a>> {
    places
        .iter()
        .enumerate()
        .map(|(i, p)| Tool {
            key: ToolKey(i as u128 + 1),
            body,
            place: *p,
        })
        .collect()
}

/// Hole grids: 1, 100 and 1,000 holes.
const HOLES: [(usize, usize); 3] = [(1, 1), (10, 10), (40, 25)];

#[divan::bench(args = HOLES, sample_count = 10)]
fn block_minus_cylinders_one_thread(bencher: divan::Bencher, (cols, rows): (usize, usize)) {
    let (block, rod, places) = grid(cols, rows);
    let tools = tools(&rod, &places);
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap();
    bencher.bench(|| {
        pool.install(|| black_box(boolean(OpKey(3), BooleanOp::Subtract, &block, &tools).unwrap()))
    });
}

#[divan::bench(args = HOLES, sample_count = 10)]
fn block_minus_cylinders_parallel(bencher: divan::Bencher, (cols, rows): (usize, usize)) {
    let (block, rod, places) = grid(cols, rows);
    let tools = tools(&rod, &places);
    bencher.bench(|| black_box(boolean(OpKey(3), BooleanOp::Subtract, &block, &tools).unwrap()));
}

/// Spheres of radius 0.5 on a cubic grid 0.8 apart, so each overlaps its
/// neighbours along the axes.
#[divan::bench(args = [3, 5, 10], sample_count = 3)]
fn union_of_spheres(bencher: divan::Bencher, n: usize) {
    let ball = primitive::sphere(OpKey(2), 0.5, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let places: Vec<Rigid> = (0..n * n * n)
        .skip(1)
        .map(|c| {
            let (i, j, k) = (c % n, (c / n) % n, c / (n * n));
            at(i as f64 * 0.8, j as f64 * 0.8, k as f64 * 0.8)
        })
        .collect();
    let tools = tools(&ball, &places);
    bencher.bench(|| black_box(boolean(OpKey(3), BooleanOp::Unite, &ball, &tools).unwrap()));
}

#[divan::bench]
fn crossed_cylinders(bencher: divan::Bencher) {
    let rod = primitive::cylinder(OpKey(1), 0.5, 3.0, &at(0.0, 0.0, -1.5))
        .unwrap()
        .0;
    let thin = primitive::cylinder(OpKey(2), 0.35, 3.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let y = Vector3::new(0.0, 1.0, 0.0).normalize().unwrap();
    let across = at(-1.5, 0.0, 0.0).then_after(&Rigid::rotation(y, FRAC_PI_2));
    let tools = tools(&thin, &[across]);
    bencher.bench(|| black_box(boolean(OpKey(3), BooleanOp::Intersect, &rod, &tools).unwrap()));
}

#[divan::bench]
fn torus_minus_slab(bencher: divan::Bencher) {
    let torus = primitive::torus(OpKey(1), 1.0, 0.3, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let slab = primitive::block(OpKey(2), 3.0, 3.0, 0.4, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let tools = tools(&slab, &[at(-1.5, -1.5, -0.1)]);
    bencher.bench(|| black_box(boolean(OpKey(3), BooleanOp::Subtract, &torus, &tools).unwrap()));
}

#[divan::bench(sample_count = 10)]
fn check_intersections_of_a_holed_block(bencher: divan::Bencher) {
    let (block, rod, places) = grid(10, 10);
    let (holed, _) = boolean(OpKey(3), BooleanOp::Subtract, &block, &tools(&rod, &places)).unwrap();
    bencher.bench(|| black_box(check_intersections(&holed)));
}
