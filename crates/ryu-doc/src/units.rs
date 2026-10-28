//! Quantities at the boundary. Files and the CLI carry lengths as strings
//! with units (`"10 mm"`); ryukei sees SI meters only. Angles (`deg`,
//! `rad`) and plain numbers appear in expressions ([`crate::expr`]).
//!
//! The conversion is the nearest f64 to the decimal, times the unit's f64
//! factor, rounded once more. That is the same on every platform; it does
//! not promise the nearest f64 to the exact length.

use crate::error::{Code, Error, Result};
use crate::expr::{Expr, Val, no_parameters};
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

/// What a value measures, as powers of length and angle and, for
/// electrical quantities, of mass, time and current (the SI base units
/// they are made of): a length is `{length: 1}`, an area `{length: 2}`, a
/// plain number all zeros, a volt `{length: 2, mass: 1, time: -3,
/// current: -1}`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Dim {
    pub length: i8,
    pub angle: i8,
    pub mass: i8,
    pub time: i8,
    pub current: i8,
}

impl Dim {
    pub const NUMBER: Dim = Dim::of([0, 0, 0, 0, 0]);
    pub const LENGTH: Dim = Dim::of([1, 0, 0, 0, 0]);
    pub const ANGLE: Dim = Dim::of([0, 1, 0, 0, 0]);
    pub const TIME: Dim = Dim::of([0, 0, 0, 1, 0]);
    pub const FREQUENCY: Dim = Dim::of([0, 0, 0, -1, 0]);
    pub const CURRENT: Dim = Dim::of([0, 0, 0, 0, 1]);
    pub const VOLTAGE: Dim = Dim::of([2, 0, 1, -3, -1]);
    pub const RESISTANCE: Dim = Dim::of([2, 0, 1, -3, -2]);
    pub const CAPACITANCE: Dim = Dim::of([-2, 0, -1, 4, 2]);
    pub const INDUCTANCE: Dim = Dim::of([2, 0, 1, -2, -2]);
    pub const POWER: Dim = Dim::of([2, 0, 1, -3, 0]);
    pub const MASS: Dim = Dim::of([0, 0, 1, 0, 0]);
    pub const DENSITY: Dim = Dim::of([-3, 0, 1, 0, 0]);

    /// Mass and density by name, and the symbol output writes their SI
    /// unit with. Like the electrical ones, they are read only by
    /// [`Quantity::density`], never in expressions.
    pub const MECHANICAL: [(&'static str, Dim, &'static str); 2] = [
        ("mass", Dim::MASS, "kg"),
        ("density", Dim::DENSITY, "kg/m^3"),
    ];

    /// The electrical dimensions by name, and the symbol of their SI unit.
    pub const ELECTRICAL: [(&'static str, Dim, &'static str); 8] = [
        ("voltage", Dim::VOLTAGE, "V"),
        ("current", Dim::CURRENT, "A"),
        ("resistance", Dim::RESISTANCE, "Ohm"),
        ("capacitance", Dim::CAPACITANCE, "F"),
        ("inductance", Dim::INDUCTANCE, "H"),
        ("frequency", Dim::FREQUENCY, "Hz"),
        ("power", Dim::POWER, "W"),
        ("time", Dim::TIME, "s"),
    ];

    /// The dimension with these powers of length, angle, mass, time and
    /// current.
    pub const fn of([length, angle, mass, time, current]: [i8; 5]) -> Dim {
        Dim {
            length,
            angle,
            mass,
            time,
            current,
        }
    }

    fn powers(self) -> [i32; 5] {
        [
            self.length as i32,
            self.angle as i32,
            self.mass as i32,
            self.time as i32,
            self.current as i32,
        ]
    }

    /// The dimension of a product; `None` past the powers anyone needs.
    pub fn times(self, o: Dim) -> Option<Dim> {
        let (a, b) = (self.powers(), o.powers());
        Dim::new(std::array::from_fn(|i| a[i] + b[i]))
    }

    /// The dimension of a quotient.
    pub fn per(self, o: Dim) -> Option<Dim> {
        let (a, b) = (self.powers(), o.powers());
        Dim::new(std::array::from_fn(|i| a[i] - b[i]))
    }

    /// The dimension of `self` to the power `n`.
    pub fn pow(self, n: i32) -> Option<Dim> {
        let a = self.powers();
        Dim::new(std::array::from_fn(|i| a[i] * n))
    }

    /// The square root's dimension, when every power is even.
    pub fn sqrt(self) -> Option<Dim> {
        let a = self.powers();
        a.iter()
            .all(|p| p % 2 == 0)
            .then(|| Dim::new(std::array::from_fn(|i| a[i] / 2)))
            .flatten()
    }

    fn new(powers: [i32; 5]) -> Option<Dim> {
        const MAX: i32 = 16;
        powers
            .iter()
            .all(|p| p.abs() <= MAX)
            .then(|| Dim::of(powers.map(|p| p as i8)))
    }

    /// How people read it: `length`, `angle`, `number`, `length^2`,
    /// `length*angle^-1`; an electrical dimension by its name (`voltage`),
    /// and any other product of powers in the order length, angle, mass,
    /// time, current (`mass*time^-2`).
    pub fn name(self) -> String {
        if let Some((name, _, _)) = Dim::ELECTRICAL
            .iter()
            .chain(&Dim::MECHANICAL)
            .find(|(_, d, _)| *d == self)
        {
            return (*name).to_owned();
        }
        let part = |name: &str, p: i8| match p {
            0 => None,
            1 => Some(name.to_owned()),
            p => Some(format!("{name}^{p}")),
        };
        let parts: Vec<String> = [
            part("length", self.length),
            part("angle", self.angle),
            part("mass", self.mass),
            part("time", self.time),
            part("current", self.current),
        ]
        .into_iter()
        .flatten()
        .collect();
        if parts.is_empty() {
            "number".to_owned()
        } else {
            parts.join("*")
        }
    }

    /// The dimension [`Dim::name`] writes as `s`, for the powers of length
    /// and angle ryucad's and ryupdm's files name; electrical dimensions
    /// aren't read here (see [`Quantity::electrical`]).
    pub fn parse(s: &str) -> Option<Dim> {
        if s == "number" {
            return Some(Dim::NUMBER);
        }
        let (mut length, mut angle) = (None, None);
        for part in s.split('*') {
            let (name, power) = match part.split_once('^') {
                Some((n, p)) => (n, p.parse::<i32>().ok().filter(|p| *p != 0 && *p != 1)?),
                None => (part, 1),
            };
            match name {
                // Length comes first, each at most once.
                "length" if length.is_none() && angle.is_none() => length = Some(power),
                "angle" if angle.is_none() => angle = Some(power),
                _ => return None,
            }
        }
        Dim::new([length.unwrap_or(0), angle.unwrap_or(0), 0, 0, 0])
    }

    /// The key output gives the SI value under: `m`, `rad`, `n`, or `si`
    /// for anything else.
    pub fn key(self) -> &'static str {
        match self {
            Dim::LENGTH => "m",
            Dim::ANGLE => "rad",
            Dim::NUMBER => "n",
            _ => "si",
        }
    }
}

/// Radians per degree, as an f64 constant: the same bits everywhere.
pub const DEG: f64 = core::f64::consts::PI / 180.0;

/// A unit a literal may carry: its factor to SI and what it measures.
/// Lengths are the [`Unit`]s; angles are `deg` and `rad`.
pub fn literal_unit(s: &str) -> Option<(f64, Dim)> {
    match s {
        "deg" => Some((DEG, Dim::ANGLE)),
        "rad" => Some((1.0, Dim::ANGLE)),
        _ => Unit::parse(s).map(|u| (u.factor(), Dim::LENGTH)),
    }
}

/// The SI prefixes an electrical unit may carry, and their factors as the
/// f64s nearest the exact decimals. `u`, `µ` (U+00B5) and `μ` (U+03BC)
/// are all micro; output writes `u`.
const PREFIXES: [(&str, f64); 10] = [
    ("p", 1e-12),
    ("n", 1e-9),
    ("u", 1e-6),
    ("\u{b5}", 1e-6),
    ("\u{3bc}", 1e-6),
    ("m", 1e-3),
    ("", 1.0),
    ("k", 1e3),
    ("M", 1e6),
    ("G", 1e9),
];

/// The unit of an electrical literal of dimension `dim`: its factor to SI
/// and how output writes it (`nF`, `kOhm`, `uA`). The ohm is `Ohm`, `ohm`,
/// `Ω` (U+03A9) or `Ω` (U+2126), written `Ohm`.
fn electrical_unit(dim: Dim, s: &str) -> Option<(f64, String)> {
    let (_, _, symbol) = Dim::ELECTRICAL.iter().find(|(_, d, _)| *d == dim)?;
    let spellings: &[&str] = if *symbol == "Ohm" {
        &["Ohm", "ohm", "\u{3a9}", "\u{2126}"]
    } else {
        &[symbol]
    };
    PREFIXES.iter().find_map(|(prefix, factor)| {
        let rest = s.strip_prefix(prefix)?;
        spellings.contains(&rest).then(|| {
            let prefix = if *factor == 1e-6 { "u" } else { prefix };
            (*factor, format!("{prefix}{symbol}"))
        })
    })
}

/// Every unit name, for messages and reserved names.
pub const UNIT_NAMES: [&str; 8] = ["m", "cm", "mm", "um", "in", "ft", "deg", "rad"];

/// A length or an angle as written, and its SI value (meters or
/// radians). Usually a literal, `<number> <unit>`; it may be any [`Expr`]
/// that gives the right kind, such as `2 * wall + 3 mm`, whose value
/// depends on the part's parameters.
#[derive(Clone, Debug, PartialEq)]
pub struct Quantity {
    text: String,
    si: f64,
    literal: bool,
    dim: Dim,
}

/// What a field holds, and how a bare number in it reads: a length in
/// the default unit, an angle in degrees, or a plain number (a count).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Length(Unit),
    Angle,
    Number,
}

impl Field {
    fn dim(self) -> Dim {
        match self {
            Field::Length(_) => Dim::LENGTH,
            Field::Angle => Dim::ANGLE,
            Field::Number => Dim::NUMBER,
        }
    }

    /// The factor and name of a literal's unit: the default for a bare
    /// number, else one of the field's own units.
    fn unit(self, rest: &str) -> Option<(f64, String)> {
        match (self, rest) {
            (Field::Length(u), "") => Some((u.factor(), u.to_string())),
            (Field::Length(_), r) => Unit::parse(r).map(|u| (u.factor(), u.to_string())),
            (Field::Angle, "" | "deg") => Some((DEG, "deg".to_owned())),
            (Field::Angle, "rad") => Some((1.0, "rad".to_owned())),
            (Field::Angle, _) => None,
            (Field::Number, "") => Some((1.0, String::new())),
            (Field::Number, _) => None,
        }
    }
}

impl Quantity {
    /// Parses `"12.5 mm"`, `"12.5mm"` or a bare `"12.5"` (which takes
    /// `default`). The number is decimal, optionally signed, with an
    /// optional exponent: `1`, `-0.5`, `.5`, `2.`, `1e-3`. Anything else is
    /// an expression with no parameters to read.
    pub fn parse(s: &str, default: Unit) -> Result<Quantity> {
        Quantity::eval(s, default, &no_parameters)
    }

    /// [`parse`](Self::parse), with `lookup` giving the parameters an
    /// expression reads. Only a string that is nothing but a number takes
    /// `default`; inside an expression a bare number is a plain number.
    pub fn eval(s: &str, default: Unit, lookup: &dyn Fn(&str) -> Result<Val>) -> Result<Quantity> {
        Quantity::eval_as(s, Field::Length(default), lookup)
    }

    /// A quantity for `field`: a length, or an angle (`15 deg`, `0.2 rad`,
    /// or a bare number of degrees).
    pub fn eval_as(
        s: &str,
        field: Field,
        lookup: &dyn Fn(&str) -> Result<Val>,
    ) -> Result<Quantity> {
        let t = s.trim();
        let end = number_len(t);
        if end > 0 {
            let (number, rest) = t.split_at(end);
            if let Some(unit) = field.unit(rest.trim_start()) {
                return Quantity::literal(s, number, unit, field.dim());
            }
        }
        let e = Expr::parse(t)?;
        let v = e.eval(lookup)?;
        let want = field.dim();
        if v.dim != want {
            return Err(Error::new(
                Code::UnitMismatch,
                format!(
                    "{t} is a {}; a{} {} goes here",
                    v.dim.name(),
                    if want == Dim::ANGLE { "n" } else { "" },
                    want.name()
                ),
            )
            .with("value", t)
            .with("at", e.text())
            .with("kind", v.dim.name()));
        }
        Ok(Quantity {
            text: e.text().to_owned(),
            si: v.x,
            literal: e.is_literal(),
            dim: want,
        })
    }

    /// The phase 0 form, exactly: the nearest f64 to the decimal, times the
    /// unit's factor.
    fn literal(s: &str, number: &str, (factor, unit): (f64, String), dim: Dim) -> Result<Quantity> {
        let bad = |why: &str| {
            Error::new(Code::InvalidQuantity, format!("bad quantity {s:?}: {why}")).with("value", s)
        };
        let value: f64 = number.parse().map_err(|_| bad("expected a number"))?;
        let si = value * factor;
        if !si.is_finite() {
            return Err(bad("too large"));
        }
        // A plain number has no unit to write.
        let text = if unit.is_empty() {
            number.to_owned()
        } else {
            format!("{number} {unit}")
        };
        Ok(Quantity {
            text,
            si,
            literal: true,
            dim,
        })
    }

    /// An electrical quantity of dimension `dim` (one of
    /// [`Dim::ELECTRICAL`]'s): a literal `<number> <unit>` with an SI
    /// prefix (`3.3 V`, `100nF`, `4.7 kOhm`, `20 mA`), its text written
    /// with the unit's canonical spelling. There is no default unit, and
    /// no expression: electrical units are never names in expressions, so
    /// no parameter name is reserved by them.
    pub fn electrical(s: &str, dim: Dim) -> Result<Quantity> {
        let t = s.trim();
        let end = number_len(t);
        let (number, rest) = t.split_at(end);
        let unit = (end > 0)
            .then(|| electrical_unit(dim, rest.trim_start()))
            .flatten();
        match unit {
            Some(unit) => Quantity::literal(s, number, unit, dim),
            None => Err(Error::new(
                Code::InvalidQuantity,
                format!(
                    "bad quantity {s:?}: expected a {} such as {}",
                    dim.name(),
                    match Dim::ELECTRICAL.iter().find(|(_, d, _)| *d == dim) {
                        Some((_, _, symbol)) => format!("\"1 {symbol}\""),
                        None => "a number with its unit".to_owned(),
                    }
                ),
            )
            .with("value", s)
            .with("kind", dim.name())),
        }
    }

    /// A density: a literal `<number> kg/m^3` or `<number> g/cm^3`
    /// (`7850 kg/m^3`, `7.85 g/cm^3`), kept as written with the unit's
    /// spelling; its SI value is in kg/m³. No default unit, no expression.
    pub fn density(s: &str) -> Result<Quantity> {
        let t = s.trim();
        let end = number_len(t);
        let (number, rest) = t.split_at(end);
        let unit = match rest.trim_start() {
            "kg/m^3" => Some((1.0, "kg/m^3".to_owned())),
            "g/cm^3" => Some((1000.0, "g/cm^3".to_owned())),
            _ => None,
        };
        match unit.filter(|_| end > 0) {
            Some(unit) => {
                let q = Quantity::literal(s, number, unit, Dim::DENSITY)?;
                if q.si > 0.0 {
                    Ok(q)
                } else {
                    Err(Error::new(
                        Code::InvalidQuantity,
                        format!("bad quantity {s:?}: a density must be positive"),
                    )
                    .with("value", s))
                }
            }
            None => Err(Error::new(
                Code::InvalidQuantity,
                format!("bad quantity {s:?}: expected a density such as \"7850 kg/m^3\" or \"7.85 g/cm^3\""),
            )
            .with("value", s)
            .with("kind", "density")),
        }
    }

    /// An electrical quantity's value in SI units (volts, amperes, ohms,
    /// farads, henries, hertz, watts or seconds).
    pub fn si(&self) -> f64 {
        self.si
    }

    /// A quantity from JSON: a string as [`parse`](Self::parse), or a bare
    /// number in the default unit.
    pub fn from_json(v: &Value, default: Unit) -> Result<Quantity> {
        Quantity::from_json_in(v, default, &no_parameters)
    }

    /// [`from_json`](Self::from_json), reading parameters through `lookup`.
    pub fn from_json_in(
        v: &Value,
        default: Unit,
        lookup: &dyn Fn(&str) -> Result<Val>,
    ) -> Result<Quantity> {
        Quantity::from_json_as(v, Field::Length(default), lookup)
    }

    /// A quantity for `field` from JSON: a string, or a bare number.
    pub fn from_json_as(
        v: &Value,
        field: Field,
        lookup: &dyn Fn(&str) -> Result<Val>,
    ) -> Result<Quantity> {
        match v {
            Value::String(s) => Quantity::eval_as(s, field, lookup),
            Value::Number(n) => Quantity::eval_as(&n.to_string(), field, &no_parameters),
            _ => Err(Error::new(
                Code::InvalidQuantity,
                format!("expected a quantity string such as \"10 mm\", got {v}"),
            )
            .with("value", v)),
        }
    }

    pub fn meters(&self) -> f64 {
        debug_assert_eq!(self.dim, Dim::LENGTH);
        self.si
    }

    pub fn radians(&self) -> f64 {
        debug_assert_eq!(self.dim, Dim::ANGLE);
        self.si
    }

    /// A plain number's value.
    pub fn number(&self) -> f64 {
        debug_assert_eq!(self.dim, Dim::NUMBER);
        self.si
    }

    /// What it measures: a length or an angle.
    pub fn dim(&self) -> Dim {
        self.dim
    }

    /// Whether it is a literal (`3 mm`, `-0.5 in`) rather than an
    /// expression.
    pub fn is_literal(&self) -> bool {
        self.literal
    }

    /// The canonical text: `<number> <unit>` for a literal, else the
    /// expression with canonical spacing.
    pub fn text(&self) -> String {
        self.text.clone()
    }

    /// How output reports a quantity: as written, and in meters (`m`) or
    /// radians (`rad`); `expr` marks one that isn't a literal.
    pub fn report(&self) -> Value {
        let mut v = json!({"value": self.text});
        v[self.dim.key()] = json!(self.si);
        if !self.literal {
            v["expr"] = json!(true);
        }
        v
    }
}

impl fmt::Display for Quantity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
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

    #[test]
    fn dimensions_read_back_their_names() {
        for (l, a) in [(0, 0), (1, 0), (0, 1), (2, 0), (3, 0), (1, -1), (-2, 3)] {
            let d = Dim::new([l, a, 0, 0, 0]).unwrap();
            assert_eq!(Dim::parse(&d.name()), Some(d), "{}", d.name());
        }
        for bad in [
            "",
            "mass",
            "angle*length",
            "length^1",
            "length^0",
            "length*length",
            "length^x",
        ] {
            assert_eq!(Dim::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn electrical_dimensions_are_named_and_never_parsed() {
        for (name, d, _) in Dim::ELECTRICAL {
            assert_eq!(d.name(), name);
            assert_eq!(Dim::parse(name), None, "{name}");
        }
        assert_eq!(Dim::VOLTAGE.per(Dim::CURRENT), Some(Dim::RESISTANCE));
        assert_eq!(Dim::VOLTAGE.times(Dim::CURRENT), Some(Dim::POWER));
        assert_eq!(
            Dim::RESISTANCE.times(Dim::CAPACITANCE),
            Some(Dim::TIME),
            "an RC constant is a time"
        );
        assert_eq!(Dim::of([0, 0, 1, -2, 0]).name(), "mass*time^-2");
        assert_eq!(
            Dim::LENGTH.times(Dim::ANGLE).unwrap().name(),
            "length*angle"
        );
        assert_eq!(Dim::POWER.sqrt(), None);
    }

    #[test]
    fn electrical_literals() {
        for (s, d, text, si) in [
            ("3.3 V", Dim::VOLTAGE, "3.3 V", 3.3),
            ("100nF", Dim::CAPACITANCE, "100 nF", 100.0 * 1e-9),
            ("4.7 kOhm", Dim::RESISTANCE, "4.7 kOhm", 4.7 * 1e3),
            ("4.7 kohm", Dim::RESISTANCE, "4.7 kOhm", 4.7 * 1e3),
            ("10 \u{3a9}", Dim::RESISTANCE, "10 Ohm", 10.0),
            ("10 \u{2126}", Dim::RESISTANCE, "10 Ohm", 10.0),
            ("20 mA", Dim::CURRENT, "20 mA", 20.0 * 1e-3),
            ("2.2 \u{b5}H", Dim::INDUCTANCE, "2.2 uH", 2.2 * 1e-6),
            ("2.2 \u{3bc}H", Dim::INDUCTANCE, "2.2 uH", 2.2 * 1e-6),
            ("8 MHz", Dim::FREQUENCY, "8 MHz", 8.0 * 1e6),
            ("1.5 W", Dim::POWER, "1.5 W", 1.5),
            ("10 ns", Dim::TIME, "10 ns", 10.0 * 1e-9),
            ("90 Ohm", Dim::RESISTANCE, "90 Ohm", 90.0),
        ] {
            let q = Quantity::electrical(s, d).unwrap();
            assert_eq!(q.text(), text, "{s}");
            assert_eq!(q.si(), si, "{s}");
            assert_eq!(q.dim(), d, "{s}");
            assert_eq!(q.report(), json!({"value": text, "si": si}), "{s}");
        }
        for (s, d) in [
            ("3.3", Dim::VOLTAGE),
            ("3.3 A", Dim::VOLTAGE),
            ("3.3 mm", Dim::VOLTAGE),
            ("1 kF", Dim::LENGTH),
            ("2 * 1.65 V", Dim::VOLTAGE),
            ("V", Dim::VOLTAGE),
            ("1 TOhm", Dim::RESISTANCE),
        ] {
            let e = Quantity::electrical(s, d).unwrap_err();
            assert_eq!(e.code, Code::InvalidQuantity, "{s}");
        }
    }

    #[test]
    fn densities() {
        let q = Quantity::density("7850 kg/m^3").unwrap();
        assert_eq!((q.text().as_str(), q.si()), ("7850 kg/m^3", 7850.0));
        let q = Quantity::density(" 7.85  g/cm^3").unwrap();
        assert_eq!((q.text().as_str(), q.si()), ("7.85 g/cm^3", 7850.0));
        assert_eq!(Dim::DENSITY.name(), "density");
        assert_eq!(Dim::MASS.name(), "mass");
        assert_eq!(Dim::parse("density"), None);
        for bad in [
            "7850",
            "kg/m^3",
            "7850 kg",
            "0 kg/m^3",
            "-1 g/cm^3",
            "7850 kg/m3",
        ] {
            let e = Quantity::density(bad).unwrap_err();
            assert_eq!(e.code, Code::InvalidQuantity, "{bad}");
        }
    }

    #[test]
    fn electrical_units_reserve_no_parameter_names() {
        // Expressions read only ryucad's units, so a parameter called `s`
        // or `ms` still reads as before.
        let lookup = |name: &str| match name {
            "s" | "ms" | "ohm" => Ok(Val::new(0.002, Dim::LENGTH)),
            _ => no_parameters(name),
        };
        for name in ["s", "ms", "ohm"] {
            let q = Quantity::eval(&format!("2 * {name}"), Unit::Mm, &lookup).unwrap();
            assert_eq!(q.meters(), 0.004, "{name}");
        }
        assert_eq!(UNIT_NAMES.len(), 8);
    }

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
        for s in ["", "mm", "1 furlong", "1e", "1..2 mm", "1 mm mm", "1e999 m"] {
            let e = Quantity::parse(s, Unit::Mm).unwrap_err();
            assert_eq!(e.code, Code::InvalidQuantity, "{s}");
        }
        // A name is an expression reading a parameter that isn't there.
        let e = Quantity::parse("abc", Unit::Mm).unwrap_err();
        assert_eq!(e.code, Code::UnknownParameter);
    }

    #[test]
    fn expressions_give_lengths() {
        let wall = |name: &str| match name {
            "wall" => Ok(Val::new(0.003, Dim::LENGTH)),
            _ => no_parameters(name),
        };
        let x = Quantity::eval("2*wall+1mm", Unit::In, &wall).unwrap();
        assert_eq!(
            (x.text(), x.meters()),
            ("2 * wall + 1 mm".to_owned(), 0.007)
        );
        assert!(!x.is_literal());
        assert_eq!(
            x.report(),
            json!({"value": "2 * wall + 1 mm", "m": 0.007, "expr": true})
        );
        // A sign with a space is an expression now, and still a literal.
        let x = q("- 1 mm");
        assert_eq!(
            (x.text(), x.meters(), x.is_literal()),
            ("-1 mm".to_owned(), -0.001, true)
        );
        // Bare numbers inside an expression are plain numbers.
        let e = Quantity::eval("wall + 3", Unit::Mm, &wall).unwrap_err();
        assert_eq!(e.code, Code::UnitMismatch);
        for s in ["15 deg", "2", "wall * wall"] {
            let e = Quantity::eval(&format!("({s})"), Unit::Mm, &wall).unwrap_err();
            assert_eq!(e.code, Code::UnitMismatch, "{s}");
        }
        let e = Quantity::parse("15 deg", Unit::Mm).unwrap_err();
        assert_eq!(
            (e.code, e.details["kind"].as_str()),
            (Code::UnitMismatch, Some("angle"))
        );
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

    fn angle(s: &str) -> Result<Quantity> {
        Quantity::eval_as(s, Field::Angle, &no_parameters)
    }

    #[test]
    fn angles_are_degrees_unless_they_say_radians() {
        for (s, text, rad) in [
            ("90", "90 deg", 90.0 * DEG),
            ("15 deg", "15 deg", 15.0 * DEG),
            ("15deg", "15 deg", 15.0 * DEG),
            ("0.5 rad", "0.5 rad", 0.5),
            ("-30", "-30 deg", -30.0 * DEG),
        ] {
            let a = angle(s).unwrap();
            assert_eq!(
                (a.text(), a.radians(), a.is_literal()),
                (text.to_owned(), rad, true),
                "{s}"
            );
            assert_eq!(a.dim(), Dim::ANGLE);
        }
        assert_eq!(
            angle("90").unwrap().report(),
            json!({"value": "90 deg", "rad": 90.0 * DEG})
        );
        // An expression of an angle; a length or a number is refused.
        let half = angle("pi / 2 * 1 rad").unwrap();
        assert_eq!(
            (half.text(), half.radians()),
            ("pi / 2 * 1 rad".to_owned(), core::f64::consts::FRAC_PI_2)
        );
        assert!(!half.is_literal());
        for s in ["10 mm", "(2)", "1 furlong"] {
            assert_ne!(angle(s).unwrap_err().code, Code::Internal, "{s}");
        }
        let e = angle("10 mm").unwrap_err();
        assert_eq!(
            (e.code, e.details["kind"].as_str()),
            (Code::UnitMismatch, Some("length"))
        );
        let x = Quantity::from_json_as(&json!(45), Field::Angle, &no_parameters).unwrap();
        assert_eq!(x.text(), "45 deg");
    }

    #[test]
    fn plain_numbers() {
        let n = |s: &str| Quantity::eval_as(s, Field::Number, &no_parameters);
        let six = n("6").unwrap();
        assert_eq!(
            (six.text(), six.number(), six.dim()),
            ("6".to_owned(), 6.0, Dim::NUMBER)
        );
        assert!(six.is_literal());
        assert_eq!(six.report(), json!({"value": "6", "n": 6.0}));
        assert_eq!(n("2 * 3").unwrap().number(), 6.0);
        let x = Quantity::from_json_as(&json!(4), Field::Number, &no_parameters).unwrap();
        assert_eq!(x.text(), "4");
        for (s, code) in [("6 mm", Code::UnitMismatch), ("6 deg", Code::UnitMismatch)] {
            assert_eq!(n(s).unwrap_err().code, code, "{s}");
        }
    }

    #[test]
    fn dimensions_combine() {
        let area = Dim::LENGTH.times(Dim::LENGTH).unwrap();
        assert_eq!(area.name(), "length^2");
        assert_eq!(area.sqrt(), Some(Dim::LENGTH));
        assert_eq!(Dim::LENGTH.sqrt(), None);
        assert_eq!(area.per(Dim::LENGTH), Some(Dim::LENGTH));
        assert_eq!(
            Dim::LENGTH.per(Dim::ANGLE).unwrap().name(),
            "length*angle^-1"
        );
        assert_eq!(Dim::NUMBER.name(), "number");
        assert_eq!(Dim::ANGLE.pow(3).unwrap().name(), "angle^3");
        assert_eq!(Dim::LENGTH.pow(17), None);
        assert_eq!(
            [
                Dim::LENGTH.key(),
                Dim::ANGLE.key(),
                Dim::NUMBER.key(),
                area.key()
            ],
            ["m", "rad", "n", "si"]
        );
    }

    #[test]
    fn literal_units() {
        assert_eq!(literal_unit("mm"), Some((0.001, Dim::LENGTH)));
        assert_eq!(literal_unit("rad"), Some((1.0, Dim::ANGLE)));
        assert_eq!(literal_unit("deg"), Some((DEG, Dim::ANGLE)));
        assert_eq!(DEG * 180.0, core::f64::consts::PI);
        assert_eq!(literal_unit("furlong"), None);
        for u in UNIT_NAMES {
            assert!(literal_unit(u).is_some(), "{u}");
        }
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
