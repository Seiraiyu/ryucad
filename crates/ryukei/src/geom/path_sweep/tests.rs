use super::*;
use crate::geom::{Axis, Circle, KnotVector, Line, NurbsCurve, Surface};
use core::f64::consts::{PI, TAU};
use std::sync::Arc;

/// A wavy cubic path over `[0, 2]`, starting at the origin along `x`.
pub(crate) fn wavy_path() -> Curve {
    let k = KnotVector::clamped(3, &[0.0, 1.0, 2.0], &[4, 1, 4]).unwrap();
    let pts = vec![
        Point3::new(0.0, 0.0, 0.0),
        Point3::new(1.0, 0.0, 0.0),
        Point3::new(2.0, 1.0, 0.5),
        Point3::new(3.0, 0.5, 1.5),
        Point3::new(4.0, 0.0, 2.0),
    ];
    Curve::Nurbs(Arc::new(NurbsCurve::new(k, pts, None).unwrap()))
}

/// The plane across the path's start, `x` along `y`.
pub(crate) fn across() -> Frame {
    Frame::new(
        Point3::ORIGIN,
        Vector3::new(1.0, 0.0, 0.0),
        Vector3::new(0.0, 1.0, 0.0),
    )
    .unwrap()
}

/// An arc of radius 0.2 round the path's start, in the plane across it.
pub(crate) fn arc() -> (Curve, [f64; 2]) {
    (
        Curve::Circle(Circle::new(across(), 0.2).unwrap()),
        [0.3, 2.5],
    )
}

/// The arc along a line, minimal, twisted and growing: cheap to certify.
pub(crate) fn straight() -> Sweep {
    let line = Curve::Line(Line::new(Point3::ORIGIN, Vector3::new(1.0, 0.1, 0.0)).unwrap());
    Sweep::new(
        arc(),
        (line, [0.0, 2.0]),
        across(),
        FrameLaw::Minimal,
        0.7,
        1.5,
    )
    .unwrap()
}

/// The arc along the wavy path, minimal, twisted and growing.
pub(crate) fn wavy() -> Sweep {
    Sweep::new(
        arc(),
        (wavy_path(), [0.0, 2.0]),
        across(),
        FrameLaw::Minimal,
        0.7,
        1.5,
    )
    .unwrap()
}

/// A straight segment of profile along the wavy path, keeping `z` level.
pub(crate) fn level() -> Sweep {
    let segment = Line::new(Point3::new(0.0, -0.1, 0.1), Vector3::new(0.0, 1.0, 0.2)).unwrap();
    Sweep::new(
        (Curve::Line(segment), [0.0, 0.25]),
        (wavy_path(), [0.0, 2.0]),
        across(),
        FrameLaw::Binormal(Frame::WORLD.z()),
        0.0,
        1.0,
    )
    .unwrap()
}

fn sweeps() -> Vec<Sweep> {
    vec![wavy(), level()]
}

/// Parameters over a sweep's domain.
fn grid(w: &Sweep) -> Vec<(f64, f64)> {
    let [[u0, u1], [s0, s1]] = w.domain();
    let mut out = Vec::new();
    for i in 0..=8 {
        for j in 0..=10 {
            out.push((
                u0 + (u1 - u0) * f64::from(i) / 8.0,
                s0 + (s1 - s0) * f64::from(j) / 10.0,
            ));
        }
    }
    out
}

/// The sweep starts on its profile, and each section is the profile
/// moved rigidly and scaled.
#[test]
fn sections_are_the_profile_moved_and_scaled() {
    for w in sweeps() {
        let [u0, u1] = w.profile_range();
        let [s0, s1] = w.path_range();
        for (u, s) in grid(&w) {
            if s == s0 {
                assert!(w.point(u, s).distance(w.profile().point(u)) <= 1e-12);
            }
            // Distances within a section scale by σ.
            let sigma = 1.0 + (w.scale() - 1.0) * (s - s0) / (s1 - s0);
            let v = (u0 + u1) / 2.0;
            let d0 = w.profile().point(u).distance(w.profile().point(v));
            let d = w.point(u, s).distance(w.point(v, s));
            assert!(
                (d - sigma * d0).abs() <= 1e-12,
                "{u} {s}: {d} against {}",
                sigma * d0
            );
        }
    }
}

/// `eval` agrees with central differences of `point`.
#[test]
fn derivatives_are_the_surfaces() {
    for w in sweeps() {
        let h = 1e-5;
        for (u, s) in grid(&w) {
            let [[u0, u1], [s0, s1]] = w.domain();
            let (u, s) = (u.clamp(u0 + h, u1 - h), s.clamp(s0 + h, s1 - h));
            // The path's third derivative jumps at its knot, and with it
            // the second derivatives along it: differences across it
            // aren't derivatives.
            if (s - 1.0).abs() <= 2.0 * h {
                continue;
            }
            let e = w.eval(u, s);
            let du = (w.point(u + h, s) - w.point(u - h, s)) / (2.0 * h);
            let ds = (w.point(u, s + h) - w.point(u, s - h)) / (2.0 * h);
            assert!(
                (e.du - du).length() <= 1e-7 && (e.dv - ds).length() <= 1e-7,
                "{u} {s}: {:?} {:?} / {:?} {:?}",
                e.du,
                du,
                e.dv,
                ds
            );
            let duu = (w.eval(u + h, s).du - w.eval(u - h, s).du) / (2.0 * h);
            let dss = (w.eval(u, s + h).dv - w.eval(u, s - h).dv) / (2.0 * h);
            let dus = (w.eval(u, s + h).du - w.eval(u, s - h).du) / (2.0 * h);
            assert!((e.dvv - dss).length() <= 1e-5, "{u} {s}");
            assert!((e.duu - duu).length() <= 1e-5 && (e.duv - dus).length() <= 1e-5);
            let n = w.normal(u, s).unwrap().get();
            assert!(n.dot(e.du).abs() <= 1e-9 && n.dot(e.dv).abs() <= 1e-9);
        }
    }
}

#[test]
fn enclosures_hold_over_boxes() {
    for w in sweeps() {
        let [[u0, u1], [s0, s1]] = w.domain();
        for (x, y, a) in [(0.0, 0.0, 1.0), (0.2, 0.4, 0.1), (0.6, 0.45, 0.3)] {
            let cell = [
                [u0 + (u1 - u0) * x, u0 + (u1 - u0) * (x + a).min(1.0)],
                [s0 + (s1 - s0) * y, s0 + (s1 - s0) * (y + a).min(1.0)],
            ];
            let Some(q) = w.jet_box(cell) else {
                assert!(a > 0.5);
                continue;
            };
            for i in 0..=6 {
                for k in 0..=6 {
                    let u = cell[0][0] + (cell[0][1] - cell[0][0]) * f64::from(i) / 6.0;
                    let s = cell[1][0] + (cell[1][1] - cell[1][0]) * f64::from(k) / 6.0;
                    let e = w.eval(u, s);
                    let want = [e.point.to_vector(), e.du, e.dv, e.duu, e.duv, e.dvv];
                    for (c, qc) in q.iter().enumerate() {
                        let got = [qc.v, qc.d[0], qc.d[1], qc.h[0], qc.h[1], qc.h[2]];
                        for (g, x) in got.iter().zip(want) {
                            let x = x.to_array()[c];
                            let slack = 1e-12 * (1.0 + x.abs());
                            assert!(g.lo() - slack <= x && x <= g.hi() + slack);
                        }
                    }
                }
            }
        }
    }
}

/// Along a line, a minimal sweep with no twist or scale is the profile's
/// extrusion; round a circle whose axis lies in the profile's plane, its
/// revolution.
#[test]
fn lines_extrude_and_circles_revolve() {
    let dir = Vector3::new(1.0, 0.0, 0.0);
    let line = Curve::Line(Line::new(Point3::ORIGIN, dir).unwrap());
    let w = Sweep::new(
        arc(),
        (line, [0.0, 3.0]),
        across(),
        FrameLaw::Minimal,
        0.0,
        1.0,
    )
    .unwrap();
    for (u, s) in grid(&w) {
        let want = w.profile().point(u) + dir * s;
        assert!(w.point(u, s).distance(want) <= 1e-12);
    }
    // A circle of radius 2 about the axis through (0, 2, 0) along z,
    // starting at the origin heading along x.
    let f = Frame::new(
        Point3::new(0.0, 2.0, 0.0),
        Vector3::new(0.0, 0.0, 1.0),
        Vector3::new(0.0, -1.0, 0.0),
    )
    .unwrap();
    let circle = Curve::Circle(Circle::new(f, 2.0).unwrap());
    let w = Sweep::new(
        arc(),
        (circle, [0.0, PI]),
        across(),
        FrameLaw::Minimal,
        0.0,
        1.0,
    )
    .unwrap();
    let axis = Axis::new(Point3::new(0.0, 2.0, 0.0), Vector3::new(0.0, 0.0, 1.0)).unwrap();
    for (u, s) in grid(&w) {
        let turn = Rigid::rotation(axis.dir(), s);
        let at = axis.origin().to_vector();
        let q = w.profile().point(u) - at;
        let want = Point3::ORIGIN + at + turn.apply_vector(q.to_vector());
        assert!(w.point(u, s).distance(want) <= 1e-9, "{u} {s}");
    }
}

/// Points on the sweep project to themselves, and no point of a dense
/// grid is nearer than the projection.
#[test]
fn projection_finds_the_nearest_point() {
    for w in sweeps() {
        for (u, s) in grid(&w).into_iter().step_by(7) {
            let p = w.point(u, s);
            assert!(w.project(p).distance <= crate::tol::LINEAR_RESOLUTION);
            let off = p + Vector3::new(0.03, -0.05, 0.04);
            let foot = w.project(off);
            let [[u0, u1], [s0, s1]] = w.domain();
            let mut least = f64::INFINITY;
            for i in 0..=60 {
                for k in 0..=60 {
                    let q = w.point(
                        u0 + (u1 - u0) * f64::from(i) / 60.0,
                        s0 + (s1 - s0) * f64::from(k) / 60.0,
                    );
                    least = least.min(q.distance(off));
                }
            }
            assert!(
                foot.distance <= least + 1e-12,
                "{} against {least}",
                foot.distance
            );
            let (fu, fs) = foot.param;
            assert!(w.point(fu, fs).distance(foot.point) <= 1e-12);
        }
    }
}

#[test]
fn moved_and_reflected_sweeps() {
    let r = Rigid::rotation(Frame::WORLD.y(), 0.4)
        .then_after(&Rigid::translation(Vector3::new(1.0, 2.0, 3.0)));
    let m = Mirror::new(
        Point3::new(0.5, 0.0, 0.0),
        UnitVector3::new_unchecked(Vector3::new(0.6, 0.8, 0.0)),
    );
    for w in sweeps() {
        let moved = w.transformed(&r);
        let reflected = w.mirrored(&m).unwrap();
        for (u, s) in grid(&w) {
            let p = w.point(u, s);
            assert!(moved.point(u, s).distance(r.apply_point(p)) <= 1e-12);
            assert!(
                reflected.point(u, s).distance(m.apply_point(p)) <= 1e-12,
                "{u} {s}"
            );
            let n = m.apply_vector(w.normal(u, s).unwrap().get());
            assert!((reflected.normal(u, s).unwrap().get() + n).length() <= 1e-9);
        }
        for b in [moved.bounds(), reflected.bounds()] {
            assert!(b.min().x.is_finite());
        }
    }
}

/// A closed profile along a closed path is a tube with no seams.
#[test]
fn a_tube_round_a_closed_path() {
    let k = KnotVector::periodic(3, &[0.0, 1.0, 2.0, 3.0, 4.0], &[1, 1, 1, 1, 1]).unwrap();
    let pts = vec![
        Point3::new(2.0, 0.0, 0.0),
        Point3::new(0.0, 2.5, 0.5),
        Point3::new(-2.0, 0.0, 0.0),
        Point3::new(0.0, -1.5, -0.5),
    ];
    let path = Curve::Nurbs(Arc::new(NurbsCurve::new(k, pts, None).unwrap()));
    let start = path.point(0.0);
    let t = path.tangent(0.0).unwrap();
    let f = Frame::new(start, t.get(), Vector3::new(0.0, 0.0, 1.0)).unwrap();
    let ring = Curve::Circle(Circle::new(f, 0.3).unwrap());
    let carry = Carry::new(
        path.clone(),
        [0.0, 4.0],
        f.x().get(),
        FrameLaw::Minimal,
        0.0,
    )
    .unwrap();
    let spin = frame::turned(carry.axes_at(0.0).unwrap(), carry.axes_at(4.0).unwrap());
    let w = Sweep::new(
        (ring, [0.0, TAU]),
        (path, [0.0, 4.0]),
        f,
        FrameLaw::Minimal,
        -spin,
        1.0,
    )
    .unwrap();
    assert_eq!(w.periods(), (Some(TAU), Some(4.0)));
    assert_eq!(w.closed_genus(), Some(1));
    for u in [0.0, 1.0, 4.0] {
        assert!(w.point(u, 0.0).distance(w.point(u, 4.0)) <= 1e-12);
        assert!(w.point(u, 0.5).distance(w.point(u, 4.5)) <= 1e-12);
    }
}

#[test]
fn folds_and_stalls_are_refused() {
    let r = |profile: (Curve, [f64; 2]), law: FrameLaw, scale: f64| {
        Sweep::new(
            profile,
            (wavy_path(), [0.0, 2.0]),
            across(),
            law,
            0.0,
            scale,
        )
    };
    // A profile wider than the path bends: it folds.
    let wide = Line::new(Point3::new(0.0, -5.0, 0.0), Vector3::new(0.0, 1.0, 0.0)).unwrap();
    let start = std::time::Instant::now();
    assert_eq!(
        r((Curve::Line(wide), [0.0, 10.0]), FrameLaw::Minimal, 1.0),
        Err(Error::Degenerate)
    );
    assert!(start.elapsed().as_secs() < 30);
    // A profile along the path's tangent stalls.
    let along = Line::new(Point3::ORIGIN, Vector3::new(1.0, 0.0, 0.0)).unwrap();
    assert_eq!(
        r((Curve::Line(along), [0.0, 0.1]), FrameLaw::Minimal, 1.0),
        Err(Error::Degenerate)
    );
    assert_eq!(r(arc(), FrameLaw::Minimal, 0.0), Err(Error::Degenerate));
    assert_eq!(r(arc(), FrameLaw::Minimal, f64::NAN), Err(Error::NonFinite));
    assert_eq!(
        r((arc().0, [0.0, 7.0]), FrameLaw::Minimal, 1.0),
        Err(Error::Degenerate)
    );
    // Surfaces built from it stay out of the closed forms.
    let s = Surface::Sweep(Arc::new(wavy()));
    assert_eq!(s.frame(), None);
    assert!(s.to_nurbs(s.domain()).is_err());
}

/// The bounds the mesher uses hold at points of each box.
#[test]
fn derivative_bounds_hold() {
    for w in sweeps() {
        let [[u0, u1], [s0, s1]] = w.domain();
        for (x, y, a) in [(0.1, 0.2, 0.1), (0.5, 0.6, 0.05)] {
            let cell = [
                [u0 + (u1 - u0) * x, u0 + (u1 - u0) * (x + a)],
                [s0 + (s1 - s0) * y, s0 + (s1 - s0) * (y + a)],
            ];
            let b = w.derivative_bound(cell);
            assert!(b.turn_u.is_finite() && b.turn_v.is_finite());
            for i in 0..=8 {
                for k in 0..=8 {
                    let u = cell[0][0] + (cell[0][1] - cell[0][0]) * f64::from(i) / 8.0;
                    let s = cell[1][0] + (cell[1][1] - cell[1][0]) * f64::from(k) / 8.0;
                    let e = w.eval(u, s);
                    assert!(e.duu.length() <= b.duu && e.duv.length() <= b.duv);
                    assert!(e.dvv.length() <= b.dvv);
                    let h = 1e-6;
                    let n = |u: f64, s: f64| w.normal(u, s).unwrap().get();
                    let nu = (n(u + h, s) - n(u - h, s)).length() / (2.0 * h);
                    let ns = (n(u, s + h) - n(u, s - h)).length() / (2.0 * h);
                    assert!(nu <= b.turn_u * 1.001 + 1e-6 && ns <= b.turn_v * 1.001 + 1e-6);
                }
            }
        }
    }
}

/// The NURBS form keeps within its bound of the sweep at the same
/// parameters (mapped along the profile), and its bound within the
/// tolerance asked for.
#[test]
fn nurbs_forms_keep_within_their_bound() {
    for w in sweeps() {
        let [s0, s1] = w.path_range();
        for tolerance in [1e-5, 1e-7] {
            let (n, [ru, rs], bound) = w.to_nurbs_within([s0, s1], tolerance).unwrap();
            assert!(bound <= tolerance);
            assert_eq!(rs, Reparam::Identity);
            for (u, s) in grid(&w) {
                let q = n.point(ru.to_nurbs(u), s);
                assert!(q.distance(w.point(u, s)) <= bound + 1e-12, "{u} {s}");
            }
        }
        // Over part of the path too; off it, refused.
        let mid = (s0 + s1) / 2.0;
        let (n, _, bound) = w.to_nurbs_within([s0, mid], 1e-6).unwrap();
        assert_eq!(n.domain()[1], [s0, mid]);
        assert!(bound <= 1e-6);
        assert!(w.to_nurbs_within([s0 - 0.1, mid], 1e-6).is_err());
        assert!(w.to_nurbs_within([mid, mid], 1e-6).is_err());
    }
}

/// The derivatives up to third order agree with the second-order jets,
/// and the third with differences of the second.
#[test]
fn third_derivatives_match_differences() {
    use crate::geom::nurbs::third::index;
    for w in sweeps() {
        for (u, s) in grid(&w) {
            // Inside the domain, off the path's knot at 1, where the
            // third derivative jumps.
            let [[u0, u1], [s0, s1]] = w.domain();
            let (u, s) = (u.clamp(u0 + 1e-3, u1 - 1e-3), s.clamp(s0 + 1e-3, s1 - 1e-3));
            if (s - 1.0).abs() < 2e-3 {
                continue;
            }
            let d = w.jets3_at(u, s).map(|x| Vector3::new(x[0], x[1], x[2]));
            let e = w.eval(u, s);
            for (got, want) in [
                (d[0], e.point.to_vector()),
                (d[1], e.du),
                (d[2], e.dv),
                (d[3], e.duu),
                (d[4], e.duv),
                (d[5], e.dvv),
            ] {
                assert!(
                    (got - want).length() <= 1e-9 * (1.0 + want.length()),
                    "{u} {s}"
                );
            }
            let h = 1e-5;
            let du = |u: f64, s: f64| w.eval(u, s);
            let checks = [
                (
                    index(3, 0),
                    (du(u + h, s).duu - du(u - h, s).duu) / (2.0 * h),
                ),
                (
                    index(2, 1),
                    (du(u, s + h).duu - du(u, s - h).duu) / (2.0 * h),
                ),
                (
                    index(1, 2),
                    (du(u + h, s).dvv - du(u - h, s).dvv) / (2.0 * h),
                ),
                (
                    index(0, 3),
                    (du(u, s + h).dvv - du(u, s - h).dvv) / (2.0 * h),
                ),
            ];
            for (k, fd) in checks {
                assert!(
                    (d[k] - fd).length() <= 1e-4 * (1.0 + fd.length()),
                    "{k} at {u} {s}: {:?} against {fd:?}",
                    d[k]
                );
            }
        }
    }
}

/// Enclosures of the third-order jets over a box hold at its points.
#[test]
fn third_order_enclosures_hold() {
    for w in sweeps() {
        let [[u0, u1], [s0, s1]] = w.domain();
        let cell = [
            [u0 + (u1 - u0) * 0.3, u0 + (u1 - u0) * 0.4],
            [s0 + (s1 - s0) * 0.6, s0 + (s1 - s0) * 0.7],
        ];
        let b = w.jet3_box(cell).unwrap();
        for i in 0..=6 {
            for k in 0..=6 {
                let u = cell[0][0] + (cell[0][1] - cell[0][0]) * f64::from(i) / 6.0;
                let s = cell[1][0] + (cell[1][1] - cell[1][0]) * f64::from(k) / 6.0;
                let d = w.jets3_at(u, s);
                for (m, v) in d.iter().enumerate() {
                    for c in 0..3 {
                        assert!(b[m][c].contains(v[c]), "{m} {c} at {u} {s}");
                    }
                }
            }
        }
    }
}

/// A sweep offsets: each point moves along the natural normal by the
/// distance, and the offset is refused where it would fold.
#[test]
fn sweeps_offset() {
    for w in sweeps() {
        let d = 0.02;
        let s = Surface::Sweep(Arc::new(w.clone()));
        let Some(Ok(Surface::Offset(o))) = s.offset(d) else {
            panic!("an offset");
        };
        for (u, t) in grid(&w) {
            let want = w.point(u, t) + w.normal(u, t).unwrap().get() * d;
            assert!(o.point(u, t).distance(want) <= 1e-12, "{u} {t}");
            assert_eq!(o.normal(u, t), w.normal(u, t));
        }
        // Projection reaches the offset's own points.
        let [[u0, u1], _] = w.domain();
        let (u, t) = ((u0 + 2.0 * u1) / 3.0, 1.3);
        let q = o.point(u, t);
        assert!(o.project(q).point.distance(q) <= 1e-9);
    }
    // The arc of radius 0.2 offset inwards by more than its radius turns
    // inside out.
    let s = Surface::Sweep(Arc::new(wavy()));
    assert_eq!(
        s.offset(-0.25).map(|r| r.err()),
        Some(Some(Error::Degenerate))
    );
}
