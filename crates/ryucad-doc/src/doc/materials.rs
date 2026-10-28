//! The project's material library, `materials.json` beside the manifest:
//! each material's density and, optionally, its colour, and which one a
//! part with no `material` property takes. A part names its material by
//! its PDM `material` property. Nothing a rebuild makes reads it: only
//! mass properties do.
//!
//! ```json
//! {
//!   "default": "steel",
//!   "format": 1,
//!   "kind": "ryucad.materials",
//!   "materials": {
//!     "aluminium 6061": {"colour": "#b8bcc2", "density": "2.7 g/cm^3"},
//!     "steel": {"density": "7850 kg/m^3"}
//!   }
//! }
//! ```

use super::canonical;
use super::files::{Obj, header, invalid};
use super::project::conflicted;
use super::source::Source;
use crate::error::{Code, Error, Result};
use crate::units::Quantity;
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

/// Where the library lives, relative to the project.
pub const MATERIALS: &str = "materials.json";

pub const KIND: &str = "ryucad.materials";

/// The newest format this build reads.
pub const FORMAT: u64 = 1;

/// One material.
#[derive(Clone, Debug, PartialEq)]
pub struct Material {
    /// Positive, as [`Quantity::density`] reads it.
    pub density: Quantity,
    /// `#rrggbb`, lower case.
    pub colour: Option<String>,
}

/// The whole library.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Materials {
    pub materials: BTreeMap<String, Material>,
    /// A material of `materials`, for parts that name none.
    pub default: Option<String>,
}

/// A material's name: one line of 1 to 64 characters with no control
/// characters and no space at either end, as a PDM `material` property
/// holds it.
pub fn check_name(name: &str) -> Result<()> {
    let ok = !name.is_empty()
        && name.chars().count() <= 64
        && name.trim() == name
        && !name.chars().any(char::is_control);
    if ok {
        Ok(())
    } else {
        Err(Error::new(
            Code::InvalidParam,
            format!("bad material name {name:?}: expected 1 to 64 characters on one line, with no space at either end"),
        )
        .with("name", name))
    }
}

/// A colour as `#rrggbb`, made lower case.
pub fn check_colour(c: &str) -> Result<String> {
    let ok = c.len() == 7 && c.starts_with('#') && c[1..].chars().all(|x| x.is_ascii_hexdigit());
    if ok {
        Ok(c.to_ascii_lowercase())
    } else {
        Err(Error::new(
            Code::InvalidParam,
            format!("bad colour {c:?}: expected #rrggbb"),
        )
        .with("colour", c))
    }
}

impl Materials {
    pub fn to_json(&self) -> Value {
        let materials: Map<String, Value> = self
            .materials
            .iter()
            .map(|(name, m)| {
                let mut v = json!({"density": m.density.text()});
                if let Some(c) = &m.colour {
                    v["colour"] = json!(c);
                }
                (name.clone(), v)
            })
            .collect();
        let mut v = json!({"format": FORMAT, "kind": KIND, "materials": materials});
        if let Some(d) = &self.default {
            v["default"] = json!(d);
        }
        v
    }

    /// Reads a library: names, densities and colours that read, and a
    /// default that names one of them.
    pub fn from_json(v: &Value) -> Result<Materials> {
        let mut o = header(v, KIND, FORMAT)?;
        let list = o.take("materials")?;
        let map = list
            .as_object()
            .ok_or_else(|| invalid("/materials", "materials must be an object"))?;
        let mut materials = BTreeMap::new();
        for (name, m) in map {
            let at = format!("/materials/{}", name.replace('~', "~0").replace('/', "~1"));
            check_name(name).map_err(|e| invalid(&at, &e.message))?;
            let mut f = Obj::new(m, &at)?;
            let density = f.take("density")?;
            let density = density
                .as_str()
                .ok_or_else(|| {
                    invalid(
                        &format!("{at}/density"),
                        "expected a density such as \"7850 kg/m^3\"",
                    )
                })
                .and_then(|s| {
                    Quantity::density(s).map_err(|e| e.with("pointer", format!("{at}/density")))
                })?;
            let colour = match f.take_opt("colour") {
                None => None,
                Some(Value::String(c)) if check_colour(&c).as_deref() == Ok(c.as_str()) => Some(c),
                Some(_) => {
                    return Err(invalid(
                        &format!("{at}/colour"),
                        "expected a colour #rrggbb in lower case",
                    ));
                }
            };
            f.finish()?;
            materials.insert(name.clone(), Material { density, colour });
        }
        let default = match o.take_opt("default") {
            None => None,
            Some(Value::String(d)) if materials.contains_key(&d) => Some(d),
            Some(d) => {
                return Err(invalid(
                    "/default",
                    &format!("default {d} names no material of the library"),
                ));
            }
        };
        o.finish()?;
        Ok(Materials { materials, default })
    }

    /// The material a part takes: the one its `material` property names,
    /// else the default. `None` when it names none the library has, or
    /// names none and there is no default.
    pub fn of(&self, material: Option<&str>) -> Option<(&str, &Material)> {
        let name = material.or(self.default.as_deref())?;
        self.materials
            .get_key_value(name)
            .map(|(k, m)| (k.as_str(), m))
    }
}

/// The project's library, or `None` when it has none.
pub fn read(src: &dyn Source) -> Result<Option<Materials>> {
    let Some(bytes) = src.read(MATERIALS).map_err(|e| e.with("path", MATERIALS))? else {
        return Ok(None);
    };
    let text = std::str::from_utf8(&bytes).map_err(|_| {
        Error::new(Code::InvalidJson, format!("{MATERIALS} is not UTF-8")).with("path", MATERIALS)
    })?;
    let v = canonical::parse(text).map_err(|e| e.with("path", MATERIALS))?;
    if let Some(e) = conflicted(&v, MATERIALS) {
        return Err(e);
    }
    Materials::from_json(&v)
        .map(Some)
        .map_err(|e| e.with("path", MATERIALS))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap as Files;

    struct Mem(Files<String, Vec<u8>>);

    impl Source for Mem {
        fn list(&self, _: &str) -> Result<Vec<(String, super::super::source::Kind)>> {
            Ok(Vec::new())
        }
        fn read(&self, path: &str) -> Result<Option<Vec<u8>>> {
            Ok(self.0.get(path).cloned())
        }
    }

    fn library() -> Value {
        json!({
            "default": "steel",
            "format": 1,
            "kind": KIND,
            "materials": {
                "aluminium 6061": {"colour": "#b8bcc2", "density": "2.7 g/cm^3"},
                "steel": {"density": "7850 kg/m^3"}
            }
        })
    }

    #[test]
    fn a_library_reads_and_writes_back_canonically() {
        let m = Materials::from_json(&library()).unwrap();
        assert_eq!(m.default.as_deref(), Some("steel"));
        assert_eq!(m.materials["aluminium 6061"].density.si(), 2700.0);
        assert_eq!(m.to_json(), library());
        let text = canonical::to_string(&m.to_json());
        let src = Mem(Files::from([(MATERIALS.to_owned(), text.into_bytes())]));
        assert_eq!(read(&src).unwrap(), Some(m.clone()));
        assert_eq!(read(&Mem(Files::new())).unwrap(), None);
        assert_eq!(m.of(Some("aluminium 6061")).unwrap().0, "aluminium 6061");
        assert_eq!(m.of(None).unwrap().0, "steel");
        assert_eq!(m.of(Some("brass")), None);
    }

    #[test]
    fn bad_libraries_say_where() {
        let mut v = library();
        v["materials"]["steel"]["density"] = json!("7850");
        let e = Materials::from_json(&v).unwrap_err();
        assert_eq!(
            (e.code, e.details["pointer"].as_str()),
            (Code::InvalidQuantity, Some("/materials/steel/density"))
        );
        let mut v = library();
        v["default"] = json!("brass");
        assert_eq!(
            Materials::from_json(&v).unwrap_err().details["pointer"],
            "/default"
        );
        let mut v = library();
        v["materials"]["steel"]["colour"] = json!("#ABCDEF");
        assert_eq!(
            Materials::from_json(&v).unwrap_err().details["pointer"],
            "/materials/steel/colour"
        );
        let mut v = library();
        v["materials"][" steel"] = json!({"density": "1 kg/m^3"});
        assert_eq!(
            Materials::from_json(&v).unwrap_err().code,
            Code::InvalidDocument
        );
        let mut v = library();
        v["format"] = json!(2);
        assert_eq!(
            Materials::from_json(&v).unwrap_err().code,
            Code::FormatTooNew
        );
        assert!(check_name("a/b ✓").is_ok());
        assert_eq!(check_colour("#ABCDEF").unwrap(), "#abcdef");
        assert!(check_colour("abcdef").is_err());
    }
}
