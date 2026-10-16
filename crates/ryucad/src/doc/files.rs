//! The three file kinds: the project manifest, a part's feature list and a
//! feature. Each reads from and writes to a JSON value; the file system is
//! [`super::project`]'s business.

use super::id::Id;
use crate::error::{Code, Error, Result};
use crate::units::{Quantity, Unit};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

/// The format version this ryucad reads and writes, for every kind.
pub const FORMAT: u64 = 1;

pub const PROJECT_KIND: &str = "ryucad.project";
pub const PART_KIND: &str = "ryucad.part";
pub const FEATURE_KIND: &str = "ryucad.feature";

/// The project manifest, `ryucad.json`.
#[derive(Clone, Debug, PartialEq)]
pub struct Manifest {
    pub default_unit: Unit,
}

/// A part's ordered feature list, `parts/<part>/part.json`.
#[derive(Clone, Debug, PartialEq)]
pub struct PartFile {
    pub features: Vec<Id>,
}

/// The primitive feature types of phase 0.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FeatureType {
    Box,
    Cylinder,
    Cone,
    Sphere,
    Torus,
}

/// A feature, `parts/<part>/features/<id>.json`.
#[derive(Clone, Debug, PartialEq)]
pub struct Feature {
    pub ty: FeatureType,
    pub params: BTreeMap<String, Quantity>,
    pub placement: Placement,
}

/// Where a feature's local frame sits: an origin, an axis `z` and a
/// reference direction `x` (made perpendicular to `z` on rebuild).
#[derive(Clone, Debug, PartialEq)]
pub struct Placement {
    pub origin: [Quantity; 3],
    pub z: [f64; 3],
    pub x: [f64; 3],
}

impl FeatureType {
    pub const ALL: [FeatureType; 5] = [
        FeatureType::Box,
        FeatureType::Cylinder,
        FeatureType::Cone,
        FeatureType::Sphere,
        FeatureType::Torus,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            FeatureType::Box => "box",
            FeatureType::Cylinder => "cylinder",
            FeatureType::Cone => "cone",
            FeatureType::Sphere => "sphere",
            FeatureType::Torus => "torus",
        }
    }

    pub fn parse(s: &str) -> Result<FeatureType> {
        FeatureType::ALL
            .into_iter()
            .find(|t| t.as_str() == s)
            .ok_or_else(|| {
                Error::new(
                    Code::UnknownFeatureType,
                    format!(
                        "unknown feature type {s:?}; known: box, cylinder, cone, sphere, torus"
                    ),
                )
                .with("type", s)
            })
    }

    /// The required parameters, each a length greater than zero.
    pub fn required(self) -> &'static [&'static str] {
        match self {
            FeatureType::Box => &["x", "y", "z"],
            FeatureType::Cylinder | FeatureType::Cone => &["radius", "height"],
            FeatureType::Sphere => &["radius"],
            FeatureType::Torus => &["major_radius", "minor_radius"],
        }
    }

    /// The optional parameters, each a length of zero or more.
    pub fn optional(self) -> &'static [&'static str] {
        match self {
            FeatureType::Cone => &["top_radius"],
            _ => &[],
        }
    }
}

impl Placement {
    /// The world frame, with the origin written in `unit`.
    pub fn identity(unit: Unit) -> Placement {
        let zero = Quantity::parse("0", unit).expect("0 parses");
        Placement {
            origin: [zero.clone(), zero.clone(), zero],
            z: [0.0, 0.0, 1.0],
            x: [1.0, 0.0, 0.0],
        }
    }

    pub fn to_json(&self) -> Value {
        json!({
            "origin": self.origin.iter().map(|q| q.text()).collect::<Vec<_>>(),
            "x": self.x.map(number),
            "z": self.z.map(number),
        })
    }

    /// Reads a placement object; `at` is its JSON pointer for errors.
    pub fn from_json(v: &Value, default: Unit, at: &str) -> Result<Placement> {
        let mut o = Obj::new(v, at)?;
        let origin = o.take("origin")?;
        let origin = triple(&origin, &format!("{at}/origin"), |v, at| {
            Quantity::from_json(v, default).map_err(|e| e.with("pointer", at))
        })?;
        let z = vector(&o.take("z")?, &format!("{at}/z"))?;
        let x = vector(&o.take("x")?, &format!("{at}/x"))?;
        o.finish()?;
        Ok(Placement { origin, z, x })
    }
}

impl Manifest {
    pub fn to_json(&self) -> Value {
        json!({"default_unit": self.default_unit, "format": FORMAT, "kind": PROJECT_KIND})
    }

    pub fn from_json(v: &Value) -> Result<Manifest> {
        let mut o = header(v, PROJECT_KIND)?;
        let unit = o.take("default_unit")?;
        let default_unit = unit.as_str().and_then(Unit::parse).ok_or_else(|| {
            invalid(
                "/default_unit",
                "default_unit must be one of m, cm, mm, um, in, ft",
            )
        })?;
        o.finish()?;
        Ok(Manifest { default_unit })
    }
}

impl PartFile {
    pub fn to_json(&self) -> Value {
        json!({"features": self.features, "format": FORMAT, "kind": PART_KIND})
    }

    pub fn from_json(v: &Value) -> Result<PartFile> {
        let mut o = header(v, PART_KIND)?;
        let list = o.take("features")?;
        let items = list
            .as_array()
            .ok_or_else(|| invalid("/features", "features must be an array of feature IDs"))?;
        let mut features: Vec<Id> = Vec::with_capacity(items.len());
        for (i, item) in items.iter().enumerate() {
            let at = format!("/features/{i}");
            let s = item
                .as_str()
                .ok_or_else(|| invalid(&at, "a feature ID must be a string"))?;
            let id = Id::parse(s, "feature").map_err(|e| e.with("pointer", &at))?;
            if features.contains(&id) {
                return Err(
                    Error::new(Code::DuplicateId, format!("feature {id} is listed twice"))
                        .with("id", &id)
                        .with("pointer", &at),
                );
            }
            features.push(id);
        }
        o.finish()?;
        Ok(PartFile { features })
    }
}

impl Feature {
    pub fn to_json(&self) -> Value {
        let params: Map<String, Value> = self
            .params
            .iter()
            .map(|(k, q)| (k.clone(), Value::String(q.text())))
            .collect();
        json!({
            "format": FORMAT,
            "kind": FEATURE_KIND,
            "params": params,
            "placement": self.placement.to_json(),
            "type": self.ty.as_str(),
        })
    }

    /// Reads and validates a feature. Bare numbers take `default`.
    pub fn from_json(v: &Value, default: Unit) -> Result<Feature> {
        let mut o = header(v, FEATURE_KIND)?;
        let ty = o.take("type")?;
        let ty = FeatureType::parse(
            ty.as_str()
                .ok_or_else(|| invalid("/type", "type must be a string"))?,
        )
        .map_err(|e| e.with("pointer", "/type"))?;
        let params = o.take("params")?;
        let params = Feature::params_from_json(ty, &params, default)?;
        let placement = Placement::from_json(&o.take("placement")?, default, "/placement")?;
        o.finish()?;
        Ok(Feature {
            ty,
            params,
            placement,
        })
    }

    /// Validates a params object for `ty`: every required parameter present
    /// and positive, optional ones non-negative, no others.
    pub fn params_from_json(
        ty: FeatureType,
        v: &Value,
        default: Unit,
    ) -> Result<BTreeMap<String, Quantity>> {
        let map = v
            .as_object()
            .ok_or_else(|| invalid("/params", "params must be an object"))?;
        let mut params = BTreeMap::new();
        for (k, v) in map {
            let at = format!("/params/{k}");
            let required = ty.required().contains(&k.as_str());
            if !required && !ty.optional().contains(&k.as_str()) {
                return Err(param_error(ty, k, &at, "is not a parameter of"));
            }
            let q = Quantity::from_json(v, default).map_err(|e| e.with("pointer", &at))?;
            let m = q.meters();
            if (required && m <= 0.0) || m < 0.0 {
                let want = if required {
                    "must be greater than zero for"
                } else {
                    "must not be negative for"
                };
                return Err(param_error(ty, k, &at, want).with("value", q.text()));
            }
            params.insert(k.clone(), q);
        }
        if let Some(missing) = ty.required().iter().find(|k| !params.contains_key(**k)) {
            return Err(param_error(ty, missing, "/params", "is required for"));
        }
        Ok(params)
    }

    /// A parameter in meters; 0 for an absent optional one.
    pub fn meters(&self, name: &str) -> f64 {
        self.params.get(name).map_or(0.0, Quantity::meters)
    }
}

fn param_error(ty: FeatureType, name: &str, at: &str, what: &str) -> Error {
    let known = [ty.required(), ty.optional()].concat().join(", ");
    Error::new(
        Code::InvalidParam,
        format!(
            "{name} {what} a {}; its parameters are {known}",
            ty.as_str()
        ),
    )
    .with("param", name)
    .with("pointer", at)
}

/// A JSON number for `x`: an integer when `x` is one, so `[0, 0, 1]`
/// stays `[0, 0, 1]`.
fn number(x: f64) -> Value {
    if x.fract() == 0.0 && x.abs() < 9_007_199_254_740_992.0 {
        json!(x as i64)
    } else {
        json!(x)
    }
}

fn invalid(at: &str, msg: &str) -> Error {
    Error::new(Code::InvalidDocument, msg).with("pointer", at)
}

/// Checks `kind` and `format`, migrating older formats, and returns the
/// remaining fields.
fn header<'a>(v: &'a Value, kind: &str) -> Result<Obj<'a>> {
    let mut o = Obj::new(v, "")?;
    let k = o.take("kind")?;
    if k.as_str() != Some(kind) {
        return Err(invalid(
            "/kind",
            &format!("expected kind {kind:?}, found {k}"),
        ));
    }
    let f = o.take("format")?;
    let format = f
        .as_u64()
        .filter(|f| *f >= 1)
        .ok_or_else(|| invalid("/format", "format must be a positive integer"))?;
    if format > FORMAT {
        return Err(Error::new(
            Code::FormatTooNew,
            format!(
                "this file has format {format}; this ryucad reads up to {FORMAT}. Upgrade ryucad"
            ),
        )
        .with("format", format)
        .with("supported", FORMAT));
    }
    migrate(format);
    Ok(o)
}

/// Brings an older format up to [`FORMAT`], one step at a time. Format 1 is
/// the first, so there is nothing to do yet; each later format adds a
/// `format => step` arm that rewrites the fields in place.
fn migrate(format: u64) {
    debug_assert!(format <= FORMAT);
}

/// The fields of a JSON object, taken one by one so leftovers are errors.
struct Obj<'a> {
    map: BTreeMap<&'a str, &'a Value>,
    at: String,
}

impl<'a> Obj<'a> {
    fn new(v: &'a Value, at: &str) -> Result<Obj<'a>> {
        let map = v
            .as_object()
            .ok_or_else(|| invalid(at, "expected a JSON object"))?;
        Ok(Obj {
            map: map.iter().map(|(k, v)| (k.as_str(), v)).collect(),
            at: at.to_owned(),
        })
    }

    fn take(&mut self, key: &str) -> Result<Value> {
        self.map.remove(key).cloned().ok_or_else(|| {
            invalid(
                &format!("{}/{key}", self.at),
                &format!("missing field {key:?}"),
            )
        })
    }

    fn finish(self) -> Result<()> {
        match self.map.keys().next() {
            None => Ok(()),
            Some(k) => Err(invalid(
                &format!("{}/{k}", self.at),
                &format!("unexpected field {k:?}"),
            )),
        }
    }
}

fn triple<T>(v: &Value, at: &str, item: impl Fn(&Value, &str) -> Result<T>) -> Result<[T; 3]> {
    let a = v
        .as_array()
        .filter(|a| a.len() == 3)
        .ok_or_else(|| invalid(at, "expected an array of three"))?;
    Ok([
        item(&a[0], &format!("{at}/0"))?,
        item(&a[1], &format!("{at}/1"))?,
        item(&a[2], &format!("{at}/2"))?,
    ])
}

fn vector(v: &Value, at: &str) -> Result<[f64; 3]> {
    triple(v, at, |v, at| {
        v.as_f64().ok_or_else(|| invalid(at, "expected a number"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::canonical;

    fn cylinder_json() -> Value {
        json!({
            "format": 1,
            "kind": "ryucad.feature",
            "params": {"height": "40 mm", "radius": 12.5},
            "placement": {"origin": ["0 mm", "0", "10 mm"], "x": [1, 0, 0], "z": [0, 0, 1]},
            "type": "cylinder"
        })
    }

    #[test]
    fn manifest_round_trip() {
        let m = Manifest {
            default_unit: Unit::In,
        };
        let v = m.to_json();
        assert_eq!(
            v,
            json!({"default_unit": "in", "format": 1, "kind": "ryucad.project"})
        );
        assert_eq!(Manifest::from_json(&v).unwrap(), m);
    }

    #[test]
    fn part_round_trip() {
        let p = PartFile {
            features: vec![
                Id::parse("b", "feature").unwrap(),
                Id::parse("a", "feature").unwrap(),
            ],
        };
        let v = p.to_json();
        assert_eq!(v["features"], json!(["b", "a"]));
        assert_eq!(PartFile::from_json(&v).unwrap(), p);
    }

    #[test]
    fn feature_reads_and_writes_explicit_units() {
        let f = Feature::from_json(&cylinder_json(), Unit::Mm).unwrap();
        assert_eq!(f.ty, FeatureType::Cylinder);
        assert_eq!(f.meters("radius"), 0.0125);
        assert_eq!(
            canonical::to_string(&f.to_json()),
            "{\n  \"format\": 1,\n  \"kind\": \"ryucad.feature\",\n  \"params\": {\n    \"height\": \"40 mm\",\n    \"radius\": \"12.5 mm\"\n  },\n  \"placement\": {\n    \"origin\": [\n      \"0 mm\",\n      \"0 mm\",\n      \"10 mm\"\n    ],\n    \"x\": [\n      1,\n      0,\n      0\n    ],\n    \"z\": [\n      0,\n      0,\n      1\n    ]\n  },\n  \"type\": \"cylinder\"\n}\n"
        );
        // Written once, it reads back identically in any default unit.
        assert_eq!(Feature::from_json(&f.to_json(), Unit::Ft).unwrap(), f);
    }

    #[test]
    fn errors_point_at_the_field() {
        let cases: [(&str, Value, Code, &str); 10] = [
            ("/type", json!("prism"), Code::UnknownFeatureType, "/type"),
            (
                "/params/radius",
                json!("-1 mm"),
                Code::InvalidParam,
                "/params/radius",
            ),
            (
                "/params/radius",
                json!("1 parsec"),
                Code::InvalidQuantity,
                "/params/radius",
            ),
            (
                "/params/width",
                json!("1 mm"),
                Code::InvalidParam,
                "/params/width",
            ),
            (
                "/placement/z",
                json!([0, 1]),
                Code::InvalidDocument,
                "/placement/z",
            ),
            (
                "/placement/x/2",
                json!("1"),
                Code::InvalidDocument,
                "/placement/x/2",
            ),
            (
                "/placement/origin/1",
                json!(true),
                Code::InvalidQuantity,
                "/placement/origin/1",
            ),
            ("/extra", json!(1), Code::InvalidDocument, "/extra"),
            ("/format", json!(2), Code::FormatTooNew, ""),
            (
                "/kind",
                json!("ryucad.part"),
                Code::InvalidDocument,
                "/kind",
            ),
        ];
        for (path, value, code, pointer) in cases {
            let mut v = cylinder_json();
            *v.pointer_mut_or_insert(path) = value;
            let e = Feature::from_json(&v, Unit::Mm).unwrap_err();
            assert_eq!(e.code, code, "{path}: {e}");
            if !pointer.is_empty() {
                assert_eq!(e.details["pointer"], pointer, "{path}");
            }
        }
    }

    #[test]
    fn missing_params() {
        let mut v = cylinder_json();
        v["params"].as_object_mut().unwrap().remove("height");
        let e = Feature::from_json(&v, Unit::Mm).unwrap_err();
        assert_eq!(e.code, Code::InvalidParam);
        assert_eq!(e.details["param"], "height");
        v.as_object_mut().unwrap().remove("placement");
        v["params"]["height"] = json!(1);
        let e = Feature::from_json(&v, Unit::Mm).unwrap_err();
        assert_eq!(e.details["pointer"], "/placement");
    }

    #[test]
    fn cone_top_radius_is_optional_and_may_be_zero() {
        let mut v = cylinder_json();
        v["type"] = json!("cone");
        assert!(Feature::from_json(&v, Unit::Mm).is_ok());
        v["params"]["top_radius"] = json!(0);
        assert_eq!(
            Feature::from_json(&v, Unit::Mm)
                .unwrap()
                .meters("top_radius"),
            0.0
        );
        v["params"]["radius"] = json!(0);
        assert_eq!(
            Feature::from_json(&v, Unit::Mm).unwrap_err().code,
            Code::InvalidParam
        );
    }

    #[test]
    fn duplicate_features_in_a_part() {
        let v = json!({"features": ["a", "a"], "format": 1, "kind": "ryucad.part"});
        let e = PartFile::from_json(&v).unwrap_err();
        assert_eq!(e.code, Code::DuplicateId);
        assert_eq!(e.details["pointer"], "/features/1");
    }

    #[test]
    fn migration_seam() {
        // Format 1 passes through; anything newer is refused before any
        // other field is read, and format 0 never existed.
        let mut v = Manifest {
            default_unit: Unit::Mm,
        }
        .to_json();
        assert!(Manifest::from_json(&v).is_ok());
        v["format"] = json!(2);
        v["default_unit"] = json!("parsec");
        let e = Manifest::from_json(&v).unwrap_err();
        assert_eq!(e.code, Code::FormatTooNew);
        assert_eq!(e.details["supported"], 1);
        v["format"] = json!(0);
        assert_eq!(
            Manifest::from_json(&v).unwrap_err().code,
            Code::InvalidDocument
        );
    }

    /// `pointer_mut`, creating the last key if it is missing.
    trait PointerInsert {
        fn pointer_mut_or_insert(&mut self, path: &str) -> &mut Value;
    }

    impl PointerInsert for Value {
        fn pointer_mut_or_insert(&mut self, path: &str) -> &mut Value {
            if self.pointer(path).is_none() {
                let (parent, key) = path.rsplit_once('/').unwrap();
                self.pointer_mut(parent).unwrap()[key] = Value::Null;
            }
            self.pointer_mut(path).unwrap()
        }
    }
}
