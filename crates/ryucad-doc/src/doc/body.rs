//! Body features: a solid hollowed to a wall (`shell`); a solid grown,
//! shrunk, or its faces pushed and pulled (`offset`); a solid's mirror
//! image (`mirror`: a solid of its own, or joined to, cut from or
//! intersected with solids, as an extrude's body is); and a solid split
//! by a plane (`split`: the target keeps the side behind the plane, and
//! the side in front founds a solid named by the split, or is thrown
//! away). A shell, offset or split acts on one solid, its `target`, named
//! by its founder; the faces it names are references into that solid as
//! the feature sees it.

use super::files::{Obj, invalid};
use super::geometry::GeomRef;
use super::id::Id;
use super::reference::Reference;
use super::solid::{Combine, Operation, targets_from_json};
use crate::error::{Code, Error, Result};
use crate::expr::Env;
use crate::units::{Field, Quantity, Unit};
use serde_json::{Value, json};

/// The feature format shells and offsets are new in.
pub const FORMAT: u64 = 9;

/// The feature format mirrors and splits are new in.
pub const MIRROR_FORMAT: u64 = 10;

/// Which sides of its plane a split keeps.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Keep {
    /// Both: the target keeps the side behind, and the side in front
    /// founds a solid named by the split (written as nothing).
    #[default]
    Both,
    /// The side behind only: a trim, which founds nothing.
    Behind,
}

impl Keep {
    pub fn as_str(self) -> &'static str {
        match self {
            Keep::Both => "both",
            Keep::Behind => "behind",
        }
    }

    pub fn parse(s: &str) -> Option<Keep> {
        match s {
            "both" => Some(Keep::Both),
            "behind" => Some(Keep::Behind),
            _ => None,
        }
    }
}

/// A mirror: `solid`, as the mirror sees it, reflected in `plane`, its
/// copy founding a solid (`new`) or joined to, cut from or intersected
/// with `combine`'s targets.
#[derive(Clone, Debug, PartialEq)]
pub struct Mirror {
    pub solid: Id,
    pub plane: GeomRef,
    pub combine: Combine,
}

/// A split: `target` cut by `plane` (its normal turned round by `flip`).
/// The target keeps the side behind the plane, against its normal; with
/// `keep: both` the side in front founds a solid named by the split.
#[derive(Clone, Debug, PartialEq)]
pub struct Split {
    pub target: Id,
    pub plane: GeomRef,
    pub flip: bool,
    pub keep: Keep,
}

impl Mirror {
    /// Its fields; the operation always, even `new`, so a file says what
    /// the copy does.
    pub fn fields(&self) -> Value {
        let mut v = json!({"solid": self.solid, "plane": self.plane.to_json(),
                           "operation": self.combine.operation.as_str()});
        if !self.combine.is_new() {
            v["targets"] = json!(self.combine.targets);
        }
        v
    }

    #[doc(hidden)]
    pub fn from_fields(o: &mut Obj) -> Result<Mirror> {
        let solid = solid_id(o, "solid")?;
        let plane = GeomRef::from_json(&o.take("plane")?, "/plane")?;
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
                    "a mirror whose copy founds a new solid has no targets",
                ));
            }
            (_, true) => {
                return Err(invalid(
                    "/targets",
                    &format!(
                        "a mirror whose copy {}s names the solids it acts on: targets is required",
                        operation.as_str()
                    ),
                ));
            }
            (_, false) => Combine { operation, targets },
        };
        Ok(Mirror {
            solid,
            plane,
            combine,
        })
    }
}

impl Split {
    /// Its fields; no flip and both sides kept are written as nothing.
    pub fn fields(&self) -> Value {
        let mut v = json!({"target": self.target, "plane": self.plane.to_json()});
        if self.flip {
            v["flip"] = json!(true);
        }
        if self.keep == Keep::Behind {
            v["keep"] = json!("behind");
        }
        v
    }

    #[doc(hidden)]
    pub fn from_fields(o: &mut Obj) -> Result<Split> {
        let target = target(o)?;
        let plane = GeomRef::from_json(&o.take("plane")?, "/plane")?;
        let flip = match o.take_opt("flip") {
            None => false,
            Some(Value::Bool(b)) => b,
            Some(_) => return Err(invalid("/flip", "flip is true or false")),
        };
        let keep = match o.take_opt("keep") {
            None => Keep::Both,
            Some(Value::String(s)) => {
                Keep::parse(&s).ok_or_else(|| invalid("/keep", "keep is both or behind"))?
            }
            Some(_) => return Err(invalid("/keep", "keep is both or behind")),
        };
        Ok(Split {
            target,
            plane,
            flip,
            keep,
        })
    }
}

/// Which way a shell's wall goes from the solid's faces.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Direction {
    /// Into the solid, which keeps its outside (written as nothing).
    #[default]
    Inward,
    /// Out of it, which keeps its inside as the wall's.
    Outward,
}

impl Direction {
    pub fn as_str(self) -> &'static str {
        match self {
            Direction::Inward => "inward",
            Direction::Outward => "outward",
        }
    }

    pub fn parse(s: &str) -> Option<Direction> {
        match s {
            "inward" => Some(Direction::Inward),
            "outward" => Some(Direction::Outward),
            _ => None,
        }
    }
}

/// Faces given a wall or a distance of their own: a shell's group's
/// `thickness`, an offset's group's `distance`.
#[derive(Clone, Debug, PartialEq)]
pub struct FaceGroup {
    pub quantity: Quantity,
    pub faces: Vec<Reference>,
}

/// A shell: `target` hollowed to a wall of `thickness` (each group's faces
/// their own), `open` where it names faces; none makes a closed shell (a
/// cavity, inwards).
#[derive(Clone, Debug, PartialEq)]
pub struct Shell {
    pub target: Id,
    pub thickness: Quantity,
    pub direction: Direction,
    pub open: Vec<Reference>,
    pub faces: Vec<FaceGroup>,
}

/// An offset: every face of `target` moved along its outward normal by
/// `distance` (positive outwards, zero allowed), each group's faces by
/// their own; not every distance zero.
#[derive(Clone, Debug, PartialEq)]
pub struct Offset {
    pub target: Id,
    pub distance: Quantity,
    pub faces: Vec<FaceGroup>,
}

/// The fields a request may give a shell, an offset, a mirror or a split.
pub fn request_fields(ty: &str) -> &'static [&'static str] {
    match ty {
        "mirror" => &["solid", "plane", "operation", "targets"],
        "split" => &["target", "targets", "plane", "flip", "keep"],
        "shell" => &[
            "target",
            "targets",
            "thickness",
            "direction",
            "open",
            "faces",
        ],
        _ => &["target", "targets", "distance", "faces"],
    }
}

impl Shell {
    pub fn fields(&self) -> Value {
        let mut v = json!({"target": self.target, "thickness": self.thickness.text()});
        if self.direction == Direction::Outward {
            v["direction"] = json!("outward");
        }
        if !self.open.is_empty() {
            v["open"] = refs_json(&self.open);
        }
        if !self.faces.is_empty() {
            v["faces"] = groups_json(&self.faces, "thickness");
        }
        v
    }

    #[doc(hidden)]
    pub fn from_fields(o: &mut Obj, default: Unit, env: &Env) -> Result<Shell> {
        let target = target(o)?;
        let thickness = wall(&o.take("thickness")?, default, env, "/thickness")?;
        let direction = match o.take_opt("direction") {
            None => Direction::Inward,
            Some(Value::String(s)) => Direction::parse(&s)
                .ok_or_else(|| invalid("/direction", "direction is inward or outward"))?,
            Some(_) => return Err(invalid("/direction", "direction is inward or outward")),
        };
        let open = match o.take_opt("open") {
            Some(Value::Array(a)) if a.is_empty() => Vec::new(),
            Some(v) => refs(&v, "/open")?,
            None => Vec::new(),
        };
        let faces = groups(o, "thickness", default, env)?;
        let s = Shell {
            target,
            thickness,
            direction,
            open,
            faces,
        };
        once(s.references())?;
        Ok(s)
    }

    /// Its references, each with its role: `open.<i>`, then
    /// `faces.<i>.faces.<j>`.
    pub fn references(&self) -> Vec<(String, &Reference)> {
        let mut out: Vec<(String, &Reference)> = self
            .open
            .iter()
            .enumerate()
            .map(|(i, r)| (format!("open.{i}"), r))
            .collect();
        out.extend(group_roles(&self.faces));
        out
    }
}

impl Offset {
    pub fn fields(&self) -> Value {
        let mut v = json!({"target": self.target, "distance": self.distance.text()});
        if !self.faces.is_empty() {
            v["faces"] = groups_json(&self.faces, "distance");
        }
        v
    }

    #[doc(hidden)]
    pub fn from_fields(o: &mut Obj, default: Unit, env: &Env) -> Result<Offset> {
        let target = target(o)?;
        let distance = length(&o.take("distance")?, default, env, "/distance")?;
        let faces = groups(o, "distance", default, env)?;
        if distance.meters() == 0.0 && faces.iter().all(|g| g.quantity.meters() == 0.0) {
            return Err(Error::new(
                Code::InvalidParam,
                "an offset moves nothing: its distance and every group's are zero; give the solid a distance, or faces one of their own",
            )
            .with("pointer", "/distance"));
        }
        let s = Offset {
            target,
            distance,
            faces,
        };
        once(s.references())?;
        Ok(s)
    }

    /// Its references, each with its role: `faces.<i>.faces.<j>`.
    pub fn references(&self) -> Vec<(String, &Reference)> {
        group_roles(&self.faces).collect()
    }
}

/// The expression strings of a shell's, offset's or thicken's `faces`:
/// each group's thickness or distance, or a thicken's groups' front and
/// back.
pub fn expression_strings(groups: &mut Value) -> Vec<&mut String> {
    let mut out = Vec::new();
    let Value::Array(groups) = groups else {
        return out;
    };
    for g in groups {
        let Value::Object(g) = g else { continue };
        for (k, x) in g.iter_mut() {
            if let ("thickness" | "distance" | "front" | "back", Value::String(s)) = (k.as_str(), x)
            {
                out.push(s);
            }
        }
    }
    out
}

fn target(o: &mut Obj) -> Result<Id> {
    solid_id(o, "target")
}

/// A field naming a solid by its founder.
pub(super) fn solid_id(o: &mut Obj, key: &str) -> Result<Id> {
    let at = format!("/{key}");
    let t = o.take(key)?;
    let t = t
        .as_str()
        .ok_or_else(|| invalid(&at, &format!("{key} is a solid's feature ID")))?;
    Id::parse(t, "feature").map_err(|e| e.with("pointer", at))
}

/// A length, of either sign.
fn length(v: &Value, default: Unit, env: &Env, at: &str) -> Result<Quantity> {
    Quantity::from_json_as(v, Field::Length(default), &|n| env.get(n))
        .map_err(|e| e.with("pointer", at))
}

/// A wall's thickness: a length greater than zero.
fn wall(v: &Value, default: Unit, env: &Env, at: &str) -> Result<Quantity> {
    let q = length(v, default, env, at)?;
    if q.meters() <= 0.0 {
        return Err(Error::new(
            Code::InvalidParam,
            format!("a wall's thickness must be greater than zero, not {q}"),
        )
        .with("pointer", at)
        .with("value", q.text()));
    }
    Ok(q)
}

/// Reads `faces`, an array of groups `{<key>, faces}` (none when left out):
/// a shell's walls greater than zero, an offset's distances of any sign.
fn groups(o: &mut Obj, key: &str, default: Unit, env: &Env) -> Result<Vec<FaceGroup>> {
    let Some(v) = o.take_opt("faces") else {
        return Ok(Vec::new());
    };
    let a = v.as_array().ok_or_else(|| {
        invalid(
            "/faces",
            &format!("faces is an array of groups {{{key}, faces}}"),
        )
    })?;
    a.iter()
        .enumerate()
        .map(|(i, g)| {
            let at = format!("/faces/{i}");
            let mut f = Obj::new(g, &at)?;
            let qat = format!("{at}/{key}");
            let q = f.take(key)?;
            let quantity = if key == "thickness" {
                wall(&q, default, env, &qat)?
            } else {
                length(&q, default, env, &qat)?
            };
            let faces = refs(&f.take("faces")?, &format!("{at}/faces"))?;
            f.finish()?;
            Ok(FaceGroup { quantity, faces })
        })
        .collect()
}

/// A non-empty array of face references.
pub(super) fn refs(v: &Value, at: &str) -> Result<Vec<Reference>> {
    let a = v
        .as_array()
        .filter(|a| !a.is_empty())
        .ok_or_else(|| invalid(at, "a non-empty array of faces {feature, name}"))?;
    a.iter()
        .enumerate()
        .map(|(j, r)| Reference::from_json(r, &format!("{at}/{j}")))
        .collect()
}

pub(super) fn refs_json(refs: &[Reference]) -> Value {
    Value::Array(refs.iter().map(Reference::to_json).collect())
}

fn groups_json(groups: &[FaceGroup], key: &str) -> Value {
    Value::Array(
        groups
            .iter()
            .map(|g| {
                let mut v = json!({"faces": refs_json(&g.faces)});
                v[key] = json!(g.quantity.text());
                v
            })
            .collect(),
    )
}

fn group_roles(groups: &[FaceGroup]) -> impl Iterator<Item = (String, &Reference)> {
    groups.iter().enumerate().flat_map(|(i, g)| {
        g.faces
            .iter()
            .enumerate()
            .map(move |(j, r)| (format!("faces.{i}.faces.{j}"), r))
    })
}

/// Refuses a face named twice: open and given a wall, in two groups, or
/// twice in one list. The pointer is the second naming's.
fn once(refs: Vec<(String, &Reference)>) -> Result<()> {
    for (k, (role, r)) in refs.iter().enumerate() {
        if let Some((first, _)) = refs[..k].iter().find(|(_, x)| x == r) {
            return Err(Error::new(
                Code::InvalidParam,
                format!(
                    "face {}/{} is named twice, as {first} and {role}: a face is open or in one group, once",
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

    const A: &str = "0123456789abcdef0123456789abcdef";
    const B: &str = "fedcba9876543210fedcba9876543210";

    fn face(n: &str) -> Value {
        json!({"feature": "base", "name": n})
    }

    fn shell(v: Value) -> Result<Shell> {
        let mut o = Obj::new(&v, "")?;
        let s = Shell::from_fields(&mut o, Unit::Mm, &Env::empty())?;
        o.finish()?;
        Ok(s)
    }

    fn offset(v: Value) -> Result<Offset> {
        let mut o = Obj::new(&v, "")?;
        let s = Offset::from_fields(&mut o, Unit::Mm, &Env::empty())?;
        o.finish()?;
        Ok(s)
    }

    #[test]
    fn shells_round_trip() {
        let v = json!({"target": "base", "thickness": 2, "open": [face(A)],
                       "faces": [{"thickness": "4 mm", "faces": [face(B)]}]});
        let s = shell(v).unwrap();
        assert_eq!(s.direction, Direction::Inward);
        assert_eq!(s.thickness.meters(), 0.002);
        // Bare numbers take the default unit, written out.
        assert_eq!(
            s.fields(),
            json!({"target": "base", "thickness": "2 mm", "open": [face(A)],
                   "faces": [{"thickness": "4 mm", "faces": [face(B)]}]})
        );
        assert_eq!(shell(s.fields()).unwrap(), s);
        let roles: Vec<String> = s.references().into_iter().map(|(r, _)| r).collect();
        assert_eq!(roles, ["open.0", "faces.0.faces.0"]);
        // Outward is written; inward, no open faces and no groups aren't.
        let s = shell(
            json!({"target": "base", "thickness": "1 mm", "direction": "outward",
                             "open": []}),
        )
        .unwrap();
        assert_eq!(
            s.fields(),
            json!({"target": "base", "thickness": "1 mm", "direction": "outward"})
        );
        let s =
            shell(json!({"target": "base", "thickness": "1 mm", "direction": "inward"})).unwrap();
        assert_eq!(s.fields(), json!({"target": "base", "thickness": "1 mm"}));
    }

    #[test]
    fn offsets_round_trip() {
        let s = offset(json!({"target": "base", "distance": "1 mm"})).unwrap();
        assert!(s.faces.is_empty());
        assert_eq!(offset(s.fields()).unwrap(), s);
        // Zero for every face but a group's, whose distance may be negative.
        let v = json!({"target": "base", "distance": 0,
                       "faces": [{"distance": "-3 mm", "faces": [face(A), face(B)]}]});
        let s = offset(v).unwrap();
        assert_eq!(
            s.fields(),
            json!({"target": "base", "distance": "0 mm",
                   "faces": [{"distance": "-3 mm", "faces": [face(A), face(B)]}]})
        );
        let roles: Vec<String> = s.references().into_iter().map(|(r, _)| r).collect();
        assert_eq!(roles, ["faces.0.faces.0", "faces.0.faces.1"]);
    }

    #[test]
    fn shells_and_offsets_are_checked() {
        for (v, code, pointer) in [
            (json!({"thickness": 1}), Code::InvalidDocument, "/target"),
            (
                json!({"target": "Base", "thickness": 1}),
                Code::InvalidId,
                "/target",
            ),
            (
                json!({"target": "base"}),
                Code::InvalidDocument,
                "/thickness",
            ),
            (
                json!({"target": "base", "thickness": 0}),
                Code::InvalidParam,
                "/thickness",
            ),
            (
                json!({"target": "base", "thickness": "1 deg"}),
                Code::UnitMismatch,
                "/thickness",
            ),
            (
                json!({"target": "base", "thickness": 1, "direction": "up"}),
                Code::InvalidDocument,
                "/direction",
            ),
            (
                json!({"target": "base", "thickness": 1, "open": [face("x")]}),
                Code::InvalidDocument,
                "/open/0/name",
            ),
            (
                json!({"target": "base", "thickness": 1, "faces": [{"thickness": 2, "faces": []}]}),
                Code::InvalidDocument,
                "/faces/0/faces",
            ),
            (
                json!({"target": "base", "thickness": 1,
                       "faces": [{"thickness": -2, "faces": [face(A)]}]}),
                Code::InvalidParam,
                "/faces/0/thickness",
            ),
            (
                json!({"target": "base", "thickness": 1,
                       "faces": [{"distance": 2, "faces": [face(A)]}]}),
                Code::InvalidDocument,
                "/faces/0/thickness",
            ),
            (
                json!({"target": "base", "thickness": 1, "open": [face(A)],
                       "faces": [{"thickness": 2, "faces": [face(A)]}]}),
                Code::InvalidParam,
                "/faces/0/faces/0",
            ),
            (
                json!({"target": "base", "thickness": 1, "open": [face(A), face(A)]}),
                Code::InvalidParam,
                "/open/1",
            ),
        ] {
            let e = shell(v.clone()).unwrap_err();
            assert_eq!(
                (e.code, e.details["pointer"].as_str()),
                (code, Some(pointer)),
                "{v}"
            );
        }
        for (v, code, pointer) in [
            (
                json!({"target": "base", "distance": 0}),
                Code::InvalidParam,
                "/distance",
            ),
            (
                json!({"target": "base", "distance": 0,
                       "faces": [{"distance": 0, "faces": [face(A)]}]}),
                Code::InvalidParam,
                "/distance",
            ),
            (
                json!({"target": "base", "distance": 1,
                       "faces": [{"distance": 2, "faces": [face(A)]},
                                 {"distance": 3, "faces": [face(A)]}]}),
                Code::InvalidParam,
                "/faces/1/faces/0",
            ),
            (
                json!({"target": "base", "distance": 1, "open": [face(A)]}),
                Code::InvalidDocument,
                "/open",
            ),
        ] {
            let e = offset(v.clone()).unwrap_err();
            assert_eq!(
                (e.code, e.details["pointer"].as_str()),
                (code, Some(pointer)),
                "{v}"
            );
        }
    }

    #[test]
    fn sizes_read_parameters() {
        let p = [("t".to_owned(), crate::expr::Expr::parse("1.5 mm").unwrap())].into();
        let v = json!({"target": "base", "thickness": "2*t",
                       "faces": [{"thickness": "t + 1 mm", "faces": [face(A)]}]});
        let mut o = Obj::new(&v, "").unwrap();
        let s = Shell::from_fields(&mut o, Unit::Mm, &Env::new(&p)).unwrap();
        assert_eq!(s.thickness.meters(), 0.003);
        assert_eq!(s.thickness.text(), "2 * t");
        let mut v = s.fields();
        let strings: Vec<String> = expression_strings(&mut v["faces"])
            .into_iter()
            .map(|s| s.clone())
            .collect();
        assert_eq!(strings, ["t + 1 mm"]);
    }

    fn mirror(v: Value) -> Result<Mirror> {
        let mut o = Obj::new(&v, "")?;
        let m = Mirror::from_fields(&mut o)?;
        o.finish()?;
        Ok(m)
    }

    fn split(v: Value) -> Result<Split> {
        let mut o = Obj::new(&v, "")?;
        let s = Split::from_fields(&mut o)?;
        o.finish()?;
        Ok(s)
    }

    #[test]
    fn mirrors_round_trip_and_always_say_what_the_copy_does() {
        let v = json!({"solid": "half", "plane": "yz", "operation": "join", "targets": ["half"]});
        let m = mirror(v.clone()).unwrap();
        assert_eq!(m.combine.operation, crate::doc::solid::Operation::Join);
        assert_eq!(m.fields(), v);
        // A new copy is written as new, never as nothing.
        let v = json!({"solid": "half", "plane": {"feature": "mid"}, "operation": "new"});
        let m = mirror(v.clone()).unwrap();
        assert!(m.combine.is_new());
        assert_eq!(m.fields(), v);
        let face = json!({"feature": "half", "name": A});
        let m = mirror(json!({"solid": "half", "plane": face, "operation": "cut",
                              "targets": ["base"]}))
        .unwrap();
        assert_eq!(mirror(m.fields()).unwrap(), m);
    }

    #[test]
    fn splits_round_trip() {
        let s = split(json!({"target": "housing", "plane": {"feature": "parting"}})).unwrap();
        assert_eq!((s.flip, s.keep), (false, Keep::Both));
        // Both sides kept and no flip are written as nothing.
        let v = json!({"target": "housing", "plane": {"feature": "parting"},
                       "flip": false, "keep": "both"});
        assert_eq!(
            split(v).unwrap().fields(),
            json!({"target": "housing", "plane": {"feature": "parting"}})
        );
        let v = json!({"target": "housing", "plane": "xy", "flip": true, "keep": "behind"});
        let s = split(v.clone()).unwrap();
        assert_eq!(s.fields(), v);
        assert_eq!(split(s.fields()).unwrap(), s);
    }

    #[test]
    fn mirrors_and_splits_are_checked() {
        for (v, code, pointer) in [
            (
                json!({"plane": "yz", "operation": "new"}),
                Code::InvalidDocument,
                "/solid",
            ),
            (
                json!({"solid": "half", "operation": "new"}),
                Code::InvalidDocument,
                "/plane",
            ),
            (
                json!({"solid": "half", "plane": "yz"}),
                Code::InvalidDocument,
                "/operation",
            ),
            (
                json!({"solid": "half", "plane": "up", "operation": "new"}),
                Code::InvalidDocument,
                "/plane",
            ),
            (
                json!({"solid": "half", "plane": "yz", "operation": "join"}),
                Code::InvalidDocument,
                "/targets",
            ),
            (
                json!({"solid": "half", "plane": "yz", "operation": "new", "targets": ["a"]}),
                Code::InvalidDocument,
                "/targets",
            ),
            (
                json!({"solid": "Half", "plane": "yz", "operation": "new"}),
                Code::InvalidId,
                "/solid",
            ),
        ] {
            let e = mirror(v.clone()).unwrap_err();
            assert_eq!(
                (e.code, e.details["pointer"].as_str()),
                (code, Some(pointer)),
                "{v}"
            );
        }
        for (v, code, pointer) in [
            (json!({"plane": "xy"}), Code::InvalidDocument, "/target"),
            (json!({"target": "b"}), Code::InvalidDocument, "/plane"),
            (
                json!({"target": "b", "plane": "xy", "flip": 1}),
                Code::InvalidDocument,
                "/flip",
            ),
            (
                json!({"target": "b", "plane": "xy", "keep": "front"}),
                Code::InvalidDocument,
                "/keep",
            ),
        ] {
            let e = split(v.clone()).unwrap_err();
            assert_eq!(
                (e.code, e.details["pointer"].as_str()),
                (code, Some(pointer)),
                "{v}"
            );
        }
    }
}
