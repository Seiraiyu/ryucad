//! Face tweaks (design 400): faces of one solid, its `target`, moved
//! (`move_face`: translated along a direction, turned about an axis, or
//! offset along their outward normals), given another face's surface or
//! a plane (`replace_face`), or drafted about a neutral plane or a
//! parting line (`draft`, plan 17.B). A tweak keeps every face, edge and
//! vertex and their names, and makes nothing. Faces are references into the
//! target as the feature sees it.

use super::body::{FaceGroup, refs, refs_json, solid_id};
use super::edit::Retrim;
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

/// The feature format drafts are new in (plan 17.B). Taken in the order
/// stages merge (16 face tweaks, 17 ryucad 21.D's imports placed): if
/// another stage holds 18 when this merges, move this constant to the next
/// free number; nothing else names it.
pub const DRAFT_FORMAT: u64 = 18;

/// The least and greatest size of a draft's angle, as ryukei 9 takes it
/// (`tol::ANGULAR_RESOLUTION`; `tweak::MAX_ANGLE`, a right angle less a
/// hundredth of a radian). ryucad's tests hold them to the kernel's.
pub const MIN_DRAFT_ANGLE: f64 = 1e-11;
pub const MAX_DRAFT_ANGLE: f64 = core::f64::consts::FRAC_PI_2 - 0.01;

/// The feature types this module reads.
pub const TYPES: [&str; 3] = ["move_face", "replace_face", "draft"];

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
    /// Whether it may change the solid's topology and make its broken
    /// blends again (ryucad 22); keep, keep when written neither.
    pub retrim: Retrim,
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
    /// Whether it may change the solid's topology and make its broken
    /// blends again (ryucad 22); keep, keep when written neither.
    pub retrim: Retrim,
}

/// What a draft's faces turn about.
#[derive(Clone, Debug, PartialEq)]
pub enum DraftHinge {
    /// The plane `neutral` names (an origin or datum plane, a sketch, a
    /// planar face); the pull is its normal.
    Neutral { plane: GeomRef },
    /// Edges of the target the faces turn about (`parting`), and the pull:
    /// any axis, or a plane's normal.
    Parting {
        edges: Vec<Reference>,
        pull: GeomRef,
    },
}

/// A `draft`: `faces` of `target` drafted to `angle` from the pull about
/// `hinge` (each group's faces to its own), the pull turned round by
/// `reverse`, the faces in `flip` drafted against it; with `tangent`,
/// faces tangent to a drafted face join the draft (true unless written
/// false). A positive angle tapers a boss inward along the pull and opens
/// a hole.
#[derive(Clone, Debug, PartialEq)]
pub struct Draft {
    pub target: Id,
    pub angle: Quantity,
    pub hinge: DraftHinge,
    pub reverse: bool,
    pub faces: Vec<Reference>,
    pub groups: Vec<FaceGroup>,
    pub flip: Vec<Reference>,
    pub tangent: bool,
    /// Whether it may change the solid's topology and make its broken
    /// blends again (ryucad 22); keep, keep when written neither.
    pub retrim: Retrim,
}

/// The fields a request may give a tweak of type `ty`.
pub fn request_fields(ty: &str) -> &'static [&'static str] {
    match ty {
        "replace_face" => &["target", "targets", "groups", "topology", "blends"],
        "draft" => &[
            "target", "targets", "angle", "neutral", "parting", "pull", "reverse", "faces",
            "groups", "flip", "tangent", "topology", "blends",
        ],
        _ => &[
            "target",
            "targets",
            "by",
            "direction",
            "distance",
            "axis",
            "angle",
            "faces",
            "groups",
            "tangent",
            "topology",
            "blends",
        ],
    }
}

impl MoveFace {
    /// Its fields: `by` always, tangent only when false, no `faces` or
    /// `groups` when there are none.
    pub fn fields(&self) -> Value {
        let mut v = json!({"target": self.target, "by": self.by.as_str()});
        match &self.by {
            MoveBy::Translate {
                direction,
                distance,
            } => {
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
        self.retrim.write(&mut v);
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
                MoveBy::Translate {
                    direction,
                    distance,
                }
            }
            Some("rotate") => {
                let axis = GeomRef::from_json(&o.take("axis")?, "/axis")?;
                let angle =
                    Quantity::from_json_as(&o.take("angle")?, Field::Angle, &|n| env.get(n))
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
                    return Err(still("distance", "its distance and every group's are zero"));
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
        let retrim = Retrim::take(o)?;
        let m = MoveFace {
            target,
            by,
            faces,
            tangent,
            retrim,
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
        let mut v = json!({
            "target": self.target,
            "groups": self.groups.iter()
                .map(|g| json!({"with": g.with.to_json(), "faces": refs_json(&g.faces)}))
                .collect::<Vec<_>>(),
        });
        self.retrim.write(&mut v);
        v
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
        let retrim = Retrim::take(o)?;
        let r = ReplaceFace {
            target,
            groups,
            retrim,
        };
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

impl Draft {
    /// Its fields: the hinge's, `reverse` only when true, `tangent` only
    /// when false, no `faces`, `groups` or `flip` when there are none.
    pub fn fields(&self) -> Value {
        let mut v = json!({"target": self.target, "angle": self.angle.text()});
        match &self.hinge {
            DraftHinge::Neutral { plane } => v["neutral"] = plane.to_json(),
            DraftHinge::Parting { edges, pull } => {
                v["parting"] = refs_json(edges);
                v["pull"] = pull.to_json();
            }
        }
        if self.reverse {
            v["reverse"] = json!(true);
        }
        if !self.faces.is_empty() {
            v["faces"] = refs_json(&self.faces);
        }
        if !self.groups.is_empty() {
            v["groups"] = Value::Array(
                self.groups
                    .iter()
                    .map(|g| json!({"angle": g.quantity.text(), "faces": refs_json(&g.faces)}))
                    .collect(),
            );
        }
        if !self.flip.is_empty() {
            v["flip"] = refs_json(&self.flip);
        }
        if !self.tangent {
            v["tangent"] = json!(false);
        }
        self.retrim.write(&mut v);
        v
    }

    #[doc(hidden)]
    pub fn from_fields(o: &mut Obj, env: &Env) -> Result<Draft> {
        let target = solid_id(o, "target")?;
        let angle = draft_angle(&o.take("angle")?, "/angle", env)?;
        let (neutral, parting, pull) = (
            o.take_opt("neutral"),
            o.take_opt("parting"),
            o.take_opt("pull"),
        );
        let hinge = match (neutral, parting) {
            (Some(_), Some(_)) | (None, None) => {
                return Err(Error::new(
                    Code::InvalidParam,
                    "a draft turns its faces about a neutral plane (neutral) or a parting line (parting, with pull): give one",
                )
                .with("pointer", "/neutral"));
            }
            (Some(n), None) => {
                if pull.is_some() {
                    return Err(Error::new(
                        Code::InvalidParam,
                        "a draft about a neutral plane pulls along the plane's normal: it takes no pull (reverse turns it round)",
                    )
                    .with("pointer", "/pull"));
                }
                DraftHinge::Neutral {
                    plane: GeomRef::from_json(&n, "/neutral")?,
                }
            }
            (None, Some(p)) => {
                let edges = refs(&p, "/parting")?;
                let pull = pull.ok_or_else(|| {
                    invalid(
                        "/pull",
                        "a draft from a parting line names its pull: any axis, or a plane's normal",
                    )
                })?;
                DraftHinge::Parting {
                    edges,
                    pull: GeomRef::from_json(&pull, "/pull")?,
                }
            }
        };
        let flag = |o: &mut Obj, k: &str, default: bool| -> Result<bool> {
            match o.take_opt(k) {
                None => Ok(default),
                Some(Value::Bool(b)) => Ok(b),
                Some(_) => Err(invalid(&format!("/{k}"), &format!("{k} is true or false"))),
            }
        };
        let reverse = flag(o, "reverse", false)?;
        let faces = match o.take_opt("faces") {
            None => Vec::new(),
            Some(Value::Array(a)) if a.is_empty() => Vec::new(),
            Some(v) => refs(&v, "/faces")?,
        };
        let groups = match o.take_opt("groups") {
            None => Vec::new(),
            Some(v) => angle_groups(&v, env)?,
        };
        let flip = match o.take_opt("flip") {
            None => Vec::new(),
            Some(Value::Array(a)) if a.is_empty() => Vec::new(),
            Some(v) => refs(&v, "/flip")?,
        };
        let tangent = flag(o, "tangent", true)?;
        if faces.is_empty() && groups.is_empty() {
            return Err(invalid(
                "/faces",
                "a draft names the faces it drafts: a non-empty array of faces {feature, name}, or groups",
            ));
        }
        let retrim = Retrim::take(o)?;
        let d = Draft {
            target,
            angle,
            hinge,
            reverse,
            faces,
            groups,
            flip,
            tangent,
            retrim,
        };
        once(d.drafted())?;
        let flipped: Vec<(String, &Reference)> = d
            .flip
            .iter()
            .enumerate()
            .map(|(i, r)| (format!("flip.{i}"), r))
            .collect();
        once(flipped)?;
        let drafted = d.drafted();
        if let Some((i, r)) = d
            .flip
            .iter()
            .enumerate()
            .find(|(_, r)| !drafted.iter().any(|(_, x)| x == r))
        {
            return Err(Error::new(
                Code::InvalidParam,
                format!(
                    "face {}/{} is flipped but not drafted: list it in faces or a group too",
                    r.feature, r.name
                ),
            )
            .with("pointer", format!("/flip/{i}")));
        }
        Ok(d)
    }

    /// The faces it drafts, each with its role: `faces.<i>`, then
    /// `groups.<i>.faces.<j>`.
    pub fn drafted(&self) -> Vec<(String, &Reference)> {
        let mut out: Vec<(String, &Reference)> = self
            .faces
            .iter()
            .enumerate()
            .map(|(i, r)| (format!("faces.{i}"), r))
            .collect();
        for (i, g) in self.groups.iter().enumerate() {
            for (j, r) in g.faces.iter().enumerate() {
                out.push((format!("groups.{i}.faces.{j}"), r));
            }
        }
        out
    }

    /// The signed angle (radians) face `r` is drafted to: its group's,
    /// else the draft's.
    pub fn angle_of(&self, r: &Reference) -> f64 {
        self.groups
            .iter()
            .find(|g| g.faces.contains(r))
            .map_or(self.angle.radians(), |g| g.quantity.radians())
    }

    /// Its geometric references: `neutral`, or a parting draft's `pull`.
    pub fn geometry(&self) -> Vec<(String, &GeomRef)> {
        match &self.hinge {
            DraftHinge::Neutral { plane } => vec![("neutral".to_owned(), plane)],
            DraftHinge::Parting { pull, .. } => vec![("pull".to_owned(), pull)],
        }
    }

    /// Its references to faces, edges and vertices: its faces, then a
    /// parting line's edges (`parting.<i>`), its flipped faces
    /// (`flip.<i>`), then a neutral plane's or pull's entity.
    pub fn references(&self) -> Vec<(String, &Reference)> {
        let mut out = self.drafted();
        if let DraftHinge::Parting { edges, .. } = &self.hinge {
            out.extend(
                edges
                    .iter()
                    .enumerate()
                    .map(|(i, r)| (format!("parting.{i}"), r)),
            );
        }
        out.extend(
            self.flip
                .iter()
                .enumerate()
                .map(|(i, r)| (format!("flip.{i}"), r)),
        );
        out.extend(
            self.geometry()
                .into_iter()
                .filter_map(|(k, g)| Some((k, g.reference()?))),
        );
        out
    }
}

/// A draft's angle: an angle whose size is in
/// `[MIN_DRAFT_ANGLE, MAX_DRAFT_ANGLE]`, of either sign.
fn draft_angle(v: &Value, at: &str, env: &Env) -> Result<Quantity> {
    let q = Quantity::from_json_as(v, Field::Angle, &|n| env.get(n))
        .map_err(|e| e.with("pointer", at))?;
    let a = q.radians().abs();
    if !(MIN_DRAFT_ANGLE..=MAX_DRAFT_ANGLE).contains(&a) {
        return Err(Error::new(
            Code::InvalidParam,
            format!(
                "a draft's angle is from 1e-11 rad to 90° less 0.01 rad (about 89.43°), of either sign, not {q}"
            ),
        )
        .with("pointer", at)
        .with("value", q.text()));
    }
    Ok(q)
}

/// A draft's `groups`: `[{angle, faces}]`.
fn angle_groups(v: &Value, env: &Env) -> Result<Vec<FaceGroup>> {
    let a = v
        .as_array()
        .ok_or_else(|| invalid("/groups", "groups is an array of {angle, faces}"))?;
    a.iter()
        .enumerate()
        .map(|(i, g)| {
            let at = format!("/groups/{i}");
            let mut f = Obj::new(g, &at)?;
            let quantity = draft_angle(&f.take("angle")?, &format!("{at}/angle"), env)?;
            let faces = refs(&f.take("faces")?, &format!("{at}/faces"))?;
            f.finish()?;
            Ok(FaceGroup { quantity, faces })
        })
        .collect()
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
            let quantity =
                Quantity::from_json_as(&f.take("distance")?, Field::Length(default), &|n| {
                    env.get(n)
                })
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
        let m = moved(
            json!({"target": "p", "by": "rotate", "axis": face(B), "angle": "3 deg",
                             "faces": [face(A)]}),
        )
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
        assert_eq!(
            refs,
            ["groups.0.faces.0", "groups.1.faces.0", "groups.0.with"]
        );
    }

    #[test]
    fn tweaks_are_checked() {
        for (v, code, pointer) in [
            (
                json!({"by": "translate", "direction": "x", "distance": 1, "faces": [face(A)]}),
                Code::InvalidDocument,
                "/target",
            ),
            (
                json!({"target": "p", "direction": "x", "distance": 1, "faces": [face(A)]}),
                Code::InvalidDocument,
                "/by",
            ),
            (
                json!({"target": "p", "by": "slide", "faces": [face(A)]}),
                Code::InvalidDocument,
                "/by",
            ),
            (
                json!({"target": "p", "by": "translate", "distance": 1, "faces": [face(A)]}),
                Code::InvalidDocument,
                "/direction",
            ),
            (
                json!({"target": "p", "by": "translate", "direction": "x", "distance": 0,
                    "faces": [face(A)]}),
                Code::InvalidParam,
                "/distance",
            ),
            (
                json!({"target": "p", "by": "translate", "direction": "x", "distance": "1 deg",
                    "faces": [face(A)]}),
                Code::UnitMismatch,
                "/distance",
            ),
            (
                json!({"target": "p", "by": "rotate", "axis": "z", "angle": 0, "faces": [face(A)]}),
                Code::InvalidParam,
                "/angle",
            ),
            // A field another `by` takes.
            (
                json!({"target": "p", "by": "translate", "direction": "x", "distance": 1,
                    "angle": "3 deg", "faces": [face(A)]}),
                Code::InvalidDocument,
                "/angle",
            ),
            (
                json!({"target": "p", "by": "translate", "direction": "x", "distance": 1}),
                Code::InvalidDocument,
                "/faces",
            ),
            (
                json!({"target": "p", "by": "offset", "distance": 0, "faces": [face(A)]}),
                Code::InvalidParam,
                "/distance",
            ),
            (
                json!({"target": "p", "by": "offset", "distance": 1, "faces": [face(A)],
                    "groups": [{"distance": 2, "faces": [face(A)]}]}),
                Code::InvalidParam,
                "/groups/0/faces/0",
            ),
            (
                json!({"target": "p", "by": "translate", "direction": "x", "distance": 1,
                    "faces": [face(A), face(A)]}),
                Code::InvalidParam,
                "/faces/1",
            ),
            (
                json!({"target": "p", "by": "translate", "direction": "x", "distance": 1,
                    "faces": [face(A)], "tangent": "yes"}),
                Code::InvalidDocument,
                "/tangent",
            ),
        ] {
            let e = moved(v.clone()).unwrap_err();
            assert_eq!(
                (e.code, e.details["pointer"].as_str()),
                (code, Some(pointer)),
                "{v}"
            );
        }
        for (v, code, pointer) in [
            (json!({"target": "p"}), Code::InvalidDocument, "/groups"),
            (
                json!({"target": "p", "groups": []}),
                Code::InvalidDocument,
                "/groups",
            ),
            (
                json!({"target": "p", "groups": [{"faces": [face(A)]}]}),
                Code::InvalidDocument,
                "/groups/0/with",
            ),
            // Its own surface changes nothing.
            (
                json!({"target": "p", "groups": [{"with": face(A), "faces": [face(B), face(A)]}]}),
                Code::InvalidParam,
                "/groups/0/faces/1",
            ),
            (
                json!({"target": "p", "groups": [{"with": "xy", "faces": [face(A)]},
                                              {"with": "yz", "faces": [face(A)]}]}),
                Code::InvalidParam,
                "/groups/1/faces/0",
            ),
        ] {
            let e = replaced(v.clone()).unwrap_err();
            assert_eq!(
                (e.code, e.details["pointer"].as_str()),
                (code, Some(pointer)),
                "{v}"
            );
        }
    }

    #[test]
    fn distances_and_angles_read_parameters() {
        let p = [("d".to_owned(), crate::expr::Expr::parse("2 mm").unwrap())].into();
        let v = json!({"target": "p", "by": "offset", "distance": "d / 2", "faces": [face(A)],
                       "groups": [{"distance": "d + 1 mm", "faces": [face(B)]}]});
        let mut o = Obj::new(&v, "").unwrap();
        let m = MoveFace::from_fields(&mut o, Unit::Mm, &Env::new(&p)).unwrap();
        let MoveBy::Offset { distance, groups } = &m.by else {
            panic!()
        };
        assert_eq!(distance.meters(), 0.001);
        assert_eq!(groups[0].quantity.meters(), 0.003);
    }

    fn drafted(v: Value) -> Result<Draft> {
        let mut o = Obj::new(&v, "")?;
        let d = Draft::from_fields(&mut o, &Env::empty())?;
        o.finish()?;
        Ok(d)
    }

    fn edge(n: &str) -> Value {
        json!({"feature": "base", "name": n})
    }

    #[test]
    fn drafts_round_trip() {
        // About a neutral plane: reverse false and tangent true are written
        // as nothing.
        let v = json!({"target": "base", "angle": "2 deg", "neutral": "xy",
                       "faces": [face(A)], "reverse": false, "tangent": true});
        let d = drafted(v).unwrap();
        assert_eq!(
            d.fields(),
            json!({"target": "base", "angle": "2 deg", "neutral": "xy", "faces": [face(A)]})
        );
        assert_eq!(drafted(d.fields()).unwrap(), d);
        assert!(d.tangent && !d.reverse);
        // From a parting line with a pull, a group at its own angle, a face
        // flipped, the pull reversed, tangency off.
        let v = json!({"target": "base", "angle": "3 deg",
                       "parting": [edge(B)], "pull": "z", "reverse": true,
                       "faces": [face(A)],
                       "groups": [{"angle": "-5 deg", "faces": [face(B)]}],
                       "flip": [face(B)], "tangent": false});
        let d = drafted(v.clone()).unwrap();
        assert_eq!(d.fields(), v);
        let roles: Vec<String> = d.drafted().into_iter().map(|(r, _)| r).collect();
        assert_eq!(roles, ["faces.0", "groups.0.faces.0"]);
        let geo: Vec<String> = d.geometry().into_iter().map(|(r, _)| r).collect();
        assert_eq!(geo, ["pull"]);
        let refs: Vec<String> = d.references().into_iter().map(|(r, _)| r).collect();
        assert_eq!(refs, ["faces.0", "groups.0.faces.0", "parting.0", "flip.0"]);
        // A neutral plane that is a face is a reference too.
        let d = drafted(
            json!({"target": "base", "angle": "1 deg", "neutral": face(B),
                               "faces": [face(A)]}),
        )
        .unwrap();
        let refs: Vec<String> = d.references().into_iter().map(|(r, _)| r).collect();
        assert_eq!(refs, ["faces.0", "neutral"]);
        // Groups alone need no listed faces.
        let v = json!({"target": "base", "angle": "1 deg", "neutral": "xy",
                       "groups": [{"angle": "2 deg", "faces": [face(A)]}]});
        assert_eq!(drafted(v.clone()).unwrap().fields(), v);
    }

    #[test]
    fn drafts_are_checked() {
        for (v, code, pointer) in [
            (
                json!({"target": "p", "angle": "2 deg", "faces": [face(A)]}),
                Code::InvalidParam,
                "/neutral",
            ),
            (
                json!({"target": "p", "angle": "2 deg", "neutral": "xy", "parting": [edge(B)],
                    "pull": "z", "faces": [face(A)]}),
                Code::InvalidParam,
                "/neutral",
            ),
            (
                json!({"target": "p", "angle": "2 deg", "neutral": "xy", "pull": "z",
                    "faces": [face(A)]}),
                Code::InvalidParam,
                "/pull",
            ),
            (
                json!({"target": "p", "angle": "2 deg", "parting": [edge(B)], "faces": [face(A)]}),
                Code::InvalidDocument,
                "/pull",
            ),
            (
                json!({"target": "p", "angle": "2 deg", "parting": [], "pull": "z",
                    "faces": [face(A)]}),
                Code::InvalidDocument,
                "/parting",
            ),
            (
                json!({"target": "p", "angle": "0 deg", "neutral": "xy", "faces": [face(A)]}),
                Code::InvalidParam,
                "/angle",
            ),
            (
                json!({"target": "p", "angle": "90 deg", "neutral": "xy", "faces": [face(A)]}),
                Code::InvalidParam,
                "/angle",
            ),
            (
                json!({"target": "p", "angle": "2 mm", "neutral": "xy", "faces": [face(A)]}),
                Code::UnitMismatch,
                "/angle",
            ),
            (
                json!({"target": "p", "angle": "2 deg", "neutral": "xy", "faces": [face(A)],
                    "groups": [{"angle": "89.9 deg", "faces": [face(B)]}]}),
                Code::InvalidParam,
                "/groups/0/angle",
            ),
            (
                json!({"target": "p", "angle": "2 deg", "neutral": "xy"}),
                Code::InvalidDocument,
                "/faces",
            ),
            (
                json!({"target": "p", "angle": "2 deg", "neutral": "xy", "faces": [face(A)],
                    "groups": [{"angle": "3 deg", "faces": [face(A)]}]}),
                Code::InvalidParam,
                "/groups/0/faces/0",
            ),
            (
                json!({"target": "p", "angle": "2 deg", "neutral": "xy", "faces": [face(A)],
                    "flip": [face(B)]}),
                Code::InvalidParam,
                "/flip/0",
            ),
            (
                json!({"target": "p", "angle": "2 deg", "neutral": "xy", "faces": [face(A)],
                    "flip": [face(A), face(A)]}),
                Code::InvalidParam,
                "/flip/1",
            ),
            (
                json!({"target": "p", "angle": "2 deg", "neutral": "xy", "faces": [face(A)],
                    "reverse": "yes"}),
                Code::InvalidDocument,
                "/reverse",
            ),
            (
                json!({"target": "p", "angle": "2 deg", "neutral": "xy", "faces": [face(A)],
                    "distance": "1 mm"}),
                Code::InvalidDocument,
                "/distance",
            ),
        ] {
            let e = drafted(v.clone()).unwrap_err();
            assert_eq!(
                (e.code, e.details["pointer"].as_str()),
                (code, Some(pointer)),
                "{v}"
            );
        }
    }

    #[test]
    fn draft_angles_read_parameters_and_keep_their_sign() {
        let p = [("a".to_owned(), crate::expr::Expr::parse("2 deg").unwrap())].into();
        let v = json!({"target": "p", "angle": "a", "neutral": "xy", "faces": [face(A)],
                       "groups": [{"angle": "-a * 2", "faces": [face(B)]}]});
        let mut o = Obj::new(&v, "").unwrap();
        let d = Draft::from_fields(&mut o, &Env::new(&p)).unwrap();
        assert!((d.angle.radians() - 2f64.to_radians()).abs() < 1e-15);
        assert!((d.groups[0].quantity.radians() + 4f64.to_radians()).abs() < 1e-15);
    }

    #[test]
    fn a_tweaks_topology_and_blends_round_trip_and_raise_its_format() {
        use crate::doc::files::{Feature, feature_format};
        let edit = super::super::edit::FORMAT;
        let face = json!({"feature": "b", "name": format!("{:032x}", 1)});
        let read = |v: &Value| Feature::from_json(v, Unit::Mm, &Env::empty()).unwrap();
        let mv = json!({"format": edit, "kind": "ryucad.feature", "type": "move_face",
            "target": "b", "by": "translate", "direction": "x", "distance": "5 mm",
            "faces": [face], "topology": "change"});
        let f = read(&mv);
        assert_eq!((f.format(), feature_format(&mv)), (edit, edit));
        assert_eq!(f.to_json(), mv);
        let Feature::MoveFace(m) = &f else {
            panic!("a move")
        };
        assert!(m.retrim.topology && !m.retrim.blends);
        // Without them: ryucad 17's format and bytes.
        let mut old = mv.clone();
        old.as_object_mut().unwrap().remove("topology");
        old["format"] = json!(FORMAT);
        let f = read(&old);
        assert_eq!((f.format(), feature_format(&old)), (FORMAT, FORMAT));
        assert_eq!(f.to_json(), old);
        // `keep` spelled out reads, and isn't written.
        let mut kept = old.clone();
        kept["topology"] = json!("keep");
        assert_eq!(read(&kept).to_json(), old);
        // A draft with its blends rebuilt; a replacement with both.
        let d = json!({"format": edit, "kind": "ryucad.feature", "type": "draft", "target": "b",
            "angle": "2 deg", "neutral": "xy", "faces": [face], "blends": "rebuild"});
        assert_eq!((read(&d).format(), read(&d).to_json()), (edit, d.clone()));
        assert_eq!(feature_format(&d), edit);
        let r = json!({"format": edit, "kind": "ryucad.feature", "type": "replace_face",
            "target": "b", "groups": [{"with": "xy", "faces": [face]}],
            "topology": "change", "blends": "rebuild"});
        assert_eq!((read(&r).format(), read(&r).to_json()), (edit, r.clone()));
        assert_eq!(
            read(&r).retrim(),
            Some(super::super::edit::Retrim {
                topology: true,
                blends: true
            })
        );
        // Every tweak's request takes them.
        for ty in TYPES {
            assert!(request_fields(ty).contains(&"topology"), "{ty}");
            assert!(request_fields(ty).contains(&"blends"), "{ty}");
        }
    }
}
