//! jlcparts' normalized attributes (`{format, primary, values: {key:
//! [value, unit]}}`) as the vocabulary's text: the name by
//! `param::name_for` and a few of JLC's own names, the value from the
//! primary entry's number in SI units, written by `param`'s writers. What
//! doesn't read for its name is left out, never guessed.

use crate::category::Category;
use crate::dec::Dec;
use crate::param::{self, Kind};
use ryu_doc::units::Dim;
use serde_json::Value;

/// JLC's names the shared table leaves out, by category: on a resistor
/// `Power` is its rating, on a ceramic capacitor the temperature
/// coefficient is the dielectric's code.
fn jlc_name(name: &str, cat: Category) -> Option<&'static str> {
    let squashed: String = name
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect();
    match (squashed.as_str(), cat) {
        ("power" | "powerwatts", Category::Resistor) => Some("power_rating"),
        ("temperaturecoefficient", Category::Capacitor) => Some("dielectric"),
        ("powerwatts", _) => None,
        _ => param::name_for(name),
    }
}

/// The primary entry `[value, unit]`.
fn primary(v: &Value) -> Option<(&Value, &str)> {
    let key = v.get("primary").or_else(|| v.get("default"))?.as_str()?;
    let e = v.get("values")?.get(key)?.as_array()?;
    Some((e.first()?, e.get(1)?.as_str()?))
}

/// A string attribute's text (`Basic`, `0402`, a maker's name).
pub fn text(v: &Value) -> Option<String> {
    let (x, _) = primary(v)?;
    x.as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty() && *s != "-")
        .map(str::to_owned)
}

fn unit_dim(unit: &str) -> Option<Dim> {
    Some(match unit {
        "resistance" => Dim::RESISTANCE,
        "capacitance" => Dim::CAPACITANCE,
        "inductance" => Dim::INDUCTANCE,
        "voltage" => Dim::VOLTAGE,
        "current" => Dim::CURRENT,
        "power" => Dim::POWER,
        "frequency" => Dim::FREQUENCY,
        _ => return None,
    })
}

/// `(vocabulary name, text)` for one attribute, if it maps and reads.
pub fn vocabulary(name: &str, v: &Value, cat: Category) -> Option<(&'static str, String)> {
    let vocab = jlc_name(name, cat)?;
    let kind = param::kind_of(vocab)?;
    let out = match kind {
        Kind::Quantity(dim) => {
            let (x, unit) = primary(v)?;
            if unit_dim(unit)? != dim {
                return None;
            }
            param::si_text(x.as_f64()?, dim)?
        }
        Kind::Tolerance => {
            let (x, unit) = primary(v)?;
            match unit {
                // Values in percent, written exactly from their text.
                "percentage" | "tolerance" => {
                    let x = x.as_f64().filter(|x| x.is_finite())?;
                    format!("{}%", Dec::parse(&format!("{x}"))?.abs())
                }
                _ => return None,
            }
        }
        Kind::Temperature => {
            let vals = v.get("values")?;
            let n = |k: &str| vals.get(k)?.get(0)?.as_f64();
            match (n("temperature min"), n("temperature max")) {
                (Some(a), Some(b)) => param::range_text(a, b)?,
                _ => {
                    let (x, unit) = primary(v)?;
                    if unit != "temperature" {
                        return None;
                    }
                    let t = x.as_f64()?;
                    param::range_text(t, t)?
                }
            }
        }
        Kind::Dielectric | Kind::Package | Kind::Mounting | Kind::TemperatureCoefficient => {
            text(v)?
        }
    };
    // Only what reads back as its kind is kept.
    param::read_kind(kind, &out)?;
    Some((vocab, out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn a(primary: &str, v: Value, unit: &str) -> Value {
        json!({"format": format!("${{{primary}}}"), "primary": primary, "values": {primary: [v, unit]}})
    }

    #[test]
    fn attributes_become_vocabulary_text() {
        let cap = Category::Capacitor;
        let res = Category::Resistor;
        assert_eq!(
            vocabulary(
                "Capacitance",
                &a("capacitance", json!(1e-7), "capacitance"),
                cap
            ),
            Some(("capacitance", "100 nF".into()))
        );
        assert_eq!(
            vocabulary("Voltage Rated", &a("voltage", json!(16.0), "voltage"), cap),
            Some(("voltage_rating", "16 V".into()))
        );
        // A resistor's "Power" is its rating; a regulator's isn't read.
        assert_eq!(
            vocabulary("Power(Watts)", &a("power", json!(0.0625), "power"), res),
            Some(("power_rating", "62.5 mW".into()))
        );
        assert_eq!(
            vocabulary(
                "Power(Watts)",
                &a("power", json!(0.5), "power"),
                Category::Regulator
            ),
            None
        );
        assert_eq!(
            vocabulary("Tolerance", &a("tolerance", json!(1.0), "percentage"), res),
            Some(("tolerance", "1%".into()))
        );
        assert_eq!(
            vocabulary("Tolerance", &a("tolerance", json!(0.3), "percentage"), res),
            Some(("tolerance", "0.3%".into()))
        );
        // A ceramic's temperature coefficient is its dielectric.
        assert_eq!(
            vocabulary(
                "Temperature Coefficient",
                &a("default", json!("X7R"), "string"),
                cap
            ),
            Some(("dielectric", "X7R".into()))
        );
        assert_eq!(
            vocabulary("Package", &a("default", json!("0402"), "string"), cap),
            Some(("package", "0402".into()))
        );
        let t = json!({"format": "", "primary": "temperature min",
                       "values": {"temperature min": [-55.0, "temperature"], "temperature max": [125.0, "temperature"]}});
        assert_eq!(
            vocabulary("Operating Temperature", &t, res),
            Some(("operating_temperature", "-55 °C to 125 °C".into()))
        );
        // What doesn't read for its name is left out, never guessed.
        assert_eq!(
            vocabulary(
                "Capacitance",
                &a("capacitance", json!("NaN"), "capacitance"),
                cap
            ),
            None
        );
        assert_eq!(
            vocabulary(
                "Capacitance",
                &a("capacitance", json!(1e-7), "voltage"),
                cap
            ),
            None
        );
        assert_eq!(
            vocabulary(
                "Temperature Coefficient",
                &a("default", json!("Y9Q"), "string"),
                cap
            ),
            None
        );
        assert_eq!(
            vocabulary("Output Voltage", &a("voltage", json!(3.3), "voltage"), cap),
            None
        );
        assert_eq!(vocabulary("Package", &json!("0402"), cap), None);
        assert_eq!(
            text(&a("default", json!("Basic"), "string")).as_deref(),
            Some("Basic")
        );
        assert_eq!(text(&a("default", json!("-"), "string")), None);
    }
}
