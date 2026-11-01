use super::*;
use core::f64::consts::FRAC_PI_2;

#[test]
fn allowances_give_developed_lengths() {
    let (r, t, theta) = (0.002, 0.001, FRAC_PI_2);
    assert_eq!(
        Allowance::KFactor(0.5).length(r, t, theta),
        Some(theta * (r + 0.5 * t))
    );
    assert_eq!(Allowance::Length(0.003).length(r, t, theta), Some(0.003));
    for bad in [
        Allowance::KFactor(-0.1),
        Allowance::KFactor(1.01),
        Allowance::KFactor(f64::NAN),
        Allowance::Length(0.0),
        Allowance::Length(-1.0),
        Allowance::Length(f64::INFINITY),
    ] {
        assert_eq!(bad.length(r, t, theta), None, "{bad:?}");
    }
    assert_eq!(SheetOptions::default().tolerance, DEFAULT_TOLERANCE);
}
