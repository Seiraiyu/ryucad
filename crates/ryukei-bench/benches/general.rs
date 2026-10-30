//! Phase 8 benchmarks (design 94, plan 8.F): intersections on every
//! surface. Two bicubic NURBS (the NURBS race's block top and a pin's
//! lofted side), a NURBS cylinder against a plane through the general
//! path beside the analytic cylinder through the implicit one, and a
//! curve against a NURBS; then the NURBS race itself on one thread, a row
//! of 40 pins clear and flush.

use divan::black_box;
use ryukei::boolean::{BooleanOp, Tool, ToolKey, boolean};
use ryukei::geom::{Box3, Curve, Frame, Line, Plane, Point3, Rigid, Surface, Vector3};
use ryukei::intersect::{curve_surface, face_box, surface_surface};
use ryukei::name::OpKey;
use ryukei::primitive;
use ryukei::topo::{Body, FaceId};
use ryukei_testkit::nurbs_race;

fn main() {
    divan::main();
}

/// The face of `b` reaching highest, and its surface.
fn top(b: &Body) -> (FaceId, Surface) {
    let f = b
        .faces()
        .max_by(|&x, &y| face_box(b, x).max().z.total_cmp(&face_box(b, y).max().z))
        .unwrap();
    (f, b.surface(b.face(f).unwrap().surface).unwrap().clone())
}

/// The first face of `b` on a NURBS, and its surface.
fn spline_face(b: &Body) -> (FaceId, Surface) {
    let f = b
        .faces()
        .find(|&f| {
            matches!(
                b.surface(b.face(f).unwrap().surface),
                Some(Surface::Nurbs(_))
            )
        })
        .unwrap();
    (f, b.surface(b.face(f).unwrap().surface).unwrap().clone())
}

#[divan::bench]
fn bicubic_and_lofted_side(bencher: divan::Bencher) {
    let block = nurbs_race::block();
    let pin = nurbs_race::clear_pin()
        .transformed(&Rigid::translation(Vector3::new(10.5, 7.5, 0.0)))
        .unwrap();
    let ((fa, a), (fb, b)) = (top(&block), spline_face(&pin));
    let region = face_box(&block, fa).union(&face_box(&pin, fb));
    bencher.bench(|| black_box(surface_surface(&a, &b, &region).unwrap()));
}

/// A plane across a cylinder of radius 0.5 at 30° from its axis.
fn tilted_plane() -> Surface {
    let n = Vector3::new(0.5, 0.0, 0.866_025_403_784_438_6);
    Surface::Plane(Plane::new(
        Frame::new(Point3::new(0.0, 0.0, 0.5), n, Vector3::new(0.0, 1.0, 0.0)).unwrap(),
    ))
}

#[divan::bench(args = ["implicit", "general"])]
fn cylinder_and_plane(bencher: divan::Bencher, path: &str) {
    let (cyl, _) = primitive::cylinder(OpKey(1), 0.5, 1.0, &Rigid::IDENTITY).unwrap();
    let side = cyl
        .faces()
        .find(|&f| {
            matches!(
                cyl.surface(cyl.face(f).unwrap().surface),
                Some(Surface::Cylinder(_))
            )
        })
        .unwrap();
    let s = cyl
        .surface(cyl.face(side).unwrap().surface)
        .unwrap()
        .clone();
    let s = if path == "general" {
        Surface::Nurbs(std::sync::Arc::new(
            s.to_nurbs([[0.0, core::f64::consts::TAU], [0.0, 1.0]])
                .unwrap()
                .0,
        ))
    } else {
        s
    };
    let region = Box3::new(Point3::new(-1.0, -1.0, -0.5), Point3::new(1.0, 1.0, 1.5));
    let plane = tilted_plane();
    bencher.bench(|| black_box(surface_surface(&s, &plane, &region).unwrap()));
}

#[divan::bench]
fn line_through_bicubic(bencher: divan::Bencher) {
    let block = nurbs_race::block();
    let (_, s) = top(&block);
    let line = Curve::Line(
        Line::through(Point3::new(3.3, 4.1, -1.0), Point3::new(3.7, 4.4, 2.0)).unwrap(),
    );
    bencher.bench(|| black_box(curve_surface(&line, [0.0, 3.1], &s, 1e-8).unwrap()));
}

fn cut(block: &Body, pin: &Body, n: usize) -> Body {
    let places = nurbs_race::places(n);
    let tools: Vec<Tool> = places
        .iter()
        .enumerate()
        .map(|(i, p)| Tool {
            key: ToolKey(i as u128 + 1),
            body: pin,
            place: *p,
        })
        .collect();
    boolean(OpKey(3), BooleanOp::Subtract, block, &tools)
        .unwrap()
        .0
}

#[divan::bench(args = ["clear", "flush"], sample_count = 3)]
fn race_row(bencher: divan::Bencher, kind: &str) {
    let block = nurbs_race::block();
    let pin = if kind == "flush" {
        nurbs_race::flush_pin()
    } else {
        nurbs_race::clear_pin()
    };
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap();
    bencher.bench(|| pool.install(|| black_box(cut(&block, &pin, nurbs_race::COLS))));
}
