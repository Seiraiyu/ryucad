use super::*;
use crate::geom::Curve;
use std::sync::Arc;

fn helix(radius: f64, pitch: f64, taper: f64, turns: f64, hand: Hand) -> Helix {
    let frame = Frame::new(
        Point3::new(0.01, -0.02, 0.03),
        Vector3::new(0.2, 0.3, 1.0),
        Vector3::new(1.0, 0.0, 0.0),
    )
    .unwrap();
    Helix {
        frame,
        radius,
        pitch,
        taper,
        turns,
        hand,
    }
}

/// The largest distance between the NURBS and the helix at the same
/// angle, at 64 samples a span.
fn deviation(h: &Helix, n: &NurbsCurve) -> f64 {
    let kv = n.knots();
    let cuts = kv.knots();
    let mut worst: f64 = 0.0;
    for w in cuts.windows(2) {
        for i in 0..=64 {
            let t = w[0] + (w[1] - w[0]) * f64::from(i) / 64.0;
            worst = worst.max(n.point(t).distance(h.point(t)));
        }
    }
    worst
}

#[test]
fn its_bound_holds_against_dense_sampling() {
    for radius in [1e-4, 0.002, 0.01, 0.3] {
        for pitch in [1e-4, 0.002, 0.05] {
            for taper in [0.0, 0.2, -0.05] {
                for hand in [Hand::Right, Hand::Left] {
                    let h = helix(radius, pitch, taper, 2.5, hand);
                    if h.radius_at(h.end()) <= 0.0 {
                        continue;
                    }
                    for tol in [1e-7, 1e-5] {
                        let (n, bound) = h.to_nurbs(tol).unwrap();
                        assert!(bound <= tol, "{h:?} {tol}: {bound}");
                        let d = deviation(&h, &n);
                        assert!(d <= bound, "{h:?} {tol}: {d} past {bound}");
                    }
                }
            }
        }
    }
}

#[test]
fn its_ends_are_the_helixs_own() {
    let h = helix(0.01, 0.002, 0.1, 3.25, Hand::Left);
    let (n, _) = h.to_nurbs(1e-7).unwrap();
    assert_eq!(n.domain(), [0.0, h.end()]);
    assert_eq!(n.control_points().next(), Some(h.point(0.0)));
    assert_eq!(n.control_points().last(), Some(h.point(h.end())));
    assert!(n.point(0.0).distance(h.point(0.0)) <= 1e-15);
    assert!(n.point(h.end()).distance(h.point(h.end())) <= 1e-15);
    // So are its ends' tangents, to rounding.
    let c = Curve::Nurbs(Arc::new(n));
    for t in [0.0, h.end()] {
        let a = c.eval(t).d1.normalize().unwrap().get();
        let b = h.derivative(t).normalize().unwrap().get();
        assert!(a.cross(b).length() <= 1e-12, "{t}");
    }
}

#[test]
fn a_turn_of_a_10_mm_helix_takes_32_spans() {
    let h = helix(0.01, 0.002, 0.0, 1.0, Hand::Right);
    let (n, bound) = h.to_nurbs(1e-7).unwrap();
    assert_eq!(n.knots().knots().len() - 1, 32);
    assert!(bound < 1e-7);
}

#[test]
fn hands_mirror_each_other_and_tapers_widen_along_the_axis() {
    let frame = Frame::new(
        Point3::ORIGIN,
        Vector3::new(0.0, 0.0, 1.0),
        Vector3::new(1.0, 0.0, 0.0),
    )
    .unwrap();
    let right = Helix {
        frame,
        radius: 0.01,
        pitch: 0.004,
        taper: 0.0,
        turns: 2.0,
        hand: Hand::Right,
    };
    let left = Helix {
        hand: Hand::Left,
        ..right
    };
    for t in [0.3, 1.7, 5.0] {
        let (p, q) = (right.point(t), left.point(t));
        assert_eq!((p.x, p.y, p.z), (q.x, -q.y, q.z));
    }
    // A quarter turn on, a right hand is at +y, a left at -y.
    assert!(right.point(TAU / 4.0).y > 0.0099);
    assert!(left.point(TAU / 4.0).y < -0.0099);
    let wide = Helix {
        taper: 0.1,
        ..right
    };
    assert!(wide.radius_at(wide.end()) > wide.radius);
    assert!((wide.radius_at(wide.end()) - (0.01 + 0.008 * math::tan(0.1))).abs() < 1e-15);
    assert_eq!(wide.height(), 0.008);
}

#[test]
fn what_isnt_a_helix_is_refused() {
    let ok = helix(0.01, 0.002, 0.0, 1.0, Hand::Right);
    for (bad, err) in [
        (Helix { radius: 0.0, ..ok }, Error::Degenerate),
        (Helix { pitch: 0.0, ..ok }, Error::Degenerate),
        (Helix { turns: -1.0, ..ok }, Error::Degenerate),
        (
            Helix {
                taper: core::f64::consts::FRAC_PI_2,
                ..ok
            },
            Error::Degenerate,
        ),
        // Narrowing to nothing within its height.
        (
            Helix {
                taper: -1.4,
                turns: 10.0,
                ..ok
            },
            Error::Degenerate,
        ),
        (
            Helix {
                radius: f64::NAN,
                ..ok
            },
            Error::NonFinite,
        ),
    ] {
        assert_eq!(bad.to_nurbs(1e-7).unwrap_err(), err, "{bad:?}");
    }
    assert_eq!(ok.to_nurbs(0.0).unwrap_err(), Error::Degenerate);
    assert_eq!(ok.to_nurbs(f64::NAN).unwrap_err(), Error::Degenerate);
}
