//! OCCT and SMLib on the cases of the `boolean` benchmarks: a block minus
//! 1, 100 and 1,000 through-holes on one thread, and a union of
//! overlapping spheres. Devcontainer only (the `occt` and `smlib`
//! features).

use divan::black_box;
use ryukei::boolean::BooleanOp;
use ryukei::geom::{Rigid, Vector3};
use ryukei_bench::{Primitive, occt, smlib};

fn main() {
    divan::main();
}

fn at(x: f64, y: f64, z: f64) -> Rigid {
    Rigid::translation(Vector3::new(x, y, z))
}

const HOLES: [(usize, usize); 3] = [(1, 1), (10, 10), (40, 25)];
const ROD: Primitive = Primitive::Cylinder { r: 0.3, h: 1.0 };

fn places(cols: usize, rows: usize) -> Vec<Rigid> {
    (0..rows)
        .flat_map(|j| (0..cols).map(move |i| at(i as f64 + 0.5, j as f64 + 0.5, -0.25)))
        .collect()
}

fn block(cols: usize, rows: usize) -> Primitive {
    Primitive::Block([cols as f64, rows as f64, 0.5])
}

#[divan::bench(args = HOLES, sample_count = 10)]
fn occt_block_minus_cylinders(bencher: divan::Bencher, (cols, rows): (usize, usize)) {
    let target = occt::Solid::new(block(cols, rows), &Rigid::IDENTITY);
    let rods: Vec<occt::Solid> = places(cols, rows)
        .iter()
        .map(|p| occt::Solid::new(ROD, p))
        .collect();
    bencher.bench(|| black_box(target.boolean(BooleanOp::Subtract, &rods, false).unwrap()));
}

#[divan::bench(args = HOLES, sample_count = 3)]
fn smlib_block_minus_cylinders(bencher: divan::Bencher, (cols, rows): (usize, usize)) {
    let at = places(cols, rows);
    bencher
        .with_inputs(|| {
            let mut s = smlib::Solid::new(block(cols, rows), &Rigid::IDENTITY, false);
            for p in &at {
                s.add_tool(ROD, p);
            }
            s
        })
        .bench_values(|mut s| {
            s.combine_tools(BooleanOp::Subtract, false).unwrap();
            black_box(s)
        });
}

fn sphere_places(n: usize) -> Vec<Rigid> {
    (0..n * n * n)
        .skip(1)
        .map(|c| {
            let (i, j, k) = (c % n, (c / n) % n, c / (n * n));
            at(i as f64 * 0.8, j as f64 * 0.8, k as f64 * 0.8)
        })
        .collect()
}

#[divan::bench(args = [3, 5, 10], sample_count = 3)]
fn occt_union_of_spheres(bencher: divan::Bencher, n: usize) {
    let ball = occt::Solid::new(Primitive::Sphere(0.5), &Rigid::IDENTITY);
    let others: Vec<occt::Solid> = sphere_places(n)
        .iter()
        .map(|p| occt::Solid::new(Primitive::Sphere(0.5), p))
        .collect();
    bencher.bench(|| black_box(ball.boolean(BooleanOp::Unite, &others, false).unwrap()));
}
