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
            "faces",
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
        let a = v.as_array().filter(|a| !a.is_empty()).ok_or_else(|| {
            invalid(
                "/sides",
                "sides is a non-empty array of {edge, tangent?} or {curve}",
            )
        })?;
        let mut sides = Vec::with_capacity(a.len());
        for (i, s) in a.iter().enumerate() {
            let at = format!("/sides/{i}");
            let mut f = Obj::new(s, &at)?;
            let side = if f.has("edge") {
                let edge = Reference::from_json(&f.take("edge")?, &format!("{at}/edge"))?;
                let tangent = match f.take_opt("tangent") {
                    None => false,
                    Some(Value::Bool(b)) => b,
                    Some(_) => {
                        return Err(invalid(
                            &format!("{at}/tangent"),
                            "tangent is true or false",
                        ));
                    }
                };
                Side::Edge { edge, tangent }
            } else if f.has("curve") {
                if f.has("tangent") {
                    return Err(invalid(
                        &at,
                        "only an edge side meets its face tangentially",
                    ));
                }
                Side::Curve(GeomRef::from_json(
                    &f.take("curve")?,
                    &format!("{at}/curve"),
                )?)
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
        Ok(Fill {
            sides,
            tolerance,
            angle,
        })
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
        Ok(Sew {
            sheets,
            tolerance,
            solid,
        })
    }
}

/// What a thicken thickens: a sheet, by founder (consumed), or faces of
/// one body of either kind (extracted first, the body left as it was).
#[derive(Clone, Debug, PartialEq)]
pub enum ThickenFrom {
    Sheet(Id),
    Faces(Vec<Reference>),
}

/// Faces given a front and back of their own.
#[derive(Clone, Debug, PartialEq)]
pub struct SideGroup {
    pub front: Quantity,
    pub back: Quantity,
    pub faces: Vec<Reference>,
}

/// A thicken: `from` given `front` along its faces' normals and `back`
/// against them (each group's faces their own), the solid founded
/// (`new`) or joined to, cut from or intersected with `combine`'s targets.
#[derive(Clone, Debug, PartialEq)]
pub struct Thicken {
    pub from: ThickenFrom,
    pub front: Quantity,
    pub back: Quantity,
    pub faces: Vec<SideGroup>,
    pub combine: Combine,
}

impl Thicken {
    /// Its fields: front and back always, the operation always (as a
    /// mirror's), targets with a join, cut or intersect.
    pub fn fields(&self) -> Value {
        let from = match &self.from {
            ThickenFrom::Sheet(s) => json!({"sheet": s}),
            ThickenFrom::Faces(f) => json!({"faces": refs_json(f)}),
        };
        let mut v = json!({"from": from, "front": self.front.text(), "back": self.back.text(),
                           "operation": self.combine.operation.as_str()});
        if !self.faces.is_empty() {
            v["faces"] = Value::Array(
                self.faces
                    .iter()
                    .map(|g| {
                        json!({"front": g.front.text(), "back": g.back.text(),
                                    "faces": refs_json(&g.faces)})
                    })
                    .collect(),
            );
        }
        if !self.combine.is_new() {
            v["targets"] = json!(self.combine.targets);
        }
        v
    }

    #[doc(hidden)]
    pub fn from_fields(o: &mut Obj, default: Unit, env: &Env) -> Result<Thicken> {
        let from = {
            let v = o.take("from")?;
            let mut f = Obj::new(&v, "/from")?;
            let from = match (f.take_opt("sheet"), f.take_opt("faces")) {
                (Some(Value::String(s)), None) => ThickenFrom::Sheet(
                    Id::parse(&s, "feature").map_err(|e| e.with("pointer", "/from/sheet"))?,
                ),
                (None, Some(faces)) => ThickenFrom::Faces(refs(&faces, "/from/faces")?),
                _ => {
                    return Err(invalid(
                        "/from",
                        "from is {\"sheet\": <founder>} or {\"faces\": [faces of one body]}",
                    ));
                }
            };
            f.finish()?;
            from
        };
        let front = side(o, "front", default, env, "/front")?;
        let back = side(o, "back", default, env, "/back")?;
        let faces = match o.take_opt("faces") {
            None => Vec::new(),
            Some(v) => {
                let a = v.as_array().ok_or_else(|| {
                    invalid("/faces", "faces is an array of groups {front, back, faces}")
                })?;
                a.iter()
                    .enumerate()
                    .map(|(i, g)| {
                        let at = format!("/faces/{i}");
                        let mut f = Obj::new(g, &at)?;
                        let front = side(&mut f, "front", default, env, &format!("{at}/front"))?;
                        let back = side(&mut f, "back", default, env, &format!("{at}/back"))?;
                        if front.meters() == 0.0 && back.meters() == 0.0 {
                            return Err(Error::new(
                                Code::InvalidParam,
                                "a group's front and back are both zero: its faces would have no thickness",
                            )
                            .with("pointer", format!("{at}/front")));
                        }
                        let faces = refs(&f.take("faces")?, &format!("{at}/faces"))?;
                        f.finish()?;
                        Ok(SideGroup { front, back, faces })
                    })
                    .collect::<Result<Vec<_>>>()?
            }
        };
        if front.meters() == 0.0 && back.meters() == 0.0 && faces.is_empty() {
            return Err(Error::new(
                Code::InvalidParam,
                "a thicken gives its faces a front or a back: both are zero",
            )
            .with("pointer", "/front"));
        }
        let operation = o
            .take("operation")?
            .as_str()
            .and_then(Operation::parse)
            .ok_or_else(|| {
                invalid(
                    "/operation",
                    "operation is one of new, join, cut, intersect",
                )
            })?;
        let targets = match o.take_opt("targets") {
            None => Vec::new(),
            Some(v) => targets_from_json(&v, "/targets")?,
        };
        let combine = match (operation, targets.is_empty()) {
            (Operation::New, true) => Combine::NEW,
            (Operation::New, false) => {
                return Err(invalid(
                    "/targets",
                    "a thicken whose solid is new has no targets",
                ));
            }
            (_, true) => {
                return Err(invalid(
                    "/targets",
                    &format!(
                        "a thicken whose solid {}s names the solids it acts on: targets is required",
                        operation.as_str()
                    ),
                ));
            }
            (_, false) => Combine { operation, targets },
        };
        let t = Thicken {
            from,
            front,
            back,
            faces,
            combine,
        };
        t.check_groups()?;
        Ok(t)
    }

    /// Each group's faces: of the sheet (from a sheet) or among `from`'s
    /// faces, each in one group once; `from`'s faces each once.
    fn check_groups(&self) -> Result<()> {
        if let ThickenFrom::Faces(f) = &self.from {
            once(
                f.iter()
                    .enumerate()
                    .map(|(i, r)| (format!("from.faces.{i}"), r))
                    .collect(),
            )?;
        }
        let grouped: Vec<(String, &Reference)> = self.group_roles().collect();
        for (role, r) in &grouped {
            let ok = match &self.from {
                ThickenFrom::Sheet(s) => r.feature == *s,
                ThickenFrom::Faces(f) => f.contains(r),
            };
            if !ok {
                let of = match &self.from {
                    ThickenFrom::Sheet(s) => format!("a face of its sheet {s}"),
                    ThickenFrom::Faces(_) => "one of the faces it thickens (from.faces)".to_owned(),
                };
                return Err(Error::new(
                    Code::InvalidParam,
                    format!("{}/{}, grouped as {role}, isn't {of}", r.feature, r.name),
                )
                .with("pointer", format!("/{}", role.replace('.', "/"))));
            }
        }
        once(grouped)
    }

    fn group_roles(&self) -> impl Iterator<Item = (String, &Reference)> {
        self.faces.iter().enumerate().flat_map(|(i, g)| {
            g.faces
                .iter()
                .enumerate()
                .map(move |(j, r)| (format!("faces.{i}.faces.{j}"), r))
        })
    }

    /// The sheet it thickens and consumes, if it is from one.
    pub fn sheet(&self) -> Option<&Id> {
        match &self.from {
            ThickenFrom::Sheet(s) => Some(s),
            ThickenFrom::Faces(_) => None,
        }
    }

    /// Its references, each with its role: `from.faces.<i>`, then
    /// `faces.<i>.faces.<j>`.
    pub fn references(&self) -> Vec<(String, &Reference)> {
        let mut out: Vec<(String, &Reference)> = match &self.from {
            ThickenFrom::Faces(f) => f
                .iter()
                .enumerate()
                .map(|(i, r)| (format!("from.faces.{i}"), r))
                .collect(),
            ThickenFrom::Sheet(_) => Vec::new(),
        };
        out.extend(self.group_roles());
        out
    }
}

/// A front or back: a length of zero or more, zero when left out.
fn side(o: &mut Obj, key: &str, default: Unit, env: &Env, at: &str) -> Result<Quantity> {
    let v = o.take_opt(key).unwrap_or(json!(0));
    let q = Quantity::from_json_as(&v, Field::Length(default), &|n| env.get(n))
        .map_err(|e| e.with("pointer", at))?;
    if q.meters() < 0.0 {
        return Err(Error::new(
            Code::InvalidParam,
            format!("a thicken's {key} is zero or more, not {q}: thicken the other way with the other side"),
        )
        .with("pointer", at)
        .with("value", q.text()));
    }
    Ok(q)
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
                format!(
                    "face {}/{} is named twice, as {first} and {role}",
                    r.feature, r.name
                ),
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
        None => Quantity::from_json_as(&json!(default_text), Field::Length(default), &|n| {
            env.get(n)
        })?,
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
            (
                json!({"faces": [face(A), face(A)]}),
                Code::InvalidParam,
                "/faces/1",
            ),
            (
                json!({"faces": [face(A)], "operation": "join"}),
                Code::InvalidParam,
                "/operation",
            ),
        ] {
            let e = extract(v.clone()).unwrap_err();
            assert_eq!(
                (e.code, e.details["pointer"].as_str()),
                (code, Some(pointer)),
                "{v}"
            );
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
        assert_eq!(
            s.fields(),
            json!({"sheets": ["top", "side1", "side2"],
                                      "tolerance": "0.01 mm", "solid": true})
        );
        assert_eq!(sew(s.fields()).unwrap(), s);
        let s = sew(json!({"sheets": ["a", "b"], "tolerance": "0.05 mm", "solid": false})).unwrap();
        assert!(!s.solid);
        assert_eq!(s.tolerance.meters(), 5e-5);
        for (v, code, pointer) in [
            (json!({}), Code::InvalidDocument, "/sheets"),
            (json!({"sheets": ["a"]}), Code::InvalidParam, "/sheets"),
            (
                json!({"sheets": ["a", "a"]}),
                Code::InvalidDocument,
                "/sheets/1",
            ),
            (
                json!({"sheets": ["a", "b"], "solid": "yes"}),
                Code::InvalidDocument,
                "/solid",
            ),
            (
                json!({"sheets": ["a", "b"], "tolerance": "1 mm"}),
                Code::InvalidParam,
                "/tolerance",
            ),
            (
                json!({"sheets": ["a", "b"], "operation": "join"}),
                Code::InvalidParam,
                "/operation",
            ),
        ] {
            let e = sew(v.clone()).unwrap_err();
            assert_eq!(
                (e.code, e.details["pointer"].as_str()),
                (code, Some(pointer)),
                "{v}"
            );
        }
    }

    fn thicken(v: Value) -> Result<Thicken> {
        let mut o = Obj::new(&v, "")?;
        let t = Thicken::from_fields(&mut o, Unit::Mm, &Env::empty())?;
        o.finish()?;
        Ok(t)
    }

    #[test]
    fn thickens_round_trip() {
        let skin = |n: &str| json!({"feature": "skin", "name": n});
        let v = json!({"from": {"sheet": "skin"}, "front": 2, "operation": "new",
                       "faces": [{"front": "3 mm", "back": 0, "faces": [skin(A)]}]});
        let t = thicken(v).unwrap();
        assert_eq!(
            t.from,
            ThickenFrom::Sheet(Id::parse("skin", "feature").unwrap())
        );
        assert!(t.combine.is_new());
        // Front and back always written (back 0 when left out), the
        // operation always, even new.
        assert_eq!(
            t.fields(),
            json!({"from": {"sheet": "skin"}, "front": "2 mm", "back": "0 mm",
            "faces": [{"front": "3 mm", "back": "0 mm", "faces": [skin(A)]}], "operation": "new"})
        );
        assert_eq!(thicken(t.fields()).unwrap(), t);
        let roles: Vec<String> = t.references().into_iter().map(|(r, _)| r).collect();
        assert_eq!(roles, ["faces.0.faces.0"]);
        // From faces, joined to a solid.
        let base = |n: &str| json!({"feature": "base", "name": n});
        let v = json!({"from": {"faces": [base(A)]}, "front": "3 mm", "back": "1 mm",
                       "operation": "join", "targets": ["base"]});
        let t = thicken(v.clone()).unwrap();
        assert_eq!(t.fields(), v);
        let roles: Vec<String> = t.references().into_iter().map(|(r, _)| r).collect();
        assert_eq!(roles, ["from.faces.0"]);
    }

    #[test]
    fn thickens_are_checked() {
        const B: &str = "fedcba9876543210fedcba9876543210";
        let skin = |n: &str| json!({"feature": "skin", "name": n});
        let base = |n: &str| json!({"feature": "base", "name": n});
        for (v, code, pointer) in [
            (
                json!({"front": 1, "operation": "new"}),
                Code::InvalidDocument,
                "/from",
            ),
            (
                json!({"from": {}, "front": 1, "operation": "new"}),
                Code::InvalidDocument,
                "/from",
            ),
            (
                json!({"from": {"sheet": "s", "faces": [base(A)]}, "front": 1, "operation": "new"}),
                Code::InvalidDocument,
                "/from",
            ),
            (
                json!({"from": {"sheet": "skin"}, "operation": "new"}),
                Code::InvalidParam,
                "/front",
            ),
            (
                json!({"from": {"sheet": "skin"}, "front": -1, "operation": "new"}),
                Code::InvalidParam,
                "/front",
            ),
            (
                json!({"from": {"sheet": "skin"}, "front": 1}),
                Code::InvalidDocument,
                "/operation",
            ),
            (
                json!({"from": {"sheet": "skin"}, "front": 1, "operation": "join"}),
                Code::InvalidDocument,
                "/targets",
            ),
            (
                json!({"from": {"sheet": "skin"}, "front": 1, "operation": "new",
                    "faces": [{"front": 2, "back": 0, "faces": [base(A)]}]}),
                Code::InvalidParam,
                "/faces/0/faces/0",
            ),
            (
                json!({"from": {"faces": [base(A)]}, "front": 1, "operation": "new",
                    "faces": [{"front": 2, "back": 0, "faces": [base(B)]}]}),
                Code::InvalidParam,
                "/faces/0/faces/0",
            ),
            (
                json!({"from": {"sheet": "skin"}, "front": 1, "operation": "new",
                    "faces": [{"front": 0, "back": 0, "faces": [skin(A)]}]}),
                Code::InvalidParam,
                "/faces/0/front",
            ),
            (
                json!({"from": {"faces": [base(A), base(A)]}, "front": 1, "operation": "new"}),
                Code::InvalidParam,
                "/from/faces/1",
            ),
        ] {
            let e = thicken(v.clone()).unwrap_err();
            assert_eq!(
                (e.code, e.details["pointer"].as_str()),
                (code, Some(pointer)),
                "{v}"
            );
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
        let f = fill(
            json!({"sides": [{"edge": {"feature": "w", "name": A}, "tangent": false},
                                       {"curve": "x"}, {"curve": "y"}],
                            "tolerance": "0.01 mm", "angle": 2}),
        )
        .unwrap();
        assert_eq!(
            f.fields()["sides"][0],
            json!({"edge": {"feature": "w", "name": A}})
        );
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
            (
                json!({"sides": [{"edge": {"feature": "w", "name": "x"}}]}),
                Code::InvalidDocument,
                "/sides/0/edge/name",
            ),
            (
                json!({"sides": [{"curve": "x", "tangent": true}]}),
                Code::InvalidDocument,
                "/sides/0",
            ),
            (
                json!({"sides": [{"wire": "x"}]}),
                Code::InvalidDocument,
                "/sides/0",
            ),
            (
                json!({"sides": [{"curve": "x"}], "tolerance": "1 mm"}),
                Code::InvalidParam,
                "/tolerance",
            ),
            (
                json!({"sides": [{"curve": "x"}], "tolerance": "1 deg"}),
                Code::UnitMismatch,
                "/tolerance",
            ),
            (
                json!({"sides": [{"curve": "x"}], "angle": "90 deg"}),
                Code::InvalidParam,
                "/angle",
            ),
            (
                json!({"sides": [{"curve": "x"}], "angle": 0}),
                Code::InvalidParam,
                "/angle",
            ),
            (
                json!({"sides": [{"curve": "x"}], "targets": ["b"]}),
                Code::InvalidParam,
                "/operation",
            ),
        ] {
            let err = e(v.clone());
            assert_eq!(
                (err.code, err.details["pointer"].as_str()),
                (code, Some(pointer)),
                "{v}"
            );
        }
    }
}
