//! Units and measures (ISO 10303-41), and the units of a representation
//! context in meters and radians.

use super::args::{Args, Id};
use super::{Entities, Entity, wrong};
use crate::ImportError;
use crate::part21::{Param, Part};

/// What a unit measures.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quantity {
    Length,
    PlaneAngle,
    SolidAngle,
    /// Anything else (mass, time, …), or not said.
    Other,
}

/// A named unit.
#[derive(Clone, Debug, PartialEq)]
pub enum Unit {
    /// `SI_UNIT`: an SI prefix (`MILLI`, …) and name (`METRE`, `RADIAN`,
    /// …), as written.
    Si {
        quantity: Quantity,
        prefix: Option<Box<str>>,
        name: Box<str>,
    },
    /// `CONVERSION_BASED_UNIT`: a name (`INCH`, `DEGREE`, …) and a
    /// measure with a unit that says how big one is.
    Conversion {
        quantity: Quantity,
        name: Box<str>,
        factor: Id,
    },
}

/// A context's units, as factors to meters and radians.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Units {
    /// Meters in one of the context's length units.
    pub length: f64,
    /// Radians in one of its plane angle units, if it has one.
    pub angle: Option<f64>,
    /// Its distance accuracy (`distance_accuracy_value`) in meters, if it
    /// gives one.
    pub uncertainty: Option<f64>,
}

/// Units known by name, with their exact sizes: a conversion-based unit of
/// one of these names, within a millionth of its size, is taken as exactly
/// it, so files that round `25.4` or `π/180` all read alike.
const KNOWN: [(&str, f64); 6] = [
    ("INCH", 0.0254),
    ("FOOT", 0.3048),
    ("YARD", 0.9144),
    ("MILE", 1609.344),
    ("MIL", 2.54e-5),
    ("DEGREE", std::f64::consts::PI / 180.0),
];

/// How deep conversion-based units may nest before it's taken for a
/// cycle.
const MAX_DEPTH: usize = 8;

/// A simple record of a measure, or `None`.
pub(super) fn simple(id: Id, name: &str, params: &[Param]) -> Option<Result<Entity, ImportError>> {
    Some(match name {
        "LENGTH_MEASURE_WITH_UNIT" | "PLANE_ANGLE_MEASURE_WITH_UNIT" | "MEASURE_WITH_UNIT" => {
            (|| {
                let mut a = Args::new(id, params, 2)?;
                Ok(Entity::Measure {
                    value: a.real()?,
                    unit: a.id()?,
                })
            })()
        }
        "UNCERTAINTY_MEASURE_WITH_UNIT" => (|| {
            let mut a = Args::new(id, params, 4)?;
            let value = a.real()?;
            let unit = a.id()?;
            let name = a.string()?;
            Ok(Entity::Uncertainty { value, unit, name })
        })(),
        "SI_UNIT" => (|| {
            let mut a = Args::new(id, params, 3)?;
            a.skip();
            si(&mut a, Quantity::Other).map(Entity::Unit)
        })(),
        _ => return None,
    })
}

/// A complex instance of a unit, or `None`.
pub(super) fn complex(id: Id, parts: &[Part]) -> Option<Result<Entity, ImportError>> {
    let part = |name: &str| parts.iter().find(|p| &*p.name == name);
    if part("NAMED_UNIT").is_none() && part("SI_UNIT").is_none() {
        return None;
    }
    let quantity = if part("LENGTH_UNIT").is_some() {
        Quantity::Length
    } else if part("PLANE_ANGLE_UNIT").is_some() {
        Quantity::PlaneAngle
    } else if part("SOLID_ANGLE_UNIT").is_some() {
        Quantity::SolidAngle
    } else {
        Quantity::Other
    };
    Some((|| {
        if let Some(si_part) = part("SI_UNIT") {
            let mut a = Args::new(id, &si_part.params, 2)?;
            return si(&mut a, quantity).map(Entity::Unit);
        }
        if let Some(c) = part("CONVERSION_BASED_UNIT") {
            let mut a = Args::new(id, &c.params, 2)?;
            return Ok(Entity::Unit(Unit::Conversion {
                quantity,
                name: a.string()?,
                factor: a.id()?,
            }));
        }
        Ok(Entity::Other("NAMED_UNIT".into()))
    })())
}

/// An SI unit's `prefix, name`; `quantity` from its other parts, else from
/// its name.
fn si(a: &mut Args, quantity: Quantity) -> Result<Unit, ImportError> {
    let prefix = a.opt_enumeration()?.map(Into::into);
    let name: Box<str> = a.enumeration()?.into();
    let quantity = match (quantity, &*name) {
        (Quantity::Other, "METRE" | "METER") => Quantity::Length,
        (Quantity::Other, "RADIAN") => Quantity::PlaneAngle,
        (Quantity::Other, "STERADIAN") => Quantity::SolidAngle,
        (q, _) => q,
    };
    Ok(Unit::Si {
        quantity,
        prefix,
        name,
    })
}

fn prefix_factor(prefix: &str) -> Option<f64> {
    Some(match prefix {
        "EXA" => 1e18,
        "PETA" => 1e15,
        "TERA" => 1e12,
        "GIGA" => 1e9,
        "MEGA" => 1e6,
        "KILO" => 1e3,
        "HECTO" => 1e2,
        "DECA" => 1e1,
        "DECI" => 1e-1,
        "CENTI" => 1e-2,
        "MILLI" => 1e-3,
        "MICRO" => 1e-6,
        "NANO" => 1e-9,
        "PICO" => 1e-12,
        "FEMTO" => 1e-15,
        "ATTO" => 1e-18,
        _ => return None,
    })
}

impl Entities {
    /// A unit's quantity and size in meters, radians or steradians.
    pub fn unit_factor(&self, from: Id, unit: Id) -> Result<(Quantity, f64), ImportError> {
        self.factor(from, unit, 0)
    }

    fn factor(&self, from: Id, unit: Id, depth: usize) -> Result<(Quantity, f64), ImportError> {
        if depth > MAX_DEPTH {
            return Err(ImportError::ReferenceCycle { instance: unit });
        }
        let malformed = |what| ImportError::Malformed {
            instance: unit,
            attribute: 0,
            what,
        };
        match self.get(from, unit)? {
            Entity::Unit(Unit::Si {
                quantity,
                prefix,
                name,
            }) => {
                if !matches!(&**name, "METRE" | "METER" | "RADIAN" | "STERADIAN") {
                    return Ok((*quantity, 1.0));
                }
                let scale = match prefix {
                    None => 1.0,
                    Some(p) => prefix_factor(p).ok_or_else(|| malformed("unknown SI prefix"))?,
                };
                Ok((*quantity, scale))
            }
            Entity::Unit(Unit::Conversion {
                quantity,
                name,
                factor,
            }) => {
                let (value, of) = match self.get(unit, *factor)? {
                    Entity::Measure { value, unit } => (*value, *unit),
                    _ => return Err(wrong(*factor, "a measure with a unit")),
                };
                let (base, scale) = self.factor(*factor, of, depth + 1)?;
                let quantity = if *quantity == Quantity::Other {
                    base
                } else {
                    *quantity
                };
                if base != Quantity::Other && base != quantity {
                    return Err(malformed("a unit converted from another quantity's"));
                }
                let size = value * scale;
                if !(size > 0.0 && size.is_finite()) {
                    return Err(malformed("a unit's size must be positive"));
                }
                let known = KNOWN
                    .iter()
                    .find(|(n, k)| n.eq_ignore_ascii_case(name) && (size - k).abs() <= 1e-6 * k);
                Ok((quantity, known.map_or(size, |&(_, k)| k)))
            }
            _ => Err(wrong(unit, "a unit")),
        }
    }

    /// The units of representation context `context`. A context without a
    /// length unit is `MissingUnit`: the file's lengths can't be read
    /// without one, and none is guessed.
    pub fn units(&self, from: Id, context: Id) -> Result<Units, ImportError> {
        let (units, uncertainty) = match self.get(from, context)? {
            Entity::Context {
                units, uncertainty, ..
            } => (units, uncertainty),
            _ => return Err(wrong(context, "a representation context")),
        };
        let twice = |what| ImportError::Malformed {
            instance: context,
            attribute: 0,
            what,
        };
        let mut length = None;
        let mut angle = None;
        for &u in units {
            match self.unit_factor(context, u)? {
                (Quantity::Length, f) if length.replace(f).is_some() => {
                    return Err(twice("a context with two length units"));
                }
                (Quantity::PlaneAngle, f) if angle.replace(f).is_some() => {
                    return Err(twice("a context with two plane angle units"));
                }
                _ => {}
            }
        }
        let length = length.ok_or(ImportError::MissingUnit {
            context,
            quantity: "length",
        })?;
        let mut accuracy = None;
        for &u in uncertainty {
            if let Entity::Uncertainty { value, unit, name } = self.get(context, u)?
                && &**name == "distance_accuracy_value"
                && accuracy.is_none()
            {
                let (q, f) = self.unit_factor(u, *unit)?;
                if q == Quantity::Length {
                    accuracy = Some(value * f);
                }
            }
        }
        Ok(Units {
            length,
            angle,
            uncertainty: accuracy,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::entities;
    use super::*;

    const UNITS: &str = "#28=( LENGTH_UNIT() NAMED_UNIT(*) SI_UNIT(.MILLI.,.METRE.) );\
        #29=( NAMED_UNIT(*) PLANE_ANGLE_UNIT() SI_UNIT($,.RADIAN.) );\
        #30=( NAMED_UNIT(*) SI_UNIT($,.STERADIAN.) SOLID_ANGLE_UNIT() );\
        #31=UNCERTAINTY_MEASURE_WITH_UNIT(LENGTH_MEASURE(1.E-07),#28,\
        'distance_accuracy_value','confusion accuracy');\
        #46=( CONVERSION_BASED_UNIT('INCH',#48) LENGTH_UNIT() NAMED_UNIT(#47) );\
        #47=DIMENSIONAL_EXPONENTS(1.,0.,0.,0.,0.,0.,0.);\
        #48=LENGTH_MEASURE_WITH_UNIT(25.4,#28);\
        #49=( CONVERSION_BASED_UNIT('DEGREE',#50) NAMED_UNIT(#47) PLANE_ANGLE_UNIT() );\
        #50=PLANE_ANGLE_MEASURE_WITH_UNIT(PLANE_ANGLE_MEASURE(0.01745329252),#29);\
        #51=( CONVERSION_BASED_UNIT('CUBIT',#52) LENGTH_UNIT() NAMED_UNIT(#47) );\
        #52=LENGTH_MEASURE_WITH_UNIT(LENGTH_MEASURE(457.2),#28);\
        #53=( CONVERSION_BASED_UNIT('LOOP',#54) LENGTH_UNIT() NAMED_UNIT(#47) );\
        #54=LENGTH_MEASURE_WITH_UNIT(2.,#53);\
        #55=( LENGTH_UNIT() NAMED_UNIT(*) SI_UNIT(.HUGE.,.METRE.) );\
        #56=( CONVERSION_BASED_UNIT('INCH',#57) LENGTH_UNIT() NAMED_UNIT(#47) );\
        #57=LENGTH_MEASURE_WITH_UNIT(25.4,#29);";

    fn with(contexts: &str) -> Entities {
        entities(&format!("{UNITS}{contexts}"))
    }

    #[test]
    fn unit_sizes() {
        let e = with("");
        assert_eq!(e.unit_factor(0, 28), Ok((Quantity::Length, 1e-3)));
        assert_eq!(e.unit_factor(0, 29), Ok((Quantity::PlaneAngle, 1.0)));
        assert_eq!(e.unit_factor(0, 30), Ok((Quantity::SolidAngle, 1.0)));
        // 25.4 mm is 0.025400000000000002 in f64 arithmetic; an inch is
        // exactly 0.0254 m.
        assert_eq!(e.unit_factor(0, 46), Ok((Quantity::Length, 0.0254)));
        assert_eq!(
            e.unit_factor(0, 49),
            Ok((Quantity::PlaneAngle, std::f64::consts::PI / 180.0))
        );
        assert_eq!(e.unit_factor(0, 51), Ok((Quantity::Length, 457.2 * 1e-3)));
        assert_eq!(
            e.unit_factor(0, 53),
            Err(ImportError::ReferenceCycle { instance: 53 })
        );
        assert!(matches!(
            e.unit_factor(0, 55),
            Err(ImportError::Malformed {
                what: "unknown SI prefix",
                ..
            })
        ));
        assert!(matches!(
            e.unit_factor(0, 56),
            Err(ImportError::Malformed {
                what: "a unit converted from another quantity's",
                ..
            })
        ));
        assert_eq!(
            e.unit_factor(0, 31),
            Err(ImportError::WrongEntity {
                instance: 31,
                expected: "a unit"
            })
        );
    }

    #[test]
    fn context_units() {
        let e = with(
            "#1=( GEOMETRIC_REPRESENTATION_CONTEXT(3) \
             GLOBAL_UNCERTAINTY_ASSIGNED_CONTEXT((#31)) GLOBAL_UNIT_ASSIGNED_CONTEXT(\
             (#28,#29,#30)) REPRESENTATION_CONTEXT('','') );\
             #2=( GEOMETRIC_REPRESENTATION_CONTEXT(3) GLOBAL_UNIT_ASSIGNED_CONTEXT(\
             (#46,#49)) REPRESENTATION_CONTEXT('','') );\
             #3=( GEOMETRIC_REPRESENTATION_CONTEXT(3) GLOBAL_UNIT_ASSIGNED_CONTEXT(\
             (#29,#30)) REPRESENTATION_CONTEXT('','') );\
             #4=( GEOMETRIC_REPRESENTATION_CONTEXT(3) GLOBAL_UNIT_ASSIGNED_CONTEXT(\
             (#28,#46)) REPRESENTATION_CONTEXT('','') );",
        );
        assert_eq!(
            e.units(0, 1),
            Ok(Units {
                length: 1e-3,
                angle: Some(1.0),
                uncertainty: Some(1e-7 * 1e-3)
            })
        );
        assert_eq!(
            e.units(0, 2),
            Ok(Units {
                length: 0.0254,
                angle: Some(std::f64::consts::PI / 180.0),
                uncertainty: None
            })
        );
        assert_eq!(
            e.units(0, 3),
            Err(ImportError::MissingUnit {
                context: 3,
                quantity: "length"
            })
        );
        assert!(matches!(
            e.units(0, 4),
            Err(ImportError::Malformed {
                what: "a context with two length units",
                ..
            })
        ));
        assert!(e.units(0, 28).is_err());
    }

    #[test]
    fn a_simple_si_unit() {
        let e = entities("#1=SI_UNIT(*,.KILO.,.METRE.);#2=SI_UNIT(*,$,.SECOND.);");
        assert_eq!(e.unit_factor(0, 1), Ok((Quantity::Length, 1e3)));
        assert_eq!(e.unit_factor(0, 2), Ok((Quantity::Other, 1.0)));
    }
}
