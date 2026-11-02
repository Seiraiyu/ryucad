//! Body features: a solid hollowed to a wall (`shell`), and a solid grown,
//! shrunk, or its faces pushed and pulled (`offset`). Each acts on one
//! solid, its `target`, named by its founder; the faces it names are
//! references into that solid as the feature sees it. Plan 15.B adds
//! mirrors and splits here.

use super::files::{Obj, invalid};
use super::id::Id;
use super::reference::Reference;
use crate::error::{Code, Error, Result};
use crate::expr::Env;
use crate::units::{Field, Quantity, Unit};
use serde_json::{Value, json};

/// The feature format shells and offsets are new in.
pub const FORMAT: u64 = 9;

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

/// The fields a request may give a shell or an offset.
pub fn request_fields(ty: &str) -> &'static [&'static str] {
    match ty {
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

/// The expression strings of a shell's or offset's `faces`: each group's
/// thickness or distance.
pub fn expression_strings(groups: &mut Value) -> Vec<&mut String> {
    let mut out = Vec::new();
    let Value::Array(groups) = groups else {
        return out;
    };
    for g in groups {
        let Value::Object(g) = g else { continue };
        for (k, x) in g.iter_mut() {
            if let ("thickness" | "distance", Value::String(s)) = (k.as_str(), x) {
                out.push(s);
            }
        }
    }
    out
}

fn target(o: &mut Obj) -> Result<Id> {
    let t = o.take("target")?;
    let t = t
        .as_str()
        .ok_or_else(|| invalid("/target", "target is a solid's feature ID"))?;
    Id::parse(t, "feature").map_err(|e| e.with("pointer", "/target"))
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
}
