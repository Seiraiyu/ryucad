//! Bit-exact determinism check. Every CI platform must reproduce the hashes
//! in `tests/golden/determinism.txt`. After a deliberate change, regenerate
//! with `UPDATE_GOLDEN=1 cargo test -p ryukei --test determinism` and review
//! the diff like any other change.

use ryukei::geom::{Affine, Box3, Point3, Rigid, Vector3};
use ryukei::math;
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

#[test]
fn determinism() {
    let mut golden = Golden::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/golden/determinism.txt"
    ));
    math_section(&mut golden);
    geom_section(&mut golden);
    golden.finish();
}
