//! DigiKey's parameters onto the vocabulary: its names through
//! `param::name_for` (and the few it spells its own way), its value text
//! cleaned to what the vocabulary reads (`0402 (1005 Metric)` is `0402`,
//! `-55°C ~ 125°C` reads as a range, `Surface Mount` is `smd`), and kept
//! only where it reads as its kind. A ceramic's `Temperature Coefficient`
//! is its dielectric (`X7R`).

use crate::param::{self, Kind};

/// DigiKey's names `param::name_for` doesn't take, letters and digits in
/// lower case.
const OWN: [(&str, &str); 3] = [
    ("currentratingamps", "current_rating"),
    ("voltagebreakdownmin", "voltage_rating"),
    ("frequencytolerance", "tolerance"),
];

fn name_of(parameter: &str) -> Option<&'static str> {
    param::name_for(parameter).or_else(|| {
        let s: String = parameter
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .map(|c| c.to_ascii_lowercase())
            .collect();
        OWN.iter().find(|(k, _)| *k == s).map(|(_, n)| *n)
    })
}

fn clean(kind: Kind, value: &str) -> Option<String> {
    let v = value.trim();
    if v.is_empty() || v == "-" || v == "*" {
        return None;
    }
    // DigiKey lists the same value in two forms (`0.063W, 1/16W`).
    let first = v.split(',').next()?.trim();
    Some(match kind {
        Kind::Package => first.split(" (").next()?.trim().to_owned(),
        Kind::Mounting => {
            let l = first.to_ascii_lowercase();
            if l.contains("surface") {
                "smd".to_owned()
            } else if l.contains("through") {
                "through".to_owned()
            } else {
                "other".to_owned()
            }
        }
        // `µ` and DigiKey's plural `Ohms` as ryu-doc reads them.
        _ => first.replace('µ', "u").replace("Ohms", "Ohm"),
    })
}

/// `(vocabulary name, text)` for one of DigiKey's parameters, if it maps
/// and reads.
pub fn vocabulary(parameter: &str, value: &str) -> Option<(&'static str, String)> {
    let name = name_of(parameter)?;
    let kind = param::kind_of(name)?;
    let text = clean(kind, value)?;
    match param::read(name, &text)? {
        Ok(_) => Some((name, text)),
        Err(_) if name == "temperature_coefficient" => {
            param::read("dielectric", &text)?.ok()?;
            Some(("dielectric", text))
        }
        Err(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digikeys_parameters_map_onto_the_vocabulary() {
        let v = |p: &str, t: &str| vocabulary(p, t);
        assert_eq!(
            v("Capacitance", "0.1 µF"),
            Some(("capacitance", "0.1 uF".into()))
        );
        assert_eq!(
            v("Voltage - Rated", "16V"),
            Some(("voltage_rating", "16V".into()))
        );
        assert_eq!(v("Tolerance", "±10%"), Some(("tolerance", "±10%".into())));
        assert_eq!(
            v("Temperature Coefficient", "X7R"),
            Some(("dielectric", "X7R".into()))
        );
        assert_eq!(
            v("Temperature Coefficient", "±100ppm/°C"),
            Some(("temperature_coefficient", "±100ppm/°C".into()))
        );
        assert_eq!(
            v("Package / Case", "0402 (1005 Metric)"),
            Some(("package", "0402".into()))
        );
        assert_eq!(
            v("Operating Temperature", "-55°C ~ 125°C"),
            Some(("operating_temperature", "-55°C ~ 125°C".into()))
        );
        assert_eq!(
            v("Mounting Type", "Surface Mount, MLCC"),
            Some(("mounting", "smd".into()))
        );
        assert_eq!(
            v("Mounting Type", "Through Hole"),
            Some(("mounting", "through".into()))
        );
        assert_eq!(
            v("Power (Watts)", "0.063W, 1/16W"),
            Some(("power_rating", "0.063W".into()))
        );
        assert_eq!(
            v("Current Rating (Amps)", "2A"),
            Some(("current_rating", "2A".into()))
        );
        assert_eq!(
            v("Resistance", "100 kOhms"),
            Some(("resistance", "100 kOhm".into()))
        );
        assert_eq!(v("Capacitance", "-"), None);
        assert_eq!(v("Series", "CL"), None);
        assert_eq!(v("Ratings", "AEC-Q200"), None);
    }
}
