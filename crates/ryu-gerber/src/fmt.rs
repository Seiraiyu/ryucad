//! Numbers and text as the formats write them, from integer nanometres.

use crate::Error;
use ryu_board::geom::{Nm, Point};

/// The largest coordinate magnitude written: `%FSLAX46Y46*%` holds four
/// integer and six decimal digits of millimetres, and a unit of the sixth
/// decimal is a nanometre.
pub const LIMIT: Nm = 9_999_999_999;

/// A Gerber coordinate: its nanometres as an integer (leading zeros
/// omitted), or `OutOfRange` naming `at`.
pub fn coord(v: Nm, at: Point) -> Result<String, Error> {
    if v.abs() > LIMIT {
        return Err(Error::OutOfRange(at));
    }
    Ok(v.to_string())
}

/// Millimetres with six decimals, exact: `0.250000`.
pub fn mm6(v: Nm) -> String {
    let sign = if v < 0 { "-" } else { "" };
    let a = v.unsigned_abs();
    format!("{sign}{}.{:06}", a / 1_000_000, a % 1_000_000)
}

/// Millimetres with as few decimals as hold the nanometres exactly, and
/// at least one: `1.5`, `0.0`, `0.000123`.
pub fn mm(v: Nm) -> String {
    let s = mm6(v);
    let t = s.trim_end_matches('0');
    if t.ends_with('.') {
        format!("{t}0")
    } else {
        t.to_owned()
    }
}

/// An attribute field: printable ASCII as is, but `%`, `*`, `,` and `\`
/// (which the format reserves) and anything else as `\uXXXX` (UTF-16).
pub fn field(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if (c.is_ascii_graphic() && !matches!(c, '%' | '*' | ',' | '\\')) || c == ' ' {
            out.push(c);
        } else {
            for u in c.encode_utf16(&mut [0; 2]) {
                out.push_str(&format!("\\u{u:04X}"));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn millimetres_are_written_exactly() {
        assert_eq!(mm6(250_000), "0.250000");
        assert_eq!(mm6(1_000_000_001), "1000.000001");
        assert_eq!(mm6(-1), "-0.000001");
        assert_eq!(mm(1_500_000), "1.5");
        assert_eq!(mm(0), "0.0");
        assert_eq!(mm(-2_000_000), "-2.0");
        assert_eq!(mm(123), "0.000123");
        let at = Point::new(0, 0);
        assert_eq!(coord(-12_345, at).unwrap(), "-12345");
        assert_eq!(coord(LIMIT, at).unwrap(), "9999999999");
        assert_eq!(coord(LIMIT + 1, at), Err(Error::OutOfRange(at)));
    }

    #[test]
    fn attribute_fields_escape_what_the_format_reserves() {
        assert_eq!(field("rail:gnd"), "rail:gnd");
        assert_eq!(field("s1/n 2"), "s1/n 2");
        assert_eq!(field("a,b*c%d\\e"), "a\\u002Cb\\u002Ac\\u0025d\\u005Ce");
        assert_eq!(field("µ"), "\\u00B5");
    }
}
