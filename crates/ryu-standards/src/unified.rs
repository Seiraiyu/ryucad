//! Unified inch screw threads: UNC, UNF, UNEF and the constant-pitch UN
//! series, internal classes 1B, 2B and 3B.
//!
//! Sources:
//! - Sizes, their major diameters (number sizes 0.060 + 0.013 n in) and
//!   each series' threads per inch are facts, as FED-STD-H28/2B (public
//!   domain) tabulates them.
//! - Internal limits are FED-STD-H28/2B's formulas: the minimum minor
//!   diameter is the basic minor diameter D − 5√3/8 · p, the minimum
//!   major diameter is D, and the maximum minor diameter adds the class's
//!   minor-diameter tolerance (H28/2B's section on it). They are checked
//!   against H28/2B's printed limits (`tests/h28_2b.rs`), and `PRINTED`
//!   holds its value where it prints otherwise.
//!
//! External classes (1A, 2A, 3A) arrive with ryucad 28.B.

use crate::exact::{Big, isqrt, prod, round_div};
use crate::{NM_PER_IN, Nm, Source, Valued};

/// A unified size and the TPI of each series it is in.
#[derive(Debug, PartialEq, Eq)]
pub struct Size {
    pub name: &'static str,
    pub major: Nm,
    pub unc: Option<u16>,
    pub unf: Option<u16>,
    pub unef: Option<u16>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Series {
    Unc,
    Unf,
    Unef,
}

/// Thousandths of an inch to nanometres.
const fn mil(n: i64) -> Nm {
    n * 25_400
}

/// Number size n: 0.060 + 0.013 n in.
const fn number(n: i64) -> Nm {
    mil(60 + 13 * n)
}

/// A fraction of an inch.
const fn frac(num: i64, den: i64) -> Nm {
    NM_PER_IN * num / den
}

macro_rules! s {
    ($name:expr, $major:expr, $c:expr, $f:expr, $e:expr) => {
        Size { name: $name, major: $major, unc: $c, unf: $f, unef: $e }
    };
}

/// Every size, by major diameter. Reconcile with H28/2B's tables before
/// committing: a size or TPI it lists that this lacks is added.
pub const SIZES: &[Size] = &[
    s!("#0", number(0), None, Some(80), None),
    s!("#1", number(1), Some(64), Some(72), None),
    s!("#2", number(2), Some(56), Some(64), None),
    s!("#3", number(3), Some(48), Some(56), None),
    s!("#4", number(4), Some(40), Some(48), None),
    s!("#5", number(5), Some(40), Some(44), None),
    s!("#6", number(6), Some(32), Some(40), None),
    s!("#8", number(8), Some(32), Some(36), None),
    s!("#10", number(10), Some(24), Some(32), None),
    s!("#12", number(12), Some(24), Some(28), Some(32)),
    s!("1/4", frac(1, 4), Some(20), Some(28), Some(32)),
    s!("5/16", frac(5, 16), Some(18), Some(24), Some(32)),
    s!("3/8", frac(3, 8), Some(16), Some(24), Some(32)),
    s!("7/16", frac(7, 16), Some(14), Some(20), Some(28)),
    s!("1/2", frac(1, 2), Some(13), Some(20), Some(28)),
    s!("9/16", frac(9, 16), Some(12), Some(18), Some(24)),
    s!("5/8", frac(5, 8), Some(11), Some(18), Some(24)),
    s!("11/16", frac(11, 16), None, None, Some(24)),
    s!("3/4", frac(3, 4), Some(10), Some(16), Some(20)),
    s!("13/16", frac(13, 16), None, None, Some(20)),
    s!("7/8", frac(7, 8), Some(9), Some(14), Some(20)),
    s!("15/16", frac(15, 16), None, None, Some(20)),
    s!("1", NM_PER_IN, Some(8), Some(12), Some(20)),
    s!("1-1/16", frac(17, 16), None, None, Some(18)),
    s!("1-1/8", frac(9, 8), Some(7), Some(12), Some(18)),
    s!("1-3/16", frac(19, 16), None, None, Some(18)),
    s!("1-1/4", frac(5, 4), Some(7), Some(12), Some(18)),
    s!("1-5/16", frac(21, 16), None, None, Some(18)),
    s!("1-3/8", frac(11, 8), Some(6), Some(12), Some(18)),
    s!("1-7/16", frac(23, 16), None, None, Some(18)),
    s!("1-1/2", frac(3, 2), Some(6), Some(12), Some(18)),
];

/// The constant-pitch series' threads per inch.
pub const UN_TPI: [u16; 8] = [4, 6, 8, 12, 16, 20, 28, 32];

pub fn size(name: &str) -> Option<&'static Size> {
    SIZES.iter().find(|s| s.name == name)
}

pub fn tpi(series: Series, s: &Size) -> Option<u16> {
    match series {
        Series::Unc => s.unc,
        Series::Unf => s.unf,
        Series::Unef => s.unef,
    }
}

/// An internal class.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    B1,
    B2,
    B3,
}

impl Class {
    pub const DEFAULT: Class = Class::B2;
    pub const ALL: [Class; 3] = [Class::B1, Class::B2, Class::B3];

    pub fn parse(t: &str) -> Option<Class> {
        match t {
            "1B" => Some(Class::B1),
            "2B" => Some(Class::B2),
            "3B" => Some(Class::B3),
            _ => None,
        }
    }

    pub fn text(self) -> &'static str {
        match self {
            Class::B1 => "1B",
            Class::B2 => "2B",
            Class::B3 => "3B",
        }
    }
}

pub use crate::iso_metric_limits::{Limits, Which};

/// H28/2B's printed values where the formulas round otherwise. Empty
/// until Task 8's oracle names one.
pub const PRINTED: &[(&str, u16, &str, Which, Nm, &str)] = &[];

/// The basic minor diameter D − 5√3/8 · p, with p = 1/tpi in, to the
/// nanometre (the floor of an irrational number is never a tie).
pub fn basic_minor(major: Nm, tpi: u16) -> Nm {
    // 8 · 5√3/8 · p in thousandths of a nanometre: √(75 · (25.4·10^9)² / tpi²).
    let x8 = isqrt(75 * (25_400_000_000u128).pow(2) / u128::from(tpi).pow(2)) as i128;
    round_div(i128::from(major) * 8_000 - x8, 8_000) as Nm
}

/// Ten-thousandths of an inch.
const TENTH_MIL: Nm = 2_540;

/// The class's minor-diameter tolerance in ten-thousandths of an inch.
/// Reconcile each coefficient with H28/2B's text before committing.
fn minor_tolerance(major: Nm, tpi: u16, class: Class) -> i128 {
    let n = i128::from(tpi);
    let quarter = major >= NM_PER_IN / 4;
    match class {
        Class::B1 | Class::B2 if quarter => {
            if tpi <= 4 {
                // 0.15 p, in 0.0001 in: 1500 / n.
                round_div(1_500, n)
            } else {
                // 0.25 p - 0.40 p², in 0.0001 in: 2500/n - 4000/n².
                round_div(2_500 * n - 4_000, n * n)
            }
        }
        _ => small_tolerance(major, tpi),
    }
}

/// 0.05 p^(2/3) + 0.03 p/D - 0.002 in, in 0.0001 in (nearest; the
/// p^(2/3) term decided by cubing).
fn small_tolerance(major: Nm, tpi: u16) -> i128 {
    let n = u128::from(tpi);
    let d = major as u128; // nm; D in inches is d / 25.4e6.
    // Is the tolerance at least k/2 (half ten-thousandths)? Then the
    // nearest whole k is found by stepping.
    let at_least_half = |k2: i128| {
        // 0.05 p^(2/3) >= R = k2/2e4 - 0.03·25.4e6/(n d) + 0.002.
        // R = (k2·n·d + 40·n·d - 2·762_000·10^4... ) / (2e4·n·d):
        let nd = (n * d) as i128;
        let r_num = k2 * nd + 40 * nd - 2 * 762_000 * 10_000;
        let r_den = 20_000 * nd;
        if r_num <= 0 {
            return true;
        }
        // (0.05)^3 p^2 = 1 / (8000 n^2) >= r_num^3 / r_den^3.
        let left: Big = prod(&[(r_den as u128, 3)]);
        left >= prod(&[(8_000, 1), (n, 2), (r_num as u128, 3)])
    };
    let mut k = 0i128;
    while at_least_half(2 * k + 1) {
        k += 1;
    }
    k
}

/// The limits of `size` at `tpi` in `class`.
pub fn internal_limits(size: &Size, tpi: u16, class: Class) -> Limits {
    let formula = Source::Formula("FED-STD-H28/2B");
    let min = basic_minor(size.major, tpi);
    let tol = minor_tolerance(size.major, tpi, class) as Nm * TENTH_MIL;
    // Printed to 0.001 in (25,400 nm), halves up.
    let thou = |nm: Nm| round_div(i128::from(nm), 25_400) as Nm * 25_400;
    let mut out = Limits {
        minor: (
            Valued::new(thou(min), formula),
            Valued::new(thou(min + tol), formula),
        ),
        major_min: Valued::new(size.major, formula),
    };
    for &(name, t, c, which, value, table) in PRINTED {
        if (name, t, c) == (size.name, tpi, class.text()) {
            let v = Valued::new(value, Source::Public(table));
            match which {
                Which::MinorMin => out.minor.0 = v,
                Which::MinorMax => out.minor.1 = v,
                Which::MajorMin => out.major_min = v,
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_and_series() {
        let q = size("1/4").unwrap();
        assert_eq!(q.major, NM_PER_IN / 4);
        assert_eq!((q.unc, q.unf, q.unef), (Some(20), Some(28), Some(32)));
        assert_eq!(size("#10").unwrap().major, 4_826_000); // 0.190 in
        assert_eq!(size("#0").unwrap().unc, None);
        assert_eq!(size("#0").unwrap().unf, Some(80));
        assert_eq!(size("1-1/2").unwrap().unc, Some(6));
        assert_eq!(tpi(Series::Unc, size("1/2").unwrap()), Some(13));
        assert!(SIZES.windows(2).all(|w| w[0].major < w[1].major));
    }

    #[test]
    fn quarter_twenty_2b_by_hand() {
        // H28/2B: 1/4-20 UNC-2B minor 0.196 to 0.207, major at least 0.2500.
        let l = internal_limits(size("1/4").unwrap(), 20, Class::B2);
        assert_eq!(
            (l.minor.0.value, l.minor.1.value, l.major_min.value),
            (4_978_400, 5_257_800, 6_350_000)
        );
        assert_eq!(l.minor.1.source, crate::Source::Formula("FED-STD-H28/2B"));
    }

    #[test]
    fn classes() {
        for t in ["1B", "2B", "3B"] {
            assert_eq!(Class::parse(t).unwrap().text(), t);
        }
        assert_eq!(
            Class::parse("2A"),
            None,
            "external classes arrive with 28.B"
        );
        assert_eq!(Class::DEFAULT, Class::B2);
    }
}
