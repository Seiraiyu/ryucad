//! Unified inch screw threads: UNC, UNF, UNEF and the constant-pitch UN
//! series, internal classes 1B, 2B and 3B.
//!
//! Sources:
//! - Sizes, their major diameters (number sizes 0.060 + 0.013 n in) and
//!   each series' threads per inch are facts, as FED-STD-H28/2B (public
//!   domain) tabulates them.
//! - Internal limits are formulas: the minimum minor diameter is the
//!   basic minor diameter D − 5√3/8 · p, the minimum major diameter is D,
//!   and the maximum minor diameter adds the class's published
//!   minor-diameter tolerance (`minor_tolerance`). H28/2B adopts them from
//!   ASME B1.1 without restating them; its own printed limits (Appendix A,
//!   Tables II.A.1 and II.A.2) fix the bounds and the rounding, and every
//!   one of their 340 rows to 1-3/4 in is checked (`tests/h28_2b.rs`).
//!   `PRINTED` holds the one value it prints otherwise.
//! - Sizes above 1-3/4 in are not built in (from 2 in, UNC has 4.5
//!   threads per inch); a project's table adds them.
//!
//! - External classes 1A, 2A and 3A (ryucad 28.B) are the published
//!   formulas: the major-diameter tolerance 0.060 ∛p² in (1A: 0.090), the
//!   allowance 0.300 × the 2A pitch-diameter tolerance
//!   (0.0015 ∛D + 0.0015 √LE + 0.015 ∛p²) for 1A and 2A and none for 3A,
//!   each to 0.0001 in. H28/2B's printed external limits (Tables II.A.1
//!   and II.A.2) fix the length of engagement LE and the rounding, and
//!   every row to 1-3/4 in is checked (`tests/h28_2b_external.rs`);
//!   `PRINTED_EXTERNAL` holds the values it prints otherwise.

use crate::exact::{Big, icbrt, isqrt, prod, round_div};
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

/// Every size to 1-3/4 in, by major diameter, as H28/2B's Tables II.A.1
/// and II.A.2 list them (`tests/h28_2b.rs`).
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
    s!("1-9/16", frac(25, 16), None, None, Some(18)),
    s!("1-5/8", frac(13, 8), None, None, Some(18)),
    s!("1-11/16", frac(27, 16), None, None, Some(18)),
    s!("1-3/4", frac(7, 4), Some(5), None, None),
    // Larger sizes are not built in: from 2 in, UNC's 4.5 threads per inch
    // is not a whole number. A project's table adds them.
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

/// H28/2B's printed values where the formulas round otherwise: size,
/// threads per inch, class, which limit, the printed value, the table.
/// 1-1/2-18 UNEF's maximum works out to 1.4399 + 0.0127 = 1.4526 in,
/// printed 1.453 by the rule every other row follows; H28/2B prints
/// 1.452 (and a percent of thread to match).
pub const PRINTED: &[(&str, u16, &str, Which, Nm, &str)] = &[
    (
        "1-1/2",
        18,
        "1B",
        Which::MinorMax,
        36_880_800,
        "FED-STD-H28/2B Table II.A.1",
    ),
    (
        "1-1/2",
        18,
        "2B",
        Which::MinorMax,
        36_880_800,
        "FED-STD-H28/2B Table II.A.1",
    ),
];

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
/// H28/2B states no formula of its own (it refers to ASME B1.1); the
/// formulas here are the published ones, and their bounds are the ones
/// H28/2B's printed limits (Tables II.A.1 and II.A.2) all agree with.
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
        // Never more than 0.394 p (the printed limits of #0 to #2 hold the
        // formula to it; 0.394 is the only third decimal they all agree
        // with), nor less than 0.23 p - 1.5 p² or 0.120 p, whichever is
        // greater (3B from 3/8 in: the first at 14 threads per inch and
        // finer, the second at 13 and coarser).
        _ => small_tolerance(major, tpi)
            .min(round_div(3_940, n))
            .max(round_div(2_300 * n - 15_000, n * n))
            .max(round_div(1_200, n)),
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
    // H28/2B's printed limits round so (each rule fixed by its rows):
    // - the minimum is the basic minor diameter to 0.0001 in, printed to
    //   0.001 in from #6 (0.138 in) up, for every class;
    // - 1B and 2B's maximum is that 0.0001 in minimum plus the tolerance
    //   (1/4-20: 0.1959 + 0.0115 = 0.2074, printed 0.207), printed as the
    //   minimum is;
    // - 3B's maximum is the unrounded basic minor diameter plus the
    //   tolerance, to 0.0001 in (#8-32: 0.130171 + 0.0087 = 0.1389), and
    //   always printed so.
    let tenth = |nm: Nm| round_div(i128::from(nm), i128::from(TENTH_MIL)) as Nm * TENTH_MIL;
    let thou = |nm: Nm| {
        if size.major < number(6) {
            nm
        } else {
            round_div(i128::from(nm), 25_400) as Nm * 25_400
        }
    };
    let basic = basic_minor(size.major, tpi);
    let tol = minor_tolerance(size.major, tpi, class) as Nm * TENTH_MIL;
    let min = tenth(basic);
    let max = match class {
        Class::B3 => tenth(basic + tol),
        Class::B1 | Class::B2 => thou(min + tol),
    };
    let mut out = Limits {
        minor: (Valued::new(thou(min), formula), Valued::new(max, formula)),
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

/// An external class.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExtClass {
    A1,
    A2,
    A3,
}

impl ExtClass {
    pub const DEFAULT: ExtClass = ExtClass::A2;
    pub const ALL: [ExtClass; 3] = [ExtClass::A1, ExtClass::A2, ExtClass::A3];

    pub fn parse(t: &str) -> Option<ExtClass> {
        ExtClass::ALL.into_iter().find(|c| c.text() == t)
    }

    pub fn text(self) -> &'static str {
        match self {
            ExtClass::A1 => "1A",
            ExtClass::A2 => "2A",
            ExtClass::A3 => "3A",
        }
    }
}

/// Billionths of an inch, for the roots below.
const NANO_IN: u128 = 1_000_000_000;

/// The nearest whole ten-thousandth of an inch of `[lo, hi)` billionths
/// of an inch times `scale`, halves up, if both ends agree; else none (a
/// tie the bounds can't settle, which the oracle test would show).
fn tenths(lo: u128, hi: u128, scale: u128) -> Option<i128> {
    let r = |x: u128| ((x * 2 / scale / 100_000) + 1) / 2;
    (r(lo) == r(hi)).then(|| r(lo) as i128)
}

/// ∛p² for p = 1/tpi in, in billionths of an inch: `[floor, floor + 1)`.
fn cbrt_p2(tpi: u16) -> u128 {
    icbrt(NANO_IN.pow(3) / u128::from(tpi).pow(2))
}

/// The major-diameter tolerance in ten-thousandths of an inch.
fn major_tolerance(tpi: u16, class: ExtClass) -> i128 {
    let c = cbrt_p2(tpi);
    let k: u128 = if class == ExtClass::A1 { 90 } else { 60 };
    // k/1000 × ∛p², in billionths × 1000.
    tenths(k * c, k * (c + 1), 1_000).expect("a tie in a unified tolerance")
}

/// The length of engagement the allowance's formula takes, in pitches:
/// the one H28/2B's printed allowances bear out (see the module doc).
const LE_PITCHES: u128 = 9;

/// The allowance (1A, 2A) in ten-thousandths of an inch.
fn allowance(major: Nm, tpi: u16, class: ExtClass) -> i128 {
    if class == ExtClass::A3 {
        return 0;
    }
    let d = major as u128; // nm; inches are d / 25.4e6
    let a = icbrt(d * NANO_IN.pow(3) / 25_400_000); // ∛D
    let b = isqrt(LE_PITCHES * NANO_IN.pow(2) / u128::from(tpi)); // √LE
    let c = cbrt_p2(tpi); // ∛p²
    // 0.300 × (0.0015 a + 0.0015 b + 0.015 c), in billionths × 10^5.
    let lo = 3 * (15 * a + 15 * b + 150 * c);
    let hi = 3 * (15 * (a + 1) + 15 * (b + 1) + 150 * (c + 1));
    tenths(lo, hi, 100_000).expect("a tie in a unified allowance")
}

/// An external thread's limits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExtLimits {
    pub major: (Valued<Nm>, Valued<Nm>),
    pub basic_minor: Valued<Nm>,
}

/// H28/2B's printed external values where the formulas round otherwise:
/// size, threads per inch, class, which limit (`MajorMin` or the greatest,
/// `MinorMax` standing for it), the printed value, the table.
pub const PRINTED_EXTERNAL: &[(&str, u16, &str, Which, Nm, &str)] = &[];

pub fn external_limits(size: &Size, tpi: u16, class: ExtClass) -> ExtLimits {
    let formula = Source::Formula("FED-STD-H28/2B");
    let max = size.major - allowance(size.major, tpi, class) as Nm * TENTH_MIL;
    let min = max - major_tolerance(tpi, class) as Nm * TENTH_MIL;
    let mut out = ExtLimits {
        major: (Valued::new(min, formula), Valued::new(max, formula)),
        basic_minor: Valued::new(
            basic_minor(size.major, tpi),
            Source::Formula("FED-STD-H28/2B"),
        ),
    };
    for &(name, t, c, which, value, table) in PRINTED_EXTERNAL {
        if (name, t, c) == (size.name, tpi, class.text()) {
            let v = Valued::new(value, Source::Public(table));
            match which {
                Which::MajorMin => out.major.0 = v,
                _ => out.major.1 = v,
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

    #[test]
    fn quarter_twenty_2a_by_hand() {
        // H28/2B: 1/4-20 UNC-2A major 0.2408 to 0.2489 in; 3A 0.2419 to 0.2500.
        let l = external_limits(size("1/4").unwrap(), 20, ExtClass::A2);
        assert_eq!((l.major.0.value, l.major.1.value), (6_116_320, 6_322_060));
        let l = external_limits(size("1/4").unwrap(), 20, ExtClass::A3);
        assert_eq!((l.major.0.value, l.major.1.value), (6_144_260, 6_350_000));
        assert_eq!(l.basic_minor.value, basic_minor(NM_PER_IN / 4, 20));
    }
}
