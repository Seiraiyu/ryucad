//! Phase 3 benchmarks: meshing each primitive and the NURBS test bodies at
//! coarse, display and fine tolerances, on one thread and on all of them,
//! and a scene of 1000 bodies meshed body by body in parallel.

use divan::black_box;
use rayon::prelude::*;
use ryukei::geom::{Rigid, Vector3};
use ryukei::mesh::{MeshParams, tessellate};
use ryukei::name::OpKey;
use ryukei::primitive;
use ryukei::topo::Body;
use ryukei_testkit::Lcg;
use ryukei_testkit::mesh::bump_block;
use ryukei_testkit::nurbs::convert_body;

fn main() {
    divan::main();
}

/// Coarse, display and fine chords for bodies about 1 m across.
const CHORDS: [f64; 3] = [1e-2, 1e-3, 1e-4];

const KINDS: [&str; 8] = [
    "block",
    "cylinder",
    "cone",
    "sphere",
    "torus",
    "nurbs-cylinder",
    "nurbs-cone",
    "bump",
];

fn body(kind: &str, key: OpKey, at: &Rigid) -> Body {
    match kind {
        "block" => primitive::block(key, 1.0, 0.75, 0.5, at).unwrap().0,
        "cylinder" => primitive::cylinder(key, 0.5, 1.0, at).unwrap().0,
        "cone" => primitive::cone(key, 0.5, 1.0, at).unwrap().0,
        "sphere" => primitive::sphere(key, 0.5, at).unwrap().0,
        "torus" => primitive::torus(key, 0.5, 0.2, at).unwrap().0,
        "nurbs-cylinder" | "nurbs-cone" => {
            let mut b = if kind == "nurbs-cone" {
                primitive::cone(key, 0.5, 1.0, at).unwrap().0
            } else {
                primitive::cylinder(key, 0.5, 1.0, at).unwrap().0
            };
            convert_body(&mut b).unwrap();
            b
        }
        _ => bump_block(key, [1.0, 0.75, 0.5], 0.2, at).unwrap(),
    }
}

/// One thread: the work itself.
#[divan::bench(args = KINDS, consts = [0, 1, 2])]
fn one_thread<const C: usize>(bencher: divan::Bencher, kind: &str) {
    let b = body(kind, OpKey(1), &Rigid::IDENTITY);
    let p = MeshParams::new(CHORDS[C], 0.5);
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap();
    bencher.bench(|| pool.install(|| black_box(tessellate(&b, &p).unwrap())));
}

/// All threads: edges and faces in parallel within one body.
#[divan::bench(args = KINDS, consts = [0, 1, 2])]
fn all_threads<const C: usize>(bencher: divan::Bencher, kind: &str) {
    let b = body(kind, OpKey(1), &Rigid::IDENTITY);
    let p = MeshParams::new(CHORDS[C], 0.5);
    bencher.bench(|| black_box(tessellate(&b, &p).unwrap()));
}

/// 1000 primitives of every kind at scattered placements.
fn scene() -> Vec<Body> {
    let mut g = Lcg::new(1000);
    (0..1000u128)
        .map(|i| {
            let axis = Vector3::new(g.next_f64(-1.0, 1.0), g.next_f64(-1.0, 1.0), 1.0)
                .normalize()
                .unwrap();
            let shift = Vector3::new(
                g.next_f64(-100.0, 100.0),
                g.next_f64(-100.0, 100.0),
                g.next_f64(-100.0, 100.0),
            );
            let at =
                Rigid::translation(shift).then_after(&Rigid::rotation(axis, g.next_f64(-3.0, 3.0)));
            body(KINDS[(i % 5) as usize], OpKey(i), &at)
        })
        .collect()
}

/// The scene body by body on one thread, at the coarse chord.
#[divan::bench(sample_count = 5)]
fn scene_serial(bencher: divan::Bencher) {
    let bodies = scene();
    let p = MeshParams::new(CHORDS[0], 0.5);
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap();
    bencher.bench(|| {
        pool.install(|| {
            bodies
                .iter()
                .map(|b| tessellate(b, &p).unwrap().triangles.len())
                .sum::<usize>()
        })
    });
}

/// The scene with bodies in parallel on all threads.
#[divan::bench(sample_count = 5)]
fn scene_parallel(bencher: divan::Bencher) {
    let bodies = scene();
    let p = MeshParams::new(CHORDS[0], 0.5);
    bencher.bench(|| {
        bodies
            .par_iter()
            .map(|b| tessellate(b, &p).unwrap().triangles.len())
            .sum::<usize>()
    });
}
