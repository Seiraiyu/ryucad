//! NURBS from bytes. Constructors must refuse bad input without panicking.
//! A valid curve or surface must evaluate to finite points inside its hull
//! bounds, and its projection of a point must be a point of it no farther
//! away than any sample. Any panic is a bug.

#![no_main]

use libfuzzer_sys::fuzz_target;
use ryukei::geom::{KnotVector, NurbsCurve, NurbsSurface, Point3};
use ryukei_testkit::euler_script::Bytes;

/// A knot vector from bytes; sometimes an invalid one.
fn knots(b: &mut Bytes, max_degree: usize) -> Option<KnotVector> {
    let periodic = b.byte() % 4 == 0;
    let degree = 1 + b.pick(max_degree);
    let spans = 1 + b.pick(5);
    let mut knots = vec![b.range(-3.0, 3.0)];
    for _ in 0..spans {
        let last = knots[knots.len() - 1];
        knots.push(last + b.range(0.0, 2.0));
    }
    let mut mults: Vec<usize> = (0..=spans).map(|_| 1 + b.pick(degree + 1)).collect();
    if b.byte() % 8 != 0 {
        // Mostly well-formed ends.
        if periodic {
            mults[spans] = mults[0];
        } else {
            mults[0] = degree + 1;
            mults[spans] = degree + 1;
        }
    }
    let k = if periodic {
        KnotVector::periodic(degree, &knots, &mults)
    } else {
        KnotVector::clamped(degree, &knots, &mults)
    };
    k.ok()
}

fn point(b: &mut Bytes) -> Point3 {
    // Now and then outside the size box.
    let r = if b.byte() % 32 == 0 { 600.0 } else { 6.0 };
    Point3::new(b.range(-r, r), b.range(-r, r), b.range(-r, r))
}

/// Weights, now and then an invalid one.
fn weights(b: &mut Bytes, n: usize) -> Vec<f64> {
    (0..n)
        .map(|_| if b.byte() % 64 == 0 { -1.0 } else { b.range(0.05, 5.0) })
        .collect()
}

/// What projection promises: within linear resolution of the nearest
/// point. Hull bounds use the same allowance for rounding.
const EPS: f64 = ryukei::tol::LINEAR_RESOLUTION;

fn curve(b: &mut Bytes) {
    let Some(k) = knots(b, 6) else {
        return;
    };
    let n = k.control_count() + usize::from(b.byte() % 32 == 0);
    let points = (0..n).map(|_| point(b)).collect();
    let w = weights(b, n);
    let Ok(c) = NurbsCurve::new(k, points, Some(w)) else {
        return;
    };
    let [t0, t1] = c.domain();
    let bounds = c.bounds([t0, t1]).grow(EPS);
    let samples: Vec<Point3> = (0..=16)
        .map(|i| c.point(t0 + (t1 - t0) * f64::from(i) / 16.0))
        .collect();
    for q in &samples {
        assert!(q.x.is_finite() && q.y.is_finite() && q.z.is_finite());
        assert!(bounds.contains_with_tol(*q, ryukei::tol::Tolerance::RESOLUTION));
    }
    let p = point(b);
    let foot = c.project(p);
    assert_eq!(c.point(foot.param), foot.point);
    for q in &samples {
        assert!(foot.distance <= q.distance(p) + EPS, "{foot:?} beaten by {q:?}");
    }
}

fn surface(b: &mut Bytes) {
    let (Some(ku), Some(kv)) = (knots(b, 3), knots(b, 3)) else {
        return;
    };
    let n = ku.control_count() * kv.control_count();
    let points = (0..n).map(|_| point(b)).collect();
    let w = weights(b, n);
    let Ok(s) = NurbsSurface::new([ku, kv], points, Some(w)) else {
        return;
    };
    let [[u0, u1], [v0, v1]] = s.domain();
    let bounds = s.bounds().grow(EPS);
    let mut samples = Vec::new();
    for i in 0..=8 {
        for j in 0..=8 {
            let u = u0 + (u1 - u0) * f64::from(i) / 8.0;
            let v = v0 + (v1 - v0) * f64::from(j) / 8.0;
            samples.push(s.point(u, v));
        }
    }
    for q in &samples {
        assert!(q.x.is_finite() && q.y.is_finite() && q.z.is_finite());
        assert!(bounds.contains_with_tol(*q, ryukei::tol::Tolerance::RESOLUTION));
    }
    let p = point(b);
    let foot = s.project(p);
    assert_eq!(s.point(foot.param.0, foot.param.1), foot.point);
    for q in &samples {
        assert!(foot.distance <= q.distance(p) + EPS, "{foot:?} beaten by {q:?}");
    }
}

fuzz_target!(|data: &[u8]| {
    let Some((&mode, rest)) = data.split_first() else {
        return;
    };
    let mut b = Bytes::new(rest);
    if mode % 2 == 0 {
        curve(&mut b);
    } else {
        surface(&mut b);
    }
});
