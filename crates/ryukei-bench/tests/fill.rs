//! Fills head to head with OCCT (design 295, plan 12.G): a flat pentagon,
//! a saddle pentagon, a quad of arcs and lines, a bent triangle, and a
//! quad tangent to a quarter cylinder sheet's side, filled by ryukei
//! (`fill::fill`) and by OCCT's `BRepFill_Filling` (an approximation: its
//! plate surface meets the boundary within its own errors). Fills are not
//! unique, so only the flat pentagon's area (a plane either way) is
//! judged, to a millionth; every other case prints both kernels' areas,
//! ryukei's certified deviations and angles beside OCCT's G0 and G1
//! errors, and both kernels' times (design 295 races sewing only). OCCT
//! reads the cylinder sheet from a STEP file ryustep writes, which its
//! reader scales to millimetres, so that case's other sides are scaled to
//! millimetres in the shim and its area and G0 error scaled back.
//! SMLib's shim has no filling. Devcontainer only (`--features occt`);
//! run in release.

use core::f64::consts::{FRAC_PI_2, PI, TAU};
use ryukei::fill::{FillBoundary, FillOptions, FillSide, fill};
use ryukei::geom::{
    Circle, Curve, Cylinder, Frame, KnotVector, Line, NurbsCurve, Point3, Surface, UnitVector3,
};
use ryukei::math;
use ryukei::measure::{Entity, Properties, properties};
use ryukei::name::{Name, OpKey, Role, SheetRole};
use ryukei::topo::Body;
use ryukei_bench::occt;
use ryustep::{LengthUnit, Product, WriteModel, WriteOptions, write};
use std::sync::Arc;
use std::time::Instant;

/// Millimetres in meters: OCCT reads STEP in millimetres.
const MM: f64 = 1e3;

const OPTIONS: FillOptions = FillOptions {
    tolerance: 1e-7,
    angle: 0.01,
};

fn p(x: f64, y: f64, z: f64) -> Point3 {
    Point3::new(x, y, z)
}

/// A curve over a range, as both kernels take it: a line, or an exact
/// NURBS cut to its range.
type Piece = (Curve, [f64; 2]);

fn segment(a: Point3, b: Point3) -> Piece {
    let d = b - a;
    (Curve::Line(Line::new(a, d).unwrap()), [0.0, d.length()])
}

fn bezier(pts: &[Point3]) -> Piece {
    let d = pts.len() - 1;
    let k = KnotVector::clamped(d, &[0.0, 1.0], &[d + 1, d + 1]).unwrap();
    let n = NurbsCurve::new(k, pts.to_vec(), None).unwrap();
    (Curve::Nurbs(Arc::new(n)), [0.0, 1.0])
}

/// A unit circle's arc, as its exact NURBS form over the arc.
fn arc(frame: Frame, a: f64, b: f64) -> Piece {
    let c = Curve::Circle(Circle::new(frame, 1.0).unwrap());
    let (n, r) = c.to_nurbs([a, b]).unwrap();
    let part = n.part([r.to_nurbs(a), r.to_nurbs(b)]).unwrap();
    let range = part.domain();
    (Curve::Nurbs(Arc::new(part)), range)
}

fn area(b: &Body) -> f64 {
    match properties(b, Entity::Body).unwrap() {
        Properties::Sheet { area } => area.value(),
        p => panic!("not a sheet: {p:?}"),
    }
}

fn polygon(points: &[Point3]) -> Vec<Piece> {
    let n = points.len();
    (0..n)
        .map(|k| segment(points[k], points[(k + 1) % n]))
        .collect()
}

#[test]
fn fills_against_occt() {
    let dir = tempfile::tempdir().unwrap();
    let pentagon = |z: &dyn Fn(usize) -> f64| -> Vec<Point3> {
        (0..5)
            .map(|k| {
                let a = TAU * k as f64 / 5.0;
                p(math::cos(a), math::sin(a), z(k))
            })
            .collect()
    };
    let free: Vec<(&str, Vec<Piece>, bool)> = vec![
        ("flat pentagon", polygon(&pentagon(&|_| 0.0)), true),
        (
            "saddle pentagon",
            polygon(&pentagon(&|k| if k % 2 == 0 { 0.3 } else { -0.2 })),
            false,
        ),
        (
            "arcs and lines",
            vec![
                arc(Frame::WORLD, 0.0, FRAC_PI_2),
                arc(
                    Frame::from_axes(p(0.0, 2.0, 0.0), UnitVector3::X, UnitVector3::Y).unwrap(),
                    PI,
                    1.5 * PI,
                ),
                segment(p(0.0, 2.0, -1.0), p(1.0, 1.0, -1.0)),
                segment(p(1.0, 1.0, -1.0), p(1.0, 0.0, 0.0)),
            ],
            false,
        ),
        (
            "bent triangle",
            vec![
                bezier(&[p(0.0, 0.0, 0.0), p(1.0, 0.0, 0.4), p(2.0, 0.0, 0.0)]),
                segment(p(2.0, 0.0, 0.0), p(0.0, 1.0, 0.0)),
                segment(p(0.0, 1.0, 0.0), p(0.0, 0.0, 0.0)),
            ],
            false,
        ),
    ];
    for (name, pieces, judged) in free {
        let sides = pieces
            .iter()
            .map(|(c, r)| FillSide::Curve {
                curve: c.clone(),
                range: *r,
            })
            .collect();
        let t = Instant::now();
        let (b, _, report) = fill(OpKey(1), &FillBoundary { sides }, &OPTIONS).unwrap();
        let ours = t.elapsed().as_secs_f64();
        let theirs = occt::fill(None, &[], &pieces, 1.0);
        eprintln!(
            "{name}: ryukei area {:.9} in {ours:.3} s, deviations {:?}, inner {:?}; OCCT {theirs:?}",
            area(&b),
            report.deviations,
            report.inner,
        );
        if judged {
            let o = theirs.expect("OCCT fills a flat pentagon");
            assert!(
                (area(&b) - o.area).abs() <= 1e-6 * o.area,
                "{name}: ryukei {} against OCCT {}",
                area(&b),
                o.area
            );
        }
    }
    // A quad tangent to a quarter cylinder sheet's side at u = π/2.
    let cyl = Surface::Cylinder(Cylinder::new(Frame::WORLD, 1.0).unwrap());
    let (sheet, _) =
        ryukei::sheet::from_surface(OpKey(7), &cyl, [[0.0, FRAC_PI_2], [0.0, 1.0]]).unwrap();
    let face = Name::derive(OpKey(7), Role::Sheet(SheetRole::Face), &[]);
    let side = Name::derive(OpKey(7), Role::Sheet(SheetRole::SideU1), &[face]);
    let rest = vec![
        bezier(&[p(0.0, 1.0, 1.0), p(-0.5, 1.0, 1.0), p(-1.0, 0.5, 1.0)]),
        bezier(&[p(-1.0, 0.5, 1.0), p(-1.2, 0.1, 0.5), p(-1.0, 0.5, 0.0)]),
        bezier(&[p(-1.0, 0.5, 0.0), p(-0.5, 1.0, 0.0), p(0.0, 1.0, 0.0)]),
    ];
    let mut sides = vec![FillSide::Edge {
        body: &sheet,
        edge: side,
        tangent: true,
    }];
    sides.extend(rest.iter().map(|(c, r)| FillSide::Curve {
        curve: c.clone(),
        range: *r,
    }));
    let t = Instant::now();
    let (b, _, report) = fill(OpKey(1), &FillBoundary { sides }, &OPTIONS).unwrap();
    let ours = t.elapsed().as_secs_f64();
    assert!(report.angles[0].unwrap() <= OPTIONS.angle);
    let bytes = write(
        &WriteModel {
            products: vec![Product {
                id: "sheet".into(),
                name: "sheet".into(),
                bodies: vec![sheet.clone()],
                ..Default::default()
            }],
        },
        &WriteOptions {
            unit: LengthUnit::Meter,
            ..WriteOptions::default()
        },
    )
    .unwrap();
    let path = dir.path().join("sheet.stp");
    std::fs::write(&path, &bytes).unwrap();
    let theirs = occt::Solid::read_step(&path, false)
        .and_then(|s| occt::fill(Some(&s), &[p(0.0, MM, 0.5 * MM)], &rest, MM))
        .map(|f| occt::Filled {
            area: f.area / (MM * MM),
            g0: f.g0 / MM,
            ..f
        });
    eprintln!(
        "tangent quad: ryukei area {:.9} in {ours:.3} s, angle {:?}; OCCT {theirs:?}",
        area(&b),
        report.angles[0],
    );
}
