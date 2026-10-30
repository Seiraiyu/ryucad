//! Phase 8 benchmarks (design 94, plan 8.F): offset bodies and shells of a
//! box and of its NURBS form, grown, shrunk and hollowed.

use divan::black_box;
use ryukei::geom::{Point3, Rigid};
use ryukei::intersect::face_box;
use ryukei::name::OpKey;
use ryukei::offset::{OffsetSpec, ShellSpec, offset_body, shell};
use ryukei::primitive;
use ryukei::topo::Body;

fn main() {
    divan::main();
}

fn boxes(kind: &str) -> Body {
    let (mut b, _) = primitive::block(OpKey(1), 1.0, 0.6, 0.4, &Rigid::IDENTITY).unwrap();
    if kind == "spline" {
        ryukei_testkit::nurbs::convert_body(&mut b).unwrap();
    }
    b
}

#[divan::bench(args = ["analytic", "spline"], consts = [false, true])]
fn offset<const SHRINK: bool>(bencher: divan::Bencher, kind: &str) {
    let b = boxes(kind);
    let spec = OffsetSpec {
        distance: if SHRINK { -0.1 } else { 0.1 },
        faces: vec![],
    };
    bencher.bench(|| black_box(offset_body(OpKey(2), &b, &spec).unwrap()));
}

#[divan::bench(args = ["analytic", "spline"])]
fn shell_open_top(bencher: divan::Bencher, kind: &str) {
    let b = boxes(kind);
    let top = Point3::new(0.5, 0.3, 0.4);
    let face = b
        .faces()
        .find(|&f| {
            b.surface(b.face(f).unwrap().surface).unwrap().distance(top) <= 1e-9
                && face_box(&b, f).max().z > 0.39
                && face_box(&b, f).min().z > 0.39
        })
        .unwrap();
    let spec = ShellSpec {
        thickness: 0.05,
        outward: false,
        open: vec![b.face(face).unwrap().name],
        faces: vec![],
    };
    bencher.bench(|| black_box(shell(OpKey(3), &b, &spec).unwrap()));
}
