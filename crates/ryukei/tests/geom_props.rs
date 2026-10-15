//! Property tests for the vector and transform types.

use proptest::prelude::*;
use ryukei::geom::{Affine, Point3, Rigid, Vector3};

fn coord() -> impl Strategy<Value = f64> {
    -500.0f64..500.0
}

fn point() -> impl Strategy<Value = Point3> {
    (coord(), coord(), coord()).prop_map(|(x, y, z)| Point3::new(x, y, z))
}

fn rigid() -> impl Strategy<Value = Rigid> {
    (point(), point(), -10.0f64..10.0).prop_filter_map("degenerate axis", |(a, t, angle)| {
        let axis = a.to_vector().normalize()?;
        Some(Rigid::translation(t.to_vector()).then_after(&Rigid::rotation(axis, angle)))
    })
}

/// Error bound for a rigid round trip at the size-box scale. Intermediate
/// coordinates stay below 2048 m (a point up to 866 m from the origin plus
/// a translation up to 866 m), where an ulp is 4.5e-13. Each way, a
/// component takes up to six roundings of half an ulp: 1.4e-12, so 2.7e-12
/// for the round trip and 4.7e-12 across three components. 1e-12 was
/// exceeded (1.006e-12) by the case saved in `geom_props.proptest-regressions`.
const ROUND_TRIP: f64 = 5e-12;

proptest! {
    #[test]
    fn rigid_inverse_round_trips(r in rigid(), p in point()) {
        let q = r.inverse().apply_point(r.apply_point(p));
        prop_assert!(p.distance(q) <= ROUND_TRIP, "{p:?} -> {q:?}");
    }

    #[test]
    fn rigid_preserves_distances(r in rigid(), p in point(), q in point()) {
        let before = p.distance(q);
        let after = r.apply_point(p).distance(r.apply_point(q));
        prop_assert!((before - after).abs() <= ROUND_TRIP);
    }

    #[test]
    fn rigid_is_orthonormal(r in rigid()) {
        prop_assert!(r.orthonormality_error() <= 8.0 * f64::EPSILON);
    }

    #[test]
    fn affine_matches_rigid(r in rigid(), p in point()) {
        prop_assert_eq!(Affine::from(r).apply_point(p), r.apply_point(p));
    }

    #[test]
    fn affine_scale_round_trips(
        sx in 0.1f64..10.0, sy in -10.0f64..-0.1, sz in 0.1f64..10.0, p in point(),
    ) {
        let s = Affine::scale(Vector3::new(sx, sy, sz));
        let q = s.inverse().unwrap().apply_point(s.apply_point(p));
        prop_assert!(p.distance(q) <= ROUND_TRIP);
    }

    #[test]
    fn cross_is_orthogonal(a in point(), b in point()) {
        let (u, v) = (a.to_vector(), b.to_vector());
        let c = u.cross(v);
        // |c . u| is bounded by rounding relative to |u|^2 |v|.
        let bound = 16.0 * f64::EPSILON * u.length_squared() * v.length();
        prop_assert!(c.dot(u).abs() <= bound);
    }
}
