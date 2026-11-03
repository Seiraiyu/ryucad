//! Tests of fills.

use super::coons::{self, cubic_coefficients, greville, interpolate};
use super::*;
use crate::check::{check, check_intersections};
use crate::geom::nurbs::skin::compatible_mapped;
use crate::geom::{Circle, Cylinder, Frame, KnotVector, Line, Surface, UnitVector3, Vector3};
use std::sync::Arc;
use crate::measure::{Entity, Properties, properties};
use crate::mesh::{MeshParams, check_mesh, tessellate};
use crate::topo::BodyKind;
use core::f64::consts::{FRAC_1_SQRT_2, FRAC_PI_2, PI, TAU};

const K: OpKey = OpKey(0xf111);

const OPTIONS: FillOptions = FillOptions {
    tolerance: 1e-7,
    angle: 0.01,
};

/// A line segment from `a` to `b` as a side.
fn segment(a: Point3, b: Point3) -> FillSide<'static> {
    let d = b - a;
    FillSide::Curve {
        curve: Curve::Line(Line::new(a, d).unwrap()),
        range: [0.0, d.length()],
    }
}

fn p(x: f64, y: f64, z: f64) -> Point3 {
    Point3::new(x, y, z)
}

/// A Bézier curve through its control points as a side.
fn bezier(points: &[Point3]) -> FillSide<'static> {
    let d = points.len() - 1;
    let k = KnotVector::clamped(d, &[0.0, 1.0], &[d + 1, d + 1]).unwrap();
    FillSide::Curve {
        curve: Curve::Nurbs(Arc::new(NurbsCurve::new(k, points.to_vec(), None).unwrap())),
        range: [0.0, 1.0],
    }
}

/// A polygon's sides.
fn polygon(points: &[Point3]) -> FillBoundary<'static> {
    let n = points.len();
    FillBoundary {
        sides: (0..n).map(|k| segment(points[k], points[(k + 1) % n])).collect(),
    }
}

fn area(b: &Body) -> f64 {
    let Properties::Sheet { area } = properties(b, Entity::Body).unwrap() else {
        panic!("a sheet");
    };
    area.value()
}

/// Checks a fill: a sheet that checks clean and meshes.
fn sound(b: &Body) {
    assert_eq!(b.kind(), BodyKind::Sheet);
    assert_eq!(check(b), vec![]);
    assert_eq!(check_intersections(b), vec![]);
    let mesh = tessellate(b, &MeshParams::new(1e-3, 0.3)).unwrap();
    assert!(mesh.faults.is_empty(), "{:?}", mesh.faults);
    assert_eq!(check_mesh(b, &mesh), Ok(()));
}

#[test]
fn compatible_maps_find_the_curves_own_parameter() {
    // A rational quarter arc cut off its knots, so its end weights aren't
    // 1 and the Möbius factor moves its parameter.
    let c = Circle::new(Frame::WORLD, 2.0).unwrap();
    let (n, r) = Curve::Circle(c).to_nurbs([0.0, TAU]).unwrap();
    let part = n.part([r.to_nurbs(0.3), r.to_nurbs(1.2)]).unwrap();
    let line = {
        let k = KnotVector::clamped(1, &[0.0, 1.0], &[2, 2]).unwrap();
        NurbsCurve::new(k, vec![p(0.0, 0.0, 0.0), p(1.0, 0.0, 0.0)], None).unwrap()
    };
    let (knots, nets, maps) = compatible_mapped(&[part.clone(), line], 3).unwrap();
    let c2 = NurbsCurve::new(
        knots,
        nets[0].iter().map(|h| Point3::ORIGIN + h.p).collect(),
        Some(nets[0].iter().map(|h| h.w).collect()),
    )
    .unwrap();
    assert_eq!(c2.degree(), 3);
    for i in 0..=16 {
        let u = i as f64 / 16.0;
        let t = maps[0].to_curve(u);
        assert!(c2.point(u).distance(part.point(t)) < 1e-12, "{u}");
    }
}

#[test]
fn cubics_and_interpolation_in_a_basis() {
    let k = KnotVector::clamped(4, &[0.0, 0.3, 0.7, 1.0], &[5, 2, 1, 5]).unwrap();
    for (h, poly) in coons::HERMITE.iter().map(|h| (cubic_coefficients(&k, *h), *h)) {
        for i in 0..=20 {
            let t = i as f64 / 20.0;
            let b = k.basis(t);
            let s: f64 = b
                .values(0)
                .iter()
                .enumerate()
                .map(|(r, &n)| n * h[b.span() - 4 + r])
                .sum();
            let want = poly[0] + t * (poly[1] + t * (poly[2] + t * poly[3]));
            assert!((s - want).abs() < 1e-14, "{t}: {s} {want}");
        }
    }
    let xi = greville(&k);
    let values: Vec<Vector3> = xi.iter().map(|&t| Vector3::new(t.sin(), t * t, 1.0)).collect();
    let c = interpolate(&k, &values);
    for (&t, v) in xi.iter().zip(&values) {
        let b = k.basis(t);
        let s = b
            .values(0)
            .iter()
            .enumerate()
            .fold(Vector3::ZERO, |acc, (r, &n)| acc + c[b.span() - 4 + r] * n);
        assert!((s - *v).length() < 1e-13);
    }
}

#[test]
fn a_square_is_one_planar_face() {
    let pts = [p(0.0, 0.0, 0.0), p(1.0, 0.0, 0.0), p(1.0, 1.0, 0.0), p(0.0, 1.0, 0.0)];
    let (b, journal, report) = fill(K, &polygon(&pts), &OPTIONS).unwrap();
    sound(&b);
    assert_eq!(b.faces().count(), 1);
    assert_eq!(b.edges().count(), 4);
    assert!(matches!(b.surfaces[b.faces[b.faces().next().unwrap()].surface], Surface::Plane(_)));
    assert!((area(&b) - 1.0).abs() < 1e-9);
    assert_eq!(report.deviations, vec![0.0; 4]);
    assert_eq!(report.angles, vec![None; 4]);
    assert_eq!(report.inner, None);
    assert_eq!(journal.events().len(), 9);
    // Its front faces +z: the loop runs counterclockwise about it.
    let f = b.faces().next().unwrap();
    let s = &b.surfaces[b.faces[f].surface];
    assert!(s.normal(0.5, 0.5).unwrap().get().z > 0.99);
}

#[test]
fn a_flat_triangle_is_one_plane_face() {
    let pts = [p(0.0, 0.0, 0.0), p(2.0, 0.0, 0.0), p(0.0, 1.0, 0.0)];
    let (b, _, report) = fill(K, &polygon(&pts), &OPTIONS).unwrap();
    sound(&b);
    assert_eq!(b.faces().count(), 1);
    assert_eq!(b.edges().count(), 3);
    assert_eq!(b.vertices().count(), 3);
    assert_eq!(report.inner, None);
    assert!((area(&b) - 1.0).abs() < 1e-9);
    let f = b.faces().next().unwrap();
    assert!(matches!(b.surfaces[b.faces[f].surface], Surface::Plane(_)));
}

#[test]
fn a_bent_triangle_is_three_patches() {
    let boundary = FillBoundary {
        sides: vec![
            bezier(&[p(0.0, 0.0, 0.0), p(1.0, 0.0, 0.4), p(2.0, 0.0, 0.0)]),
            segment(p(2.0, 0.0, 0.0), p(0.0, 1.0, 0.0)),
            segment(p(0.0, 1.0, 0.0), p(0.0, 0.0, 0.0)),
        ],
    };
    let (b, _, report) = fill(K, &boundary, &OPTIONS).unwrap();
    sound(&b);
    assert_eq!(b.faces().count(), 3);
    assert_eq!(b.edges().count(), 9);
    assert_eq!(b.vertices().count(), 7);
    assert!(report.inner.unwrap() <= OPTIONS.angle);
}

#[test]
fn a_flat_pentagon_is_one_plane_face() {
    let pts: Vec<Point3> = (0..5)
        .map(|k| {
            let a = TAU * k as f64 / 5.0;
            p(a.cos(), a.sin(), 0.0)
        })
        .collect();
    let (b, _, report) = fill(K, &polygon(&pts), &OPTIONS).unwrap();
    sound(&b);
    assert_eq!(b.faces().count(), 1);
    assert_eq!(b.edges().count(), 5);
    let want = 2.5 * (TAU / 5.0).sin();
    assert!((area(&b) - want).abs() < 1e-9, "{}", area(&b));
    assert_eq!(report.inner, None);
}

#[test]
fn a_saddle_pentagon_is_five_curved_patches() {
    let pts: Vec<Point3> = (0..5)
        .map(|k| {
            let a = TAU * k as f64 / 5.0;
            p(a.cos(), a.sin(), if k % 2 == 0 { 0.3 } else { -0.2 })
        })
        .collect();
    let (b, _, report) = fill(K, &polygon(&pts), &OPTIONS).unwrap();
    sound(&b);
    assert_eq!(b.faces().count(), 5);
    assert!(report.inner.unwrap() <= OPTIONS.angle, "{report:?}");
}

#[test]
fn arcs_bound_a_patch_exactly() {
    // Four quarter arcs of unit circles round the faces of a cube's
    // corner region: a curved quad whose sides are rational.
    let arc = |frame: Frame, a: f64, b: f64| FillSide::Curve {
        curve: Curve::Circle(Circle::new(frame, 1.0).unwrap()),
        range: [a, b],
    };
    let z = UnitVector3::Z;
    let x = UnitVector3::X;
    let y = UnitVector3::Y;
    let boundary = FillBoundary {
        sides: vec![
            arc(Frame::from_axes(p(0.0, 0.0, 0.0), z, x).unwrap(), 0.0, FRAC_PI_2),
            arc(Frame::from_axes(p(0.0, 2.0, 0.0), x, y).unwrap(), PI, 1.5 * PI),
            segment(p(0.0, 2.0, -1.0), p(1.0, 1.0, -1.0)),
            segment(p(1.0, 1.0, -1.0), p(1.0, 0.0, 0.0)),
        ],
    };
    let r = fill(K, &boundary, &OPTIONS);
    let (b, _, report) = r.unwrap();
    sound(&b);
    assert!(report.deviations.iter().all(|&d| d < 1e-12), "{report:?}");
    let _ = FRAC_1_SQRT_2;
}

#[test]
fn a_tangent_side_meets_its_face_within_the_angle() {
    // A quarter cylinder sheet; its side at u = π/2 is the line x = 0,
    // y = 1 along z, where the cylinder's normal is +y.
    let cyl = Surface::Cylinder(Cylinder::new(Frame::WORLD, 1.0).unwrap());
    let (sheet, _) = crate::sheet::from_surface(OpKey(7), &cyl, [[0.0, FRAC_PI_2], [0.0, 1.0]]).unwrap();
    let edge = sheet
        .edges()
        .find(|&e| {
            let d = &sheet.edges[e];
            let c = &sheet.curves[d.curve];
            let m = c.point(0.5 * (d.interval[0] + d.interval[1]));
            m.x.abs() < 1e-9 && (m.y - 1.0).abs() < 1e-9
        })
        .unwrap();
    let name = sheet.edges[edge].name;
    // The fill's other sides leave the corners in the cylinder's tangent
    // plane (along -x), then drop away from it, so the bilinear patch
    // leaves the side out of the plane between the corners and the
    // correction has work to do.
    let boundary = FillBoundary {
        sides: vec![
            FillSide::Edge {
                body: &sheet,
                edge: name,
                tangent: true,
            },
            bezier(&[p(0.0, 1.0, 1.0), p(-0.5, 1.0, 1.0), p(-1.0, 0.5, 1.0)]),
            segment(p(-1.0, 0.5, 1.0), p(-1.0, 0.5, 0.0)),
            bezier(&[p(-1.0, 0.5, 0.0), p(-0.5, 1.0, 0.0), p(0.0, 1.0, 0.0)]),
        ],
    };
    let (b, _, report) = fill(K, &boundary, &OPTIONS).unwrap();
    sound(&b);
    let a = report.angles[0].unwrap();
    assert!(a <= OPTIONS.angle, "{a}");
}

#[test]
fn refusals() {
    let pts = [p(0.0, 0.0, 0.0), p(1.0, 0.0, 0.0), p(1.0, 1.0, 0.0)];
    let two = FillBoundary {
        sides: vec![segment(pts[0], pts[1]), segment(pts[1], pts[0])],
    };
    assert_eq!(
        fill(K, &two, &OPTIONS).unwrap_err(),
        FillError::TooFewSides.into()
    );
    let open = FillBoundary {
        sides: vec![
            segment(pts[0], pts[1]),
            segment(pts[1], pts[2]),
            segment(pts[2], p(0.0, 0.1, 0.0)),
        ],
    };
    assert!(matches!(
        fill(K, &open, &OPTIONS).unwrap_err(),
        Error::InvalidFill(FillError::NotClosed { side: 0, .. })
    ));
    let bad = FillOptions {
        tolerance: 0.0,
        angle: 0.01,
    };
    assert_eq!(
        fill(K, &polygon(&pts), &bad).unwrap_err(),
        FillError::BadOptions.into()
    );
    // A straight corner: the sides at (1, 0, 0) run on in one line.
    let straight = polygon(&[p(0.0, 0.0, 0.0), p(1.0, 0.0, 0.0), p(2.0, 0.0, 0.0), p(1.0, 1.0, 0.0)]);
    assert_eq!(
        fill(K, &straight, &OPTIONS).unwrap_err(),
        FillError::Degenerate { side: Some(1) }.into()
    );
    let (sheet, _, _) = fill(K, &polygon(&pts), &OPTIONS).unwrap();
    let missing = FillBoundary {
        sides: vec![
            FillSide::Edge {
                body: &sheet,
                edge: Name::from_u128(1),
                tangent: false,
            },
            segment(pts[1], pts[2]),
            segment(pts[2], pts[0]),
        ],
    };
    assert_eq!(
        fill(K, &missing, &OPTIONS).unwrap_err(),
        FillError::NoSuchEdge { side: 0 }.into()
    );
}
