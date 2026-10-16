//! ryukei against OCCT, meshing the same bodies at the same chord and
//! angle, on one thread and in parallel. Needs the `occt` feature.

use divan::black_box;
use ryukei::geom::Rigid;
use ryukei::mesh::{MeshParams, tessellate};
use ryukei::name::OpKey;
use ryukei::primitive;
use ryukei::topo::Body;
use ryukei_bench::occt::{Primitive, Solid};

fn main() {
    divan::main();
}

const CHORDS: [f64; 3] = [1e-2, 1e-3, 1e-4];
const KINDS: [&str; 5] = ["block", "cylinder", "cone", "sphere", "torus"];

fn primitive_of(kind: &str) -> Primitive {
    match kind {
        "block" => Primitive::Block([1.0, 0.75, 0.5]),
        "cylinder" => Primitive::Cylinder { r: 0.5, h: 1.0 },
        "cone" => Primitive::Cone {
            r0: 0.5,
            r1: 0.0,
            h: 1.0,
        },
        "sphere" => Primitive::Sphere(0.5),
        _ => Primitive::Torus {
            major: 0.5,
            minor: 0.2,
        },
    }
}

fn body(kind: &str) -> Body {
    let (k, at) = (OpKey(1), Rigid::IDENTITY);
    match kind {
        "block" => primitive::block(k, 1.0, 0.75, 0.5, &at),
        "cylinder" => primitive::cylinder(k, 0.5, 1.0, &at),
        "cone" => primitive::cone(k, 0.5, 1.0, &at),
        "sphere" => primitive::sphere(k, 0.5, &at),
        _ => primitive::torus(k, 0.5, 0.2, &at),
    }
    .unwrap()
    .0
}

#[divan::bench(args = KINDS, consts = [0, 1, 2])]
fn ryukei_one_thread<const C: usize>(bencher: divan::Bencher, kind: &str) {
    let b = body(kind);
    let p = MeshParams::new(CHORDS[C], 0.5);
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap();
    bencher.bench(|| pool.install(|| black_box(tessellate(&b, &p).unwrap())));
}

#[divan::bench(args = KINDS, consts = [0, 1, 2])]
fn ryukei_parallel<const C: usize>(bencher: divan::Bencher, kind: &str) {
    let b = body(kind);
    let p = MeshParams::new(CHORDS[C], 0.5);
    bencher.bench(|| black_box(tessellate(&b, &p).unwrap()));
}

#[divan::bench(args = KINDS, consts = [0, 1, 2])]
fn occt_one_thread<const C: usize>(bencher: divan::Bencher, kind: &str) {
    let s = Solid::new(primitive_of(kind), &Rigid::IDENTITY);
    bencher.bench(|| black_box(s.mesh(CHORDS[C], 0.5, false)));
}

#[divan::bench(args = KINDS, consts = [0, 1, 2])]
fn occt_parallel<const C: usize>(bencher: divan::Bencher, kind: &str) {
    let s = Solid::new(primitive_of(kind), &Rigid::IDENTITY);
    bencher.bench(|| black_box(s.mesh(CHORDS[C], 0.5, true)));
}
