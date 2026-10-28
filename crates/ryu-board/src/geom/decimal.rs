//! Decimal text to integers exactly, from the text, with no floating point.
//!
//! In millimetres up to six decimals and in microns up to three are exact;
//! in inches up to five decimals and in mils up to two; degrees up to three.
//! Anything finer is rounded to the nearest integer, ties to even, and the
//! answer says by how much, so the caller can count it.

use super::{COORD_LIMIT, Mdeg, Nm};

/// The unit a length is written in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Unit {
    Mm,
    Micron,
    Inch,
    Mil,
    Nm,
}

impl Unit {
    /// Nanometres in one of this unit.
    pub const fn nm(self) -> i128 {
        match self {
            Unit::Mm => 1_000_000,
            Unit::Micron => 1_000,
            Unit::Inch => 25_400_000,
            Unit::Mil => 25_400,
            Unit::Nm => 1,
        }
    }
}

/// A value read from decimal text, and how far rounding moved it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Exact {
    pub value: i64,
    /// `value` less the exact value, as `num / den` (`num` 0 when exact).
    /// Its magnitude is at most a half. Digits past the 30th significant
    /// one, or past the 38th decimal of the result, decide the rounding
    /// but aren't counted in it.
    pub error_num: i128,
    pub error_den: i128,
}

impl Exact {
    pub fn rounded(&self) -> bool {
        self.error_num != 0 || self.error_den == 0
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DecimalError {
    /// No text.
    Empty,
    /// A character that can't be there, by its byte offset.
    Syntax { at: usize },
    /// The value is past what the caller allows.
    OutOfRange,
}

impl std::fmt::Display for DecimalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecimalError::Empty => write!(f, "no number"),
            DecimalError::Syntax { at } => write!(f, "not a number at byte {at}"),
            DecimalError::OutOfRange => write!(f, "number out of range"),
        }
    }
}

impl std::error::Error for DecimalError {}

/// A length in nanometres, within ±[`COORD_LIMIT`].
pub fn nm(text: &str, unit: Unit) -> Result<Exact, DecimalError> {
    scaled(text, unit.nm(), COORD_LIMIT)
}

/// A length that must be exact to the nanometre, as [`nm`] but taking only
/// the value.
pub fn nm_value(text: &str, unit: Unit) -> Result<Nm, DecimalError> {
    nm(text, unit).map(|e| e.value)
}

/// An angle in degrees, as millidegrees reduced into one turn.
pub fn mdeg(text: &str) -> Result<(Mdeg, Exact), DecimalError> {
    let e = scaled(text, 1_000, 1 << 50)?;
    Ok((Mdeg::new(e.value), e))
}

/// A ratio in millionths (`1.5` is 1,500,000).
pub fn millionths(text: &str) -> Result<Exact, DecimalError> {
    scaled(text, 1_000_000, 1 << 50)
}

const MAX_DIGITS: u32 = 30;

/// `text` read as a decimal number, times `per_unit`, rounded to the
/// nearest integer, ties to even; `OutOfRange` past `±limit`.
///
/// The grammar is `[+-]? digits ('.' digits?)? ([eE] [+-]? digits)?` or
/// `[+-]? '.' digits ...`: at least one digit before the exponent.
pub fn scaled(text: &str, per_unit: i128, limit: i64) -> Result<Exact, DecimalError> {
    let b = text.as_bytes();
    if b.is_empty() {
        return Err(DecimalError::Empty);
    }
    let mut i = 0;
    let negative = match b[0] {
        b'-' => {
            i = 1;
            true
        }
        b'+' => {
            i = 1;
            false
        }
        _ => false,
    };
    // The significant digits as an integer `m`, the power of ten `e` they
    // are scaled by, and whether any nonzero digit was dropped (`sticky`).
    let mut m: i128 = 0;
    let mut digits = 0u32;
    let mut e: i64 = 0;
    let mut sticky = false;
    let mut any = false;
    let mut point = false;
    while i < b.len() {
        match b[i] {
            c @ b'0'..=b'9' => {
                any = true;
                let d = i128::from(c - b'0');
                if digits < MAX_DIGITS {
                    if m != 0 || d != 0 {
                        digits += 1;
                    }
                    m = m * 10 + d;
                    if point {
                        e -= 1;
                    }
                } else {
                    sticky |= d != 0;
                    if !point {
                        e += 1;
                    }
                }
            }
            b'.' if !point => point = true,
            b'e' | b'E' if any => break,
            _ => return Err(DecimalError::Syntax { at: i }),
        }
        i += 1;
    }
    if !any {
        return Err(DecimalError::Syntax { at: i.min(b.len()) });
    }
    if i < b.len() {
        // The exponent.
        i += 1;
        let mut neg = false;
        if i < b.len() && (b[i] == b'-' || b[i] == b'+') {
            neg = b[i] == b'-';
            i += 1;
        }
        let start = i;
        let mut x: i64 = 0;
        while i < b.len() {
            match b[i] {
                c @ b'0'..=b'9' => x = (x * 10 + i64::from(c - b'0')).min(10_000),
                _ => return Err(DecimalError::Syntax { at: i }),
            }
            i += 1;
        }
        if i == start {
            return Err(DecimalError::Syntax { at: i });
        }
        e += if neg { -x } else { x };
    }
    if m == 0 {
        return Ok(Exact {
            value: 0,
            error_num: 0,
            error_den: 1,
        });
    }
    // value = m × 10^e × per_unit (+ the sticky digits).
    let mut n = m * per_unit;
    let mut d: i128 = 1;
    if e >= 0 {
        for _ in 0..e {
            n = n.checked_mul(10).ok_or(DecimalError::OutOfRange)?;
        }
    } else {
        let mut p = -e;
        while p > 38 {
            sticky |= n % 10 != 0;
            n /= 10;
            p -= 1;
        }
        d = 10i128.pow(p as u32);
    }
    let q = n / d;
    let r = n % d;
    let up = match (2 * r).cmp(&d) {
        std::cmp::Ordering::Less => false,
        std::cmp::Ordering::Greater => true,
        std::cmp::Ordering::Equal => sticky || q % 2 == 1,
    };
    let q = if up { q + 1 } else { q };
    // The error, value less exact, from the digits kept.
    let mut error_num = q * d - n;
    let mut error_den = d;
    if sticky && error_num == 0 {
        // Below anything we can count: say it was rounded.
        error_den = 0;
    }
    if q > i128::from(limit) {
        return Err(DecimalError::OutOfRange);
    }
    let mut value = q as i64;
    if negative {
        value = -value;
        error_num = -error_num;
    }
    if error_den != 0 {
        let g = gcd(error_num.abs(), error_den);
        if g > 1 {
            error_num /= g;
            error_den /= g;
        }
    }
    Ok(Exact {
        value,
        error_num,
        error_den,
    })
}

fn gcd(mut a: i128, mut b: i128) -> i128 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(text: &str, unit: Unit) -> i64 {
        nm(text, unit).unwrap().value
    }

    #[test]
    fn millimetres_to_six_decimals_are_exact() {
        assert_eq!(v("1.5", Unit::Mm), 1_500_000);
        assert_eq!(v("0.000001", Unit::Mm), 1);
        assert_eq!(v("-12.345678", Unit::Mm), -12_345_678);
        assert_eq!(v(".5", Unit::Mm), 500_000);
        assert_eq!(v("5.", Unit::Mm), 5_000_000);
        assert_eq!(v("+2", Unit::Mm), 2_000_000);
        assert!(!nm("0.123456", Unit::Mm).unwrap().rounded());
    }

    #[test]
    fn microns_inches_and_mils() {
        assert_eq!(v("0.001", Unit::Micron), 1);
        assert_eq!(v("1", Unit::Inch), 25_400_000);
        assert_eq!(v("0.00001", Unit::Inch), 254);
        assert_eq!(v("0.01", Unit::Mil), 254);
        assert_eq!(v("39.3701", Unit::Mil), 1_000_001);
        assert!(!nm("0.00001", Unit::Inch).unwrap().rounded());
        assert!(!nm("0.01", Unit::Mil).unwrap().rounded());
    }

    #[test]
    fn finer_rounds_half_to_even() {
        // 0.5 nm, 1.5 nm, 2.5 nm, -2.5 nm.
        let e = nm("0.0000005", Unit::Mm).unwrap();
        assert_eq!((e.value, e.error_num, e.error_den), (0, -1, 2));
        assert_eq!(v("0.0000015", Unit::Mm), 2);
        assert_eq!(v("0.0000025", Unit::Mm), 2);
        assert_eq!(v("-0.0000025", Unit::Mm), -2);
        // 25.4 nm.
        let e = nm("0.000001", Unit::Inch).unwrap();
        assert_eq!((e.value, e.error_num, e.error_den), (25, -2, 5));
        // A tie broken by a digit past the thirtieth.
        let t = format!("0.0000025{}1", "0".repeat(40));
        assert_eq!(v(&t, Unit::Mm), 3);
        assert!(nm(&t, Unit::Mm).unwrap().rounded());
    }

    #[test]
    fn exponents() {
        assert_eq!(v("1e3", Unit::Mm), 1_000_000_000);
        assert_eq!(v("1.5E-3", Unit::Mm), 1_500);
        assert_eq!(v("0e999", Unit::Mm), 0);
        assert_eq!(nm("1e999", Unit::Mm), Err(DecimalError::OutOfRange));
        assert_eq!(v("1e-60", Unit::Mm), 0);
    }

    #[test]
    fn faults() {
        assert_eq!(nm("", Unit::Mm), Err(DecimalError::Empty));
        assert_eq!(nm("-", Unit::Mm), Err(DecimalError::Syntax { at: 1 }));
        assert_eq!(nm("1x", Unit::Mm), Err(DecimalError::Syntax { at: 1 }));
        assert_eq!(nm("1.2.3", Unit::Mm), Err(DecimalError::Syntax { at: 3 }));
        assert_eq!(nm("e5", Unit::Mm), Err(DecimalError::Syntax { at: 0 }));
        assert_eq!(nm("1e", Unit::Mm), Err(DecimalError::Syntax { at: 2 }));
        assert_eq!(nm("2000000", Unit::Mm), Err(DecimalError::OutOfRange));
        assert_eq!(v("1099511.627776", Unit::Mm), COORD_LIMIT);
    }

    #[test]
    fn degrees_and_ratios() {
        assert_eq!(mdeg("90").unwrap().0, Mdeg::new(90_000));
        assert_eq!(mdeg("-90").unwrap().0, Mdeg::new(270_000));
        assert_eq!(mdeg("45.0005").unwrap().0, Mdeg::new(45_000));
        assert!(mdeg("45.0005").unwrap().1.rounded());
        assert_eq!(mdeg("12.345").unwrap().0.get(), 12_345);
        assert_eq!(millionths("1.5").unwrap().value, 1_500_000);
        assert_eq!(millionths("0.25").unwrap().value, 250_000);
    }
}
