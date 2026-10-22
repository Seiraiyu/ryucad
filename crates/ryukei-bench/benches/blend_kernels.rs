//! OCCT and SMLib on the cases of the `blend` benchmarks: a plate's 1,
//! 100 and 1,000 hole rims filleted on one thread, every edge of a block,
//! and a boss filleted to a shaft. Devcontainer only (the `occt` and
//! `smlib` features).

use divan::black_box;
use ryukei::boolean::BooleanOp;
use ryukei::geom::{Rigid, Vector3};
use ryukei_bench::{BenchBlend, BenchKind, Primitive, occt, smlib};
use std::f64::consts::FRAC_PI_2;

fn main() {
    divan::main();
}

fn at(x: f64, y: f64, z: f64) -> Rigid {
    Rigid::translation(Vector3::new(x, y, z))
}

const RIMS: [(usize, usize); 3] = [(1, 1), (10, 10), (40, 25)];
const ROD: Primitive = Primitive::Cylinder { r: 0.3, h: 1.0 };

fn places(cols: usize, rows: usize) -> Vec<Rigid> {
    (0..rows)
        .flat_map(|j| (0..cols).map(move |i| at(i as f64 + 0.5, j as f64 + 0.5, -0.25)))
        .collect()
}

fn fillet(at: [f64; 3], r: f64) -> BenchBlend {
    BenchBlend {
        at,
        kind: BenchKind::Fillet(r),
    }
}

fn rims(cols: usize, rows: usize) -> Vec<BenchBlend> {
    places(cols, rows)
        .iter()
        .map(|p| {
            let t = p.translation_part();
            fillet([t.x, t.y + 0.3, 0.5], 0.05)
        })
        .collect()
}

fn block(cols: usize, rows: usize) -> Primitive {
    Primitive::Block([cols as f64, rows as f64, 0.5])
}

#[divan::bench(args = RIMS, sample_count = 3)]
fn occt_plate_rims(bencher: divan::Bencher, (cols, rows): (usize, usize)) {
    let rods: Vec<occt::Solid> = places(cols, rows)
        .iter()
        .map(|p| occt::Solid::new(ROD, p))
        .collect();
    let plate = occt::Solid::new(block(cols, rows), &Rigid::IDENTITY)
        .boolean(BooleanOp::Subtract, &rods, false)
        .unwrap();
    let blends = rims(cols, rows);
    bencher.bench(|| black_box(plate.blend(&blends).unwrap()));
}

#[divan::bench(args = RIMS, sample_count = 3)]
fn smlib_plate_rims(bencher: divan::Bencher, (cols, rows): (usize, usize)) {
    let at = places(cols, rows);
    let blends = rims(cols, rows);
    bencher
        .with_inputs(|| {
            let mut s = smlib::Solid::new(block(cols, rows), &Rigid::IDENTITY, false);
            for p in &at {
                s.add_tool(ROD, p);
            }
            s.combine_tools(BooleanOp::Subtract, false).unwrap();
            s
        })
        .bench_values(|mut s| black_box(s.fillet(&blends, false).map(|()| s)));
}

fn block_edges() -> Vec<BenchBlend> {
    let mut out = Vec::new();
    for (a, b) in [(0.0, 0.0), (2.0, 0.0), (0.0, 2.0), (2.0, 2.0)] {
        for e in [[1.0, a, b], [a, 1.0, b], [a, b, 1.0]] {
            out.push(fillet(e, 0.3));
        }
    }
    out
}

#[divan::bench(sample_count = 10)]
fn occt_every_edge_of_a_block(bencher: divan::Bencher) {
    let b = occt::Solid::new(Primitive::Block([2.0; 3]), &Rigid::IDENTITY);
    let blends = block_edges();
    bencher.bench(|| black_box(b.blend(&blends).unwrap()));
}

#[divan::bench(sample_count = 10)]
fn smlib_every_edge_of_a_block(bencher: divan::Bencher) {
    let blends = block_edges();
    bencher
        .with_inputs(|| smlib::Solid::new(Primitive::Block([2.0; 3]), &Rigid::IDENTITY, false))
        .bench_values(|mut s| black_box(s.fillet(&blends, false).map(|()| s)));
}

#[divan::bench(sample_count = 3)]
fn occt_a_boss_on_a_shaft(bencher: divan::Bencher) {
    let y = Vector3::new(0.0, 1.0, 0.0).normalize().unwrap();
    let along_x = Rigid::rotation(y, FRAC_PI_2).then_after(&at(0.0, 0.0, -2.0));
    let shaft = occt::Solid::new(Primitive::Cylinder { r: 1.0, h: 4.0 }, &along_x);
    let boss = occt::Solid::new(Primitive::Cylinder { r: 0.5, h: 2.0 }, &Rigid::IDENTITY);
    let b = shaft.boolean(BooleanOp::Unite, &[boss], false).unwrap();
    let blends = [fillet([0.0, 0.5, 0.75f64.sqrt()], 0.1)];
    bencher.bench(|| black_box(b.blend(&blends).unwrap()));
}
