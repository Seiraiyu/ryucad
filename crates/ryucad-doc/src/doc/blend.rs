//! Fillet and chamfer features: edges of one solid, named by reference,
//! rounded or bevelled in groups of one size. Each edge is grown to the
//! chain of edges meeting it tangentially unless `propagate` is false;
//! the solid is the one the edges' references name.

use super::files::{Obj, invalid};
use super::reference::Reference;
use crate::error::{Code, Error, Result};
use crate::expr::Env;
use crate::units::{Field, Quantity, Unit};
use serde_json::{Value, json};

/// A fillet: groups of edges, each rounded by a ball of its radius.
#[derive(Clone, Debug, PartialEq)]
pub struct Fillet {
    pub groups: Vec<FilletGroup>,
    /// Grow each edge to its tangent chain.
    pub propagate: bool,
}

/// Edges rounded by one radius.
#[derive(Clone, Debug, PartialEq)]
pub struct FilletGroup {
    pub radius: Quantity,
    pub edges: Vec<Reference>,
}

/// A chamfer: groups of edges, each bevelled by its form.
#[derive(Clone, Debug, PartialEq)]
pub struct Chamfer {
    pub groups: Vec<ChamferGroup>,
    /// Grow each edge to its tangent chain.
    pub propagate: bool,
}

/// Edges bevelled alike.
#[derive(Clone, Debug, PartialEq)]
pub struct ChamferGroup {
    pub form: ChamferForm,
    pub edges: Vec<Reference>,
}

/// How far a chamfer is set back from its edge along each face.
#[derive(Clone, Debug, PartialEq)]
pub enum ChamferForm {
    /// `d` along both faces.
    Equal { d: Quantity },
    /// `d1` along `face`, `d2` along the other.
    Distances {
        d1: Quantity,
        d2: Quantity,
        face: Reference,
    },
    /// `d` along `face`, the chamfer at `angle` to it.
    DistanceAngle {
        d: Quantity,
        angle: Quantity,
        face: Reference,
    },
}

impl ChamferForm {
    pub const NAMES: [&str; 3] = ["equal", "distances", "distance_angle"];

    pub fn type_name(&self) -> &'static str {
        match self {
            ChamferForm::Equal { .. } => "equal",
            ChamferForm::Distances { .. } => "distances",
            ChamferForm::DistanceAngle { .. } => "distance_angle",
        }
    }

    /// The face the first distance runs along, if the form has one.
    pub fn face(&self) -> Option<&Reference> {
        match self {
            ChamferForm::Equal { .. } => None,
            ChamferForm::Distances { face, .. } | ChamferForm::DistanceAngle { face, .. } => {
                Some(face)
            }
        }
    }
}

impl Fillet {
    pub fn fields(&self) -> Value {
        let groups: Vec<Value> = self
            .groups
            .iter()
            .map(|g| json!({"radius": g.radius.text(), "edges": edges_json(&g.edges)}))
            .collect();
        let mut v = json!({"groups": groups});
        propagate_json(&mut v, self.propagate);
        v
    }

    #[doc(hidden)]
    pub fn from_fields(o: &mut Obj, default: Unit, env: &Env) -> Result<Fillet> {
        let groups = groups_json(o, |g, at| {
            let radius = size(
                &g.take("radius")?,
                Field::Length(default),
                env,
                at,
                "radius",
            )?;
            let edges = edges_from_json(&g.take("edges")?, &format!("{at}/edges"))?;
            Ok(FilletGroup { radius, edges })
        })?;
        Ok(Fillet {
            groups,
            propagate: propagate(o)?,
        })
    }

    /// Its references, each with its role: `groups.<i>.edges.<j>`.
    pub fn references(&self) -> Vec<(String, &Reference)> {
        let mut out = Vec::new();
        for (i, g) in self.groups.iter().enumerate() {
            out.extend(edge_roles(i, &g.edges));
        }
        out
    }
}

impl Chamfer {
    pub fn fields(&self) -> Value {
        let groups: Vec<Value> = self
            .groups
            .iter()
            .map(|g| {
                let mut v = json!({"form": g.form.type_name(), "edges": edges_json(&g.edges)});
                match &g.form {
                    ChamferForm::Equal { d } => v["d"] = json!(d.text()),
                    ChamferForm::Distances { d1, d2, face } => {
                        v["d1"] = json!(d1.text());
                        v["d2"] = json!(d2.text());
                        v["face"] = face.to_json();
                    }
                    ChamferForm::DistanceAngle { d, angle, face } => {
                        v["d"] = json!(d.text());
                        v["angle"] = json!(angle.text());
                        v["face"] = face.to_json();
                    }
                }
                v
            })
            .collect();
        let mut v = json!({"groups": groups});
        propagate_json(&mut v, self.propagate);
        v
    }

    #[doc(hidden)]
    pub fn from_fields(o: &mut Obj, default: Unit, env: &Env) -> Result<Chamfer> {
        let length = Field::Length(default);
        let groups = groups_json(o, |g, at| {
            let form = g.take("form")?;
            let form = match form.as_str() {
                Some("equal") => ChamferForm::Equal {
                    d: size(&g.take("d")?, length, env, at, "d")?,
                },
                Some("distances") => ChamferForm::Distances {
                    d1: size(&g.take("d1")?, length, env, at, "d1")?,
                    d2: size(&g.take("d2")?, length, env, at, "d2")?,
                    face: Reference::from_json(&g.take("face")?, &format!("{at}/face"))?,
                },
                Some("distance_angle") => {
                    let d = size(&g.take("d")?, length, env, at, "d")?;
                    let angle = size(&g.take("angle")?, Field::Angle, env, at, "angle")?;
                    if angle.radians() > core::f64::consts::FRAC_PI_2 {
                        return Err(bad_size(
                            &format!("{at}/angle"),
                            "a chamfer's angle to its face is at most 90 deg",
                            &angle,
                        ));
                    }
                    let face = Reference::from_json(&g.take("face")?, &format!("{at}/face"))?;
                    ChamferForm::DistanceAngle { d, angle, face }
                }
                _ => {
                    return Err(invalid(
                        &format!("{at}/form"),
                        "a chamfer's form is equal, distances or distance_angle",
                    ));
                }
            };
            let edges = edges_from_json(&g.take("edges")?, &format!("{at}/edges"))?;
            Ok(ChamferGroup { form, edges })
        })?;
        Ok(Chamfer {
            groups,
            propagate: propagate(o)?,
        })
    }

    /// Its references, each with its role: `groups.<i>.edges.<j>`, and a
    /// group's `groups.<i>.face`.
    pub fn references(&self) -> Vec<(String, &Reference)> {
        let mut out = Vec::new();
        for (i, g) in self.groups.iter().enumerate() {
            out.extend(edge_roles(i, &g.edges));
            if let Some(f) = g.form.face() {
                out.push((format!("groups.{i}.face"), f));
            }
        }
        out
    }
}

/// The expression strings of a fillet's or chamfer's `groups`: their
/// sizes and angles.
pub fn expression_strings(groups: &mut Value) -> Vec<&mut String> {
    let mut out = Vec::new();
    let Value::Array(groups) = groups else {
        return out;
    };
    for g in groups {
        let Value::Object(g) = g else { continue };
        for (k, x) in g.iter_mut() {
            if let ("radius" | "d" | "d1" | "d2" | "angle", Value::String(s)) = (k.as_str(), x) {
                out.push(s);
            }
        }
    }
    out
}

fn edge_roles(i: usize, edges: &[Reference]) -> impl Iterator<Item = (String, &Reference)> {
    edges
        .iter()
        .enumerate()
        .map(move |(j, e)| (format!("groups.{i}.edges.{j}"), e))
}

fn edges_json(edges: &[Reference]) -> Vec<Value> {
    edges.iter().map(Reference::to_json).collect()
}

/// `propagate` is true unless written false.
fn propagate_json(v: &mut Value, propagate: bool) {
    if !propagate {
        v["propagate"] = json!(false);
    }
}

fn propagate(o: &mut Obj) -> Result<bool> {
    match o.take_opt("propagate") {
        None => Ok(true),
        Some(Value::Bool(b)) => Ok(b),
        Some(_) => Err(invalid("/propagate", "propagate is true or false")),
    }
}

/// Reads `groups`, a non-empty array of objects, each by `group` (given
/// the object's fields and its pointer).
fn groups_json<G>(o: &mut Obj, group: impl Fn(&mut Obj, &str) -> Result<G>) -> Result<Vec<G>> {
    let v = o.take("groups")?;
    let a = v
        .as_array()
        .filter(|a| !a.is_empty())
        .ok_or_else(|| invalid("/groups", "groups is a non-empty array of groups of edges"))?;
    a.iter()
        .enumerate()
        .map(|(i, g)| {
            let at = format!("/groups/{i}");
            let mut fields = Obj::new(g, &at)?;
            let g = group(&mut fields, &at)?;
            fields.finish()?;
            Ok(g)
        })
        .collect()
}

/// Reads a non-empty array of edge references.
fn edges_from_json(v: &Value, at: &str) -> Result<Vec<Reference>> {
    let a = v
        .as_array()
        .filter(|a| !a.is_empty())
        .ok_or_else(|| invalid(at, "edges is a non-empty array of {feature, name}"))?;
    a.iter()
        .enumerate()
        .map(|(j, e)| Reference::from_json(e, &format!("{at}/{j}")))
        .collect()
}

/// A size or an angle greater than zero: `key` of the group at `at`.
fn size(v: &Value, field: Field, env: &Env, at: &str, key: &str) -> Result<Quantity> {
    let at = format!("{at}/{key}");
    let q =
        Quantity::from_json_as(v, field, &|n| env.get(n)).map_err(|e| e.with("pointer", &at))?;
    let si = match field {
        Field::Length(_) => q.meters(),
        Field::Angle => q.radians(),
        Field::Number => q.number(),
    };
    if si <= 0.0 {
        return Err(bad_size(
            &at,
            &format!("{key} must be greater than zero"),
            &q,
        ));
    }
    Ok(q)
}

fn bad_size(at: &str, msg: &str, q: &Quantity) -> Error {
    Error::new(Code::InvalidParam, format!("{msg}, not {q}"))
        .with("pointer", at)
        .with("value", q.text())
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "0123456789abcdef0123456789abcdef";
    const B: &str = "fedcba9876543210fedcba9876543210";

    fn edge(n: &str) -> Value {
        json!({"feature": "box1", "name": n})
    }

    fn fillet(v: Value) -> Result<Fillet> {
        let mut o = Obj::new(&v, "")?;
        let f = Fillet::from_fields(&mut o, Unit::Mm, &Env::empty())?;
        o.finish()?;
        Ok(f)
    }

    fn chamfer(v: Value) -> Result<Chamfer> {
        let mut o = Obj::new(&v, "")?;
        let c = Chamfer::from_fields(&mut o, Unit::Mm, &Env::empty())?;
        o.finish()?;
        Ok(c)
    }

    #[test]
    fn fillets_round_trip() {
        let v = json!({"groups": [
            {"radius": "2 mm", "edges": [edge(A)]},
            {"radius": 1, "edges": [edge(B), edge(A)]}
        ]});
        let f = fillet(v).unwrap();
        assert!(f.propagate);
        assert_eq!(f.groups[1].radius.meters(), 0.001);
        // Bare numbers take the default unit, written out.
        assert_eq!(
            f.fields(),
            json!({"groups": [
                {"radius": "2 mm", "edges": [edge(A)]},
                {"radius": "1 mm", "edges": [edge(B), edge(A)]}
            ]})
        );
        assert_eq!(fillet(f.fields()).unwrap(), f);
        let roles: Vec<String> = f.references().into_iter().map(|(r, _)| r).collect();
        assert_eq!(
            roles,
            ["groups.0.edges.0", "groups.1.edges.0", "groups.1.edges.1"]
        );
        // Only false is written.
        let f = fillet(json!({"groups": [{"radius": 2, "edges": [edge(A)]}],
                              "propagate": false}))
        .unwrap();
        assert!(!f.propagate);
        assert_eq!(f.fields()["propagate"], false);
    }

    #[test]
    fn chamfers_round_trip() {
        let face = json!({"feature": "box1", "name": B});
        let v = json!({"groups": [
            {"form": "equal", "d": "1 mm", "edges": [edge(A)]},
            {"form": "distances", "d1": "1 mm", "d2": "2 mm", "face": face, "edges": [edge(A)]},
            {"form": "distance_angle", "d": "1 mm", "angle": 30, "face": face, "edges": [edge(A)]}
        ]});
        let c = chamfer(v).unwrap();
        match &c.groups[2].form {
            ChamferForm::DistanceAngle { angle, .. } => {
                assert!((angle.radians() - core::f64::consts::FRAC_PI_6).abs() < 1e-15);
                assert_eq!(angle.text(), "30 deg");
            }
            f => panic!("{f:?}"),
        }
        assert_eq!(chamfer(c.fields()).unwrap(), c);
        let roles: Vec<String> = c.references().into_iter().map(|(r, _)| r).collect();
        assert_eq!(
            roles,
            [
                "groups.0.edges.0",
                "groups.1.edges.0",
                "groups.1.face",
                "groups.2.edges.0",
                "groups.2.face"
            ]
        );
    }

    #[test]
    fn blends_are_checked() {
        let face = json!({"feature": "box1", "name": B});
        for (v, code, pointer) in [
            (json!({}), Code::InvalidDocument, "/groups"),
            (json!({"groups": []}), Code::InvalidDocument, "/groups"),
            (
                json!({"groups": [{"radius": 1, "edges": []}]}),
                Code::InvalidDocument,
                "/groups/0/edges",
            ),
            (
                json!({"groups": [{"radius": 0, "edges": [edge(A)]}]}),
                Code::InvalidParam,
                "/groups/0/radius",
            ),
            (
                json!({"groups": [{"radius": "1 deg", "edges": [edge(A)]}]}),
                Code::UnitMismatch,
                "/groups/0/radius",
            ),
            (
                json!({"groups": [{"radius": 1, "edges": [edge("x")]}]}),
                Code::InvalidDocument,
                "/groups/0/edges/0/name",
            ),
            (
                json!({"groups": [{"radius": 1, "edges": [edge(A)], "d": 1}]}),
                Code::InvalidDocument,
                "/groups/0/d",
            ),
            (
                json!({"groups": [{"radius": 1, "edges": [edge(A)]}], "propagate": 1}),
                Code::InvalidDocument,
                "/propagate",
            ),
        ] {
            let e = fillet(v.clone()).unwrap_err();
            assert_eq!(
                (e.code, e.details["pointer"].as_str()),
                (code, Some(pointer)),
                "{v}"
            );
        }
        for (v, code, pointer) in [
            (
                json!({"groups": [{"form": "round", "d": 1, "edges": [edge(A)]}]}),
                Code::InvalidDocument,
                "/groups/0/form",
            ),
            (
                json!({"groups": [{"form": "distances", "d1": 1, "d2": 2, "edges": [edge(A)]}]}),
                Code::InvalidDocument,
                "/groups/0/face",
            ),
            (
                json!({"groups": [{"form": "equal", "d": 1, "face": face, "edges": [edge(A)]}]}),
                Code::InvalidDocument,
                "/groups/0/face",
            ),
            (
                json!({"groups": [{"form": "distance_angle", "d": 1, "angle": 91, "face": face,
                                   "edges": [edge(A)]}]}),
                Code::InvalidParam,
                "/groups/0/angle",
            ),
            (
                json!({"groups": [{"form": "distance_angle", "d": 1, "angle": "1 mm", "face": face,
                                   "edges": [edge(A)]}]}),
                Code::UnitMismatch,
                "/groups/0/angle",
            ),
            (
                json!({"groups": [{"form": "equal", "d": "-1 mm", "edges": [edge(A)]}]}),
                Code::InvalidParam,
                "/groups/0/d",
            ),
        ] {
            let e = chamfer(v.clone()).unwrap_err();
            assert_eq!(
                (e.code, e.details["pointer"].as_str()),
                (code, Some(pointer)),
                "{v}"
            );
        }
    }

    #[test]
    fn sizes_read_parameters() {
        let p = [("r".to_owned(), crate::expr::Expr::parse("1.5 mm").unwrap())].into();
        let v = json!({"groups": [{"radius": "2*r", "edges": [edge(A)]}]});
        let mut o = Obj::new(&v, "").unwrap();
        let f = Fillet::from_fields(&mut o, Unit::Mm, &Env::new(&p)).unwrap();
        assert_eq!(f.groups[0].radius.meters(), 0.003);
        assert_eq!(f.groups[0].radius.text(), "2 * r");
        let mut v = f.fields();
        let strings: Vec<String> = expression_strings(&mut v["groups"])
            .into_iter()
            .map(|s| s.clone())
            .collect();
        assert_eq!(strings, ["2 * r"]);
    }
}
