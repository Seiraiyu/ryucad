//! SMLib meshing the same bodies as the `mesh` and `occt` benchmarks, at
//! the same chords and angle, on one thread and in parallel. Needs the
//! `smlib` feature. An SMLib body is `Send` but not `Sync`, so each one
//! sits behind a `Mutex`.

use divan::black_box;
use ryukei::geom::Rigid;
use ryukei_bench::Primitive;
use ryukei_bench::smlib::Solid;
use std::sync::Mutex;

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

#[divan::bench(args = KINDS, consts = [0, 1, 2])]
fn smlib_one_thread<const C: usize>(bencher: divan::Bencher, kind: &str) {
    let s = Mutex::new(Solid::new(primitive_of(kind), &Rigid::IDENTITY, false));
    bencher.bench(|| black_box(s.lock().unwrap().mesh(CHORDS[C], 0.5, false)));
}

#[divan::bench(args = KINDS, consts = [0, 1, 2])]
fn smlib_parallel<const C: usize>(bencher: divan::Bencher, kind: &str) {
    let s = Mutex::new(Solid::new(primitive_of(kind), &Rigid::IDENTITY, false));
    bencher.bench(|| black_box(s.lock().unwrap().mesh(CHORDS[C], 0.5, true)));
}
