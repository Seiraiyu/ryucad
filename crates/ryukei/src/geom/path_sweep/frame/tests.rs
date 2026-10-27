use super::*;
use crate::geom::{Circle, Frame, KnotVector, Line, NurbsCurve, Point3};
use core::f64::consts::PI;
use std::sync::Arc;

/// A wavy cubic in space, over `[0, 2]`.
fn wavy() -> Curve {
    let k = KnotVector::clamped(3, &[0.0, 1.0, 2.0], &[4, 1, 4]).unwrap();
    let pts = vec![
        Point3::new(0.0, 0.0, 0.0),
        Point3::new(1.0, 1.0, 0.5),
        Point3::new(2.0, -0.5, 1.0),
        Point3::new(3.0, 0.5, 2.0),
        Point3::new(4.0, 0.0, 2.5),
    ];
    Curve::Nurbs(Arc::new(NurbsCurve::new(k, pts, None).unwrap()))
}

fn circle() -> Curve {
    Curve::Circle(Circle::new(Frame::WORLD, 2.0).unwrap())
}

fn line() -> Curve {
    Curve::Line(Line::new(Point3::ORIGIN, Vector3::new(1.0, 2.0, 2.0)).unwrap())
}

fn z() -> UnitVector3 {
    Frame::WORLD.z()
}

/// Frames to test: each law along each path, with and without twist.
fn carries() -> Vec<Carry> {
    let seed = Vector3::new(0.0, 0.0, 1.0);
    let mut out = Vec::new();
    for (path, range) in [
        (wavy(), [0.0, 2.0]),
        (circle(), [0.3, 5.0]),
        (circle(), [0.0, TAU]),
        (line(), [-1.0, 2.0]),
    ] {
        // A closed range's frame closes only twisted by whole turns.
        let closed = path.period() == Some(range[1] - range[0]);
        for twist in [0.0, if closed { TAU } else { 1.3 }] {
            out.push(Carry::new(path.clone(), range, seed, FrameLaw::Minimal, twist).unwrap());
            let b = UnitVector3::new_unchecked(Vector3::new(0.6, 0.0, 0.8));
            out.push(Carry::new(path.clone(), range, seed, FrameLaw::Binormal(b), twist).unwrap());
        }
    }
    out
}

fn grid(c: &Carry) -> Vec<f64> {
    let [s0, s1] = c.range();
    (0..=40)
        .map(|i| s0 + (s1 - s0) * f64::from(i) / 40.0)
        .collect()
}

fn value(j: &JVec<f64>) -> Vector3 {
    Vector3::new(j[0].v, j[1].v, j[2].v)
}

fn along(j: &JVec<f64>) -> Vector3 {
    Vector3::new(j[0].d[1], j[1].d[1], j[2].d[1])
}

fn along2(j: &JVec<f64>) -> Vector3 {
    Vector3::new(j[0].h[2], j[1].h[2], j[2].h[2])
}

#[test]
fn axes_are_orthonormal_right_handed_and_along_the_tangent() {
    for c in carries() {
        for s in grid(&c) {
            let (p, [f1, f2, t]) = c.jets_at(s).unwrap();
            let (f1, f2, t) = (value(&f1), value(&f2), value(&t));
            let d = c.path().eval(s);
            assert!((value(&p) - d.point.to_vector()).length() <= 1e-12);
            assert!((t - d.d1 / d.d1.length()).length() <= 1e-12, "{c:?} {s}");
            for v in [f1, f2, t] {
                assert!((v.length() - 1.0).abs() <= 1e-12);
            }
            assert!(f1.dot(f2).abs() <= 1e-12 && f1.dot(t).abs() <= 1e-12);
            assert!((f1.cross(f2) - t).length() <= 1e-12, "{c:?} {s}");
        }
    }
}

/// A minimal frame without twist turns about the tangent by at most a
/// sliver: `f₁' · f₂` is the frame's spin.
#[test]
fn minimal_frames_do_not_spin() {
    for (path, range) in [(wavy(), [0.0, 2.0]), (circle(), [0.3, 5.0])] {
        let c = Carry::new(
            path,
            range,
            Vector3::new(0.0, 0.0, 1.0),
            FrameLaw::Minimal,
            0.0,
        )
        .unwrap();
        for s in grid(&c) {
            let (_, [f1, f2, t]) = c.jets_at(s).unwrap();
            let spin = along(&f1).dot(value(&f2));
            let bend = along(&t).length();
            assert!(
                spin.abs() <= 1e-5 * (1.0 + bend),
                "{s}: {spin} against {bend}"
            );
        }
    }
}

/// Along a circle, a rotation-minimizing frame started in the plane turns
/// with the radius.
#[test]
fn minimal_frames_turn_with_a_circle() {
    let c = Carry::new(
        circle(),
        [0.0, TAU],
        Vector3::new(1.0, 0.0, 0.0),
        FrameLaw::Minimal,
        0.0,
    )
    .unwrap();
    for s in grid(&c) {
        let [f1, f2, _] = c.axes_at(s).unwrap();
        let radial = Vector3::new(math::cos(s), math::sin(s), 0.0);
        assert!((f1 - radial).length() <= 1e-9, "{s}");
        assert!((f2 + Vector3::new(0.0, 0.0, 1.0)).length() <= 1e-9);
    }
}

#[test]
fn binormal_frames_keep_their_direction_in_the_plane() {
    let b = UnitVector3::new_unchecked(Vector3::new(0.6, 0.0, 0.8));
    let c = Carry::new(
        wavy(),
        [0.0, 2.0],
        Vector3::ZERO,
        FrameLaw::Binormal(b),
        0.0,
    )
    .unwrap();
    for s in grid(&c) {
        let [f1, f2, t] = c.axes_at(s).unwrap();
        let want = b.get() - t * b.get().dot(t);
        assert!((f2 - want / want.length()).length() <= 1e-12);
        assert!(f1.dot(b.get()).abs() <= 1e-12);
    }
}

#[test]
fn twist_turns_the_frame_about_the_tangent() {
    let seed = Vector3::new(0.0, 0.0, 1.0);
    let plain = Carry::new(wavy(), [0.0, 2.0], seed, FrameLaw::Minimal, 0.0).unwrap();
    let twisted = Carry::new(wavy(), [0.0, 2.0], seed, FrameLaw::Minimal, PI).unwrap();
    for s in grid(&plain) {
        let [a1, a2, _] = plain.axes_at(s).unwrap();
        let [b1, _, _] = twisted.axes_at(s).unwrap();
        let phi = PI * s / 2.0;
        let (sin, cos) = math::sin_cos(phi);
        assert!((b1 - (a1 * cos + a2 * sin)).length() <= 1e-12, "{s}");
    }
}

/// The frame and its first two derivatives agree either side of every
/// sample of a minimal frame.
#[test]
fn minimal_frames_are_smooth_across_their_samples() {
    let c = Carry::new(
        wavy(),
        [0.0, 2.0],
        Vector3::new(0.0, 0.0, 1.0),
        FrameLaw::Minimal,
        0.4,
    )
    .unwrap();
    let samples = c.samples();
    assert!(samples.len() > 4);
    for s in samples {
        // Away from the path's own knot, where its third derivative jumps.
        if (s - 1.0).abs() < 1e-9 {
            continue;
        }
        let h = 1e-7;
        let (_, a) = c.jets_at(s - h).unwrap();
        let (_, b) = c.jets_at(s + h).unwrap();
        for k in 0..3 {
            assert!((value(&a[k]) - value(&b[k])).length() <= 1e-6);
            assert!((along(&a[k]) - along(&b[k])).length() <= 1e-5);
            assert!((along2(&a[k]) - along2(&b[k])).length() <= 1e-4, "{s}");
        }
    }
}

/// Every value and derivative at points of a box lies in the box's
/// enclosures.
#[test]
fn enclosures_hold_over_stretches() {
    for c in carries() {
        let [s0, s1] = c.range();
        for (x, w) in [(0.0, 1.0), (0.1, 0.05), (0.37, 0.3), (0.9, 0.1)] {
            let r = [s0 + (s1 - s0) * x, s0 + (s1 - s0) * (x + w)];
            // A box too wide to certify the frame over has no enclosures.
            let Some((pb, ab)) = c.jet_box(r) else {
                assert!(w > 0.2, "{c:?} {r:?}");
                continue;
            };
            for i in 0..=8 {
                let s = r[0] + (r[1] - r[0]) * f64::from(i) / 8.0;
                let (p, a) = c.jets_at(s).unwrap();
                let pairs = [(&pb, &p), (&ab[0], &a[0]), (&ab[1], &a[1]), (&ab[2], &a[2])];
                for (enc, at) in pairs {
                    for k in 0..3 {
                        for (e, x) in [
                            (enc[k].v, at[k].v),
                            (enc[k].d[1], at[k].d[1]),
                            (enc[k].h[2], at[k].h[2]),
                        ] {
                            let slack = 1e-12 * (1.0 + x.abs());
                            assert!(
                                e.lo() - slack <= x && x <= e.hi() + slack,
                                "{:?} {r:?} at {s}: {x} not in {e:?}",
                                c.path()
                            );
                        }
                    }
                }
            }
        }
    }
}

/// A closed path's frame wraps: a box past its end is the stretch at its
/// start.
#[test]
fn closed_paths_wrap() {
    let c = Carry::new(
        circle(),
        [0.0, TAU],
        Vector3::new(1.0, 0.0, 0.0),
        FrameLaw::Minimal,
        0.0,
    )
    .unwrap();
    assert!(c.closed());
    let a = c.axes_at(0.25).unwrap();
    let b = c.axes_at(0.25 + TAU).unwrap();
    for k in 0..3 {
        assert!((a[k] - b[k]).length() <= 1e-12);
    }
    let (_, ab) = c.jet_box([TAU - 0.1, TAU + 0.1]).unwrap();
    for s in [TAU - 0.1, 0.05] {
        let f = c.axes_at(s).unwrap()[0].to_array();
        for k in 0..3 {
            assert!(ab[0][k].v.lo() - 1e-12 <= f[k] && f[k] <= ab[0][k].v.hi() + 1e-12);
        }
    }
}

/// A minimal frame round a circle comes back as it started; round a
/// closed curve out of a plane it comes back turned.
#[test]
fn closed_frames_close_or_turn() {
    let c = Carry::new(
        circle(),
        [0.0, TAU],
        Vector3::new(0.3, 0.0, 1.0),
        FrameLaw::Minimal,
        0.0,
    )
    .unwrap();
    let ends = [c.axes_at(0.0).unwrap(), c.axes_at(TAU).unwrap()];
    assert!(turned(ends[0], ends[1]).abs() <= 1e-9);
    // A crooked loop: the frame's turn there is the area its tangent
    // sweeps on the sphere, not a whole turn.
    let k = KnotVector::periodic(3, &[0.0, 1.0, 2.0, 3.0, 4.0], &[1, 1, 1, 1, 1]).unwrap();
    let pts = vec![
        Point3::new(1.0, 0.0, 0.5),
        Point3::new(0.0, 1.5, -0.5),
        Point3::new(-1.0, 0.0, 0.9),
        Point3::new(0.0, -0.7, -0.2),
    ];
    let saddle = Curve::Nurbs(Arc::new(NurbsCurve::new(k, pts, None).unwrap()));
    let c = Carry::new(
        saddle.clone(),
        [0.0, 4.0],
        Vector3::new(0.0, 0.0, 1.0),
        FrameLaw::Minimal,
        0.0,
    )
    .unwrap();
    let ends = [c.axes_at(0.0).unwrap(), c.axes_at(4.0).unwrap()];
    let alpha = turned(ends[0], ends[1]);
    assert!(alpha.abs() > 1e-3, "{alpha}");
    // Twisted back by that, it closes.
    let c = Carry::new(
        saddle,
        [0.0, 4.0],
        Vector3::new(0.0, 0.0, 1.0),
        FrameLaw::Minimal,
        -alpha,
    )
    .unwrap();
    let ends = [c.axes_at(0.0).unwrap(), c.axes_at(4.0).unwrap()];
    assert!(turned(ends[0], ends[1]).abs() <= 1e-12);
}

#[test]
fn undefined_frames_are_refused() {
    let seed = Vector3::new(0.0, 0.0, 1.0);
    // A seed along the tangent, a binormal along it, ranges off the path.
    let up = Curve::Line(Line::new(Point3::ORIGIN, Vector3::new(0.0, 0.0, 1.0)).unwrap());
    let refused = [
        Carry::new(up.clone(), [0.0, 1.0], seed, FrameLaw::Minimal, 0.0),
        Carry::new(up, [0.0, 1.0], seed, FrameLaw::Binormal(z()), 0.0),
        Carry::new(wavy(), [-0.5, 1.0], seed, FrameLaw::Minimal, 0.0),
        Carry::new(wavy(), [1.0, 1.0], seed, FrameLaw::Minimal, 0.0),
        Carry::new(circle(), [0.0, 7.0], seed, FrameLaw::Minimal, 0.0),
        Carry::new(wavy(), [0.0, 1.0], seed, FrameLaw::Minimal, f64::NAN),
    ];
    for r in refused {
        assert!(r.is_err(), "{r:?}");
    }
}

/// A binormal law whose direction the tangent passes through inside the
/// range can't be certified anywhere near that point: halving stops at
/// its depth, quickly, with `Degenerate`.
#[test]
fn a_frame_undefined_inside_is_refused_quickly() {
    // (s, 0, s²) over [-1, 1]: the tangent is x at s = 0.
    let k = KnotVector::clamped(2, &[-1.0, 1.0], &[3, 3]).unwrap();
    let pts = vec![
        Point3::new(-1.0, 0.0, 1.0),
        Point3::new(0.0, 0.0, -1.0),
        Point3::new(1.0, 0.0, 1.0),
    ];
    let path = Curve::Nurbs(Arc::new(NurbsCurve::new(k, pts, None).unwrap()));
    let x = Frame::WORLD.x();
    let start = std::time::Instant::now();
    let r = Carry::new(path, [-1.0, 1.0], Vector3::ZERO, FrameLaw::Binormal(x), 0.0);
    assert_eq!(r, Err(Error::Degenerate));
    assert!(start.elapsed().as_secs() < 10);
}
