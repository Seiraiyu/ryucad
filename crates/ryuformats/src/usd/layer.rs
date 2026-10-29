//! A small model of a USD layer: what ryucad writes and nothing more.
//!
//! A [`Layer`] holds its metadata and its root prims; a [`Prim`] its
//! specifier, type, name, metadata, attributes and children, in the order
//! they are added, which is the order they are written. Attributes are
//! `uniform` or varying with one default value and no time samples.
//! Names are checked when a prim or an attribute is made: a prim name is
//! an identifier (`[A-Za-z_][A-Za-z0-9_]*`), an attribute name one or more
//! identifiers joined by `:`. Values hold only finite numbers: a scene
//! never makes another, and the writers refuse one.

use std::fmt;

/// A name a prim or an attribute can't take.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BadName(pub String);

impl fmt::Display for BadName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?} isn't a USD name", self.0)
    }
}

impl std::error::Error for BadName {}

/// Whether `s` is a USD identifier.
pub fn is_identifier(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some(c) if c == '_' || c.is_ascii_alphabetic())
        && chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

/// Whether `s` is a property name: identifiers joined by `:`.
pub fn is_property_name(s: &str) -> bool {
    !s.is_empty() && s.split(':').all(is_identifier)
}

/// A layer: its metadata and its root prims.
#[derive(Clone, Debug, PartialEq)]
pub struct Layer {
    pub default_prim: Option<String>,
    pub meters_per_unit: f64,
    pub up_axis: UpAxis,
    pub custom_layer_data: Dict,
    pub prims: Vec<Prim>,
}

/// The stage's up axis.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpAxis {
    Y,
    Z,
}

impl UpAxis {
    pub fn as_str(self) -> &'static str {
        match self {
            UpAxis::Y => "Y",
            UpAxis::Z => "Z",
        }
    }
}

/// How a prim is specified.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Specifier {
    Def,
    Class,
}

impl Specifier {
    pub fn as_str(self) -> &'static str {
        match self {
            Specifier::Def => "def",
            Specifier::Class => "class",
        }
    }
}

/// A prim: its specifier, type (if any), name, metadata, attributes and
/// children.
#[derive(Clone, Debug, PartialEq)]
pub struct Prim {
    pub specifier: Specifier,
    pub type_name: Option<String>,
    pub name: String,
    pub meta: PrimMeta,
    pub attributes: Vec<Attribute>,
    pub children: Vec<Prim>,
}

/// A prim's metadata, written in this order: applied API schemas
/// (`prepend apiSchemas`), `customData`, `instanceable`, `kind`, and
/// internal references (`prepend references`, prim paths in this layer).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PrimMeta {
    pub api_schemas: Vec<String>,
    pub custom_data: Dict,
    pub instanceable: Option<bool>,
    pub kind: Option<String>,
    pub references: Vec<String>,
}

impl PrimMeta {
    pub fn is_empty(&self) -> bool {
        self.api_schemas.is_empty()
            && self.custom_data.0.is_empty()
            && self.instanceable.is_none()
            && self.kind.is_none()
            && self.references.is_empty()
    }
}

impl Prim {
    /// A prim named `name` of `type_name` (none for a typeless prim).
    pub fn new(specifier: Specifier, type_name: Option<&str>, name: &str) -> Result<Prim, BadName> {
        if !is_identifier(name) {
            return Err(BadName(name.to_owned()));
        }
        if let Some(t) = type_name
            && !is_identifier(t)
        {
            return Err(BadName(t.to_owned()));
        }
        Ok(Prim {
            specifier,
            type_name: type_name.map(str::to_owned),
            name: name.to_owned(),
            meta: PrimMeta::default(),
            attributes: Vec::new(),
            children: Vec::new(),
        })
    }

    /// Adds a `uniform` attribute.
    pub fn uniform(&mut self, name: &str, value: Value) -> Result<&mut Attribute, BadName> {
        self.push(Attribute::new(name, value)?.uniform())
    }

    /// Adds a varying attribute.
    pub fn varying(&mut self, name: &str, value: Value) -> Result<&mut Attribute, BadName> {
        self.push(Attribute::new(name, value)?)
    }

    fn push(&mut self, a: Attribute) -> Result<&mut Attribute, BadName> {
        self.attributes.push(a);
        Ok(self.attributes.last_mut().expect("just pushed"))
    }
}

/// An attribute: its name, whether it is `uniform`, whether it is
/// `custom` (no schema declares it), its value and, for a primvar, its
/// interpolation.
#[derive(Clone, Debug, PartialEq)]
pub struct Attribute {
    pub name: String,
    pub uniform: bool,
    pub custom: bool,
    pub value: Value,
    pub interpolation: Option<String>,
}

impl Attribute {
    pub fn new(name: &str, value: Value) -> Result<Attribute, BadName> {
        if !is_property_name(name) {
            return Err(BadName(name.to_owned()));
        }
        Ok(Attribute {
            name: name.to_owned(),
            uniform: false,
            custom: false,
            value,
            interpolation: None,
        })
    }

    pub fn uniform(mut self) -> Attribute {
        self.uniform = true;
        self
    }

    /// Marks it `custom`.
    pub fn custom(&mut self) -> &mut Attribute {
        self.custom = true;
        self
    }

    /// Gives it an interpolation (`faceVarying`, `uniform`, …).
    pub fn interpolation(&mut self, i: &str) -> &mut Attribute {
        self.interpolation = Some(i.to_owned());
        self
    }
}

/// An attribute's value, with its USD type.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Bool(bool),
    Int(i32),
    Double(f64),
    Token(String),
    String(String),
    IntArray(Vec<i32>),
    UIntArray(Vec<u32>),
    DoubleArray(Vec<f64>),
    TokenArray(Vec<String>),
    StringArray(Vec<String>),
    Int2Array(Vec<[i32; 2]>),
    Double2Array(Vec<[f64; 2]>),
    Double3Array(Vec<[f64; 3]>),
    Point3dArray(Vec<[f64; 3]>),
    Vector3dArray(Vec<[f64; 3]>),
    Point3fArray(Vec<[f32; 3]>),
    Normal3fArray(Vec<[f32; 3]>),
    Float3Array(Vec<[f32; 3]>),
    /// Row by row, the translation in the last row.
    Matrix4d([[f64; 4]; 4]),
}

impl Value {
    /// Its USD type name.
    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Bool(_) => "bool",
            Value::Int(_) => "int",
            Value::Double(_) => "double",
            Value::Token(_) => "token",
            Value::String(_) => "string",
            Value::IntArray(_) => "int[]",
            Value::UIntArray(_) => "uint[]",
            Value::DoubleArray(_) => "double[]",
            Value::TokenArray(_) => "token[]",
            Value::StringArray(_) => "string[]",
            Value::Int2Array(_) => "int2[]",
            Value::Double2Array(_) => "double2[]",
            Value::Double3Array(_) => "double3[]",
            Value::Point3dArray(_) => "point3d[]",
            Value::Vector3dArray(_) => "vector3d[]",
            Value::Point3fArray(_) => "point3f[]",
            Value::Normal3fArray(_) => "normal3f[]",
            Value::Float3Array(_) => "float3[]",
            Value::Matrix4d(_) => "matrix4d",
        }
    }

    /// Whether every number it holds is finite.
    pub fn is_finite(&self) -> bool {
        let f64s = |v: &[f64]| v.iter().all(|x| x.is_finite());
        let f32s = |v: &[[f32; 3]]| v.iter().flatten().all(|x| x.is_finite());
        match self {
            Value::Double(x) => x.is_finite(),
            Value::DoubleArray(v) => f64s(v),
            Value::Double2Array(v) => v.iter().flatten().all(|x| x.is_finite()),
            Value::Double3Array(v) | Value::Point3dArray(v) | Value::Vector3dArray(v) => {
                v.iter().flatten().all(|x| x.is_finite())
            }
            Value::Point3fArray(v) | Value::Normal3fArray(v) | Value::Float3Array(v) => f32s(v),
            Value::Matrix4d(m) => m.iter().flatten().all(|x| x.is_finite()),
            _ => true,
        }
    }
}

/// A dictionary (`customData`, `customLayerData`): entries in the order
/// given, which is the order written.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Dict(pub Vec<(String, DictValue)>);

/// A dictionary's value.
#[derive(Clone, Debug, PartialEq)]
pub enum DictValue {
    String(String),
    Double(f64),
    Dict(Dict),
}

impl Dict {
    /// Adds a string entry.
    pub fn string(&mut self, key: &str, value: &str) -> &mut Dict {
        self.0
            .push((key.to_owned(), DictValue::String(value.to_owned())));
        self
    }

    /// Adds a nested dictionary.
    pub fn dict(&mut self, key: &str, value: Dict) -> &mut Dict {
        self.0.push((key.to_owned(), DictValue::Dict(value)));
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_checked() {
        assert!(is_identifier("_9a"));
        assert!(is_identifier("Prototypes"));
        assert!(!is_identifier("9a"));
        assert!(!is_identifier("a-b"));
        assert!(!is_identifier(""));
        assert!(is_property_name("brep:edge3dLine:curve3d:line:origin"));
        assert!(!is_property_name("brep::x"));
        assert!(!is_property_name("primvars:ryucad:face-name"));
        assert!(Prim::new(Specifier::Def, Some("Xform"), "a-b").is_err());
        assert!(Prim::new(Specifier::Def, Some("Bad Type"), "a").is_err());
        let mut p = Prim::new(Specifier::Def, None, "a").unwrap();
        assert!(p.uniform("x:", Value::Bool(true)).is_err());
        p.uniform("x:y", Value::Bool(true)).unwrap().custom();
        assert!(p.attributes[0].uniform && p.attributes[0].custom);
    }

    #[test]
    fn values_know_their_types() {
        assert_eq!(Value::Int2Array(vec![]).type_name(), "int2[]");
        assert_eq!(Value::Matrix4d([[0.0; 4]; 4]).type_name(), "matrix4d");
        assert!(!Value::DoubleArray(vec![1.0, f64::NAN]).is_finite());
        assert!(!Value::Point3fArray(vec![[0.0, f32::INFINITY, 0.0]]).is_finite());
        assert!(Value::TokenArray(vec!["same".into()]).is_finite());
    }
}
