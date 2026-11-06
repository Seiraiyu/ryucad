//! The part vocabulary (design 347): parameter names with a type, read
//! from the text a library component keeps (its `parameters` stay text as
//! written). Names outside it stay text, compared exactly. Ratios and
//! temperatures are kinds of the vocabulary, not `ryu-doc` dimensions:
//! temperature in °C is affine, so it isn't one.

use crate::dec::Dec;
use crate::error::{Code, Error};
use ryu_doc::units::{Dim, Quantity};
use serde::Serialize;

/// What a vocabulary name's values are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// An electrical quantity of one dimension.
    Quantity(Dim),
    /// A ratio (`1%`, `100 ppm`) or a quantity (`±0.25 pF`).
    Tolerance,
    /// A ratio per kelvin (`100 ppm/K`, `100 ppm/°C`).
    TemperatureCoefficient,
    /// One of [`DIELECTRICS`] (`NP0` is `C0G`).
    Dielectric,
    /// Text compared without regard to case (`0402`, `SOT-23-5`).
    Package,
    /// A range in °C (`-55 °C to 125 °C`), or one temperature.
    Temperature,
    /// `smd`, `through` or `other`.
    Mounting,
}

/// The vocabulary: each name, its kind and an example of its values.
/// Later stages add names; these never change.
pub const VOCABULARY: [(&str, Kind, &str); 13] = [
    ("resistance", Kind::Quantity(Dim::RESISTANCE), "4.7 kOhm"),
    ("capacitance", Kind::Quantity(Dim::CAPACITANCE), "100 nF"),
    ("inductance", Kind::Quantity(Dim::INDUCTANCE), "2.2 uH"),
    ("voltage_rating", Kind::Quantity(Dim::VOLTAGE), "16 V"),
    ("current_rating", Kind::Quantity(Dim::CURRENT), "3 A"),
    ("power_rating", Kind::Quantity(Dim::POWER), "62.5 mW"),
    ("frequency", Kind::Quantity(Dim::FREQUENCY), "12 MHz"),
    ("tolerance", Kind::Tolerance, "1%"),
    (
        "temperature_coefficient",
        Kind::TemperatureCoefficient,
        "100 ppm/K",
    ),
    ("dielectric", Kind::Dielectric, "X7R"),
    ("package", Kind::Package, "0402"),
    (
        "operating_temperature",
        Kind::Temperature,
        "-55 °C to 125 °C",
    ),
    ("mounting", Kind::Mounting, "smd"),
];

/// How sources spell each vocabulary name, letters and digits only in
/// lower case (`Voltage - Rated` is `voltagerated`). One table for KiCad
/// import and every adapter, so a part from any source has the same
/// names. Bare `voltage`, `current` and `power` are left out: on a
/// regulator they mean its output, not a rating.
const SPELLINGS: [(&str, &[&str]); 13] = [
    ("resistance", &["resistance"]),
    ("capacitance", &["capacitance"]),
    ("inductance", &["inductance"]),
    (
        "voltage_rating",
        &[
            "voltagerating",
            "voltagerated",
            "ratedvoltage",
            "voltageratingdc",
        ],
    ),
    (
        "current_rating",
        &["currentrating", "currentrated", "ratedcurrent"],
    ),
    (
        "power_rating",
        &["powerrating", "powerrated", "ratedpower", "powerwatts"],
    ),
    ("frequency", &["frequency"]),
    ("tolerance", &["tolerance"]),
    (
        "temperature_coefficient",
        &["temperaturecoefficient", "tempco"],
    ),
    (
        "dielectric",
        &[
            "dielectric",
            "dielectricmaterial",
            "temperaturecharacteristic",
        ],
    ),
    ("package", &["package", "packagecase", "casepackage"]),
    (
        "operating_temperature",
        &[
            "operatingtemperature",
            "operatingtemperaturerange",
            "temperaturerange",
        ],
    ),
    ("mounting", &["mounting", "mountingtype"]),
];

/// The vocabulary name a source's parameter name stands for, if any.
pub fn name_for(source_name: &str) -> Option<&'static str> {
    let squashed: String = source_name
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect();
    SPELLINGS
        .iter()
        .find(|(_, s)| s.contains(&squashed.as_str()))
        .map(|(n, _)| *n)
}

pub const DIELECTRICS: [&str; 8] = ["C0G", "X5R", "X6S", "X7R", "X7S", "X8R", "Y5V", "Z5U"];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Mounting {
    Smd,
    Through,
    Other,
}

/// A value as its kind reads it. Quantities are SI values, ratios
/// fractions (`1%` is 0.01), temperatures °C.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    Quantity { dim: Dim, si: Dec },
    Ratio(Dec),
    Absolute { dim: Dim, si: Dec },
    PerKelvin(Dec),
    Dielectric(&'static str),
    Package(String),
    Range { low: Dec, high: Dec },
    Mounting(Mounting),
}

/// Every vocabulary name, in the vocabulary's order.
pub fn names() -> Vec<&'static str> {
    VOCABULARY.iter().map(|(n, _, _)| *n).collect()
}

/// The kind of a vocabulary name.
pub fn kind_of(name: &str) -> Option<Kind> {
    VOCABULARY
        .iter()
        .find(|(n, _, _)| *n == name)
        .map(|(_, k, _)| *k)
}

/// An electrical quantity's exact SI value, from `ryu-doc`'s reading of it:
/// `100nF`, `0.1 uF` and `100 nF` alike. `None` for text that isn't a
/// quantity of `dim`.
pub fn quantity(text: &str, dim: Dim) -> Option<Dec> {
    let canonical = Quantity::electrical(text, dim).ok()?.text();
    let (number, unit) = canonical.split_once(' ')?;
    let (_, _, symbol) = Dim::ELECTRICAL.iter().find(|(_, d, _)| *d == dim)?;
    let prefix = match unit.strip_suffix(symbol)? {
        "p" => -12,
        "n" => -9,
        "u" => -6,
        "m" => -3,
        "" => 0,
        "k" => 3,
        "M" => 6,
        "G" => 9,
        _ => return None,
    };
    Dec::parse(number)?.shift(prefix)
}

/// A vocabulary parameter's value: `None` for a name outside the
/// vocabulary, else the value or `parameter_invalid` naming the
/// parameter and the text.
pub fn read(name: &str, text: &str) -> Option<Result<Value, Error>> {
    let (name, kind, example) = VOCABULARY.iter().find(|(n, _, _)| *n == name)?;
    Some(read_kind(*kind, text).ok_or_else(|| {
        Error::new(
            Code::ParameterInvalid,
            format!(
                "{name} is {} such as {example:?}; {text:?} isn't one",
                describe(*kind)
            ),
        )
        .with("parameter", *name)
        .with("value", text)
    }))
}

fn describe(k: Kind) -> String {
    match k {
        Kind::Quantity(d) => format!("a {}", d.name()),
        Kind::Tolerance => "a tolerance (a percentage, ppm or a quantity)".into(),
        Kind::TemperatureCoefficient => "a temperature coefficient".into(),
        Kind::Dielectric => format!("a dielectric ({})", DIELECTRICS.join(", ")),
        Kind::Package => "a package".into(),
        Kind::Temperature => "a temperature range in °C".into(),
        Kind::Mounting => "a mounting (smd, through or other)".into(),
    }
}

/// `text` as a value of `kind`, if it reads.
pub fn read_kind(kind: Kind, text: &str) -> Option<Value> {
    let t = text.trim();
    match kind {
        Kind::Quantity(dim) => Some(Value::Quantity {
            dim,
            si: quantity(t, dim)?,
        }),
        Kind::Tolerance => tolerance(t),
        Kind::TemperatureCoefficient => per_kelvin(t),
        Kind::Dielectric => dielectric(t).map(Value::Dielectric),
        Kind::Package => (!t.is_empty()).then(|| Value::Package(t.to_lowercase())),
        Kind::Temperature => range(t),
        Kind::Mounting => match t.to_ascii_lowercase().as_str() {
            "smd" => Some(Value::Mounting(Mounting::Smd)),
            "through" => Some(Value::Mounting(Mounting::Through)),
            "other" => Some(Value::Mounting(Mounting::Other)),
            _ => None,
        },
    }
}

/// `t` less a `±`, `+/-` or `+-` before it, and whether it had one.
fn unsigned(t: &str) -> (&str, bool) {
    for p in ["±", "+/-", "+-"] {
        if let Some(r) = t.strip_prefix(p) {
            return (r.trim_start(), true);
        }
    }
    (t, false)
}

fn compact(t: &str) -> String {
    t.chars().filter(|c| !c.is_whitespace()).collect()
}

fn tolerance(t: &str) -> Option<Value> {
    let (m, _) = unsigned(t);
    let c = compact(m);
    if let Some(n) = c.strip_suffix('%') {
        return Some(Value::Ratio(Dec::parse(n)?.abs().shift(-2)?));
    }
    if let Some(n) = c.strip_suffix("ppm") {
        return Some(Value::Ratio(Dec::parse(n)?.abs().shift(-6)?));
    }
    Dim::ELECTRICAL.iter().find_map(|(_, dim, _)| {
        Some(Value::Absolute {
            dim: *dim,
            si: quantity(m, *dim)?.abs(),
        })
    })
}

fn per_kelvin(t: &str) -> Option<Value> {
    let (m, magnitude) = unsigned(t);
    let c = compact(m);
    for (suffix, by) in [
        ("ppm/K", -6),
        ("ppm/°C", -6),
        ("ppm/℃", -6),
        ("ppm/C", -6),
        ("%/K", -2),
        ("%/°C", -2),
    ] {
        if let Some(n) = c.strip_suffix(suffix) {
            let v = Dec::parse(n)?.shift(by)?;
            return Some(Value::PerKelvin(if magnitude { v.abs() } else { v }));
        }
    }
    None
}

fn dielectric(t: &str) -> Option<&'static str> {
    let u = t.to_ascii_uppercase();
    if u == "NP0" || u == "NPO" {
        return Some("C0G");
    }
    DIELECTRICS.iter().find(|d| **d == u).copied()
}

fn celsius(t: &str) -> Option<Dec> {
    let t = t.trim();
    let n = ["°C", "℃", "C"]
        .iter()
        .find_map(|s| t.strip_suffix(s))
        .unwrap_or(t);
    Dec::parse(n.trim())
}

fn range(t: &str) -> Option<Value> {
    for sep in [" to ", "~", "..", "…"] {
        if let Some((a, b)) = t.split_once(sep) {
            let (low, high) = (celsius(a)?, celsius(b)?);
            return (low <= high).then_some(Value::Range { low, high });
        }
    }
    let one = celsius(t)?;
    Some(Value::Range {
        low: one,
        high: one,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(name: &str, text: &str) -> Value {
        read(name, text)
            .unwrap_or_else(|| panic!("{name} is in the vocabulary"))
            .unwrap_or_else(|e| panic!("{name}={text}: {e}"))
    }

    #[test]
    fn values_read_by_value_not_text() {
        assert_eq!(v("capacitance", "100nF"), v("capacitance", "0.1 uF"));
        assert_eq!(v("capacitance", "100 nF"), v("capacitance", "100000 pF"));
        assert_eq!(v("resistance", "4.7 kOhm"), v("resistance", "4700 \u{3a9}"));
        assert_eq!(v("tolerance", "±5%"), v("tolerance", "50000 ppm"));
        assert_eq!(
            v("tolerance", "1 %"),
            Value::Ratio(Dec::new(1, -2).unwrap())
        );
        assert_eq!(
            v("tolerance", "±0.25 pF"),
            Value::Absolute {
                dim: Dim::CAPACITANCE,
                si: Dec::new(25, -14).unwrap()
            }
        );
        assert_eq!(
            v("temperature_coefficient", "100 ppm/K"),
            v("temperature_coefficient", "100ppm/°C")
        );
        assert_eq!(
            v("temperature_coefficient", "-200 ppm/K"),
            Value::PerKelvin(Dec::new(-2, -4).unwrap())
        );
        assert_eq!(v("dielectric", "np0"), Value::Dielectric("C0G"));
        assert_eq!(v("package", " 0402 "), v("package", "0402"));
        assert_eq!(v("package", "SOT-23-5"), Value::Package("sot-23-5".into()));
        assert_eq!(
            v("operating_temperature", "-55 °C to 125 °C"),
            v("operating_temperature", "-55°C~+125°C")
        );
        assert_eq!(
            v("operating_temperature", "85 °C"),
            Value::Range {
                low: Dec::new(85, 0).unwrap(),
                high: Dec::new(85, 0).unwrap()
            }
        );
        assert_eq!(v("mounting", "SMD"), Value::Mounting(Mounting::Smd));
        assert!(read("colour", "red").is_none());
        assert_eq!(names().len(), VOCABULARY.len());
    }

    #[test]
    fn values_that_dont_read_are_parameter_invalid() {
        for (n, t) in [
            ("capacitance", "100 nH"),
            ("capacitance", "lots"),
            ("tolerance", "5"),
            ("dielectric", "X9Z"),
            ("package", "  "),
            ("operating_temperature", "125 °C to -55 °C"),
            ("mounting", "glued"),
            ("voltage_rating", ""),
        ] {
            let e = read(n, t).unwrap().unwrap_err();
            assert_eq!(e.code, Code::ParameterInvalid, "{n}={t}");
            assert_eq!(e.details["parameter"], n);
            assert_eq!(e.details["value"], t);
        }
    }

    #[test]
    fn quantities_are_exact_si_values() {
        assert_eq!(
            quantity("3.3 V", Dim::VOLTAGE),
            quantity("3300 mV", Dim::VOLTAGE)
        );
        assert_eq!(quantity("4.7 uH", Dim::INDUCTANCE), Dec::new(47, -7));
        assert_eq!(quantity("0 V", Dim::VOLTAGE), Some(Dec::ZERO));
        assert_eq!(quantity("3.3 A", Dim::VOLTAGE), None);
    }

    #[test]
    fn sources_spellings_map_onto_the_vocabulary() {
        for (spelt, name) in [
            ("Voltage - Rated", "voltage_rating"),
            ("Voltage Rating", "voltage_rating"),
            ("Rated Current", "current_rating"),
            ("Capacitance", "capacitance"),
            ("Package / Case", "package"),
            ("Tolerance", "tolerance"),
            ("Temperature Coefficient", "temperature_coefficient"),
            ("Operating Temperature", "operating_temperature"),
            ("Mounting Type", "mounting"),
        ] {
            assert_eq!(name_for(spelt), Some(name), "{spelt}");
        }
        for (name, _, _) in VOCABULARY {
            assert_eq!(name_for(name), Some(name), "{name} maps to itself");
        }
        for other in ["Colour", "Voltage", "Power", "ki_keywords", ""] {
            assert_eq!(name_for(other), None, "{other}");
        }
    }
}
