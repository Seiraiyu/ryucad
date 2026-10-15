//! Bit-exact determinism check. Every CI platform must reproduce the hashes
//! in `tests/golden/determinism.txt`. After a deliberate change, regenerate
//! with `UPDATE_GOLDEN=1 cargo test -p ryukei --test determinism` and review
//! the diff like any other change.

use ryukei::geom::{Affine, Box3, Point2, Point3, Rigid, Vector3};
use ryukei::math;
use ryukei::predicates::{self, Sign};
use ryukei_testkit::{Golden, Lcg};

fn math_section(golden: &mut Golden) {
    let mut s = golden.section("math");
    let mut g = Lcg::new(0x6d61_7468);
    for _ in 0..4096 {
        let x = g.next_f64(-1.0e3, 1.0e3);
        let y = g.next_f64(-1.0e3, 1.0e3);
        let p = g.next_f64(1.0e-6, 1.0e3);
        let u = g.next_f64(-1.0, 1.0);
        for v in [
            math::sqrt(p),
            math::fma(x, y, p),
            math::sin(x),
            math::cos(x),
            math::sin_cos(x).0,
            math::sin_cos(x).1,
            math::tan(x),
            math::asin(u),
            math::acos(u),
            math::atan2(y, x),
            math::exp(u * 700.0),
            math::ln(p),
            math::pow(p, u * 10.0),
            math::hypot(x, y),
            math::cbrt(x),
        ] {
            s.f64(v);
        }
    }
}

fn geom_section(golden: &mut Golden) {
    let mut s = golden.section("geom");
    let mut g = Lcg::new(0x6765_6f6d);
    let mut chain = Rigid::IDENTITY;
    for _ in 0..1024 {
        let mut p = || {
            Point3::new(
                g.next_f64(-500.0, 500.0),
                g.next_f64(-500.0, 500.0),
                g.next_f64(-500.0, 500.0),
            )
        };
        let (a, b, c) = (p(), p(), p());
        let angle = g.next_f64(-10.0, 10.0);
        let step = match (b - a).normalize() {
            Some(axis) => {
                Rigid::translation(c.to_vector() * 1e-3).then_after(&Rigid::rotation(axis, angle))
            }
            None => Rigid::IDENTITY,
        };
        chain = step.then_after(&chain);
        let q = chain.apply_point(a);
        let v = (b - a).cross(c - a);
        let n = v
            .normalize()
            .map_or(Vector3::ZERO, |u| chain.apply_unit(u).get());
        let aff = Affine::scale(Vector3::new(angle, 2.0, -0.5)).then_after(&Affine::from(chain));
        let r = aff.inverse().map_or(Point3::ORIGIN, |i| i.apply_point(q));
        let bx = Box3::new(a, b).transform_rigid(&chain);
        for x in [
            q.x,
            q.y,
            q.z,
            v.x,
            v.y,
            v.z,
            n.x,
            n.y,
            n.z,
            r.x,
            r.y,
            r.z,
            bx.min().x,
            bx.max().z,
            a.distance(c),
            aff.determinant(),
        ] {
            s.f64(x);
        }
    }
    chain = chain.reorthonormalize();
    for row in chain.rotation_matrix() {
        for x in row {
            s.f64(x);
        }
    }
}

fn sign_bits(s: Sign) -> u64 {
    match s {
        Sign::Negative => 0,
        Sign::Zero => 1,
        Sign::Positive => 2,
    }
}

fn predicates_section(golden: &mut Golden) {
    let mut s = golden.section("predicates");
    let mut g = Lcg::new(0x7072_6564);
    for _ in 0..1024 {
        let mut c = || g.next_f64(-500.0, 500.0);
        let (a, b, d) = (
            Point3::new(c(), c(), c()),
            Point3::new(c(), c(), c()),
            Point3::new(c(), c(), c()),
        );
        let (t, u) = (c() / 250.0, c() / 250.0);
        // Near-degenerate: on the plane/line through a, b, d up to rounding.
        let on_plane = a + (b - a) * t + (d - a) * u;
        let on_line = Point2::new(a.x, a.y).lerp(Point2::new(b.x, b.y), t);
        let e = Point3::new(c(), c(), c());
        for sign in [
            predicates::orient2d(Point2::new(a.x, a.y), Point2::new(b.x, b.y), on_line),
            predicates::orient3d(a, b, d, on_plane),
            predicates::orient3d(a, b, d, e),
            predicates::incircle(
                Point2::new(a.x, a.y),
                Point2::new(b.x, b.y),
                Point2::new(d.x, d.y),
                on_line,
            ),
            predicates::insphere(a, b, d, e, on_plane),
        ] {
            s.u64(sign_bits(sign));
        }
    }
}

#[test]
fn determinism() {
    let mut golden = Golden::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/golden/determinism.txt"
    ));
    math_section(&mut golden);
    geom_section(&mut golden);
    predicates_section(&mut golden);
    golden.finish();
}
