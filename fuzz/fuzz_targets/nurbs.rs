//! NURBS from bytes. Constructors must refuse bad input without panicking.
//! A valid curve or surface must evaluate to finite points inside its hull
//! bounds, and its projection of a point must be a point of it no farther
//! away than any sample. Any panic is a bug. The cases are read by
//! `ryukei_testkit::nurbs_bytes`, which ryukei's regression tests share.

#![no_main]

use libfuzzer_sys::fuzz_target;
use ryukei::geom::{NurbsCurve, NurbsSurface, Point3};
use ryukei_testkit::nurbs_bytes::{Case, case};

/// What projection promises: within linear resolution of the nearest
/// point. Hull bounds use the same allowance for rounding.
const EPS: f64 = ryukei::tol::LINEAR_RESOLUTION;

fn curve(c: &NurbsCurve, p: Point3) {
    let [t0, t1] = c.domain();
    let bounds = c.bounds([t0, t1]).grow(EPS);
    let samples: Vec<Point3> = (0..=16)
        .map(|i| c.point(t0 + (t1 - t0) * f64::from(i) / 16.0))
        .collect();
    for q in &samples {
        assert!(q.x.is_finite() && q.y.is_finite() && q.z.is_finite());
        assert!(bounds.contains_with_tol(*q, ryukei::tol::Tolerance::RESOLUTION));
    }
    let foot = c.project(p);
    assert_eq!(c.point(foot.param), foot.point);
    for q in &samples {
        assert!(foot.distance <= q.distance(p) + EPS, "{foot:?} beaten by {q:?}");
    }
}

fn surface(s: &NurbsSurface, p: Point3) {
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
    let foot = s.project(p);
    assert_eq!(s.point(foot.param.0, foot.param.1), foot.point);
    for q in &samples {
        assert!(foot.distance <= q.distance(p) + EPS, "{foot:?} beaten by {q:?}");
    }
}

fuzz_target!(|data: &[u8]| {
    match case(data) {
        Some(Case::Curve(c, p)) => curve(&c, p),
        Some(Case::Surface(s, p)) => surface(&s, p),
        None => {}
    }
});
