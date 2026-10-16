//! Phase 2 benchmarks: NURBS evaluation by degree, Bézier decomposition,
//! projection onto curves and surfaces with the Bézier cache cold and warm,
//! exact conversion, and checking bodies whose geometry is all NURBS.

use divan::black_box;
use ryukei::check::check;
use ryukei::geom::{Frame, NurbsCurve, NurbsSurface, Point3, Sphere, Surface, Torus};
use ryukei::name::OpKey;
use ryukei::primitive::{block, cylinder};
use ryukei::topo::Body;
use ryukei_testkit::Lcg;
use ryukei_testkit::nurbs::{convert_body, curve, surface};

fn main() {
    divan::main();
}

const CURVE_DEGREES: [usize; 4] = [1, 3, 5, 9];
const SURFACE_DEGREES: [usize; 3] = [1, 2, 3];

/// A clamped curve of `degree` with 8 spans, always the same.
fn a_curve(degree: usize) -> NurbsCurve {
    curve(&mut Lcg::new(7), degree, 8, false)
}

/// A clamped surface of `degree` both ways with 4 × 4 spans.
fn a_surface(degree: usize) -> NurbsSurface {
    surface(&mut Lcg::new(7), [degree; 2], [4, 4], [false; 2])
}

/// Query points around the random control nets.
fn points() -> Vec<Point3> {
    let mut g = Lcg::new(11);
    (0..16)
        .map(|_| {
            Point3::new(
                g.next_f64(-8.0, 8.0),
                g.next_f64(-8.0, 8.0),
                g.next_f64(-8.0, 8.0),
            )
        })
        .collect()
}

#[divan::bench(args = CURVE_DEGREES)]
fn curve_eval(bencher: divan::Bencher, degree: usize) {
    let c = a_curve(degree);
    let [a, b] = c.domain();
    bencher.bench(|| {
        for i in 0..64 {
            black_box(c.eval(a + (b - a) * f64::from(i) / 63.0));
        }
    });
}

#[divan::bench(args = SURFACE_DEGREES)]
fn surface_eval(bencher: divan::Bencher, degree: usize) {
    let s = a_surface(degree);
    let [[u0, u1], [v0, v1]] = s.domain();
    bencher.bench(|| {
        for i in 0..8 {
            for j in 0..8 {
                let (u, v) = (f64::from(i) / 7.0, f64::from(j) / 7.0);
                black_box(s.eval(u0 + (u1 - u0) * u, v0 + (v1 - v0) * v));
            }
        }
    });
}

/// Bézier decomposition, timed through the first call that needs it.
#[divan::bench(args = CURVE_DEGREES)]
fn curve_decomposition(bencher: divan::Bencher, degree: usize) {
    bencher
        .with_inputs(|| a_curve(degree))
        .bench_values(|c| black_box(c.bounds(c.domain())));
}

#[divan::bench(args = SURFACE_DEGREES)]
fn surface_decomposition(bencher: divan::Bencher, degree: usize) {
    bencher
        .with_inputs(|| a_surface(degree))
        .bench_values(|s| black_box(s.bounds()));
}

#[divan::bench(args = CURVE_DEGREES)]
fn project_curve_cold(bencher: divan::Bencher, degree: usize) {
    let ps = points();
    bencher
        .with_inputs(|| a_curve(degree))
        .bench_values(|c| black_box(c.project(ps[0])));
}

#[divan::bench(args = CURVE_DEGREES)]
fn project_curve_warm(bencher: divan::Bencher, degree: usize) {
    let (c, ps) = (a_curve(degree), points());
    c.bounds(c.domain());
    bencher.bench(|| {
        for &p in &ps {
            black_box(c.project(p));
        }
    });
}

#[divan::bench(args = SURFACE_DEGREES)]
fn project_surface_cold(bencher: divan::Bencher, degree: usize) {
    let ps = points();
    bencher
        .with_inputs(|| a_surface(degree))
        .bench_values(|s| black_box(s.project(ps[0])));
}

#[divan::bench(args = SURFACE_DEGREES)]
fn project_surface_warm(bencher: divan::Bencher, degree: usize) {
    let (s, ps) = (a_surface(degree), points());
    s.bounds();
    bencher.bench(|| {
        for &p in &ps {
            black_box(s.project(p));
        }
    });
}

#[divan::bench]
fn convert_sphere_and_torus() {
    let sphere = Surface::Sphere(Sphere::new(Frame::WORLD, 2.0).unwrap());
    let torus = Surface::Torus(Torus::new(Frame::WORLD, 3.0, 1.0).unwrap());
    for s in [&sphere, &torus] {
        black_box(black_box(s).to_nurbs([[0.0; 2]; 2]).unwrap());
    }
}

fn nurbs_body(kind: &str) -> Body {
    let (mut b, _) = match kind {
        "block" => block(OpKey(1), 1.0, 2.0, 3.0, &ryukei::geom::Rigid::IDENTITY),
        _ => cylinder(OpKey(1), 1.0, 2.0, &ryukei::geom::Rigid::IDENTITY),
    }
    .unwrap();
    convert_body(&mut b).unwrap();
    b
}

#[divan::bench(args = ["block", "cylinder"])]
fn check_nurbs_body(bencher: divan::Bencher, kind: &str) {
    let b = nurbs_body(kind);
    bencher.bench(|| check(black_box(&b)));
}
