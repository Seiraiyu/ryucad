//! Surface features (ryucad 18.B, design 388 phases 4 and 5): a sheet
//! spanning a loop of edges and curves (`fill`), faces of a body copied
//! into a sheet (`extract`), sheets sewn into one body (`sew`, which
//! consumes them), and a sheet or a body's faces given thickness as a
//! solid (`thicken`, joined to, cut from or intersected with solids as an
//! extrude's body is). Each makes a body of its own, named by its ID.

use super::files::{Obj, invalid};
use super::geometry::GeomRef;
use super::id::Id;
use super::reference::Reference;
use super::solid::{Combine, Operation, targets_from_json};
use crate::error::{Code, Error, Result};
use crate::expr::Env;
use crate::units::{Field, Quantity, Unit};
use serde_json::{Value, json};

/// The feature format fills, extracts, sews and thickens are new in.
pub const FORMAT: u64 = 15;

/// The smallest and largest tolerance a fill or a sew takes, in meters:
/// the kernel's linear resolution and its tolerance cap.
pub const TOLERANCE_RANGE: (f64, f64) = (1e-8, 1e-4);

/// One side of a fill, in order round its loop.
#[derive(Clone, Debug, PartialEq)]
pub enum Side {
    /// An edge of any body by stable name; with `tangent` the fill meets
    /// its one face tangentially (the edge must be laminar).
    Edge { edge: Reference, tangent: bool },
    /// A curve: a sketch's line, arc or circle, a path or a helix (each of
    /// whose pieces is a side), or an edge taken as a curve.
    Curve(GeomRef),
}

/// A fill: a sheet spanning `sides`, each meeting the next within
/// `tolerance`, tangent sides and inner edges within `angle`.
#[derive(Clone, Debug, PartialEq)]
pub struct Fill {
    pub sides: Vec<Side>,
    pub tolerance: Quantity,
    pub angle: Quantity,
}

/// The fields a request may give a fill, an extract, a sew or a thicken.
pub fn request_fields(ty: &str) -> &'static [&'static str] {
    match ty {
        "fill" => &["sides", "edges", "curve", "tolerance", "angle"],
        "extract" => &["faces", "face", "face_list"],
        "sew" => &["sheets", "tolerance", "make_solid"],
        _ => &[
            "sheet",
            "sheets",
            "from_faces",
            "face",
            "face_list",
            "front",
            "back",
            "face_sides",
            "operation",
            "targets",
        ],
    }
}

impl Fill {
    /// Its fields: every side, a tangent flag only when true, the
    /// tolerance and angle always.
    pub fn fields(&self) -> Value {
        let sides: Vec<Value> = self
            .sides
            .iter()
            .map(|s| match s {
                Side::Edge { edge, tangent } => {
                    let mut v = json!({"edge": edge.to_json()});
                    if *tangent {
                        v["tangent"] = json!(true);
                    }
                    v
                }
                Side::Curve(g) => json!({"curve": g.to_json()}),
            })
            .collect();
        json!({"sides": sides, "tolerance": self.tolerance.text(), "angle": self.angle.text()})
    }

    #[doc(hidden)]
    pub fn from_fields(o: &mut Obj, default: Unit, env: &Env) -> Result<Fill> {
        new_only(o, "a fill")?;
        let v = o.take("sides")?;
        let a = v
            .as_array()
            .filter(|a| !a.is_empty())
            .ok_or_else(|| invalid("/sides", "sides is a non-empty array of {edge, tangent?} or {curve}"))?;
        let mut sides = Vec::with_capacity(a.len());
        for (i, s) in a.iter().enumerate() {
            let at = format!("/sides/{i}");
            let mut f = Obj::new(s, &at)?;
            let side = if f.has("edge") {
                let edge = Reference::from_json(&f.take("edge")?, &format!("{at}/edge"))?;
                let tangent = match f.take_opt("tangent") {
                    None => false,
                    Some(Value::Bool(b)) => b,
                    Some(_) => return Err(invalid(&format!("{at}/tangent"), "tangent is true or false")),
                };
                Side::Edge { edge, tangent }
            } else if f.has("curve") {
                if f.has("tangent") {
                    return Err(invalid(&at, "only an edge side meets its face tangentially"));
                }
                Side::Curve(GeomRef::from_json(&f.take("curve")?, &format!("{at}/curve"))?)
            } else {
                return Err(invalid(&at, "a side is {edge, tangent?} or {curve}"));
            };
            f.finish()?;
            sides.push(side);
        }
        let tolerance = tolerance(o, "0.001 mm", default, env)?;
        let angle = match o.take_opt("angle") {
            None => Quantity::from_json_as(&json!("1 deg"), Field::Angle, &|n| env.get(n))?,
            Some(v) => Quantity::from_json_as(&v, Field::Angle, &|n| env.get(n))
                .map_err(|e| e.with("pointer", "/angle"))?,
        };
        let r = angle.radians();
        if !(r > 0.0 && r < core::f64::consts::FRAC_PI_2) {
            return Err(Error::new(
                Code::InvalidParam,
                format!("a fill's angle is more than 0 and less than 90 deg, not {angle}"),
            )
            .with("pointer", "/angle")
            .with("value", angle.text()));
        }
        Ok(Fill { sides, tolerance, angle })
    }

    /// Its edge sides' references, each with its role `sides.<i>.edge`.
    pub fn references(&self) -> Vec<(String, &Reference)> {
        self.sides
            .iter()
            .enumerate()
            .filter_map(|(i, s)| match s {
                Side::Edge { edge, .. } => Some((format!("sides.{i}.edge"), edge)),
                Side::Curve(_) => None,
            })
            .collect()
    }

    /// Its curve sides, each with its role `sides.<i>.curve`.
    pub fn geometry(&self) -> Vec<(String, &GeomRef)> {
        self.sides
            .iter()
            .enumerate()
            .filter_map(|(i, s)| match s {
                Side::Curve(g) => Some((format!("sides.{i}.curve"), g)),
                Side::Edge { .. } => None,
            })
            .collect()
    }
}

/// An extract: faces of one body, as the extract sees it, copied into a
/// sheet of its own; the body is left as it was.
#[derive(Clone, Debug, PartialEq)]
pub struct Extract {
    pub faces: Vec<Reference>,
}

impl Extract {
    pub fn fields(&self) -> Value {
        json!({"faces": refs_json(&self.faces)})
    }

    #[doc(hidden)]
    pub fn from_fields(o: &mut Obj) -> Result<Extract> {
        new_only(o, "an extract")?;
        let faces = refs(&o.take("faces")?, "/faces")?;
        let x = Extract { faces };
        once(x.references())?;
        Ok(x)
    }

    /// Its faces, each with its role `faces.<i>`.
    pub fn references(&self) -> Vec<(String, &Reference)> {
        self.faces
            .iter()
            .enumerate()
            .map(|(i, r)| (format!("faces.{i}"), r))
            .collect()
    }
}

/// A sew: two or more sheets, by founder, sewn along their laminar edges
/// within `tolerance` into one body, which the sew founds and which
/// consumes them; a result that closes up is a solid when `solid` is
/// true, else a closed sheet.
#[derive(Clone, Debug, PartialEq)]
pub struct Sew {
    pub sheets: Vec<Id>,
    pub tolerance: Quantity,
    pub solid: bool,
}

impl Sew {
    pub fn fields(&self) -> Value {
        json!({"sheets": self.sheets, "tolerance": self.tolerance.text(), "solid": self.solid})
    }

    #[doc(hidden)]
    pub fn from_fields(o: &mut Obj, default: Unit, env: &Env) -> Result<Sew> {
        new_only(o, "a sew")?;
        let sheets = targets_from_json(&o.take("sheets")?, "/sheets")?;
        if sheets.len() < 2 {
            return Err(Error::new(
                Code::InvalidParam,
                "a sew joins two sheets or more: name each by the feature that founded it",
            )
            .with("pointer", "/sheets"));
        }
        let tolerance = tolerance(o, "0.01 mm", default, env)?;
        let solid = match o.take_opt("solid") {
            None => true,
            Some(Value::Bool(b)) => b,
            Some(_) => return Err(invalid("/solid", "solid is true or false")),
        };
        Ok(Sew { sheets, tolerance, solid })
    }
}

/// Refuses an operation or targets on a feature that founds a sheet of
/// its own (`what`: "an extract").
fn new_only(o: &Obj, what: &str) -> Result<()> {
    if o.has("operation") || o.has("targets") {
        return Err(Error::new(
            Code::InvalidParam,
            format!("{what} founds a body of its own: it has no operation or targets"),
        )
        .with("pointer", "/operation"));
    }
    Ok(())
}

/// A non-empty array of face references.
fn refs(v: &Value, at: &str) -> Result<Vec<Reference>> {
    let a = v
        .as_array()
        .filter(|a| !a.is_empty())
        .ok_or_else(|| invalid(at, "a non-empty array of faces {feature, name}"))?;
    a.iter()
        .enumerate()
        .map(|(j, r)| Reference::from_json(r, &format!("{at}/{j}")))
        .collect()
}

fn refs_json(refs: &[Reference]) -> Value {
    Value::Array(refs.iter().map(Reference::to_json).collect())
}

/// Refuses a face named twice; the pointer is the second naming's.
fn once(refs: Vec<(String, &Reference)>) -> Result<()> {
    for (k, (role, r)) in refs.iter().enumerate() {
        if let Some((first, _)) = refs[..k].iter().find(|(_, x)| x == r) {
            return Err(Error::new(
                Code::InvalidParam,
                format!("face {}/{} is named twice, as {first} and {role}", r.feature, r.name),
            )
            .with("pointer", format!("/{}", role.replace('.', "/")))
            .with("first", format!("/{}", first.replace('.', "/"))));
        }
    }
    Ok(())
}

/// `tolerance` (else `default_text`): a length within
/// [`TOLERANCE_RANGE`].
fn tolerance(o: &mut Obj, default_text: &str, default: Unit, env: &Env) -> Result<Quantity> {
    let q = match o.take_opt("tolerance") {
        None => Quantity::from_json_as(&json!(default_text), Field::Length(default), &|n| env.get(n))?,
        Some(v) => Quantity::from_json_as(&v, Field::Length(default), &|n| env.get(n))
            .map_err(|e| e.with("pointer", "/tolerance"))?,
    };
    let (lo, hi) = TOLERANCE_RANGE;
    let m = q.meters();
    if !(lo..=hi).contains(&m) {
        return Err(Error::new(
            Code::InvalidParam,
            format!("a tolerance is from 0.00001 mm to 0.1 mm, not {q}"),
        )
        .with("pointer", "/tolerance")
        .with("value", q.text()));
    }
    Ok(q)
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "0123456789abcdef0123456789abcdef";

    fn fill(v: Value) -> Result<Fill> {
        let mut o = Obj::new(&v, "")?;
        let f = Fill::from_fields(&mut o, Unit::Mm, &Env::empty())?;
        o.finish()?;
        Ok(f)
    }

    fn extract(v: Value) -> Result<Extract> {
        let mut o = Obj::new(&v, "")?;
        let x = Extract::from_fields(&mut o)?;
        o.finish()?;
        Ok(x)
    }

    #[test]
    fn extracts_round_trip_and_are_checked() {
        const B: &str = "fedcba9876543210fedcba9876543210";
        let face = |n: &str| json!({"feature": "base", "name": n});
        let x = extract(json!({"faces": [face(A), face(B)]})).unwrap();
        assert_eq!(x.fields(), json!({"faces": [face(A), face(B)]}));
        assert_eq!(extract(x.fields()).unwrap(), x);
        let roles: Vec<String> = x.references().into_iter().map(|(r, _)| r).collect();
        assert_eq!(roles, ["faces.0", "faces.1"]);
        for (v, code, pointer) in [
            (json!({}), Code::InvalidDocument, "/faces"),
            (json!({"faces": []}), Code::InvalidDocument, "/faces"),
            (json!({"faces": [face(A), face(A)]}), Code::InvalidParam, "/faces/1"),
            (json!({"faces": [face(A)], "operation": "join"}), Code::InvalidParam, "/operation"),
        ] {
            let e = extract(v.clone()).unwrap_err();
            assert_eq!((e.code, e.details["pointer"].as_str()), (code, Some(pointer)), "{v}");
        }
    }

    fn sew(v: Value) -> Result<Sew> {
        let mut o = Obj::new(&v, "")?;
        let s = Sew::from_fields(&mut o, Unit::Mm, &Env::empty())?;
        o.finish()?;
        Ok(s)
    }

    #[test]
    fn sews_round_trip_and_are_checked() {
        let s = sew(json!({"sheets": ["top", "side1", "side2"]})).unwrap();
        // The tolerance and solid are always written, defaults included.
        assert_eq!(s.fields(), json!({"sheets": ["top", "side1", "side2"],
                                      "tolerance": "0.01 mm", "solid": true}));
        assert_eq!(sew(s.fields()).unwrap(), s);
        let s = sew(json!({"sheets": ["a", "b"], "tolerance": "0.05 mm", "solid": false})).unwrap();
        assert!(!s.solid);
        assert_eq!(s.tolerance.meters(), 5e-5);
        for (v, code, pointer) in [
            (json!({}), Code::InvalidDocument, "/sheets"),
            (json!({"sheets": ["a"]}), Code::InvalidParam, "/sheets"),
            (json!({"sheets": ["a", "a"]}), Code::InvalidDocument, "/sheets/1"),
            (json!({"sheets": ["a", "b"], "solid": "yes"}), Code::InvalidDocument, "/solid"),
            (json!({"sheets": ["a", "b"], "tolerance": "1 mm"}), Code::InvalidParam, "/tolerance"),
            (json!({"sheets": ["a", "b"], "operation": "join"}), Code::InvalidParam, "/operation"),
        ] {
            let e = sew(v.clone()).unwrap_err();
            assert_eq!((e.code, e.details["pointer"].as_str()), (code, Some(pointer)), "{v}");
        }
    }

    #[test]
    fn fills_round_trip() {
        let v = json!({"sides": [
            {"edge": {"feature": "wall", "name": A}, "tangent": true},
            {"curve": {"feature": "s2", "entity": "a1"}},
            {"curve": {"feature": "p1"}}]});
        let f = fill(v).unwrap();
        assert_eq!(f.sides.len(), 3);
        // The tolerance and angle are always written, defaults included.
        assert_eq!(
            f.fields(),
            json!({"sides": [
                {"edge": {"feature": "wall", "name": A}, "tangent": true},
                {"curve": {"feature": "s2", "entity": "a1"}},
                {"curve": {"feature": "p1"}}],
                "tolerance": "0.001 mm", "angle": "1 deg"})
        );
        assert_eq!(fill(f.fields()).unwrap(), f);
        // An edge that isn't tangent writes no flag.
        let f = fill(json!({"sides": [{"edge": {"feature": "w", "name": A}, "tangent": false},
                                       {"curve": "x"}, {"curve": "y"}],
                            "tolerance": "0.01 mm", "angle": 2})).unwrap();
        assert_eq!(f.fields()["sides"][0], json!({"edge": {"feature": "w", "name": A}}));
        assert_eq!(f.angle.radians(), 2f64.to_radians());
        let roles: Vec<String> = f.references().into_iter().map(|(r, _)| r).collect();
        assert_eq!(roles, ["sides.0.edge"]);
        let roles: Vec<String> = f.geometry().into_iter().map(|(r, _)| r).collect();
        assert_eq!(roles, ["sides.1.curve", "sides.2.curve"]);
    }

    #[test]
    fn fills_are_checked() {
        let e = |v: Value| fill(v).unwrap_err();
        for (v, code, pointer) in [
            (json!({}), Code::InvalidDocument, "/sides"),
            (json!({"sides": []}), Code::InvalidDocument, "/sides"),
            (json!({"sides": [{"edge": {"feature": "w", "name": "x"}}]}),
             Code::InvalidDocument, "/sides/0/edge/name"),
            (json!({"sides": [{"curve": "x", "tangent": true}]}),
             Code::InvalidDocument, "/sides/0"),
            (json!({"sides": [{"wire": "x"}]}), Code::InvalidDocument, "/sides/0"),
            (json!({"sides": [{"curve": "x"}], "tolerance": "1 mm"}),
             Code::InvalidParam, "/tolerance"),
            (json!({"sides": [{"curve": "x"}], "tolerance": "1 deg"}),
             Code::UnitMismatch, "/tolerance"),
            (json!({"sides": [{"curve": "x"}], "angle": "90 deg"}),
             Code::InvalidParam, "/angle"),
            (json!({"sides": [{"curve": "x"}], "angle": 0}), Code::InvalidParam, "/angle"),
            (json!({"sides": [{"curve": "x"}], "targets": ["b"]}), Code::InvalidParam, "/operation"),
        ] {
            let err = e(v.clone());
            assert_eq!((err.code, err.details["pointer"].as_str()), (code, Some(pointer)), "{v}");
        }
    }
}
