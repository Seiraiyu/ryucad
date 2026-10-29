//! Lengths agents type: `"25 mm"`, `"100 mil"`, or a bare number in the
//! project's default unit (millimetres outside a project), read exactly
//! from the decimal text and rounded to a nanometre once, half to even
//! (`ryu_board::geom::decimal`).

use crate::error::{Code, Error, Result};
use crate::units::Unit;
use ryu_board::geom::decimal::scaled;
use ryu_board::geom::{COORD_LIMIT, Nm, Point, Rect};

/// Nanometres in one of each unit a length may be typed in.
const UNITS: [(&str, i128); 9] = [
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

/// Nanometres in one of a project's default unit.
fn per(unit: Unit) -> i128 {
    UNITS
        .iter()
        .find(|(n, _)| *n == unit.as_str())
        .map(|(_, v)| *v)
        .expect("every default unit is a length unit")
}

fn invalid(text: &str, why: &str) -> Error {
    Error::new(
        Code::InvalidQuantity,
        format!("{text:?} isn't a length: {why}"),
    )
    .with("quantity", text)
}

/// A length typed as text, in nanometres.
pub fn length(text: &str, default: Unit) -> Result<Nm> {
    let t = text.trim();
    let split = t
        .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-' || c == '+'))
        .unwrap_or(t.len());
    let (number, unit) = (t[..split].trim(), t[split..].trim());
    let per_unit = if unit.is_empty() {
        per(default)
    } else {
        UNITS
            .iter()
            .find(|(n, _)| *n == unit)
            .map(|(_, v)| *v)
            .ok_or_else(|| invalid(text, "units are nm, um, mm, cm, m, mil, in and ft"))?
    };
    scaled(number, per_unit, COORD_LIMIT)
        .map(|e| e.value)
        .map_err(|e| invalid(text, &format!("{e:?}")))
}

/// A window `X0,Y0,X1,Y1` of four lengths, its corners in either order.
pub fn window(text: &str, default: Unit) -> Result<Rect> {
    let parts: Vec<&str> = text.split(',').collect();
    if parts.len() != 4 {
        return Err(Error::new(
            Code::InvalidParam,
            format!("{text:?} isn't a window: four lengths X0,Y0,X1,Y1"),
        )
        .with("param", "window"));
    }
    let v: Vec<Nm> = parts
        .iter()
        .map(|p| length(p, default))
        .collect::<Result<_>>()?;
    if v[0] == v[2] || v[1] == v[3] {
        return Err(Error::new(
            Code::InvalidParam,
            format!("{text:?} has no width or no height"),
        )
        .with("param", "window"));
    }
    Ok(Rect::new(Point::new(v[0], v[1]), Point::new(v[2], v[3])))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lengths_are_exact() {
        assert_eq!(length("25 mm", Unit::In).unwrap(), 25_000_000);
        assert_eq!(length("100mil", Unit::Mm).unwrap(), 2_540_000);
        assert_eq!(length("1.5", Unit::Mm).unwrap(), 1_500_000);
        assert_eq!(length("1.5", Unit::In).unwrap(), 38_100_000);
        assert_eq!(length(" -0.0000005 mm", Unit::Mm).unwrap(), 0);
        assert_eq!(length("0.0000015 mm", Unit::Mm).unwrap(), 2);
        assert_eq!(length("3 µm", Unit::Mm).unwrap(), 3_000);
        let e = length("3 furlongs", Unit::Mm).unwrap_err();
        assert_eq!(e.code, Code::InvalidQuantity);
        assert_eq!(
            length("", Unit::Mm).unwrap_err().code,
            Code::InvalidQuantity
        );
        assert_eq!(
            length("2000 m", Unit::Mm).unwrap_err().code,
            Code::InvalidQuantity
        );
    }

    #[test]
    fn windows_take_four_lengths() {
        let w = window("10,20 mm, 0,0", Unit::Mm).unwrap();
        assert_eq!(
            w,
            Rect::new(Point::new(0, 0), Point::new(10_000_000, 20_000_000))
        );
        assert_eq!(
            window("1,2,3", Unit::Mm).unwrap_err().code,
            Code::InvalidParam
        );
        assert_eq!(
            window("1,2,1,3", Unit::Mm).unwrap_err().code,
            Code::InvalidParam
        );
    }
}
