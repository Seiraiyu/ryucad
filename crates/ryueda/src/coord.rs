//! Coordinates: integer nanometres and millidegrees, in memory and in
//! files. Values an agent types may be quantities (`"0.5 mm"`, `"90 deg"`);
//! they are converted from their decimal text exactly, with no floating
//! point, and rounded half to even to a whole nanometre or millidegree
//! once. A bare JSON integer is already nanometres (or millidegrees).

use crate::error::{Code, Error, Result};
use crate::units::Unit;
use serde_json::Value;

/// A metre: how far from its origin anything in a library object may lie.
pub const METRE: i64 = 1_000_000_000;

/// Nanometres per length unit a quantity may be written in.
const LENGTHS: [(&str, i64); 9] = [
    ("nm", 1),
    ("um", 1_000),
    ("µm", 1_000),
    ("mm", 1_000_000),
    ("cm", 10_000_000),
    ("m", 1_000_000_000),
    ("mil", 25_400),
    ("in", 25_400_000),
    ("ft", 304_800_000),
];

/// Nanometres per unit of the project's default unit.
pub fn per_unit(u: Unit) -> i64 {
    match u {
        Unit::M => 1_000_000_000,
        Unit::Cm => 10_000_000,
        Unit::Mm => 1_000_000,
        Unit::Um => 1_000,
        Unit::In => 25_400_000,
        Unit::Ft => 304_800_000,
    }
}

/// The value of the decimal `text` (`-1.25`, `3e-3`, `.5`) times `factor`,
/// rounded half to even to a whole number, and whether rounding changed
/// it. `None` for text that isn't a decimal, has more than 27 significant
/// digits, or whose value doesn't fit in an `i64`.
pub fn exact(text: &str, factor: i64) -> Option<(i64, bool)> {
    let b = text.as_bytes();
    let mut i = 0;
    let negative = match b.first() {
        Some(b'-') => {
            i += 1;
            true
        }
        Some(b'+') => {
            i += 1;
            false
        }
        _ => false,
    };
    let (mut mantissa, mut digits, mut exp, mut any) = (0i128, 0u32, 0i64, false);
    let mut point = false;
    while i < b.len() {
        match b[i] {
            c @ b'0'..=b'9' => {
                any = true;
                if mantissa != 0 || c != b'0' {
                    digits += 1;
                    if digits > 27 {
                        return None;
                    }
                }
                mantissa = mantissa * 10 + i128::from(c - b'0');
                if point {
                    exp -= 1;
                }
            }
            b'.' if !point => point = true,
            _ => break,
        }
        i += 1;
    }
    if !any {
        return None;
    }
    if i < b.len() {
        if b[i] != b'e' && b[i] != b'E' {
            return None;
        }
        let e: i64 = text[i + 1..].parse().ok()?;
        if !(-400..=400).contains(&e) {
            return None;
        }
        exp += e;
    }
    let mut v = mantissa.checked_mul(i128::from(factor))?;
    let mut rounded = false;
    if exp >= 0 {
        for _ in 0..exp {
            v = v.checked_mul(10)?;
        }
    } else if exp < -38 {
        // Past i128's range of powers of ten, and far under one unit.
        rounded = v != 0;
        v = 0;
    } else {
        let d = 10i128.pow((-exp) as u32);
        let (q, r) = (v / d, v % d);
        rounded = r != 0;
        // Half to even: up past half, or at half when the quotient is odd.
        v = if r * 2 > d || (r * 2 == d && q % 2 == 1) {
            q + 1
        } else {
            q
        };
    }
    if negative {
        v = -v;
    }
    Some((i64::try_from(v).ok()?, rounded))
}

/// Splits `"0.5 mm"` into its number and its unit (`""` for none).
fn split(text: &str) -> (&str, &str) {
    let t = text.trim();
    let end = t
        .find(|c: char| !(c.is_ascii_digit() || "+-.eE".contains(c)))
        .unwrap_or(t.len());
    // What isn't a number (`5e`, `1.2.3`) is `exact`'s to refuse.
    let (n, u) = t.split_at(end);
    (n.trim(), u.trim())
}

fn bad(what: &str, v: &Value, why: &str) -> Error {
    Error::new(Code::InvalidQuantity, format!("bad {what} {v}: {why}")).with("value", v)
}

/// A length in nanometres: a JSON integer is nanometres, a string a
/// quantity (`"0.5 mm"`, `"20 mil"`; a bare number is in `default`).
pub fn nm(v: &Value, default: Unit) -> Result<i64> {
    if let Some(n) = v.as_i64() {
        return Ok(n);
    }
    let Some(text) = v.as_str() else {
        return Err(bad(
            "length",
            v,
            "give whole nanometres or a quantity such as \"0.5 mm\"",
        ));
    };
    let (number, unit) = split(text);
    let factor = if unit.is_empty() {
        per_unit(default)
    } else {
        LENGTHS
            .iter()
            .find(|(u, _)| *u == unit)
            .map(|(_, f)| *f)
            .ok_or_else(|| bad("length", v, "units are nm, um, mm, cm, m, mil, in and ft"))?
    };
    exact(number, factor)
        .map(|(n, _)| n)
        .ok_or_else(|| bad("length", v, "not a number this precise and size"))
}

/// An angle in millidegrees: a JSON integer is millidegrees, a string
/// degrees (`"90"`, `"45 deg"`). Radians aren't taken: no whole number of
/// millidegrees is exact in them.
pub fn mdeg(v: &Value) -> Result<i64> {
    if let Some(n) = v.as_i64() {
        return Ok(n);
    }
    let Some(text) = v.as_str() else {
        return Err(bad(
            "angle",
            v,
            "give whole millidegrees or degrees such as \"90 deg\"",
        ));
    };
    let (number, unit) = split(text);
    if !(unit.is_empty() || unit == "deg") {
        return Err(bad("angle", v, "angles are in degrees (deg)"));
    }
    exact(number, 1_000)
        .map(|(n, _)| n)
        .ok_or_else(|| bad("angle", v, "not a number this precise and size"))
}

/// A point `[x, y]` of two lengths.
pub fn point(v: &Value, default: Unit) -> Result<[i64; 2]> {
    match v.as_array().map(Vec::as_slice) {
        Some([x, y]) => Ok([nm(x, default)?, nm(y, default)?]),
        _ => Err(bad("point", v, "a point is [x, y]")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn decimals_convert_exactly() {
        assert_eq!(exact("2.54", 1_000_000), Some((2_540_000, false)));
        assert_eq!(exact("-0.1", 1_000_000), Some((-100_000, false)));
        assert_eq!(exact(".5", 1_000_000), Some((500_000, false)));
        assert_eq!(exact("1e-3", 1_000_000_000), Some((1_000_000, false)));
        assert_eq!(exact("+3E2", 1), Some((300, false)));
        // 0.1 isn't a binary fraction, but nothing here is binary.
        assert_eq!(exact("0.1", 10), Some((1, false)));
        assert_eq!(exact("0.000000000000000000000000001", 1), Some((0, true)));
        for bad in ["", "-", ".", "1.2.3", "1x", "e5", "1e", "0x10"] {
            assert_eq!(exact(bad, 1), None, "{bad}");
        }
        assert_eq!(exact("1e400", 1), None);
        assert_eq!(exact("1234567890123456789012345678", 1), None);
        assert_eq!(
            exact("000123456789.012345678901234567", 1),
            Some((123_456_789, true))
        );
        assert_eq!(exact("9223372036854775808", 1), None);
    }

    #[test]
    fn rounding_is_half_to_even() {
        // Seven decimals of a millimetre are a tenth of a nanometre.
        let nm = |t| exact(t, 1_000_000).unwrap();
        assert_eq!(nm("0.0000005"), (0, true));
        assert_eq!(nm("0.0000015"), (2, true));
        assert_eq!(nm("0.0000025"), (2, true));
        assert_eq!(nm("0.00000251"), (3, true));
        assert_eq!(nm("-0.0000015"), (-2, true));
        assert_eq!(nm("-0.0000025"), (-2, true));
        assert_eq!(nm("1.000001"), (1_000_001, false));
    }

    #[test]
    fn lengths_take_units_or_the_default() {
        assert_eq!(nm(&json!(254), Unit::Mm).unwrap(), 254);
        assert_eq!(nm(&json!("2.54"), Unit::Mm).unwrap(), 2_540_000);
        assert_eq!(nm(&json!("0.1"), Unit::In).unwrap(), 2_540_000);
        assert_eq!(nm(&json!("100 mil"), Unit::Mm).unwrap(), 2_540_000);
        assert_eq!(nm(&json!("5um"), Unit::Mm).unwrap(), 5_000);
        assert_eq!(nm(&json!("5 µm"), Unit::Mm).unwrap(), 5_000);
        assert_eq!(nm(&json!(" -1 m "), Unit::Mm).unwrap(), -METRE);
        for v in [json!("2 furlongs"), json!(2.5), json!(true), json!("mm")] {
            let e = nm(&v, Unit::Mm).unwrap_err();
            assert_eq!(e.code, Code::InvalidQuantity, "{v}");
        }
    }

    #[test]
    fn angles_are_degrees() {
        assert_eq!(mdeg(&json!(90_000)).unwrap(), 90_000);
        assert_eq!(mdeg(&json!("90")).unwrap(), 90_000);
        assert_eq!(mdeg(&json!("-22.5 deg")).unwrap(), -22_500);
        assert_eq!(mdeg(&json!("0.0005")).unwrap(), 0);
        assert_eq!(mdeg(&json!("0.0015")).unwrap(), 2);
        assert_eq!(
            mdeg(&json!("1 rad")).unwrap_err().code,
            Code::InvalidQuantity
        );
    }

    #[test]
    fn points_are_pairs() {
        assert_eq!(point(&json!(["1", 5]), Unit::Mm).unwrap(), [1_000_000, 5]);
        assert!(point(&json!([1]), Unit::Mm).is_err());
    }
}
