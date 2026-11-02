use super::*;
use crate::geom::{Cylinder, Plane};
use crate::math;
use core::f64::consts::{FRAC_PI_2, PI};

/// A bend of inner radius 2 mm in sheet 1 mm thick, a right angle with a
/// K-factor of a half: the strip is `ba = π/2 · 2.5 mm` across.
pub(crate) const R: f64 = 0.002;
pub(crate) const T: f64 = 0.001;
pub(crate) const BA: f64 = FRAC_PI_2 * (R + 0.5 * T);

/// The bend's axis, tilted so no coordinate is special.
pub(crate) fn axis() -> Frame {
    Frame::new(
        Point3::new(0.01, -0.02, 0.005),
        Vector3::new(0.2, 1.0, 0.3),
        Vector3::new(1.0, 0.0, -0.4),
    )
    .unwrap()
}

/// A round hole's wall in the strip: a cylinder of radius 0.8 mm along
/// the strip's `z`, its centre on the near bend line.
pub(crate) fn hole() -> Surface {
    let f = Frame::new(
        Point3::new(0.0, 0.003, 0.0),
        UnitVector3::Z.get(),
        UnitVector3::X.get(),
    )
    .unwrap();
    Surface::Cylinder(Cylinder::new(f, 0.0008).unwrap())
}

/// The hole's wall bent, over its whole turn and through the sheet.
pub(crate) fn bent_hole() -> Bent {
    Bent::new(hole(), axis(), R, FRAC_PI_2 / BA, [[0.0, TAU], [0.0, T]]).unwrap()
}

/// A slot's wall at 30 degrees to the bend line: a plane square to the
/// strip's sides.
fn slot() -> Surface {
    let f = Frame::new(
        Point3::new(0.001, 0.002, 0.0),
        Vector3::new(-0.5, math::sqrt(0.75), 0.0),
        UnitVector3::Z.get(),
    )
    .unwrap();
    Surface::Plane(Plane::new(f))
}

fn bent_slot() -> Bent {
    Bent::new(
        slot(),
        axis(),
        R,
        FRAC_PI_2 / BA,
        [[0.0, T], [-0.002, 0.002]],
    )
    .unwrap()
}

#[test]
fn the_sides_fold_onto_the_bends_cylinders() {
    let side = |z: f64| {
        let f = Frame::new(
            Point3::new(0.0, 0.0, z),
            UnitVector3::Z.get(),
            UnitVector3::X.get(),
        )
        .unwrap();
        Surface::Plane(Plane::new(f))
    };
    for z in [0.0, T] {
        let b = Bent::new(side(z), axis(), R, FRAC_PI_2 / BA, [[0.0, BA], [0.0, 0.01]]).unwrap();
        for (x, y) in [(0.0, 0.0), (0.5 * BA, 0.004), (BA, 0.01)] {
            let p = b.point(x, y);
            let l = axis().to_local(p);
            assert!((math::hypot(l.x, l.y) - (R + z)).abs() < 1e-15);
            let angle = math::atan2(l.y, l.x);
            assert!((angle - x / BA * FRAC_PI_2).abs() < 1e-12);
            assert!((l.z - y).abs() < 1e-15);
        }
    }
}

/// Central differences of `eval`'s derivatives give the next order.
#[test]
fn jets_agree_with_differences() {
    for b in [bent_hole(), bent_slot()] {
        let [du, dv] = b.domain();
        let (u, v) = (0.3 * du[0] + 0.7 * du[1], 0.6 * dv[0] + 0.4 * dv[1]);
        let d = b.jets3_at(u, v);
        let at = |k: usize| Vector3::new(d[k][0], d[k][1], d[k][2]);
        let h = 1e-6;
        let e = |u: f64, v: f64| b.jets3_at(u, v);
        let diff = |k: usize, du: f64, dv: f64| {
            let (p, m) = (e(u + du, v + dv), e(u - du, v - dv));
            Vector3::new(p[k][0] - m[k][0], p[k][1] - m[k][1], p[k][2] - m[k][2])
                * (0.5 / (du + dv))
        };
        // (order, along u, along v) -> the order it differentiates into.
        for (from, along_u, to) in [
            (0, true, 1),
            (0, false, 2),
            (1, true, 3),
            (1, false, 4),
            (2, false, 5),
            (3, true, 6),
            (3, false, 7),
            (4, false, 8),
            (5, false, 9),
        ] {
            let got = if along_u {
                diff(from, h, 0.0)
            } else {
                diff(from, 0.0, h)
            };
            let want = at(to);
            assert!(
                (got - want).length() <= 1e-5 * (1.0 + want.length()),
                "order {to}: {got:?} against {want:?}"
            );
        }
        assert_eq!(b.eval(u, v).point, b.point(u, v));
    }
}

#[test]
fn enclosures_hold_the_jets() {
    for b in [bent_hole(), bent_slot()] {
        for &cell in b.cells() {
            let q = b.jet3_box(cell).unwrap();
            for (i, j) in [(0.0, 0.0), (0.3, 0.8), (1.0, 1.0), (0.5, 0.1)] {
                let u = cell[0][0] + i * (cell[0][1] - cell[0][0]);
                let v = cell[1][0] + j * (cell[1][1] - cell[1][0]);
                let d = b.jets3_at(u, v);
                for k in 0..10 {
                    for n in 0..3 {
                        assert!(q[k][n].contains(d[k][n]), "order {k} at ({u}, {v})");
                    }
                }
            }
        }
        let bx = b.bounds();
        assert!(bx.contains_with_tol(
            b.point(b.domain()[0][0], b.domain()[1][1]),
            crate::tol::Tolerance::RESOLUTION
        ));
    }
}

/// The natural normal is along `G_u × G_v`, so a face keeps its sense
/// through the fold.
#[test]
fn the_normal_is_along_the_tangents_cross() {
    for b in [bent_hole(), bent_slot()] {
        let [du, dv] = b.domain();
        let (u, v) = (0.5 * (du[0] + du[1]), 0.5 * (dv[0] + dv[1]));
        let e = b.eval(u, v);
        let n = b.normal(u, v).unwrap().get();
        let w = e.du.cross(e.dv);
        assert!((n - w * (1.0 / w.length())).length() < 1e-12);
    }
}

#[test]
fn walls_that_fold_badly_are_refused() {
    let k = FRAC_PI_2 / BA;
    // Through the axis: `r + z` reaches zero (the slot's `u` is the
    // strip's `z`).
    let deep = [[-1.5 * R, 0.0], [-0.002, 0.002]];
    assert_eq!(
        Bent::new(slot(), axis(), R, k, deep).unwrap_err(),
        Error::Degenerate
    );
    // A strip wider than a turn wraps onto itself.
    let wide = Frame::new(Point3::ORIGIN, UnitVector3::Z.get(), UnitVector3::X.get()).unwrap();
    let side = Surface::Plane(Plane::new(wide));
    let turn = TAU / k;
    assert_eq!(
        Bent::new(
            side.clone(),
            axis(),
            R,
            k,
            [[0.0, 1.01 * turn], [0.0, 0.01]]
        )
        .unwrap_err(),
        Error::Degenerate
    );
    assert!(Bent::new(side, axis(), R, k, [[0.0, 0.99 * turn], [0.0, 0.01]]).is_ok());
    // Only the walls a flat's cuts leave.
    let cone = crate::geom::Cone::new(Frame::WORLD, 0.001, 0.3).unwrap();
    assert_eq!(
        Bent::new(Surface::Cone(cone), axis(), R, k, [[0.0, 1.0], [0.0, T]]).unwrap_err(),
        Error::Degenerate
    );
    assert_eq!(
        Bent::new(hole(), axis(), -R, k, [[0.0, TAU], [0.0, T]]).unwrap_err(),
        Error::Degenerate
    );
}

#[test]
fn moved_and_mirrored_bent_surfaces_carry_their_points() {
    let b = bent_hole();
    let r = Rigid::rotation(UnitVector3::Z, 0.7)
        .then_after(&Rigid::translation(Vector3::new(0.1, 0.2, -0.3)));
    let moved = b.transformed(&r);
    let m = Mirror::new(
        Point3::new(0.0, 0.01, 0.0),
        Vector3::new(1.0, 2.0, 0.5).normalize().unwrap(),
    );
    let (mirrored, flip) = b.mirrored(&m).unwrap();
    for (u, v) in [(0.1, 0.0), (PI, 0.0005), (5.0, T)] {
        let p = b.point(u, v);
        assert!(moved.point(u, v).distance(r.apply_point(p)) < 1e-15);
        let q = flip.apply(crate::geom::Point2::new(u, v));
        assert!(mirrored.point(q.x, q.y).distance(m.apply_point(p)) < 1e-15);
    }
}

#[test]
fn points_off_a_bent_surface_project_onto_their_feet() {
    for b in [bent_hole(), bent_slot()] {
        let s = Surface::Bent(std::sync::Arc::new(b.clone()));
        let [du, dv] = b.domain();
        for (i, j) in [(0.25, 0.5), (0.6, 0.2), (0.9, 0.8)] {
            let (u, v) = (du[0] + i * (du[1] - du[0]), dv[0] + j * (dv[1] - dv[0]));
            let n = s.normal(u, v).unwrap().get();
            let p = s.point(u, v) + n * 1e-4;
            let foot = s.project(p);
            assert!(foot.point.distance(s.point(u, v)) < 1e-9, "{foot:?}");
            assert!((foot.distance - 1e-4).abs() < 1e-9);
        }
    }
}

#[test]
fn a_bent_surface_offsets_along_its_normal() {
    let b = bent_slot();
    let s = Surface::Bent(std::sync::Arc::new(b.clone()));
    let o = s.offset_within(2e-4, b.domain()).unwrap().unwrap();
    let Surface::Offset(_) = &o else {
        panic!("an offset")
    };
    let (u, v) = (0.0004, 0.001);
    let n = s.normal(u, v).unwrap().get();
    assert!(o.point(u, v).distance(s.point(u, v) + n * 2e-4) < 1e-12);
}
