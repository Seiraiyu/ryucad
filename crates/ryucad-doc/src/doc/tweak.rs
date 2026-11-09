//! Face tweaks (design 400): faces of one solid, its `target`, moved
//! (`move_face`: translated along a direction, turned about an axis, or
//! offset along their outward normals) or given another face's surface or
//! a plane (`replace_face`). A tweak keeps every face, edge and vertex
//! and their names, and makes nothing. Faces are references into the
//! target as the feature sees it.

use super::body::{FaceGroup, refs, refs_json, solid_id};
use super::files::{Obj, invalid};
use super::geometry::GeomRef;
use super::id::Id;
use super::reference::Reference;
use crate::error::{Code, Error, Result};
use crate::expr::Env;
use crate::units::{Field, Quantity, Unit};
use serde_json::{Value, json};

/// The feature format face tweaks are new in. Taken in the order stages
/// merge (13 ryucad 21, 14 ryucad 16.B, 15 ryucad 18.B): if another
/// stage holds 16 when this merges, move this constant to the next free
/// number; nothing else names it.
pub const FORMAT: u64 = 16;

/// The feature types this module reads.
pub const TYPES: [&str; 2] = ["move_face", "replace_face"];

const BY: &str = "by is translate, rotate or offset";

/// How a `move_face` moves its faces.
#[derive(Clone, Debug, PartialEq)]
pub enum MoveBy {
    /// Along `direction` (an axis, or a plane's normal) by `distance`, of
    /// either sign.
    Translate {
        direction: GeomRef,
        distance: Quantity,
    },
    /// About `axis`, right-handed, by `angle`.
    Rotate { axis: GeomRef, angle: Quantity },
    /// Each listed face along its outward normal by `distance` (positive
    /// outwards), each group's by its own.
    Offset {
        distance: Quantity,
        groups: Vec<FaceGroup>,
    },
}

impl MoveBy {
    pub fn as_str(&self) -> &'static str {
        match self {
            MoveBy::Translate { .. } => "translate",
            MoveBy::Rotate { .. } => "rotate",
            MoveBy::Offset { .. } => "offset",
        }
    }

    /// The fields `by` takes besides `target`, `faces` and `tangent`;
    /// `None` for a `by` that isn't one.
    pub fn takes(by: &str) -> Option<&'static [&'static str]> {
        match by {
            "translate" => Some(&["direction", "distance"]),
            "rotate" => Some(&["axis", "angle"]),
            "offset" => Some(&["distance", "groups"]),
            _ => None,
        }
    }
}

/// A `move_face`: `faces` of `target` moved `by`; with `tangent`, faces
/// tangent to a moved face join it (true unless written false).
#[derive(Clone, Debug, PartialEq)]
pub struct MoveFace {
    pub target: Id,
    pub by: MoveBy,
    pub faces: Vec<Reference>,
    pub tangent: bool,
}

/// Faces given the surface `with` names: a face's (any solid of the part,
/// the target's own too), else a plane's.
#[derive(Clone, Debug, PartialEq)]
pub struct Replacement {
    pub with: GeomRef,
    pub faces: Vec<Reference>,
}

/// A `replace_face`: each group's faces of `target` on its `with`.
#[derive(Clone, Debug, PartialEq)]
pub struct ReplaceFace {
    pub target: Id,
    pub groups: Vec<Replacement>,
}

/// The fields a request may give a tweak of type `ty`.
pub fn request_fields(ty: &str) -> &'static [&'static str] {
    match ty {
        "replace_face" => &["target", "targets", "groups"],
        _ => &[
            "target", "targets", "by", "direction", "distance", "axis", "angle", "faces",
            "groups", "tangent",
        ],
    }
}

impl MoveFace {
    /// Its fields: `by` always, tangent only when false, no `faces` or
    /// `groups` when there are none.
    pub fn fields(&self) -> Value {
        let mut v = json!({"target": self.target, "by": self.by.as_str()});
        match &self.by {
            MoveBy::Translate { direction, distance } => {
                v["direction"] = direction.to_json();
                v["distance"] = json!(distance.text());
            }
            MoveBy::Rotate { axis, angle } => {
                v["axis"] = axis.to_json();
                v["angle"] = json!(angle.text());
            }
            MoveBy::Offset { distance, groups } => {
                v["distance"] = json!(distance.text());
                if !groups.is_empty() {
                    v["groups"] = Value::Array(
                        groups
                            .iter()
                            .map(|g| json!({"distance": g.quantity.text(), "faces": refs_json(&g.faces)}))
                            .collect(),
                    );
                }
            }
        }
        if !self.faces.is_empty() {
            v["faces"] = refs_json(&self.faces);
        }
        if !self.tangent {
            v["tangent"] = json!(false);
        }
        v
    }

    #[doc(hidden)]
    pub fn from_fields(o: &mut Obj, default: Unit, env: &Env) -> Result<MoveFace> {
        let target = solid_id(o, "target")?;
        let by = o.take("by")?;
        let length = |o: &mut Obj, k: &str| -> Result<Quantity> {
            let at = format!("/{k}");
            Quantity::from_json_as(&o.take(k)?, Field::Length(default), &|n| env.get(n))
                .map_err(|e| e.with("pointer", at))
        };
        let by = match by.as_str() {
            Some("translate") => {
                let direction = GeomRef::from_json(&o.take("direction")?, "/direction")?;
                let distance = length(o, "distance")?;
                if distance.meters() == 0.0 {
                    return Err(still("distance", "its distance is zero"));
                }
                MoveBy::Translate { direction, distance }
            }
            Some("rotate") => {
                let axis = GeomRef::from_json(&o.take("axis")?, "/axis")?;
                let angle = Quantity::from_json_as(&o.take("angle")?, Field::Angle, &|n| env.get(n))
                    .map_err(|e| e.with("pointer", "/angle"))?;
                if angle.radians() == 0.0 {
                    return Err(still("angle", "its angle is zero"));
                }
                MoveBy::Rotate { axis, angle }
            }
            Some("offset") => {
                let distance = length(o, "distance")?;
                let groups = groups(o, default, env)?;
                if distance.meters() == 0.0 && groups.iter().all(|g| g.quantity.meters() == 0.0) {
                    return Err(still(
                        "distance",
                        "its distance and every group's are zero",
                    ));
                }
                MoveBy::Offset { distance, groups }
            }
            _ => return Err(invalid("/by", BY)),
        };
        let faces = match o.take_opt("faces") {
            None => Vec::new(),
            Some(Value::Array(a)) if a.is_empty() => Vec::new(),
            Some(v) => refs(&v, "/faces")?,
        };
        let tangent = match o.take_opt("tangent") {
            None => true,
            Some(Value::Bool(b)) => b,
            Some(_) => return Err(invalid("/tangent", "tangent is true or false")),
        };
        let grouped = matches!(&by, MoveBy::Offset { groups, .. } if !groups.is_empty());
        if faces.is_empty() && !grouped {
            return Err(invalid(
                "/faces",
                "a move_face names the faces it moves: a non-empty array of faces {feature, name}",
            ));
        }
        let m = MoveFace {
            target,
            by,
            faces,
            tangent,
        };
        once(m.faces())?;
        Ok(m)
    }

    /// The faces it moves, each with its role: `faces.<i>`, then
    /// `groups.<i>.faces.<j>`.
    pub fn faces(&self) -> Vec<(String, &Reference)> {
        let mut out: Vec<(String, &Reference)> = self
            .faces
            .iter()
            .enumerate()
            .map(|(i, r)| (format!("faces.{i}"), r))
            .collect();
        if let MoveBy::Offset { groups, .. } = &self.by {
            for (i, g) in groups.iter().enumerate() {
                for (j, r) in g.faces.iter().enumerate() {
                    out.push((format!("groups.{i}.faces.{j}"), r));
                }
            }
        }
        out
    }

    /// Its geometric references: `direction` or `axis`.
    pub fn geometry(&self) -> Vec<(String, &GeomRef)> {
        match &self.by {
            MoveBy::Translate { direction, .. } => vec![("direction".to_owned(), direction)],
            MoveBy::Rotate { axis, .. } => vec![("axis".to_owned(), axis)],
            MoveBy::Offset { .. } => Vec::new(),
        }
    }

    /// Its references to faces, edges and vertices: its faces, then a
    /// direction's or axis's entity.
    pub fn references(&self) -> Vec<(String, &Reference)> {
        let mut out = self.faces();
        out.extend(
            self.geometry()
                .into_iter()
                .filter_map(|(k, g)| Some((k, g.reference()?))),
        );
        out
    }
}

impl ReplaceFace {
    pub fn fields(&self) -> Value {
        json!({
            "target": self.target,
            "groups": self.groups.iter()
                .map(|g| json!({"with": g.with.to_json(), "faces": refs_json(&g.faces)}))
                .collect::<Vec<_>>(),
        })
    }

    #[doc(hidden)]
    pub fn from_fields(o: &mut Obj) -> Result<ReplaceFace> {
        let target = solid_id(o, "target")?;
        let v = o.take("groups")?;
        let a = v.as_array().filter(|a| !a.is_empty()).ok_or_else(|| {
            invalid(
                "/groups",
                "groups is a non-empty array of {with, faces}: each group's faces take the surface with names",
            )
        })?;
        let mut groups = Vec::with_capacity(a.len());
        for (i, g) in a.iter().enumerate() {
            let at = format!("/groups/{i}");
            let mut f = Obj::new(g, &at)?;
            let with = GeomRef::from_json(&f.take("with")?, &format!("{at}/with"))?;
            let faces = refs(&f.take("faces")?, &format!("{at}/faces"))?;
            f.finish()?;
            if let Some(r) = with.reference()
                && let Some(j) = faces.iter().position(|x| x == r)
            {
                return Err(Error::new(
                    Code::InvalidParam,
                    format!(
                        "face {}/{} would be replaced by its own surface, which changes nothing",
                        r.feature, r.name
                    ),
                )
                .with("pointer", format!("{at}/faces/{j}")));
            }
            groups.push(Replacement { with, faces });
        }
        let r = ReplaceFace { target, groups };
        once(r.faces())?;
        Ok(r)
    }

    /// The faces it replaces, each with its role: `groups.<i>.faces.<j>`.
    pub fn faces(&self) -> Vec<(String, &Reference)> {
        self.groups
            .iter()
            .enumerate()
            .flat_map(|(i, g)| {
                g.faces
                    .iter()
                    .enumerate()
                    .map(move |(j, r)| (format!("groups.{i}.faces.{j}"), r))
            })
            .collect()
    }

    /// Its geometric references: each group's `with`.
    pub fn geometry(&self) -> Vec<(String, &GeomRef)> {
        self.groups
            .iter()
            .enumerate()
            .map(|(i, g)| (format!("groups.{i}.with"), &g.with))
            .collect()
    }

    /// Its faces, then each `with` that names a face.
    pub fn references(&self) -> Vec<(String, &Reference)> {
        let mut out = self.faces();
        out.extend(
            self.geometry()
                .into_iter()
                .filter_map(|(k, g)| Some((k, g.reference()?))),
        );
        out
    }
}

/// `invalid_param` at `/field`: the tweak moves nothing.
fn still(field: &str, why: &str) -> Error {
    Error::new(
        Code::InvalidParam,
        format!("a move_face moves nothing: {why}"),
    )
    .with("pointer", format!("/{field}"))
}

/// A move's `groups`: `[{distance, faces}]`, none when left out.
fn groups(o: &mut Obj, default: Unit, env: &Env) -> Result<Vec<FaceGroup>> {
    let Some(v) = o.take_opt("groups") else {
        return Ok(Vec::new());
    };
    let a = v
        .as_array()
        .ok_or_else(|| invalid("/groups", "groups is an array of {distance, faces}"))?;
    a.iter()
        .enumerate()
        .map(|(i, g)| {
            let at = format!("/groups/{i}");
            let mut f = Obj::new(g, &at)?;
            let quantity = Quantity::from_json_as(
                &f.take("distance")?,
                Field::Length(default),
                &|n| env.get(n),
            )
            .map_err(|e| e.with("pointer", format!("{at}/distance")))?;
            let faces = refs(&f.take("faces")?, &format!("{at}/faces"))?;
            f.finish()?;
            Ok(FaceGroup { quantity, faces })
        })
        .collect()
}

/// Refuses a face named twice, the pointer the second naming's.
fn once(faces: Vec<(String, &Reference)>) -> Result<()> {
    for (k, (role, r)) in faces.iter().enumerate() {
        if let Some((first, _)) = faces[..k].iter().find(|(_, x)| x == r) {
            return Err(Error::new(
                Code::InvalidParam,
                format!(
                    "face {}/{} is named twice, as {first} and {role}: each face is tweaked once",
                    r.feature, r.name
                ),
            )
            .with("pointer", format!("/{}", role.replace('.', "/")))
            .with("first", format!("/{}", first.replace('.', "/"))));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::files::Obj;

    const A: &str = "0123456789abcdef0123456789abcdef";
    const B: &str = "fedcba9876543210fedcba9876543210";

    fn face(n: &str) -> Value {
        json!({"feature": "base", "name": n})
    }

    fn moved(v: Value) -> Result<MoveFace> {
        let mut o = Obj::new(&v, "")?;
        let m = MoveFace::from_fields(&mut o, Unit::Mm, &Env::empty())?;
        o.finish()?;
        Ok(m)
    }

    fn replaced(v: Value) -> Result<ReplaceFace> {
        let mut o = Obj::new(&v, "")?;
        let r = ReplaceFace::from_fields(&mut o)?;
        o.finish()?;
        Ok(r)
    }

    #[test]
    fn moves_round_trip() {
        // A translation: bare numbers take the default unit, written out;
        // tangent true is written as nothing.
        let v = json!({"target": "plate", "by": "translate", "direction": "x", "distance": 5,
                       "faces": [face(A)], "tangent": true});
        let m = moved(v).unwrap();
        assert_eq!(
            m.fields(),
            json!({"target": "plate", "by": "translate", "direction": "x",
                   "distance": "5 mm", "faces": [face(A)]})
        );
        assert_eq!(moved(m.fields()).unwrap(), m);
        // A rotation about an edge, tangent off.
        let v = json!({"target": "plate", "by": "rotate", "axis": face(B), "angle": "3 deg",
                       "faces": [face(A)], "tangent": false});
        let m = moved(v.clone()).unwrap();
        assert_eq!(m.fields(), v);
        assert!(!m.tangent);
        // An offset: groups of their own, distances of either sign.
        let v = json!({"target": "plate", "by": "offset", "distance": "0.5 mm",
                       "faces": [face(A)],
                       "groups": [{"distance": "-1 mm", "faces": [face(B)]}]});
        let m = moved(v.clone()).unwrap();
        assert_eq!(m.fields(), v);
        let roles: Vec<String> = m.faces().into_iter().map(|(r, _)| r).collect();
        assert_eq!(roles, ["faces.0", "groups.0.faces.0"]);
        // An offset of groups alone needs no listed faces.
        let v = json!({"target": "plate", "by": "offset", "distance": "0 mm",
                       "groups": [{"distance": "1 mm", "faces": [face(B)]}]});
        assert_eq!(moved(v.clone()).unwrap().fields(), v);
    }

    #[test]
    fn moves_name_their_direction_and_axis_as_geometry() {
        let m = moved(json!({"target": "p", "by": "rotate", "axis": face(B), "angle": "3 deg",
                             "faces": [face(A)]}))
        .unwrap();
        let geo: Vec<String> = m.geometry().into_iter().map(|(r, _)| r).collect();
        assert_eq!(geo, ["axis"]);
        // Its references: the faces, then the axis's edge.
        let refs: Vec<String> = m.references().into_iter().map(|(r, _)| r).collect();
        assert_eq!(refs, ["faces.0", "axis"]);
        let m = moved(json!({"target": "p", "by": "translate", "direction": "z",
                             "distance": "1 mm", "faces": [face(A)]}))
        .unwrap();
        let refs: Vec<String> = m.references().into_iter().map(|(r, _)| r).collect();
        assert_eq!(refs, ["faces.0"]);
    }

    #[test]
    fn replacements_round_trip() {
        let v = json!({"target": "base",
                       "groups": [{"with": {"feature": "lid", "name": B}, "faces": [face(A)]},
                                  {"with": "xy", "faces": [face(B)]}]});
        let r = replaced(v.clone()).unwrap();
        assert_eq!(r.fields(), v);
        assert_eq!(replaced(r.fields()).unwrap(), r);
        let roles: Vec<String> = r.faces().into_iter().map(|(x, _)| x).collect();
        assert_eq!(roles, ["groups.0.faces.0", "groups.1.faces.0"]);
        let geo: Vec<String> = r.geometry().into_iter().map(|(x, _)| x).collect();
        assert_eq!(geo, ["groups.0.with", "groups.1.with"]);
        let refs: Vec<String> = r.references().into_iter().map(|(x, _)| x).collect();
        assert_eq!(refs, ["groups.0.faces.0", "groups.1.faces.0", "groups.0.with"]);
    }

    #[test]
    fn tweaks_are_checked() {
        for (v, code, pointer) in [
            (json!({"by": "translate", "direction": "x", "distance": 1, "faces": [face(A)]}),
             Code::InvalidDocument, "/target"),
            (json!({"target": "p", "direction": "x", "distance": 1, "faces": [face(A)]}),
             Code::InvalidDocument, "/by"),
            (json!({"target": "p", "by": "slide", "faces": [face(A)]}),
             Code::InvalidDocument, "/by"),
            (json!({"target": "p", "by": "translate", "distance": 1, "faces": [face(A)]}),
             Code::InvalidDocument, "/direction"),
            (json!({"target": "p", "by": "translate", "direction": "x", "distance": 0,
                    "faces": [face(A)]}),
             Code::InvalidParam, "/distance"),
            (json!({"target": "p", "by": "translate", "direction": "x", "distance": "1 deg",
                    "faces": [face(A)]}),
             Code::UnitMismatch, "/distance"),
            (json!({"target": "p", "by": "rotate", "axis": "z", "angle": 0, "faces": [face(A)]}),
             Code::InvalidParam, "/angle"),
            // A field another `by` takes.
            (json!({"target": "p", "by": "translate", "direction": "x", "distance": 1,
                    "angle": "3 deg", "faces": [face(A)]}),
             Code::InvalidDocument, "/angle"),
            (json!({"target": "p", "by": "translate", "direction": "x", "distance": 1}),
             Code::InvalidDocument, "/faces"),
            (json!({"target": "p", "by": "offset", "distance": 0, "faces": [face(A)]}),
             Code::InvalidParam, "/distance"),
            (json!({"target": "p", "by": "offset", "distance": 1, "faces": [face(A)],
                    "groups": [{"distance": 2, "faces": [face(A)]}]}),
             Code::InvalidParam, "/groups/0/faces/0"),
            (json!({"target": "p", "by": "translate", "direction": "x", "distance": 1,
                    "faces": [face(A), face(A)]}),
             Code::InvalidParam, "/faces/1"),
            (json!({"target": "p", "by": "translate", "direction": "x", "distance": 1,
                    "faces": [face(A)], "tangent": "yes"}),
             Code::InvalidDocument, "/tangent"),
        ] {
            let e = moved(v.clone()).unwrap_err();
            assert_eq!((e.code, e.details["pointer"].as_str()), (code, Some(pointer)), "{v}");
        }
        for (v, code, pointer) in [
            (json!({"target": "p"}), Code::InvalidDocument, "/groups"),
            (json!({"target": "p", "groups": []}), Code::InvalidDocument, "/groups"),
            (json!({"target": "p", "groups": [{"faces": [face(A)]}]}),
             Code::InvalidDocument, "/groups/0/with"),
            // Its own surface changes nothing.
            (json!({"target": "p", "groups": [{"with": face(A), "faces": [face(B), face(A)]}]}),
             Code::InvalidParam, "/groups/0/faces/1"),
            (json!({"target": "p", "groups": [{"with": "xy", "faces": [face(A)]},
                                              {"with": "yz", "faces": [face(A)]}]}),
             Code::InvalidParam, "/groups/1/faces/0"),
        ] {
            let e = replaced(v.clone()).unwrap_err();
            assert_eq!((e.code, e.details["pointer"].as_str()), (code, Some(pointer)), "{v}");
        }
    }

    #[test]
    fn distances_and_angles_read_parameters() {
        let p = [("d".to_owned(), crate::expr::Expr::parse("2 mm").unwrap())].into();
        let v = json!({"target": "p", "by": "offset", "distance": "d / 2", "faces": [face(A)],
                       "groups": [{"distance": "d + 1 mm", "faces": [face(B)]}]});
        let mut o = Obj::new(&v, "").unwrap();
        let m = MoveFace::from_fields(&mut o, Unit::Mm, &Env::new(&p)).unwrap();
        let MoveBy::Offset { distance, groups } = &m.by else { panic!() };
        assert_eq!(distance.meters(), 0.001);
        assert_eq!(groups[0].quantity.meters(), 0.003);
    }
}
