use super::*;
use crate::geom::{Circle, Frame};

fn cubic(z: f64) -> NurbsCurve {
    let k = KnotVector::clamped(3, &[0.0, 0.4, 1.0], &[4, 1, 4]).unwrap();
    NurbsCurve::new(
        k,
        vec![
            Point3::new(0.0, 0.0, z),
            Point3::new(1.0, 1.0, z),
            Point3::new(2.0, 0.0, z),
            Point3::new(3.0, -1.0, z),
            Point3::new(4.0, 0.0, z),
        ],
        Some(vec![1.0, 2.0, 0.5, 1.5, 3.0]),
    )
    .unwrap()
}

/// A quarter of a circle of `r` at height `z`, as a rational quadratic.
fn arc(r: f64, z: f64) -> NurbsCurve {
    let f = Frame::new(
        Point3::new(0.0, 0.0, z),
        Vector3::new(0.0, 0.0, 1.0),
        Vector3::new(1.0, 0.0, 0.0),
    )
    .unwrap();
    let (n, _) = Circle::new(f, r).unwrap().to_nurbs().unwrap();
    n.part([0.0, core::f64::consts::FRAC_PI_2]).unwrap()
}

/// The distance from each of a curve's points at many parameters to the
/// other curve.
fn same_curve(a: &NurbsCurve, b: &NurbsCurve) -> f64 {
    let [t0, t1] = a.domain();
    (0..=50)
        .map(|i| b.distance(a.point(t0 + (t1 - t0) * f64::from(i) / 50.0)))
        .fold(0.0, f64::max)
}

fn rebuilt(knots: &KnotVector, h: &[Hom<Vector3>]) -> NurbsCurve {
    NurbsCurve::new(
        knots.clone(),
        h.iter().map(|x| Point3::ORIGIN + x.p).collect(),
        Some(h.iter().map(|x| x.w).collect()),
    )
    .unwrap()
}

#[test]
fn compatible_curves_are_the_same_curves_with_unit_end_weights() {
    let curves = [cubic(0.0), arc(2.0, 1.0)];
    let (knots, nets) = compatible(&curves).unwrap();
    assert_eq!(knots.degree(), 3);
    assert_eq!(knots.domain(), [0.0, 1.0]);
    for (c, h) in curves.iter().zip(&nets) {
        let r = rebuilt(&knots, h);
        assert!(same_curve(c, &r) <= 1e-12 && same_curve(&r, c) <= 1e-12);
        assert_eq!((h[0].w, h[h.len() - 1].w), (1.0, 1.0));
        assert!(r.point(0.0).distance(c.point(c.domain()[0])) <= 1e-12);
    }
    assert_eq!(nets[0].len(), nets[1].len());
}

#[test]
fn skins_pass_through_their_sections() {
    let curves = [arc(1.0, 0.0), arc(2.0, 1.0), arc(1.5, 3.0)];
    let (knots, nets) = compatible(&curves).unwrap();
    let v = [0.0, 0.4, 1.0];
    let s = skin(&knots, &nets, &v, false, (None, None))
        .unwrap()
        .unwrap();
    for (k, c) in curves.iter().enumerate() {
        for i in 0..=10 {
            let u = f64::from(i) / 10.0;
            assert!(c.distance(s.point(u, v[k])) <= 1e-12, "{k} {u}");
        }
    }
    // With a derivative at each end.
    let d = Vector3::new(0.0, 0.0, 2.0);
    let s = skin(&knots, &nets, &v, false, (Some(d), Some(d)))
        .unwrap()
        .unwrap();
    for u in [0.0, 0.3, 1.0] {
        assert!((s.eval(u, 0.0).dv - d).length() <= 1e-9);
        assert!((s.eval(u, 1.0).dv - d).length() <= 1e-9);
        assert!(curves[1].distance(s.point(u, 0.4)) <= 1e-12);
    }
}

#[test]
fn closed_skins_come_round() {
    let curves = [arc(1.0, 0.0), arc(2.0, 1.0), arc(1.5, 2.0), arc(1.2, 0.5)];
    let (knots, nets) = compatible(&curves).unwrap();
    let v = [0.0, 0.25, 0.5, 0.8];
    let s = skin(&knots, &nets, &v, true, (None, None))
        .unwrap()
        .unwrap();
    assert_eq!(s.periods().1, Some(1.0));
    for (k, c) in curves.iter().enumerate() {
        assert!(c.distance(s.point(0.5, v[k])) <= 1e-12);
    }
}

/// Sections whose weights swing far enough interpolate to a weight at or
/// below zero, which is refused.
#[test]
fn weights_that_fall_to_zero_are_refused() {
    let k = KnotVector::clamped(2, &[0.0, 1.0], &[3, 3]).unwrap();
    let net = |w: f64, z: f64| {
        NurbsCurve::new(
            k.clone(),
            vec![
                Point3::new(0.0, 0.0, z),
                Point3::new(1.0, 1.0, z),
                Point3::new(2.0, 0.0, z),
            ],
            Some(vec![1.0, w, 1.0]),
        )
        .unwrap()
    };
    let curves = [net(50.0, 0.0), net(0.01, 1.0), net(50.0, 2.0)];
    let (knots, nets) = compatible(&curves).unwrap();
    let r = skin(&knots, &nets, &[0.0, 0.1, 1.0], false, (None, None)).unwrap();
    assert!(matches!(r, Err(SkinFault::Weights { column: 1 })));
}
