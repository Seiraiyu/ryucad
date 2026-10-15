//! Bit-exact determinism check. Every CI platform must reproduce the hashes
//! in `tests/golden/determinism.txt`. After a deliberate change, regenerate
//! with `UPDATE_GOLDEN=1 cargo test -p ryukei --test determinism` and review
//! the diff like any other change.

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

#[test]
fn determinism() {
    let mut golden = Golden::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/golden/determinism.txt"
    ));
    math_section(&mut golden);
    golden.finish();
}
