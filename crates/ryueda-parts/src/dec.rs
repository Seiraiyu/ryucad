//! Exact decimals: a mantissa and a power of ten, so `100 nF` and
//! `0.1 uF` compare equal and `16 V` passes `>= 6.3 V` with no floating
//! point.

use std::cmp::Ordering;
use std::fmt;

/// The most significant digits a decimal holds: 27, as ryueda's intent
/// comparisons always allowed.
pub const MAX_DIGITS: u32 = 27;

/// `mantissa × 10^exp`, the mantissa with no trailing zeros (zero is
/// `0 × 10^0`) and at most [`MAX_DIGITS`] digits, so equal values are
/// equal fields.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Dec {
    mantissa: i128,
    exp: i32,
}

/// How many decimal digits `m` has (0 for 0).
fn digits(m: i128) -> u32 {
    let mut n = m.unsigned_abs();
    let mut d = 0;
    while n > 0 {
        n /= 10;
        d += 1;
    }
    d
}

impl Dec {
    pub const ZERO: Dec = Dec { mantissa: 0, exp: 0 };

    /// `mantissa × 10^exp`, normalized; `None` past [`MAX_DIGITS`]
    /// significant digits or an exponent past `i32`.
    pub fn new(mantissa: i128, exp: i32) -> Option<Dec> {
        if mantissa == 0 {
            return Some(Dec::ZERO);
        }
        let (mut m, mut e) = (mantissa, exp);
        while m % 10 == 0 {
            m /= 10;
            e = e.checked_add(1)?;
        }
        (digits(m) <= MAX_DIGITS).then_some(Dec { mantissa: m, exp: e })
    }

    /// A decimal number as text: `-12.5`, `+3`, `.5`, `2.`, `1e-3`. `None`
    /// for anything else.
    pub fn parse(text: &str) -> Option<Dec> {
        let t = text.trim();
        let (number, exp) = match t.split_once(['e', 'E']) {
            Some((n, e)) => (n, e.parse::<i32>().ok()?),
            None => (t, 0),
        };
        let (negative, number) = match number.strip_prefix('-') {
            Some(n) => (true, n),
            None => (false, number.strip_prefix('+').unwrap_or(number)),
        };
        let (whole, frac) = number.split_once('.').unwrap_or((number, ""));
        if (whole.is_empty() && frac.is_empty())
            || !whole.bytes().chain(frac.bytes()).all(|b| b.is_ascii_digit())
        {
            return None;
        }
        let all = format!("{whole}{frac}");
        let all = all.trim_start_matches('0');
        let kept = all.trim_end_matches('0');
        if kept.len() > MAX_DIGITS as usize {
            return None;
        }
        let mantissa: i128 = if kept.is_empty() { 0 } else { kept.parse().ok()? };
        let exp = exp
            .checked_sub(i32::try_from(frac.len()).ok()?)?
            .checked_add(i32::try_from(all.len() - kept.len()).ok()?)?;
        Dec::new(if negative { -mantissa } else { mantissa }, exp)
    }

    pub fn mantissa(self) -> i128 {
        self.mantissa
    }

    pub fn exp(self) -> i32 {
        self.exp
    }

    /// Times `10^by`, `None` past `i32`'s exponents.
    pub fn shift(self, by: i32) -> Option<Dec> {
        if self.mantissa == 0 {
            return Some(self);
        }
        Some(Dec {
            mantissa: self.mantissa,
            exp: self.exp.checked_add(by)?,
        })
    }

    pub fn abs(self) -> Dec {
        Dec {
            mantissa: self.mantissa.abs(),
            exp: self.exp,
        }
    }
}

impl Ord for Dec {
    fn cmp(&self, other: &Dec) -> Ordering {
        let (a, b) = (self.mantissa.signum(), other.mantissa.signum());
        if a != b || a == 0 {
            return a.cmp(&b);
        }
        // One sign: by magnitude (digits plus exponent), then by the
        // mantissas padded to the same number of digits, which fits in an
        // i128 since each has at most 27.
        let size = |d: &Dec| i64::from(digits(d.mantissa)) + i64::from(d.exp);
        let pad = |d: &Dec| d.mantissa.unsigned_abs() * 10u128.pow(MAX_DIGITS - digits(d.mantissa));
        let by = size(self)
            .cmp(&size(other))
            .then_with(|| pad(self).cmp(&pad(other)));
        if a < 0 { by.reverse() } else { by }
    }
}

impl PartialOrd for Dec {
    fn partial_cmp(&self, other: &Dec) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for Dec {
    /// Plain text where it is short (`4.7`, `0.0000001`, `120000`), else
    /// `<digits>e<exp>`; [`Dec::parse`] reads either back.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sign = if self.mantissa < 0 { "-" } else { "" };
        let digits = self.mantissa.unsigned_abs().to_string();
        let len = digits.len() as i64;
        let exp = i64::from(self.exp);
        if (0..=6).contains(&exp) {
            write!(f, "{sign}{digits}{}", "0".repeat(exp as usize))
        } else if exp < 0 && -exp <= len + 6 {
            let point = len + exp;
            if point > 0 {
                let (a, b) = digits.split_at(point as usize);
                write!(f, "{sign}{a}.{b}")
            } else {
                write!(f, "{sign}0.{}{digits}", "0".repeat((-point) as usize))
            }
        } else {
            write!(f, "{sign}{digits}e{exp}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> Dec {
        Dec::parse(s).unwrap_or_else(|| panic!("{s} reads"))
    }

    #[test]
    fn decimals_read_exactly_and_compare_by_value() {
        assert_eq!(d("100e-9"), d("0.1e-6"));
        assert_eq!(d("0.10"), Dec::new(1, -1).unwrap());
        assert!(d("16") > d("6.3"));
        assert!(d("-55") < d("-40"));
        assert!(d("-0.001") < d("0"));
        assert!(d("1e30") > d("999999999999999999999999999"));
        assert!(d("-1e30") < d("-999999999999999999999999999"));
        assert_eq!(d("-0"), Dec::ZERO);
        assert_eq!(d("+.5"), d("0.5"));
        assert_eq!(d("2."), d("2"));
        for bad in [
            "", ".", "1.2.3", "1e", "e3", "--1", "1 2", "0x10",
            "1234567890123456789012345678",
        ] {
            assert_eq!(Dec::parse(bad), None, "{bad}");
        }
        // Trailing zeros don't count against the 27 digits.
        assert_eq!(
            d("1234567890123456789012345670"),
            Dec::new(123456789012345678901234567, 1).unwrap()
        );
        assert_eq!(Dec::new(10_i128.pow(30), 0), Dec::new(1, 30));
        assert_eq!(Dec::new(10_i128.pow(30) + 1, 0), None);
    }

    #[test]
    fn decimals_write_as_plain_text_where_short() {
        for (s, text) in [
            ("4.70", "4.7"),
            ("100e-9", "0.0000001"),
            ("1e-12", "1e-12"),
            ("-55", "-55"),
            ("1e9", "1e9"),
            ("120000", "120000"),
            ("-0.25", "-0.25"),
            ("0", "0"),
        ] {
            assert_eq!(d(s).to_string(), text, "{s}");
            assert_eq!(d(&d(s).to_string()), d(s), "{s} round trip");
        }
    }

    #[test]
    fn shifts_and_signs() {
        assert_eq!(d("4.7").shift(3), Some(d("4700")));
        assert_eq!(d("-5").abs(), d("5"));
        assert_eq!(Dec::ZERO.shift(i32::MAX), Some(Dec::ZERO));
        assert_eq!(d("1").shift(i32::MAX).and_then(|x| x.shift(1)), None);
        assert_eq!((d("-4.7").mantissa(), d("-4.7").exp()), (-47, -1));
    }
}
