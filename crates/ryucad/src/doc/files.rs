//! The file kinds: the project manifest, a part's feature list, its
//! parameters and a feature. Each reads from and writes to a JSON value;
//! the file system is [`super::project`]'s business.

use super::id::Id;
use super::sketch::Sketch;
use crate::error::{Code, Error, Result};
use crate::expr::{self, Env, Expr};
use crate::units::{Quantity, Unit};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

/// The format version of the manifest, part and parameters files.
pub const FORMAT: u64 = 1;

/// The newest feature format: 2 adds expressions. A feature is written at
/// the lowest format that holds it, so one with only literals stays 1.
pub const FEATURE_FORMAT: u64 = 2;

pub const PROJECT_KIND: &str = "ryucad.project";
pub const PART_KIND: &str = "ryucad.part";
pub const PARAMETERS_KIND: &str = "ryucad.parameters";
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

/// A part's named parameters, `parts/<part>/parameters.json`: each an
/// expression over the others.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ParametersFile {
    pub parameters: BTreeMap<String, Expr>,
}

/// The primitive types of phase 0.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PrimitiveType {
    Box,
    Cylinder,
    Cone,
    Sphere,
    Torus,
}

/// A feature, `parts/<part>/features/<id>.json`: its `type` says which.
#[derive(Clone, Debug, PartialEq)]
pub enum Feature {
    Primitive(Primitive),
    Sketch(Sketch),
}

/// A primitive feature: one solid of a type, sized by its params.
#[derive(Clone, Debug, PartialEq)]
pub struct Primitive {
    pub ty: PrimitiveType,
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

impl PrimitiveType {
    pub const ALL: [PrimitiveType; 5] = [
        PrimitiveType::Box,
        PrimitiveType::Cylinder,
        PrimitiveType::Cone,
        PrimitiveType::Sphere,
        PrimitiveType::Torus,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            PrimitiveType::Box => "box",
            PrimitiveType::Cylinder => "cylinder",
            PrimitiveType::Cone => "cone",
            PrimitiveType::Sphere => "sphere",
            PrimitiveType::Torus => "torus",
        }
    }

    pub fn parse(s: &str) -> Result<PrimitiveType> {
        PrimitiveType::ALL
            .into_iter()
            .find(|t| t.as_str() == s)
            .ok_or_else(|| unknown_type(s))
    }

    /// The required parameters, each a length greater than zero.
    pub fn required(self) -> &'static [&'static str] {
        match self {
            PrimitiveType::Box => &["x", "y", "z"],
            PrimitiveType::Cylinder | PrimitiveType::Cone => &["radius", "height"],
            PrimitiveType::Sphere => &["radius"],
            PrimitiveType::Torus => &["major_radius", "minor_radius"],
        }
    }

    /// The optional parameters, each a length of zero or more.
    pub fn optional(self) -> &'static [&'static str] {
        match self {
            PrimitiveType::Cone => &["top_radius"],
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

    /// Reads a placement object, its origin's expressions reading `env`;
    /// `at` is its JSON pointer for errors.
    pub fn from_json(v: &Value, default: Unit, at: &str, env: &Env) -> Result<Placement> {
        let mut o = Obj::new(v, at)?;
        let origin = o.take("origin")?;
        let origin = triple(&origin, &format!("{at}/origin"), |v, at| {
            Quantity::from_json_in(v, default, &|n| env.get(n)).map_err(|e| e.with("pointer", at))
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
        let mut o = header(v, PROJECT_KIND, FORMAT)?;
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
        let mut o = header(v, PART_KIND, FORMAT)?;
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

impl ParametersFile {
    pub fn to_json(&self) -> Value {
        let params: Map<String, Value> = self
            .parameters
            .iter()
            .map(|(k, e)| (k.clone(), Value::String(e.text().to_owned())))
            .collect();
        json!({"format": FORMAT, "kind": PARAMETERS_KIND, "parameters": params})
    }

    /// Reads a parameters file: valid names, and expressions that parse. It
    /// doesn't evaluate them ([`Env::new`] does).
    pub fn from_json(v: &Value) -> Result<ParametersFile> {
        let mut o = header(v, PARAMETERS_KIND, FORMAT)?;
        let list = o.take("parameters")?;
        let map = list
            .as_object()
            .ok_or_else(|| invalid("/parameters", "parameters must be an object"))?;
        let mut parameters = BTreeMap::new();
        for (k, v) in map {
            let at = format!("/parameters/{k}");
            let name = expr::parameter_name(k).map_err(|e| e.with("pointer", &at))?;
            parameters.insert(name, expression(v).map_err(|e| e.with("pointer", &at))?);
        }
        o.finish()?;
        Ok(ParametersFile { parameters })
    }

    /// Their values.
    pub fn env(&self) -> Env {
        Env::new(&self.parameters)
    }
}

/// An expression from JSON: a string, or a number (a plain number).
pub fn expression(v: &Value) -> Result<Expr> {
    match v {
        Value::String(s) => Expr::parse(s),
        Value::Number(n) => Expr::parse(&n.to_string()),
        _ => Err(Error::new(
            Code::InvalidQuantity,
            format!("expected an expression string such as \"2 * wall\", got {v}"),
        )
        .with("value", v)),
    }
}

/// The expression strings in a feature's JSON: its params, the
/// coordinates of its origin and a sketch's dimensions.
fn expression_strings(v: &mut Value) -> Vec<&mut String> {
    let mut out = Vec::new();
    let Some(o) = v.as_object_mut() else {
        return out;
    };
    for (k, field) in o.iter_mut() {
        match (k.as_str(), field) {
            ("params", Value::Object(m)) => out.extend(m.values_mut().filter_map(|x| match x {
                Value::String(s) => Some(s),
                _ => None,
            })),
            ("placement", Value::Object(p)) => {
                if let Some(Value::Array(a)) = p.get_mut("origin") {
                    out.extend(a.iter_mut().filter_map(|x| match x {
                        Value::String(s) => Some(s),
                        _ => None,
                    }));
                }
            }
            ("constraints", c) => out.extend(Sketch::expression_strings(c)),
            _ => {}
        }
    }
    out
}

/// The parameters a feature's JSON names, read from its text alone, so a
/// feature that doesn't evaluate still says what it reads.
pub fn feature_names(v: &Value) -> std::collections::BTreeSet<String> {
    let mut v = v.clone();
    expression_strings(&mut v)
        .into_iter()
        .filter_map(|s| Expr::parse(s).ok())
        .flat_map(|e| e.names())
        .collect()
}

/// Renames a parameter in a feature's JSON, in place. Returns whether
/// anything changed; strings that don't parse are left as they are.
pub fn rename_in_feature(v: &mut Value, from: &str, to: &str) -> bool {
    let mut changed = false;
    for s in expression_strings(v) {
        if let Ok(e) = Expr::parse(s)
            && e.names().contains(from)
        {
            *s = e.rename(from, to).text().to_owned();
            changed = true;
        }
    }
    changed
}

/// The message for a `type` no feature has.
fn unknown_type(s: &str) -> Error {
    Error::new(
        Code::UnknownFeatureType,
        format!("unknown feature type {s:?}; known: box, cylinder, cone, sphere, torus, sketch"),
    )
    .with("type", s)
}

impl Feature {
    /// The `type` the file gives.
    pub fn type_name(&self) -> &'static str {
        match self {
            Feature::Primitive(p) => p.ty.as_str(),
            Feature::Sketch(_) => "sketch",
        }
    }

    /// The lowest format that holds this feature.
    pub fn format(&self) -> u64 {
        match self {
            Feature::Primitive(p) => p.format(),
            Feature::Sketch(s) => s.format(),
        }
    }

    pub fn to_json(&self) -> Value {
        let mut v = match self {
            Feature::Primitive(p) => p.fields(),
            Feature::Sketch(s) => s.fields(),
        };
        v["format"] = json!(self.format());
        v["kind"] = json!(FEATURE_KIND);
        v["type"] = json!(self.type_name());
        v
    }

    /// Reads and validates a feature, its expressions reading `env`. Bare
    /// numbers take `default`.
    pub fn from_json(v: &Value, default: Unit, env: &Env) -> Result<Feature> {
        let mut o = header(v, FEATURE_KIND, FEATURE_FORMAT)?;
        let ty = o.take("type")?;
        let ty = ty
            .as_str()
            .ok_or_else(|| invalid("/type", "type must be a string"))?;
        let f = if ty == "sketch" {
            Feature::Sketch(Sketch::from_fields(&mut o, default, env)?)
        } else {
            let ty = PrimitiveType::parse(ty).map_err(|e| e.with("pointer", "/type"))?;
            Feature::Primitive(Primitive::from_fields(ty, &mut o, default, env)?)
        };
        o.finish()?;
        Ok(f)
    }

    /// The primitive, if it is one.
    pub fn primitive(&self) -> Option<&Primitive> {
        match self {
            Feature::Primitive(p) => Some(p),
            _ => None,
        }
    }

    /// The sketch, if it is one.
    pub fn sketch(&self) -> Option<&Sketch> {
        match self {
            Feature::Sketch(s) => Some(s),
            _ => None,
        }
    }
}

impl Primitive {
    fn format(&self) -> u64 {
        let literal = self.params.values().all(Quantity::is_literal)
            && self.placement.origin.iter().all(Quantity::is_literal);
        if literal { 1 } else { FEATURE_FORMAT }
    }

    /// Its fields in a feature file, besides the header.
    fn fields(&self) -> Value {
        let params: Map<String, Value> = self
            .params
            .iter()
            .map(|(k, q)| (k.clone(), Value::String(q.text())))
            .collect();
        json!({"params": params, "placement": self.placement.to_json()})
    }

    fn from_fields(ty: PrimitiveType, o: &mut Obj, default: Unit, env: &Env) -> Result<Primitive> {
        let params = o.take("params")?;
        let params = Primitive::params_from_json(ty, &params, default, env)?;
        let placement = Placement::from_json(&o.take("placement")?, default, "/placement", env)?;
        Ok(Primitive {
            ty,
            params,
            placement,
        })
    }

    /// Validates a params object for `ty`: every required parameter present
    /// and positive, optional ones non-negative, no others.
    pub fn params_from_json(
        ty: PrimitiveType,
        v: &Value,
        default: Unit,
        env: &Env,
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
            let q = Quantity::from_json_in(v, default, &|n| env.get(n))
                .map_err(|e| e.with("pointer", &at))?;
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

fn param_error(ty: PrimitiveType, name: &str, at: &str, what: &str) -> Error {
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
pub fn number(x: f64) -> Value {
    if x.fract() == 0.0 && x.abs() < 9_007_199_254_740_992.0 {
        json!(x as i64)
    } else {
        json!(x)
    }
}

pub(crate) fn invalid(at: &str, msg: &str) -> Error {
    Error::new(Code::InvalidDocument, msg).with("pointer", at)
}

/// Checks `kind` and `format` (at most `newest`), migrating older formats,
/// and returns the remaining fields.
fn header<'a>(v: &'a Value, kind: &str, newest: u64) -> Result<Obj<'a>> {
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
    if format > newest {
        return Err(Error::new(
            Code::FormatTooNew,
            format!(
                "this file has format {format}; this ryucad reads up to {newest}. Upgrade ryucad"
            ),
        )
        .with("format", format)
        .with("supported", newest));
    }
    migrate(format, newest);
    Ok(o)
}

/// Brings an older format up to `newest`, one step at a time. Each later
/// format adds a `format => step` arm that rewrites the fields in place.
/// Feature format 2 only adds expressions, which format 1 files don't
/// have, so it needs no step.
fn migrate(format: u64, newest: u64) {
    debug_assert!(format <= newest);
}

/// The fields of a JSON object, taken one by one so leftovers are errors.
pub(crate) struct Obj<'a> {
    map: BTreeMap<&'a str, &'a Value>,
    at: String,
}

impl<'a> Obj<'a> {
    pub(crate) fn new(v: &'a Value, at: &str) -> Result<Obj<'a>> {
        let map = v
            .as_object()
            .ok_or_else(|| invalid(at, "expected a JSON object"))?;
        Ok(Obj {
            map: map.iter().map(|(k, v)| (k.as_str(), v)).collect(),
            at: at.to_owned(),
        })
    }

    /// A field that may be left out.
    pub(crate) fn take_opt(&mut self, key: &str) -> Option<Value> {
        self.map.remove(key).cloned()
    }

    pub(crate) fn take(&mut self, key: &str) -> Result<Value> {
        self.map.remove(key).cloned().ok_or_else(|| {
            invalid(
                &format!("{}/{key}", self.at),
                &format!("missing field {key:?}"),
            )
        })
    }

    pub(crate) fn finish(self) -> Result<()> {
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

    /// The primitive a feature holds.
    fn prim(f: &Feature) -> &Primitive {
        f.primitive().expect("a primitive")
    }

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
        let f = Feature::from_json(&cylinder_json(), Unit::Mm, &Env::empty()).unwrap();
        assert_eq!(prim(&f).ty, PrimitiveType::Cylinder);
        assert_eq!(prim(&f).meters("radius"), 0.0125);
        assert_eq!(
            canonical::to_string(&f.to_json()),
            "{\n  \"format\": 1,\n  \"kind\": \"ryucad.feature\",\n  \"params\": {\n    \"height\": \"40 mm\",\n    \"radius\": \"12.5 mm\"\n  },\n  \"placement\": {\n    \"origin\": [\n      \"0 mm\",\n      \"0 mm\",\n      \"10 mm\"\n    ],\n    \"x\": [\n      1,\n      0,\n      0\n    ],\n    \"z\": [\n      0,\n      0,\n      1\n    ]\n  },\n  \"type\": \"cylinder\"\n}\n"
        );
        // Written once, it reads back identically in any default unit.
        assert_eq!(
            Feature::from_json(&f.to_json(), Unit::Ft, &Env::empty()).unwrap(),
            f
        );
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
            ("/format", json!(3), Code::FormatTooNew, ""),
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
            let e = Feature::from_json(&v, Unit::Mm, &Env::empty()).unwrap_err();
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
        let e = Feature::from_json(&v, Unit::Mm, &Env::empty()).unwrap_err();
        assert_eq!(e.code, Code::InvalidParam);
        assert_eq!(e.details["param"], "height");
        v.as_object_mut().unwrap().remove("placement");
        v["params"]["height"] = json!(1);
        let e = Feature::from_json(&v, Unit::Mm, &Env::empty()).unwrap_err();
        assert_eq!(e.details["pointer"], "/placement");
    }

    #[test]
    fn cone_top_radius_is_optional_and_may_be_zero() {
        let mut v = cylinder_json();
        v["type"] = json!("cone");
        assert!(Feature::from_json(&v, Unit::Mm, &Env::empty()).is_ok());
        v["params"]["top_radius"] = json!(0);
        assert_eq!(
            prim(&Feature::from_json(&v, Unit::Mm, &Env::empty()).unwrap()).meters("top_radius"),
            0.0
        );
        v["params"]["radius"] = json!(0);
        assert_eq!(
            Feature::from_json(&v, Unit::Mm, &Env::empty())
                .unwrap_err()
                .code,
            Code::InvalidParam
        );
    }

    fn wall() -> Env {
        ParametersFile::from_json(&json!({
            "format": 1,
            "kind": "ryucad.parameters",
            "parameters": {"hole": "wall*2", "n": 4, "wall": "3mm"}
        }))
        .unwrap()
        .env()
    }

    #[test]
    fn parameters_round_trip() {
        let v = json!({
            "format": 1,
            "kind": "ryucad.parameters",
            "parameters": {"hole": "wall*2", "n": 4, "wall": "3mm"}
        });
        let p = ParametersFile::from_json(&v).unwrap();
        assert_eq!(
            p.to_json()["parameters"],
            json!({"hole": "wall * 2", "n": "4", "wall": "3 mm"})
        );
        assert_eq!(ParametersFile::from_json(&p.to_json()).unwrap(), p);
        assert_eq!(p.env().get("hole").unwrap().x, 0.006);
    }

    #[test]
    fn parameter_files_are_checked() {
        let base = |params: Value| {
            ParametersFile::from_json(&json!({
                "format": 1, "kind": "ryucad.parameters", "parameters": params
            }))
        };
        let e = base(json!({"Wall": "1 mm"})).unwrap_err();
        assert_eq!(
            (e.code, e.details["pointer"].as_str()),
            (Code::InvalidId, Some("/parameters/Wall"))
        );
        let e = base(json!({"mm": "1 mm"})).unwrap_err();
        assert_eq!(e.code, Code::InvalidId);
        let e = base(json!({"a": "1 +"})).unwrap_err();
        assert_eq!(
            (e.code, e.details["pointer"].as_str()),
            (Code::InvalidQuantity, Some("/parameters/a"))
        );
        let e = base(json!({"a": true})).unwrap_err();
        assert_eq!(e.code, Code::InvalidQuantity);
        assert_eq!(base(json!([])).unwrap_err().code, Code::InvalidDocument);
        // An unknown name or a cycle is the env's business, not the file's.
        assert!(base(json!({"a": "b"})).is_ok());
    }

    #[test]
    fn features_read_parameters() {
        let mut v = cylinder_json();
        v["params"]["radius"] = json!("hole/2+1mm");
        let f = Feature::from_json(&v, Unit::Mm, &wall()).unwrap();
        assert_eq!(prim(&f).meters("radius"), 0.004);
        assert_eq!(prim(&f).params["radius"].text(), "hole / 2 + 1 mm");
        assert_eq!(f.format(), 2);
        assert_eq!(f.to_json()["format"], 2);
        // Written at format 2, it reads back the same.
        assert_eq!(
            Feature::from_json(&f.to_json(), Unit::Ft, &wall()).unwrap(),
            f
        );
        v["placement"]["origin"][2] = json!("-wall");
        let f = Feature::from_json(&v, Unit::Mm, &wall()).unwrap();
        assert_eq!(prim(&f).placement.origin[2].meters(), -0.003);
        for (radius, code) in [
            ("gap", Code::UnknownParameter),
            ("n", Code::UnitMismatch),
            ("wall - hole", Code::InvalidParam),
        ] {
            v["params"]["radius"] = json!(radius);
            let e = Feature::from_json(&v, Unit::Mm, &wall()).unwrap_err();
            assert_eq!(
                (e.code, e.details["pointer"].as_str()),
                (code, Some("/params/radius")),
                "{radius}"
            );
        }
        // A literal-only feature stays at format 1.
        let f = Feature::from_json(&cylinder_json(), Unit::Mm, &wall()).unwrap();
        assert_eq!(f.format(), 1);
    }

    #[test]
    fn names_are_read_from_the_text() {
        let mut v = cylinder_json();
        v["params"]["radius"] = json!("wall*2");
        v["params"]["height"] = json!("1 +");
        v["placement"]["origin"][0] = json!("x0 + pi * 1 mm");
        assert_eq!(
            feature_names(&v).into_iter().collect::<Vec<_>>(),
            ["wall", "x0"]
        );
        assert!(rename_in_feature(&mut v, "wall", "skin"));
        assert_eq!(v["params"]["radius"], "skin * 2");
        assert_eq!(v["params"]["height"], "1 +");
        assert!(!rename_in_feature(&mut v, "wall", "skin"));
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
