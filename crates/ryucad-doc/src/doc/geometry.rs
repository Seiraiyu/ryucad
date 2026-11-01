//! Geometric references: anything that can stand for a plane, an axis, a
//! point or a curve, named one way in files and one way in text.
//!
//! | File | Text | Names |
//! |---|---|---|
//! | `"xy"`, `"yz"`, `"zx"` | the same | an origin plane |
//! | `"x"`, `"y"`, `"z"` | the same | an origin axis |
//! | `"origin"` | `origin` | the part's origin point |
//! | `{"feature", "name"}` | `<part>/<founder>/<32 hex>` | a face, edge or vertex of a solid |
//! | `{"feature"}` | `<part>/<feature>` | a datum, or a sketch as its plane |
//! | `{"feature", "entity"}` | `<part>/<sketch>/<entity>` | a sketch's point, line, arc or circle |
//!
//! What a reference stands for (a plane, an axis, a point) is decided when
//! it is resolved against the model, not here: a file holds names only.

use super::files::{Obj, invalid};
use super::id::Id;
use super::pattern::OriginAxis;
use super::reference::{Reference, parse_name};
use super::sketch::OriginPlane;
use crate::error::{Code, Error, Result};
use serde_json::{Value, json};

/// A reference to something that stands for a plane, an axis, a point or
/// a curve.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum GeomRef {
    /// One of the part's origin planes.
    Plane(OriginPlane),
    /// One of the part's origin axes, through the origin.
    Axis(OriginAxis),
    /// The part's origin point.
    Origin,
    /// A face, edge or vertex of a solid, by stable name.
    Entity(Reference),
    /// A whole feature: a datum, or a sketch as its plane.
    Feature(Id),
    /// A point, line, arc or circle of a sketch.
    Sketch { feature: Id, entity: Id },
}

impl GeomRef {
    pub fn to_json(&self) -> Value {
        match self {
            GeomRef::Plane(p) => json!(p.as_str()),
            GeomRef::Axis(a) => json!(a.as_str()),
            GeomRef::Origin => json!("origin"),
            GeomRef::Entity(r) => r.to_json(),
            GeomRef::Feature(f) => json!({"feature": f}),
            GeomRef::Sketch { feature, entity } => json!({"feature": feature, "entity": entity}),
        }
    }

    /// Reads any of the file forms; `at` is its JSON pointer for errors.
    pub fn from_json(v: &Value, at: &str) -> Result<GeomRef> {
        match v {
            Value::String(s) => GeomRef::word(s).ok_or_else(|| {
                invalid(
                    at,
                    "a geometric reference is xy, yz, zx, x, y, z or origin, or an object: {feature, name}, {feature} or {feature, entity}",
                )
            }),
            Value::Object(m) if m.contains_key("name") => {
                Ok(GeomRef::Entity(Reference::from_json(v, at)?))
            }
            Value::Object(_) => {
                let mut o = Obj::new(v, at)?;
                let f = o.take("feature")?;
                let e = o.take_opt("entity");
                o.finish()?;
                let feature = f
                    .as_str()
                    .ok_or_else(|| invalid(&format!("{at}/feature"), "feature is a feature's ID"))
                    .and_then(|s| {
                        Id::parse(s, "feature").map_err(|e| e.with("pointer", format!("{at}/feature")))
                    })?;
                match e {
                    None => Ok(GeomRef::Feature(feature)),
                    Some(e) => {
                        let entity = e
                            .as_str()
                            .ok_or_else(|| {
                                invalid(&format!("{at}/entity"), "entity is a sketch entity's ID")
                            })
                            .and_then(|s| {
                                Id::parse(s, "entity")
                                    .map_err(|e| e.with("pointer", format!("{at}/entity")))
                            })?;
                        Ok(GeomRef::Sketch { feature, entity })
                    }
                }
            }
            _ => Err(invalid(
                at,
                "a geometric reference is a string (xy, x, origin, …) or an object naming a feature",
            )),
        }
    }

    /// The origin's words: `xy`, `yz`, `zx`, `x`, `y`, `z`, `origin`.
    pub fn word(s: &str) -> Option<GeomRef> {
        if s == "origin" {
            return Some(GeomRef::Origin);
        }
        if let Some(a) = OriginAxis::parse(s) {
            return Some(GeomRef::Axis(a));
        }
        OriginPlane::ALL
            .into_iter()
            .find(|p| p.as_str() == s)
            .map(GeomRef::Plane)
    }

    /// Reads the text form in `part`: an origin word, `<part>/<feature>`,
    /// `<part>/<founder>/<32 hex>` or `<part>/<sketch>/<entity>`, or a bare
    /// feature ID, taken in `part`. The origin's words come first, so a
    /// feature named `x` is written `<part>/x`.
    pub fn parse(s: &str, part: &Id) -> Result<GeomRef> {
        if let Some(w) = GeomRef::word(s) {
            return Ok(w);
        }
        let bad = |why: &str| {
            Error::new(
                Code::InvalidParam,
                format!(
                    "bad geometric reference {s:?}: {why}; use xy, x, origin, <part>/<feature>, <part>/<feature>/<name> or <part>/<sketch>/<entity>"
                ),
            )
            .with("reference", s)
        };
        let parts: Vec<&str> = s.split('/').collect();
        let (p, rest) = match parts.as_slice() {
            [f] => (part.as_str(), vec![*f]),
            [p, rest @ ..] if rest.len() <= 2 => (*p, rest.to_vec()),
            _ => return Err(bad("expected at most three parts separated by /")),
        };
        if p != part.as_str() {
            return Err(bad(&format!(
                "a feature of part {part} refers only within it"
            )));
        }
        match rest.as_slice() {
            [f] => Ok(GeomRef::Feature(Id::parse(f, "feature")?)),
            [f, n] => {
                let feature = Id::parse(f, "feature")?;
                match parse_name(n) {
                    Some(name) => Ok(GeomRef::Entity(Reference { feature, name })),
                    None => Ok(GeomRef::Sketch {
                        feature,
                        entity: Id::parse(n, "entity")?,
                    }),
                }
            }
            _ => Err(bad("name a feature after the part")),
        }
    }

    /// The text form, in `part`.
    pub fn text(&self, part: &Id) -> String {
        match self {
            GeomRef::Plane(p) => p.as_str().to_owned(),
            GeomRef::Axis(a) => a.as_str().to_owned(),
            GeomRef::Origin => "origin".to_owned(),
            GeomRef::Entity(r) => r.text(part),
            GeomRef::Feature(f) => format!("{part}/{f}"),
            GeomRef::Sketch { feature, entity } => format!("{part}/{feature}/{entity}"),
        }
    }

    /// The face, edge or vertex it names, if it names one.
    pub fn reference(&self) -> Option<&Reference> {
        match self {
            GeomRef::Entity(r) => Some(r),
            _ => None,
        }
    }

    /// The feature it reads, if it reads one: an entity's solid, a datum,
    /// or a sketch.
    pub fn feature(&self) -> Option<&Id> {
        match self {
            GeomRef::Entity(r) => Some(&r.feature),
            GeomRef::Feature(f) | GeomRef::Sketch { feature: f, .. } => Some(f),
            GeomRef::Plane(_) | GeomRef::Axis(_) | GeomRef::Origin => None,
        }
    }

    /// Whether its file form is new in feature format 7: a whole feature
    /// or a sketch's entity. Fields that took a word or an entity before
    /// keep their files' formats.
    pub fn is_new(&self) -> bool {
        matches!(self, GeomRef::Feature(_) | GeomRef::Sketch { .. })
    }

    /// Itself with every feature it names renamed by `map` (renumbering a
    /// collided feature).
    pub fn renamed(&self, map: &dyn Fn(&Id) -> Option<Id>) -> GeomRef {
        match self {
            GeomRef::Entity(r) => GeomRef::Entity(Reference {
                feature: map(&r.feature).unwrap_or_else(|| r.feature.clone()),
                name: r.name,
            }),
            GeomRef::Feature(f) => GeomRef::Feature(map(f).unwrap_or_else(|| f.clone())),
            GeomRef::Sketch { feature, entity } => GeomRef::Sketch {
                feature: map(feature).unwrap_or_else(|| feature.clone()),
                entity: entity.clone(),
            },
            g => g.clone(),
        }
    }
}

/// Reads an optional geometric reference field.
pub fn opt(o: &mut Obj, key: &str) -> Result<Option<GeomRef>> {
    o.take_opt(key)
        .map(|v| GeomRef::from_json(&v, &format!("/{key}")))
        .transpose()
}

/// Reads a required geometric reference field.
pub fn take(o: &mut Obj, key: &str) -> Result<GeomRef> {
    GeomRef::from_json(&o.take(key)?, &format!("/{key}"))
}

/// Reads a field holding exactly `n` geometric references.
pub fn take_n(o: &mut Obj, key: &str, n: usize) -> Result<Vec<GeomRef>> {
    let v = o.take(key)?;
    let a = v.as_array().filter(|a| a.len() == n).ok_or_else(|| {
        invalid(
            &format!("/{key}"),
            &format!("{key} holds exactly {n} references"),
        )
    })?;
    a.iter()
        .enumerate()
        .map(|(i, x)| GeomRef::from_json(x, &format!("/{key}/{i}")))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const NAME: &str = "0123456789abcdef0123456789abcdef";

    fn id(s: &str) -> Id {
        Id::parse(s, "feature").unwrap()
    }

    #[test]
    fn every_form_round_trips_in_files_and_text() {
        let p = id("p");
        let all = [
            (
                json!("xy"),
                "xy".to_owned(),
                GeomRef::Plane(OriginPlane::Xy),
            ),
            (
                json!("zx"),
                "zx".to_owned(),
                GeomRef::Plane(OriginPlane::Zx),
            ),
            (json!("y"), "y".to_owned(), GeomRef::Axis(OriginAxis::Y)),
            (json!("origin"), "origin".to_owned(), GeomRef::Origin),
            (
                json!({"feature": "b", "name": NAME}),
                format!("p/b/{NAME}"),
                GeomRef::Entity(Reference {
                    feature: id("b"),
                    name: parse_name(NAME).unwrap(),
                }),
            ),
            (
                json!({"feature": "d1"}),
                "p/d1".to_owned(),
                GeomRef::Feature(id("d1")),
            ),
            (
                json!({"entity": "l2", "feature": "s"}),
                "p/s/l2".to_owned(),
                GeomRef::Sketch {
                    feature: id("s"),
                    entity: Id::parse("l2", "entity").unwrap(),
                },
            ),
        ];
        for (file, text, g) in all {
            assert_eq!(GeomRef::from_json(&file, "/r").unwrap(), g, "{file}");
            assert_eq!(g.to_json(), file);
            assert_eq!(GeomRef::parse(&text, &p).unwrap(), g, "{text}");
            assert_eq!(g.text(&p), text);
        }
        // A bare feature ID is taken in the part; the origin's words win.
        assert_eq!(
            GeomRef::parse("d1", &p).unwrap(),
            GeomRef::Feature(id("d1"))
        );
        assert_eq!(
            GeomRef::parse("x", &p).unwrap(),
            GeomRef::Axis(OriginAxis::X)
        );
        assert_eq!(
            GeomRef::parse("p/x", &p).unwrap(),
            GeomRef::Feature(id("x"))
        );
        // Old files' references are this one's entity form, byte for byte.
        let r = Reference {
            feature: id("b"),
            name: parse_name(NAME).unwrap(),
        };
        assert_eq!(GeomRef::Entity(r.clone()).to_json(), r.to_json());
    }

    #[test]
    fn what_a_reference_reads_and_whether_its_form_is_new() {
        let g = GeomRef::Sketch {
            feature: id("s"),
            entity: Id::parse("p3", "entity").unwrap(),
        };
        assert_eq!(g.feature(), Some(&id("s")));
        assert!(g.is_new() && g.reference().is_none());
        assert!(!GeomRef::Origin.is_new() && GeomRef::Origin.feature().is_none());
        let e = GeomRef::parse(&format!("p/b/{NAME}"), &id("p")).unwrap();
        assert!(!e.is_new() && e.reference().is_some());
        let moved = g.renamed(&|f| (f == &id("s")).then(|| id("s-2")));
        assert_eq!(moved.feature(), Some(&id("s-2")));
    }

    #[test]
    fn bad_references() {
        let p = id("p");
        for s in ["q/d1", "p/a/b/c", "p/B", "p/s/L!", "", "p/"] {
            assert!(GeomRef::parse(s, &p).is_err(), "{s}");
        }
        for (v, pointer) in [
            (json!("xz"), "/r"),
            (json!(3), "/r"),
            (json!({"feature": 3}), "/r/feature"),
            (json!({"feature": "s", "entity": 1}), "/r/entity"),
            (json!({"feature": "s", "other": 1}), "/r/other"),
            (json!({"feature": "b", "name": "zz"}), "/r/name"),
        ] {
            let e = GeomRef::from_json(&v, "/r").unwrap_err();
            assert_eq!(e.details["pointer"], pointer, "{v}");
        }
    }
}
