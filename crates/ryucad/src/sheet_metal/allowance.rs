//! A bend's allowance as the kernel takes it (design 346's Bend
//! allowance): a K-factor as it is, a bend allowance as the developed
//! length, and a bend deduction `bd` as `2 (r + t) tan(θ / 2) - bd`,
//! defined under a half turn.

use crate::doc::sheet_metal::Allowance;
use core::f64::consts::PI;
pub use ryukei::sheet_metal::Allowance as Kernel;

/// Why a deduction gives a bend no allowance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    /// The bend turns half a turn: there is no outer mould line.
    HalfTurn,
    /// The deduction is the bend's whole length to its mould lines or
    /// more.
    TooLong,
}

impl Refused {
    /// What it says to an agent.
    pub fn message(self) -> &'static str {
        match self {
            Refused::HalfTurn => {
                "a bend deduction is defined under a half turn: give this bend its allowance when unfolding"
            }
            Refused::TooLong => {
                "the deduction is longer than the bend's own length to its mould lines"
            }
        }
    }
}

/// `a` for a bend of inner radius `r` and angle `theta` in sheet `t`
/// thick, as the kernel takes it.
pub fn kernel(a: &Allowance, r: f64, t: f64, theta: f64) -> Result<Kernel, Refused> {
    match a {
        Allowance::KFactor(k) => Ok(Kernel::KFactor(k.number())),
        Allowance::Length(q) => Ok(Kernel::Length(q.meters())),
        Allowance::Deduction(bd) => {
            if theta >= PI - 1e-9 {
                return Err(Refused::HalfTurn);
            }
            let ba = 2.0 * (r + t) * ryukei::math::tan(theta / 2.0) - bd.meters();
            if ba > ryukei::tol::LINEAR_RESOLUTION {
                Ok(Kernel::Length(ba))
            } else {
                Err(Refused::TooLong)
            }
        }
    }
}

/// The developed length `a` gives that bend.
pub fn developed(a: &Allowance, r: f64, t: f64, theta: f64) -> Result<f64, Refused> {
    Ok(match kernel(a, r, t, theta)? {
        Kernel::KFactor(k) => theta * (r + k * t),
        Kernel::Length(ba) => ba,
    })
}

/// A bend's allowance as a note writes it: `K0.44`, or `BA2.157`, the
/// developed length in millimetres to three decimals, trailing zeros
/// dropped.
pub fn note(k: &Kernel) -> String {
    match k {
        Kernel::KFactor(k) => format!("K{k}"),
        Kernel::Length(ba) => {
            let s = format!("{:.3}", ba * 1e3);
            format!("BA{}", s.trim_end_matches('0').trim_end_matches('.'))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::units::{Field, Quantity, Unit};
    use core::f64::consts::{FRAC_PI_2, PI};

    fn q(text: &str, f: Field) -> Quantity {
        Quantity::from_json_as(&serde_json::json!(text), f, &|n| {
            crate::expr::Env::empty().get(n)
        })
        .unwrap()
    }

    #[test]
    fn each_form_gives_the_kernel_its_allowance() {
        let (r, t) = (0.001, 0.0015);
        let k = Allowance::KFactor(q("0.44", Field::Number));
        assert_eq!(kernel(&k, r, t, FRAC_PI_2), Ok(Kernel::KFactor(0.44)));
        assert_eq!(
            developed(&k, r, t, FRAC_PI_2),
            Ok(FRAC_PI_2 * (r + 0.44 * t))
        );
        // A deduction is the developed length less than the flanges
        // measured to their outer mould line.
        let d = Allowance::Deduction(q("2.4 mm", Field::Length(Unit::Mm)));
        let ba = 2.0 * (r + t) * ryukei::math::tan(FRAC_PI_2 / 2.0) - 0.0024;
        assert_eq!(kernel(&d, r, t, FRAC_PI_2), Ok(Kernel::Length(ba)));
        // A deduction equal to what a K-factor gives is that K-factor.
        let ba_k = FRAC_PI_2 * (r + 0.44 * t);
        let bd = 2.0 * (r + t) * ryukei::math::tan(FRAC_PI_2 / 2.0) - ba_k;
        let d = Allowance::Deduction(q(&format!("{} mm", bd * 1000.0), Field::Length(Unit::Mm)));
        match kernel(&d, r, t, FRAC_PI_2) {
            Ok(Kernel::Length(x)) => assert!((x - ba_k).abs() < 1e-12, "{x} {ba_k}"),
            x => panic!("{x:?}"),
        }
        // At a half turn a deduction has no meaning; a long one leaves no
        // length.
        assert_eq!(developed(&d, r, t, PI), Err(Refused::HalfTurn));
        let long = Allowance::Deduction(q("50 mm", Field::Length(Unit::Mm)));
        assert_eq!(developed(&long, r, t, FRAC_PI_2), Err(Refused::TooLong));
        let a = Allowance::Length(q("3.1 mm", Field::Length(Unit::Mm)));
        assert_eq!(kernel(&a, r, t, PI), Ok(Kernel::Length(3.1 * 0.001)));
    }

    #[test]
    fn notes_read_as_shops_write_them() {
        assert_eq!(note(&Kernel::KFactor(0.44)), "K0.44");
        assert_eq!(note(&Kernel::Length(2.1e-3)), "BA2.1");
        assert_eq!(note(&Kernel::Length(2.0e-3)), "BA2");
        assert_eq!(note(&Kernel::Length(2.15749e-3)), "BA2.157");
    }
}
