//! Quantities at the boundary. Files and the CLI carry lengths as strings
//! with units (`"10 mm"`); ryukei sees SI meters only.
//!
//! The conversion is the nearest f64 to the decimal, times the unit's f64
//! factor, rounded once more. That is the same on every platform; it does
//! not promise the nearest f64 to the exact length.

use crate::error::{Code, Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::fmt;

/// A length unit. Each factor is exact in decimal and fixed forever.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Unit {
    #[serde(rename = "m")]
    M,
    #[serde(rename = "cm")]
    Cm,
    #[serde(rename = "mm")]
    Mm,
    #[serde(rename = "um")]
    Um,
    #[serde(rename = "in")]
    In,
    #[serde(rename = "ft")]
    Ft,
}

impl Unit {
    pub const ALL: [Unit; 6] = [Unit::M, Unit::Cm, Unit::Mm, Unit::Um, Unit::In, Unit::Ft];

    pub fn as_str(self) -> &'static str {
        match self {
            Unit::M => "m",
            Unit::Cm => "cm",
            Unit::Mm => "mm",
            Unit::Um => "um",
            Unit::In => "in",
            Unit::Ft => "ft",
        }
    }

    pub fn parse(s: &str) -> Option<Unit> {
        Unit::ALL.into_iter().find(|u| u.as_str() == s)
    }

    /// Meters per unit, as the f64 nearest the exact decimal.
    pub fn factor(self) -> f64 {
        match self {
            Unit::M => 1.0,
            Unit::Cm => 0.01,
            Unit::Mm => 0.001,
            Unit::Um => 1e-6,
            Unit::In => 0.0254,
            Unit::Ft => 0.3048,
        }
    }
}

impl fmt::Display for Unit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A length as written, `<number> <unit>`, and its value in meters.
#[derive(Clone, Debug, PartialEq)]
pub struct Quantity {
    number: String,
    unit: Unit,
    meters: f64,
}

impl Quantity {
    /// Parses `"12.5 mm"`, `"12.5mm"` or a bare `"12.5"` (which takes
    /// `default`). The number is decimal, optionally signed, with an
    /// optional exponent: `1`, `-0.5`, `.5`, `2.`, `1e-3`.
    pub fn parse(s: &str, default: Unit) -> Result<Quantity> {
        let bad = |why: &str| {
            Error::new(Code::InvalidQuantity, format!("bad quantity {s:?}: {why}")).with("value", s)
        };
        let t = s.trim();
        let end = number_len(t);
        if end == 0 {
            return Err(bad("expected a number"));
        }
        let (number, rest) = t.split_at(end);
        let rest = rest.trim_start();
        let unit = if rest.is_empty() {
            default
        } else {
            Unit::parse(rest).ok_or_else(|| {
                bad("unknown unit; use m, cm, mm, um, in or ft").with("unit", rest)
            })?
        };
        let value: f64 = number.parse().map_err(|_| bad("expected a number"))?;
        let meters = value * unit.factor();
        if !meters.is_finite() {
            return Err(bad("too large"));
        }
        Ok(Quantity {
            number: number.to_owned(),
            unit,
            meters,
        })
    }

    /// A quantity from JSON: a string as [`parse`](Self::parse), or a bare
    /// number in the default unit.
    pub fn from_json(v: &Value, default: Unit) -> Result<Quantity> {
        match v {
            Value::String(s) => Quantity::parse(s, default),
            Value::Number(n) => Quantity::parse(&n.to_string(), default),
            _ => Err(Error::new(
                Code::InvalidQuantity,
                format!("expected a quantity string such as \"10 mm\", got {v}"),
            )
            .with("value", v)),
        }
    }

    pub fn meters(&self) -> f64 {
        self.meters
    }

    pub fn unit(&self) -> Unit {
        self.unit
    }

    /// The canonical text, `<number> <unit>`.
    pub fn text(&self) -> String {
        format!("{} {}", self.number, self.unit)
    }

    /// How output reports a quantity: as written, and in meters.
    pub fn report(&self) -> Value {
        json!({"value": self.text(), "m": self.meters})
    }
}

impl fmt::Display for Quantity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.number, self.unit)
    }
}

/// The length of the decimal number at the start of `s`, or 0.
fn number_len(s: &str) -> usize {
    let b = s.as_bytes();
    let mut i = 0;
    if matches!(b.first(), Some(b'+' | b'-')) {
        i += 1;
    }
    let int_start = i;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    let mut digits = i - int_start;
    if i < b.len() && b[i] == b'.' {
        i += 1;
        let frac_start = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        digits += i - frac_start;
    }
    if digits == 0 {
        return 0;
    }
    // An exponent counts only if digits follow it, so "1e" leaves "e" as a
    // (bad) unit rather than a bad number.
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        let mut j = i + 1;
        if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
            j += 1;
        }
        let exp_start = j;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        if j > exp_start {
            i = j;
        }
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(s: &str) -> Quantity {
        Quantity::parse(s, Unit::Mm).unwrap()
    }

    #[test]
    fn every_unit_and_notation() {
        for (s, text, m) in [
            ("10 mm", "10 mm", 0.01),
            ("10mm", "10 mm", 0.01),
            ("  10   mm ", "10 mm", 0.01),
            ("10", "10 mm", 0.01),
            ("2 m", "2 m", 2.0),
            ("2.5 cm", "2.5 cm", 2.5 * 0.01),
            ("7 um", "7 um", 7.0 * 1e-6),
            ("1 in", "1 in", 0.0254),
            ("1 ft", "1 ft", 0.3048),
            ("-0.5 m", "-0.5 m", -0.5),
            ("+3 m", "+3 m", 3.0),
            (".5 m", ".5 m", 0.5),
            ("2. m", "2. m", 2.0),
            ("1e-3 m", "1e-3 m", 1e-3),
            ("1E+2 mm", "1E+2 mm", 0.1),
        ] {
            let x = q(s);
            assert_eq!(x.text(), text, "{s}");
            assert_eq!(x.meters(), m, "{s}");
        }
    }

    #[test]
    fn the_default_unit_applies_to_bare_numbers_only() {
        assert_eq!(Quantity::parse("3", Unit::In).unwrap().text(), "3 in");
        assert_eq!(Quantity::parse("3 m", Unit::In).unwrap().text(), "3 m");
    }

    #[test]
    fn rejects_garbage() {
        for s in [
            "",
            "mm",
            "abc",
            "1 furlong",
            "1e",
            "1..2 mm",
            "- 1 mm",
            "1 mm mm",
            "1e999 m",
        ] {
            let e = Quantity::parse(s, Unit::Mm).unwrap_err();
            assert_eq!(e.code, Code::InvalidQuantity, "{s}");
        }
    }

    #[test]
    fn json_numbers_are_bare() {
        let x = Quantity::from_json(&json!(12.5), Unit::Mm).unwrap();
        assert_eq!(x.text(), "12.5 mm");
        let x = Quantity::from_json(&json!(40), Unit::Mm).unwrap();
        assert_eq!(x.text(), "40 mm");
        let e = Quantity::from_json(&json!(true), Unit::Mm).unwrap_err();
        assert_eq!(e.code, Code::InvalidQuantity);
    }

    #[test]
    fn report_gives_both_forms() {
        assert_eq!(q("10 mm").report(), json!({"value": "10 mm", "m": 0.01}));
    }

    #[test]
    fn units_round_trip_through_serde() {
        for u in Unit::ALL {
            let v = serde_json::to_value(u).unwrap();
            assert_eq!(v, json!(u.as_str()));
            assert_eq!(serde_json::from_value::<Unit>(v).unwrap(), u);
        }
    }
}
