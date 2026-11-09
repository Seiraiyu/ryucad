//! Design 326 benchmarks, on one thread: a plate with 1, 100 and 1,000
//! holes seen from a corner, a sphere's and a torus's silhouettes, and a
//! drilled plate's section. `tests/projection.rs` races OCCT.

use divan::black_box;
use ryukei::boolean::{BooleanOp, Tool, ToolKey, boolean};
use ryukei::geom::{Frame, Plane, Point3, Rigid, Vector3};
use ryukei::name::OpKey;
use ryukei::primitive;
use ryukei::project::{Placed, View, ViewOptions, section, view};
use ryukei::topo::Body;

fn main() {
    divan::main();
}

fn one<T: Send>(f: impl FnOnce() -> T + Send) -> T {
    rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap()
        .install(f)
}

fn iso() -> View {
    View::new(Vector3::new(1.0, -1.0, 1.0), Vector3::new(0.0, 0.0, 1.0)).unwrap()
}

fn plate(cols: usize, rows: usize) -> Body {
    let block = primitive::block(OpKey(1), cols as f64, rows as f64, 0.5, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let rod = primitive::cylinder(OpKey(2), 0.3, 1.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let places: Vec<Rigid> = (0..rows)
        .flat_map(|j| {
            (0..cols).map(move |i| {
                Rigid::translation(Vector3::new(i as f64 + 0.5, j as f64 + 0.5, -0.25))
            })
        })
        .collect();
    let tools: Vec<Tool> = places
        .iter()
        .enumerate()
        .map(|(i, p)| Tool {
            key: ToolKey(i as u128 + 1),
            body: &rod,
            place: *p,
        })
        .collect();
    boolean(OpKey(3), BooleanOp::Subtract, &block, &tools)
        .unwrap()
        .0
}

#[divan::bench(args = [(1, 1), (10, 10), (40, 25)], sample_count = 3)]
fn plate_from_a_corner(bencher: divan::Bencher, (cols, rows): (usize, usize)) {
    let b = plate(cols, rows);
    let v = iso();
    bencher.bench_local(|| {
        one(|| {
            black_box(
                view(
                    &[Placed {
                        body: &b,
                        place: Rigid::IDENTITY,
                    }],
                    &v,
                    &ViewOptions::default(),
                )
                .unwrap(),
            )
        })
    });
}

#[divan::bench]
fn sphere_from_a_corner(bencher: divan::Bencher) {
    let b = primitive::sphere(OpKey(4), 1.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let v = iso();
    bencher.bench_local(|| {
        one(|| {
            black_box(
                view(
                    &[Placed {
                        body: &b,
                        place: Rigid::IDENTITY,
                    }],
                    &v,
                    &ViewOptions::default(),
                )
                .unwrap(),
            )
        })
    });
}

#[divan::bench(sample_count = 3)]
fn torus_askew(bencher: divan::Bencher) {
    let b = primitive::torus(OpKey(5), 2.0, 0.5, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let v = View::new(Vector3::new(0.3, -0.4, 0.8), Vector3::new(0.0, 0.0, 1.0)).unwrap();
    bencher.bench_local(|| {
        one(|| {
            black_box(
                view(
                    &[Placed {
                        body: &b,
                        place: Rigid::IDENTITY,
                    }],
                    &v,
                    &ViewOptions::default(),
                )
                .unwrap(),
            )
        })
    });
}

#[divan::bench(sample_count = 5)]
fn drilled_plate_section(bencher: divan::Bencher) {
    let b = plate(10, 10);
    let p = Plane::new(
        Frame::new(
            Point3::new(5.0, 0.0, 0.0),
            Vector3::new(1.0, 0.0, 0.0),
            Vector3::new(0.0, 1.0, 0.0),
        )
        .unwrap(),
    );
    bencher.bench_local(|| one(|| black_box(section(OpKey(6), &b, &p).unwrap())));
}
