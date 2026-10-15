//! Interval results must contain the exact result for points inside the
//! operand intervals.

#![no_main]

use libfuzzer_sys::fuzz_target;
use ryukei::interval::Interval;
use ryukei_testkit::exact::{rational, within};

fuzz_target!(|v: [f64; 6]| {
    if !v.iter().all(|x| x.is_finite() && x.abs() <= 1.0e100) {
        return;
    }
    let make = |a: f64, b: f64| Interval::new(a.min(b), a.max(b)).unwrap();
    let (a, b) = (make(v[0], v[1]), make(v[2], v[3]));
    let (x, y) = (v[4].clamp(a.lo(), a.hi()), v[5].clamp(b.lo(), b.hi()));
    let (qx, qy) = (rational(x), rational(y));
    let check = |i: Interval, r| assert!(within(i.lo(), i.hi(), &r), "{i:?}");
    check(a + b, &qx + &qy);
    check(a - b, &qx - &qy);
    check(a * b, &qx * &qy);
    if let Some(q) = a.checked_div(b) {
        check(q, &qx / &qy);
    }
});
