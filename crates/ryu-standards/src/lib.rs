//! Engineering standards data, product-neutral: thread standards
//! (`thread`), drill series (`drill`) and ISO 286 limits and fits
//! (`fit`), in integer nanometres.
//!
//! What may be built in is decided by the owner (design 533, question
//! 657), and only these are:
//! - values computed here from published formulas (ISO 68-1 and ISO
//!   724's basic profile, ISO 965-1's tolerances, FED-STD-H28/2B's class
//!   formulas);
//! - values from public-domain documents, chiefly FED-STD-H28 (a US
//!   government work);
//! - plain facts: nominal sizes and their pitches or threads per inch,
//!   pipe threads' designations, TPI and taper, drill series;
//! - ISO 286, as `ryucad::drawing::fit` held it before (see `fit`).
//!
//! Nothing is copied from a paid ISO, ASME or DIN table. Fastener
//! clearances, counterbores and countersinks, and pipe threads'
//! diameters, gauge lengths and tap drills, are not here: a project's
//! own table supplies them. Every value says where it came from
//! ([`Source`]).
//!
//! There is no float anywhere in this crate. Lengths are [`Nm`]: ISO's
//! micrometres and ASME's ten-thousandths of an inch (2,540 nm) are both
//! whole. Formulas with fractional powers are decided by exact integer
//! comparisons (`exact`).

// The thread formulas (next) use it; until then it is only tested.
#[allow(dead_code)]
pub(crate) mod exact;
pub mod fit;
pub mod iso_metric;
pub mod source;
pub mod thread;

pub use source::{Source, Valued};

/// A length in nanometres.
pub type Nm = i64;

pub const NM_PER_UM: Nm = 1_000;
pub const NM_PER_MM: Nm = 1_000_000;
pub const NM_PER_IN: Nm = 25_400_000;

/// `nm` as an exact decimal of millimetres, trailing zeros dropped:
/// `5_153_000` is `"5.153"`, `5_000_000` is `"5"`.
pub fn mm_decimal(nm: Nm) -> String {
    decimal(nm, NM_PER_MM, 6)
}

/// `nm` as an exact decimal of inches when it is a whole number of
/// hundred-thousandths of an inch (254 nm), else `None`:
/// `5_105_400` is `Some("0.201")`.
pub fn in_decimal(nm: Nm) -> Option<String> {
    (nm % 254 == 0).then(|| decimal(nm / 254, 100_000, 5))
}

/// `n / unit` written exactly, where `unit` is `10^places`.
fn decimal(n: i64, unit: i64, places: usize) -> String {
    let sign = if n < 0 { "-" } else { "" };
    let n = n.unsigned_abs();
    let unit = unit.unsigned_abs();
    let (whole, frac) = (n / unit, n % unit);
    if frac == 0 {
        return format!("{sign}{whole}");
    }
    let digits = format!("{frac:0places$}");
    format!("{sign}{whole}.{}", digits.trim_end_matches('0'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decimals_are_exact() {
        assert_eq!(mm_decimal(5_153_000), "5.153");
        assert_eq!(mm_decimal(5_000_000), "5");
        assert_eq!(mm_decimal(250_000), "0.25");
        assert_eq!(mm_decimal(-1_500_000), "-1.5");
        assert_eq!(in_decimal(5_105_400).as_deref(), Some("0.201"));
        assert_eq!(in_decimal(NM_PER_IN / 4).as_deref(), Some("0.25"));
        assert_eq!(in_decimal(1), None);
    }
}
