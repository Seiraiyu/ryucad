//! Extrude and revolve features: a sketch's closed loops swept into solids,
//! each feature its own body.

use super::files::{Obj, invalid};
use super::id::Id;
use super::reference::Reference;
use crate::error::{Code, Error, Result};
use crate::expr::Env;
use crate::units::{Field, Quantity, Unit};
use serde_json::{Value, json};

/// How far an extrude goes along the sketch's normal.
#[derive(Clone, Debug, PartialEq)]
pub enum Extent {
    /// From the sketch's plane to this far along the direction.
    Blind(Quantity),
    /// This long in all, half each side of the sketch's plane.
    Symmetric(Quantity),
    /// From the sketch's plane to a planar face parallel to it, or to a
    /// vertex, measured along the direction, then `offset` further (a
    /// length, of either sign; zero when left out).
    UpTo {
        target: Reference,
        offset: Option<Quantity>,
    },
}

/// An extrude: the sketch's loops along its normal.
#[derive(Clone, Debug, PartialEq)]
pub struct Extrude {
    pub sketch: Id,
    /// An entity on each chosen loop; empty means every closed loop.
    pub loops: Vec<Id>,
    pub extent: Extent,
    /// Against the sketch's normal.
    pub reverse: bool,
}

/// How far a revolve turns.
#[derive(Clone, Debug, PartialEq)]
pub enum Angle {
    Full,
    Of(Quantity),
}

/// What a revolve turns about, directed: the turn is counterclockwise
/// about it.
#[derive(Clone, Debug, PartialEq)]
pub enum RevolveAxis {
    /// A line of the sketch, from its start to its end.
    Line(Id),
    /// A straight edge of an earlier feature in the sketch's plane, from
    /// its start to its end.
    Edge(Reference),
}

/// A revolve: the sketch's loops about an axis.
#[derive(Clone, Debug, PartialEq)]
pub struct Revolve {
    pub sketch: Id,
    pub loops: Vec<Id>,
    pub axis: RevolveAxis,
    pub angle: Angle,
    /// Half the angle each side of the sketch's plane.
    pub symmetric: bool,
}

fn invalid_extent(at: &str, msg: String) -> Error {
    Error::new(Code::InvalidExtent, msg).with("pointer", at)
}

impl Extent {
    /// A blind or symmetric extent's length.
    pub fn length(&self) -> Option<&Quantity> {
        match self {
            Extent::Blind(q) | Extent::Symmetric(q) => Some(q),
            Extent::UpTo { .. } => None,
        }
    }

    pub fn type_name(&self) -> &'static str {
        match self {
            Extent::Blind(_) => "blind",
            Extent::Symmetric(_) => "symmetric",
            Extent::UpTo { .. } => "up_to",
        }
    }

    pub fn to_json(&self) -> Value {
        let mut v = json!({"type": self.type_name()});
        match self {
            Extent::Blind(q) | Extent::Symmetric(q) => v["length"] = json!(q.text()),
            Extent::UpTo { target, offset } => {
                v["target"] = target.to_json();
                if let Some(q) = offset {
                    v["offset"] = json!(q.text());
                }
            }
        }
        v
    }

    /// Reads an extent, its quantities reading `env`: a blind or symmetric
    /// length greater than zero, or an up-to target and offset.
    pub fn from_json(v: &Value, default: Unit, env: &Env) -> Result<Extent> {
        let mut o = Obj::new(v, "/extent")?;
        let ty = o.take("type")?;
        let quantity = |v: &Value, at: &str| {
            Quantity::from_json_in(v, default, &|n| env.get(n)).map_err(|e| e.with("pointer", at))
        };
        let extent = match ty.as_str() {
            Some(t @ ("blind" | "symmetric")) => {
                let q = quantity(&o.take("length")?, "/extent/length")?;
                if q.meters() <= 0.0 {
                    return Err(invalid_extent(
                        "/extent/length",
                        format!("an extrude's length must be greater than zero, not {q}"),
                    ));
                }
                if t == "blind" {
                    Extent::Blind(q)
                } else {
                    Extent::Symmetric(q)
                }
            }
            Some("up_to") => Extent::UpTo {
                target: Reference::from_json(&o.take("target")?, "/extent/target")?,
                offset: o
                    .take_opt("offset")
                    .map(|v| quantity(&v, "/extent/offset"))
                    .transpose()?,
            },
            _ => {
                return Err(invalid_extent(
                    "/extent/type",
                    format!("an extent's type is blind, symmetric or up_to, not {ty}"),
                ));
            }
        };
        o.finish()?;
        Ok(extent)
    }
}

impl Angle {
    pub fn to_json(&self) -> Value {
        match self {
            Angle::Full => json!("full"),
            Angle::Of(q) => json!(q.text()),
        }
    }

    /// Reads `"full"` or an angle in `(0, 360 deg]`.
    pub fn from_json(v: &Value, env: &Env) -> Result<Angle> {
        if v == "full" {
            return Ok(Angle::Full);
        }
        let q = Quantity::from_json_as(v, Field::Angle, &|n| env.get(n))
            .map_err(|e| e.with("pointer", "/angle"))?;
        let a = q.radians();
        if a <= 0.0 || a > core::f64::consts::TAU + ryukei::tol::ANGULAR_RESOLUTION {
            return Err(invalid_extent(
                "/angle",
                format!("a revolve's angle is in (0, 360 deg], or \"full\"; not {q}"),
            ));
        }
        Ok(Angle::Of(q))
    }
}

/// Reads the `sketch` field.
fn sketch_id(o: &mut Obj) -> Result<Id> {
    let v = o.take("sketch")?;
    let s = v
        .as_str()
        .ok_or_else(|| invalid("/sketch", "sketch is a sketch feature's ID"))?;
    Id::parse(s, "feature").map_err(|e| e.with("pointer", "/sketch"))
}

/// Reads the optional `loops` field: entity IDs.
pub fn loops_from_json(v: Option<Value>) -> Result<Vec<Id>> {
    let Some(v) = v else {
        return Ok(Vec::new());
    };
    let a = v
        .as_array()
        .ok_or_else(|| invalid("/loops", "loops is an array of entity IDs"))?;
    a.iter()
        .enumerate()
        .map(|(i, x)| {
            let at = format!("/loops/{i}");
            let s = x
                .as_str()
                .ok_or_else(|| invalid(&at, "expected an entity's ID"))?;
            Id::parse(s, "entity").map_err(|e| e.with("pointer", &at))
        })
        .collect()
}

/// Reads an optional `true`/`false` field, false when left out.
fn flag(o: &mut Obj, key: &str) -> Result<bool> {
    match o.take_opt(key) {
        None => Ok(false),
        Some(Value::Bool(b)) => Ok(b),
        Some(_) => Err(invalid(
            &format!("/{key}"),
            &format!("{key} is true or false"),
        )),
    }
}

/// Writes the fields every sweep has.
fn common(v: &mut Value, sketch: &Id, loops: &[Id]) {
    v["sketch"] = json!(sketch);
    if !loops.is_empty() {
        v["loops"] = json!(loops);
    }
}

impl Extrude {
    pub fn fields(&self) -> Value {
        let mut v = json!({"extent": self.extent.to_json()});
        common(&mut v, &self.sketch, &self.loops);
        if self.reverse {
            v["reverse"] = json!(true);
        }
        v
    }

    #[doc(hidden)]
    pub fn from_fields(o: &mut Obj, default: Unit, env: &Env) -> Result<Extrude> {
        Ok(Extrude {
            sketch: sketch_id(o)?,
            loops: loops_from_json(o.take_opt("loops"))?,
            extent: Extent::from_json(&o.take("extent")?, default, env)?,
            reverse: flag(o, "reverse")?,
        })
    }
}

impl Revolve {
    pub fn fields(&self) -> Value {
        let axis = match &self.axis {
            RevolveAxis::Line(l) => json!(l),
            RevolveAxis::Edge(r) => r.to_json(),
        };
        let mut v = json!({"axis": axis, "angle": self.angle.to_json()});
        common(&mut v, &self.sketch, &self.loops);
        if self.symmetric {
            v["symmetric"] = json!(true);
        }
        v
    }

    #[doc(hidden)]
    pub fn from_fields(o: &mut Obj, env: &Env) -> Result<Revolve> {
        let axis = match o.take("axis")? {
            Value::String(l) => {
                RevolveAxis::Line(Id::parse(&l, "entity").map_err(|e| e.with("pointer", "/axis"))?)
            }
            v @ Value::Object(_) => RevolveAxis::Edge(Reference::from_json(&v, "/axis")?),
            _ => {
                return Err(invalid(
                    "/axis",
                    "axis is a line of the sketch, or {feature, name}: a straight edge",
                ));
            }
        };
        Ok(Revolve {
            sketch: sketch_id(o)?,
            loops: loops_from_json(o.take_opt("loops"))?,
            axis,
            angle: Angle::from_json(&o.take("angle")?, env)?,
            symmetric: flag(o, "symmetric")?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::canonical;
    use crate::doc::files::Feature;

    fn read(v: &Value) -> Result<Feature> {
        let p = [(
            "depth".to_owned(),
            crate::expr::Expr::parse("5 mm").unwrap(),
        )]
        .into();
        Feature::from_json(v, Unit::Mm, &Env::new(&p))
    }

    fn extrude() -> Value {
        json!({"extent": {"length": "depth * 2", "type": "symmetric"}, "format": 2,
               "kind": "ryucad.feature", "sketch": "s", "type": "extrude"})
    }

    fn revolve() -> Value {
        json!({"angle": 90, "axis": "l5", "format": 2, "kind": "ryucad.feature",
               "loops": ["l1"], "sketch": "s", "symmetric": true, "type": "revolve"})
    }

    #[test]
    fn sweeps_round_trip() {
        for v in [extrude(), revolve()] {
            let f = read(&v).unwrap();
            let text = canonical::to_string(&f.to_json());
            assert_eq!(read(&canonical::parse(&text).unwrap()).unwrap(), f);
        }
        let Feature::Extrude(e) = read(&extrude()).unwrap() else {
            panic!()
        };
        assert_eq!(
            e.extent,
            Extent::Symmetric(
                Quantity::eval("depth * 2", Unit::Mm, &|_| {
                    Ok(crate::expr::Val::new(0.005, crate::units::Dim::LENGTH))
                })
                .unwrap()
            )
        );
        assert!(e.loops.is_empty() && !e.reverse);
        let Feature::Revolve(r) = read(&revolve()).unwrap() else {
            panic!()
        };
        assert_eq!(r.angle.to_json(), "90 deg");
        assert!(r.symmetric);
        let mut v = revolve();
        v["angle"] = json!("full");
        let Feature::Revolve(r) = read(&v).unwrap() else {
            panic!()
        };
        assert_eq!(r.angle, Angle::Full);
    }

    #[test]
    fn up_to_a_target() {
        const N: &str = "0123456789abcdef0123456789abcdef";
        let mut v = extrude();
        v["extent"] = json!({"type": "up_to", "target": {"feature": "b", "name": N},
                             "offset": "-depth"});
        let f = read(&v).unwrap();
        let Feature::Extrude(e) = &f else { panic!() };
        let Extent::UpTo { target, offset } = &e.extent else {
            panic!()
        };
        assert_eq!(target.feature.as_str(), "b");
        assert_eq!(offset.as_ref().unwrap().meters(), -0.005);
        assert!(e.extent.length().is_none());
        assert_eq!(
            f.references()
                .iter()
                .map(|(r, _)| r.as_str())
                .collect::<Vec<_>>(),
            ["target"]
        );
        let text = canonical::to_string(&f.to_json());
        assert_eq!(read(&canonical::parse(&text).unwrap()).unwrap(), f);
        v["extent"]["length"] = json!(3);
        assert_eq!(read(&v).unwrap_err().details["pointer"], "/extent/length");
    }

    #[test]
    fn an_axis_may_be_an_edge() {
        const N: &str = "0123456789abcdef0123456789abcdef";
        let mut v = revolve();
        v["axis"] = json!({"feature": "b", "name": N});
        let f = read(&v).unwrap();
        let Feature::Revolve(r) = &f else { panic!() };
        assert!(matches!(&r.axis, RevolveAxis::Edge(e) if e.feature.as_str() == "b"));
        assert_eq!(
            f.references()
                .iter()
                .map(|(r, _)| r.as_str())
                .collect::<Vec<_>>(),
            ["axis"]
        );
        assert_eq!(read(&f.to_json()).unwrap(), f);
        v["axis"] = json!(3);
        assert_eq!(read(&v).unwrap_err().details["pointer"], "/axis");
    }

    #[test]
    fn extents_are_checked() {
        let cases: [(&str, Value, Code, &str); 8] = [
            (
                "/extent/length",
                json!("-1 mm"),
                Code::InvalidExtent,
                "/extent/length",
            ),
            (
                "/extent/length",
                json!("1 deg"),
                Code::UnitMismatch,
                "/extent/length",
            ),
            (
                "/extent/type",
                json!("up_to"),
                Code::InvalidDocument,
                "/extent/target",
            ),
            (
                "/extent/type",
                json!("through"),
                Code::InvalidExtent,
                "/extent/type",
            ),
            ("/sketch", json!("S"), Code::InvalidId, "/sketch"),
            (
                "/loops",
                json!(["l1", 2]),
                Code::InvalidDocument,
                "/loops/1",
            ),
            ("/reverse", json!("yes"), Code::InvalidDocument, "/reverse"),
            (
                "/extent/glue",
                json!(1),
                Code::InvalidDocument,
                "/extent/glue",
            ),
        ];
        for (path, value, code, pointer) in cases {
            let mut v = extrude();
            let (parent, key) = path.rsplit_once('/').unwrap();
            v.pointer_mut(if parent.is_empty() { "" } else { parent })
                .unwrap()[key] = value;
            let e = read(&v).unwrap_err();
            assert_eq!(
                (e.code, e.details["pointer"].as_str()),
                (code, Some(pointer)),
                "{path}: {e}"
            );
        }
        for (angle, code) in [
            (json!(0), Code::InvalidExtent),
            (json!("361 deg"), Code::InvalidExtent),
            (json!("1 mm"), Code::UnitMismatch),
            (json!("half"), Code::UnknownParameter),
        ] {
            let mut v = revolve();
            v["angle"] = angle.clone();
            assert_eq!(read(&v).unwrap_err().code, code, "{angle}");
        }
        assert_eq!(
            read(&json!({"format": 1, "kind": "ryucad.feature", "type": "extrude"}))
                .unwrap_err()
                .code,
            Code::InvalidDocument
        );
    }
}
