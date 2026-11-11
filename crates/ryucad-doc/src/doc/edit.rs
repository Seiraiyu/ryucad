//! Direct edits (design 445, ryucad 22): a face tweak's `topology` and
//! `blends` ([`Retrim`]); `delete_face`: faces of one solid, its
//! `target`, taken out and each gap they leave healed, by a cap or by its
//! neighbours growing; `delete_blend`: blends deleted by size or by face,
//! their supports grown back; and `resize_blend`: blends given a new
//! size, keeping their names. Faces are references into the target as the
//! feature sees it; a seed is a face of a hole, boss or pocket, which is
//! deleted whole, its faces found from the seed on every rebuild.

use super::blend::{ChamferForm, chamfer_form, chamfer_form_json, size};
use super::body::{refs, refs_json, solid_id};
use super::files::{Obj, invalid};
use super::id::Id;
use super::reference::Reference;
use crate::error::{Code, Error, Result};
use crate::expr::Env;
use crate::units::{Field, Quantity, Unit};
use serde_json::{Value, json};

/// The feature format direct edits are new in: `delete_face`, and a face
/// tweak with `topology: change` or `blends: rebuild`. Taken in the order
/// stages merge (19 ryucad 18.C's splits by a tool): if another stage
/// holds 20 when this merges, move this constant to the next free number;
/// nothing else names it.
pub const FORMAT: u64 = 20;

/// The feature types this module reads.
pub const TYPES: [&str; 3] = ["delete_face", "delete_blend", "resize_blend"];

/// The fields a request may give a `delete_blend`.
pub const DELETE_BLEND_REQUEST: &[&str] = &["target", "targets", "faces", "below", "tolerance"];

/// The fields a request may give a `resize_blend`: its groups whole, or
/// its one group's fields (a fillet's `radius`, or a chamfer's `form`,
/// `d`, `d1`, `d2`, `angle` and `chamfer_face`, with the blends' `faces`).
pub const RESIZE_REQUEST: &[&str] = &[
    "target",
    "targets",
    "faces",
    "groups",
    "radius",
    "form",
    "d",
    "d1",
    "d2",
    "angle",
    "chamfer_face",
    "tolerance",
];

/// The fields a request may give a `delete_face`.
pub const DELETE_REQUEST: &[&str] = &["target", "targets", "faces", "seeds", "heal"];

/// Whether a face tweak may change the solid's topology (`topology:
/// change`: faces grow over neighbours, shrink away or split, edges and
/// vertices come and go) and make again the blends it breaks (`blends:
/// rebuild`). Each is written only when set; left out (or `keep`) it is
/// ryucad 17's tweak, bit for bit.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Retrim {
    pub topology: bool,
    pub blends: bool,
}

impl Retrim {
    /// Neither: ryucad 17's tweak.
    pub fn is_keep(&self) -> bool {
        !self.topology && !self.blends
    }

    #[doc(hidden)]
    pub fn take(o: &mut Obj) -> Result<Retrim> {
        Ok(Retrim {
            topology: word(o, "topology", "change")?,
            blends: word(o, "blends", "rebuild")?,
        })
    }

    /// Writes what is set into a tweak's fields.
    pub fn write(&self, v: &mut Value) {
        if self.topology {
            v["topology"] = json!("change");
        }
        if self.blends {
            v["blends"] = json!("rebuild");
        }
    }
}

/// `key` read as `keep` (or left out) or `other`: whether it is `other`.
fn word(o: &mut Obj, key: &str, other: &str) -> Result<bool> {
    match o.take_opt(key) {
        None => Ok(false),
        Some(Value::String(s)) if s == "keep" => Ok(false),
        Some(Value::String(s)) if s == other => Ok(true),
        Some(_) => Err(invalid(
            &format!("/{key}"),
            &format!("{key} is keep or {other}"),
        )),
    }
}

/// How a `delete_face` heals each gap: a cap where one closes it, else its
/// neighbours grow (`auto`); only a cap; only growth.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Heal {
    #[default]
    Auto,
    Cap,
    Grow,
}

impl Heal {
    pub fn as_str(&self) -> &'static str {
        match self {
            Heal::Auto => "auto",
            Heal::Cap => "cap",
            Heal::Grow => "grow",
        }
    }

    fn parse(s: &str) -> Option<Heal> {
        match s {
            "auto" => Some(Heal::Auto),
            "cap" => Some(Heal::Cap),
            "grow" => Some(Heal::Grow),
            _ => None,
        }
    }
}

const HEAL: &str = "heal is auto, cap or grow";

/// A `delete_face`: `faces` of `target` deleted, and each hole, boss or
/// pocket a face in `seeds` belongs to deleted whole, each gap healed as
/// `heal` says.
#[derive(Clone, Debug, PartialEq)]
pub struct DeleteFace {
    pub target: Id,
    pub faces: Vec<Reference>,
    pub seeds: Vec<Reference>,
    pub heal: Heal,
}

impl DeleteFace {
    /// Its fields: no `faces` or `seeds` when there are none, `heal` only
    /// when not `auto`.
    pub fn fields(&self) -> Value {
        let mut v = json!({"target": self.target});
        if !self.faces.is_empty() {
            v["faces"] = refs_json(&self.faces);
        }
        if !self.seeds.is_empty() {
            v["seeds"] = refs_json(&self.seeds);
        }
        if self.heal != Heal::Auto {
            v["heal"] = json!(self.heal.as_str());
        }
        v
    }

    #[doc(hidden)]
    pub fn from_fields(o: &mut Obj) -> Result<DeleteFace> {
        let target = solid_id(o, "target")?;
        let list = |o: &mut Obj, k: &str| -> Result<Vec<Reference>> {
            match o.take_opt(k) {
                None => Ok(Vec::new()),
                Some(Value::Array(a)) if a.is_empty() => Ok(Vec::new()),
                Some(v) => refs(&v, &format!("/{k}")),
            }
        };
        let faces = list(o, "faces")?;
        let seeds = list(o, "seeds")?;
        let heal = match o.take_opt("heal") {
            None => Heal::Auto,
            Some(Value::String(s)) => Heal::parse(&s).ok_or_else(|| invalid("/heal", HEAL))?,
            Some(_) => return Err(invalid("/heal", HEAL)),
        };
        if faces.is_empty() && seeds.is_empty() {
            return Err(invalid(
                "/faces",
                "a delete_face names the faces it deletes (faces), or a face of each hole, boss or pocket it deletes whole (seeds)",
            ));
        }
        twice(&faces, "faces")?;
        twice(&seeds, "seeds")?;
        Ok(DeleteFace {
            target,
            faces,
            seeds,
            heal,
        })
    }

    /// Its references, each with its role: `faces.<i>`, then `seeds.<i>`.
    pub fn references(&self) -> Vec<(String, &Reference)> {
        let faces = self
            .faces
            .iter()
            .enumerate()
            .map(|(i, r)| (format!("faces.{i}"), r));
        let seeds = self
            .seeds
            .iter()
            .enumerate()
            .map(|(i, r)| (format!("seeds.{i}"), r));
        faces.chain(seeds).collect()
    }
}

/// A blend edit's recognition tolerance, where written: a length from
/// `LINEAR_RESOLUTION` to `TOLERANCE_CAP` (as a fill's or a sew's).
fn tolerance(o: &mut Obj, default: Unit, env: &Env) -> Result<Option<Quantity>> {
    let Some(v) = o.take_opt("tolerance") else {
        return Ok(None);
    };
    let q = Quantity::from_json_as(&v, Field::Length(default), &|n| env.get(n))
        .map_err(|e| e.with("pointer", "/tolerance"))?;
    let (lo, hi) = super::surface::TOLERANCE_RANGE;
    if !(lo..=hi).contains(&q.meters()) {
        return Err(Error::new(
            Code::InvalidParam,
            format!("a blend edit's tolerance is from 0.00001 mm to 0.1 mm, not {q}"),
        )
        .with("pointer", "/tolerance")
        .with("value", q.text()));
    }
    Ok(Some(q))
}

/// A `delete_blend`: every blend of `target` at most `below` (a fillet's
/// radius, a chamfer's larger distance) and every blend holding one of
/// `faces` deleted, closed over their chains and the corners they reach,
/// their supports grown until they meet. `tolerance` is recognition's
/// (the kernel's 10 µm when not written).
#[derive(Clone, Debug, PartialEq)]
pub struct DeleteBlend {
    pub target: Id,
    pub below: Option<Quantity>,
    pub faces: Vec<Reference>,
    pub tolerance: Option<Quantity>,
}

impl DeleteBlend {
    /// Its fields: each written only when given.
    pub fn fields(&self) -> Value {
        let mut v = json!({"target": self.target});
        if let Some(b) = &self.below {
            v["below"] = json!(b.text());
        }
        if !self.faces.is_empty() {
            v["faces"] = refs_json(&self.faces);
        }
        if let Some(t) = &self.tolerance {
            v["tolerance"] = json!(t.text());
        }
        v
    }

    #[doc(hidden)]
    pub fn from_fields(o: &mut Obj, default: Unit, env: &Env) -> Result<DeleteBlend> {
        let target = solid_id(o, "target")?;
        let below = match o.take_opt("below") {
            None => None,
            Some(v) => {
                let q = Quantity::from_json_as(&v, Field::Length(default), &|n| env.get(n))
                    .map_err(|e| e.with("pointer", "/below"))?;
                if q.meters() < 0.0 {
                    return Err(Error::new(
                        Code::InvalidParam,
                        format!("below is a length of at least zero, not {q}"),
                    )
                    .with("pointer", "/below")
                    .with("value", q.text()));
                }
                Some(q)
            }
        };
        let faces = match o.take_opt("faces") {
            None => Vec::new(),
            Some(Value::Array(a)) if a.is_empty() => Vec::new(),
            Some(v) => refs(&v, "/faces")?,
        };
        if below.is_none() && faces.is_empty() {
            return Err(invalid(
                "/faces",
                "a delete_blend names the blends it deletes: every one at most a size (below), or those holding faces (faces)",
            ));
        }
        twice(&faces, "faces")?;
        let tolerance = tolerance(o, default, env)?;
        Ok(DeleteBlend {
            target,
            below,
            faces,
            tolerance,
        })
    }

    /// Its references, each with its role: `faces.<i>`.
    pub fn references(&self) -> Vec<(String, &Reference)> {
        self.faces
            .iter()
            .enumerate()
            .map(|(i, r)| (format!("faces.{i}"), r))
            .collect()
    }
}

/// The size a `resize_blend` group gives its blends: a fillet's radius, or
/// a chamfer's form and sizes (as `doc::blend` writes them).
#[derive(Clone, Debug, PartialEq)]
pub enum BlendSize {
    Fillet { radius: Quantity },
    Chamfer(ChamferForm),
}

/// Blends, each named by one of its faces, given one size.
#[derive(Clone, Debug, PartialEq)]
pub struct ResizeGroup {
    pub size: BlendSize,
    pub faces: Vec<Reference>,
}

/// A `resize_blend`: each group's blends (and their chains and corners)
/// made again at the group's size, keeping their names where the new
/// blend has one in the same place. `tolerance` is recognition's.
#[derive(Clone, Debug, PartialEq)]
pub struct ResizeBlend {
    pub target: Id,
    pub groups: Vec<ResizeGroup>,
    pub tolerance: Option<Quantity>,
}

impl ResizeBlend {
    pub fn fields(&self) -> Value {
        let groups: Vec<Value> = self
            .groups
            .iter()
            .map(|g| {
                let mut v = json!({"faces": refs_json(&g.faces)});
                match &g.size {
                    BlendSize::Fillet { radius } => v["radius"] = json!(radius.text()),
                    BlendSize::Chamfer(form) => chamfer_form_json(form, &mut v),
                }
                v
            })
            .collect();
        let mut v = json!({"target": self.target, "groups": groups});
        if let Some(t) = &self.tolerance {
            v["tolerance"] = json!(t.text());
        }
        v
    }

    #[doc(hidden)]
    pub fn from_fields(o: &mut Obj, default: Unit, env: &Env) -> Result<ResizeBlend> {
        let target = solid_id(o, "target")?;
        let v = o.take("groups")?;
        let a = v.as_array().filter(|a| !a.is_empty()).ok_or_else(|| {
            invalid(
                "/groups",
                "groups is a non-empty array, each a size and the blends' faces",
            )
        })?;
        let mut groups = Vec::with_capacity(a.len());
        for (i, g) in a.iter().enumerate() {
            let at = format!("/groups/{i}");
            let mut g = Obj::new(g, &at)?;
            let size = match (g.take_opt("radius"), g.take_opt("form")) {
                (Some(r), None) => BlendSize::Fillet {
                    radius: size(&r, Field::Length(default), env, &at, "radius")?,
                },
                (None, Some(f)) => BlendSize::Chamfer(chamfer_form(&f, &mut g, &at, default, env)?),
                (Some(_), Some(_)) => {
                    return Err(invalid(
                        &format!("{at}/form"),
                        "a resize's group gives a fillet's radius or a chamfer's form, not both",
                    ));
                }
                (None, None) => {
                    return Err(invalid(
                        &at,
                        "a resize's group gives a fillet's radius, or a chamfer's form and sizes",
                    ));
                }
            };
            let faces = refs(&g.take("faces")?, &format!("{at}/faces"))?;
            g.finish()?;
            groups.push(ResizeGroup { size, faces });
        }
        // A face in two groups, or twice in one: the second's pointer.
        let mut seen: Vec<(&Reference, String)> = Vec::new();
        for (i, g) in groups.iter().enumerate() {
            for (j, r) in g.faces.iter().enumerate() {
                let here = format!("/groups/{i}/faces/{j}");
                if let Some((_, first)) = seen.iter().find(|(x, _)| *x == r) {
                    return Err(Error::new(
                        Code::InvalidParam,
                        format!(
                            "face {}/{} is in the resize twice: a blend takes one size",
                            r.feature, r.name
                        ),
                    )
                    .with("pointer", here)
                    .with("first", first.clone()));
                }
                seen.push((r, here));
            }
        }
        let tolerance = tolerance(o, default, env)?;
        Ok(ResizeBlend {
            target,
            groups,
            tolerance,
        })
    }

    /// Its references, each with its role: `groups.<i>.faces.<j>`, and a
    /// chamfer group's `groups.<i>.face`.
    pub fn references(&self) -> Vec<(String, &Reference)> {
        let mut out = Vec::new();
        for (i, g) in self.groups.iter().enumerate() {
            for (j, r) in g.faces.iter().enumerate() {
                out.push((format!("groups.{i}.faces.{j}"), r));
            }
            if let BlendSize::Chamfer(form) = &g.size
                && let Some(f) = form.face()
            {
                out.push((format!("groups.{i}.face"), f));
            }
        }
        out
    }
}

/// Refuses a face listed twice in `key`, the pointer the second's.
fn twice(list: &[Reference], key: &str) -> Result<()> {
    for (k, r) in list.iter().enumerate() {
        if let Some(i) = list[..k].iter().position(|x| x == r) {
            return Err(Error::new(
                Code::InvalidParam,
                format!("face {}/{} is in {key} twice", r.feature, r.name),
            )
            .with("pointer", format!("/{key}/{k}"))
            .with("first", format!("/{key}/{i}")));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::expr::Env;
    use crate::units::Unit;

    fn blend_delete(v: Value) -> Result<DeleteBlend> {
        let mut o = Obj::new(&v, "")?;
        let d = DeleteBlend::from_fields(&mut o, Unit::Mm, &Env::empty())?;
        o.finish()?;
        Ok(d)
    }

    fn resize(v: Value) -> Result<ResizeBlend> {
        let mut o = Obj::new(&v, "")?;
        let r = ResizeBlend::from_fields(&mut o, Unit::Mm, &Env::empty())?;
        o.finish()?;
        Ok(r)
    }

    fn read(v: Value) -> Result<DeleteFace> {
        let mut o = Obj::new(&v, "")?;
        let d = DeleteFace::from_fields(&mut o)?;
        o.finish()?;
        Ok(d)
    }

    fn face(n: u128) -> Value {
        json!({"feature": "b", "name": format!("{n:032x}")})
    }

    #[test]
    fn a_delete_reads_and_writes_its_fields() {
        let v = json!({"target": "b", "faces": [face(1)], "seeds": [face(2)], "heal": "cap"});
        let d = read(v.clone()).unwrap();
        assert_eq!(d.heal, Heal::Cap);
        assert_eq!(d.fields(), v);
        // `auto` is the default and isn't written; seeds alone are enough.
        let d = read(json!({"target": "b", "seeds": [face(2)], "heal": "auto"})).unwrap();
        assert_eq!(d.fields(), json!({"target": "b", "seeds": [face(2)]}));
        let roles: Vec<String> = d.references().into_iter().map(|(k, _)| k).collect();
        assert_eq!(roles, ["seeds.0"]);
        let d =
            read(json!({"target": "b", "faces": [face(1), face(3)], "seeds": [face(2)]})).unwrap();
        let roles: Vec<String> = d.references().into_iter().map(|(k, _)| k).collect();
        assert_eq!(roles, ["faces.0", "faces.1", "seeds.0"]);
    }

    #[test]
    fn a_delete_refuses_what_it_cant_read() {
        let at = |v: Value| read(v).unwrap_err().details["pointer"].clone();
        assert_eq!(at(json!({"target": "b"})), "/faces");
        assert_eq!(
            at(json!({"target": "b", "faces": [], "seeds": []})),
            "/faces"
        );
        assert_eq!(
            at(json!({"target": "b", "faces": [face(1)], "heal": "patch"})),
            "/heal"
        );
        assert_eq!(
            at(json!({"target": "b", "faces": [face(1)], "heal": 1})),
            "/heal"
        );
        assert_eq!(
            at(json!({"target": "b", "faces": [face(1), face(1)]})),
            "/faces/1"
        );
        assert_eq!(
            at(json!({"target": "b", "seeds": [face(3), face(3)]})),
            "/seeds/1"
        );
        // A field it doesn't take.
        assert!(read(json!({"target": "b", "faces": [face(1)], "tangent": false})).is_err());
    }

    #[test]
    fn retrim_reads_keep_and_writes_only_what_changes() {
        let take = |v: Value| {
            let mut o = Obj::new(&v, "").unwrap();
            Retrim::take(&mut o)
        };
        assert_eq!(take(json!({})).unwrap(), Retrim::default());
        assert!(Retrim::default().is_keep());
        assert_eq!(
            take(json!({"topology": "keep", "blends": "keep"})).unwrap(),
            Retrim::default()
        );
        let r = take(json!({"topology": "change", "blends": "rebuild"})).unwrap();
        assert!(r.topology && r.blends && !r.is_keep());
        let mut v = json!({});
        r.write(&mut v);
        assert_eq!(v, json!({"topology": "change", "blends": "rebuild"}));
        let mut v = json!({});
        Retrim::default().write(&mut v);
        assert_eq!(v, json!({}));
        assert_eq!(
            take(json!({"topology": "rebuild"})).unwrap_err().details["pointer"],
            "/topology"
        );
        assert_eq!(
            take(json!({"blends": true})).unwrap_err().details["pointer"],
            "/blends"
        );
    }

    #[test]
    fn a_blend_delete_reads_and_writes_its_fields() {
        let v =
            json!({"target": "b", "below": "2 mm", "faces": [face(1)], "tolerance": "0.001 mm"});
        let d = blend_delete(v.clone()).unwrap();
        assert_eq!(d.below.as_ref().unwrap().meters(), 0.002);
        assert_eq!(d.tolerance.as_ref().unwrap().meters(), 1e-6);
        assert_eq!(d.fields(), v);
        let roles: Vec<String> = d.references().into_iter().map(|(k, _)| k).collect();
        assert_eq!(roles, ["faces.0"]);
        // `below` alone is enough; nothing else is written.
        let d = blend_delete(json!({"target": "b", "below": "2 mm"})).unwrap();
        assert_eq!(d.fields(), json!({"target": "b", "below": "2 mm"}));
        assert!(d.references().is_empty());
    }

    #[test]
    fn a_blend_delete_refuses_what_it_cant_read() {
        let at = |v: Value| blend_delete(v).unwrap_err().details["pointer"].clone();
        assert_eq!(at(json!({"target": "b"})), "/faces");
        assert_eq!(at(json!({"target": "b", "faces": []})), "/faces");
        assert_eq!(at(json!({"target": "b", "below": "-1 mm"})), "/below");
        assert_eq!(
            at(json!({"target": "b", "below": "1 mm", "tolerance": "1 mm"})),
            "/tolerance"
        );
        assert_eq!(
            at(json!({"target": "b", "below": "1 mm", "tolerance": "0 mm"})),
            "/tolerance"
        );
        assert_eq!(
            at(json!({"target": "b", "faces": [face(1), face(1)]})),
            "/faces/1"
        );
        assert!(blend_delete(json!({"target": "b", "below": "1 mm", "heal": "cap"})).is_err());
    }

    #[test]
    fn a_resize_reads_and_writes_its_groups() {
        let v = json!({"target": "b", "groups": [
            {"radius": "3 mm", "faces": [face(1)]},
            {"form": "distances", "d1": "1 mm", "d2": "2 mm", "face": face(9), "faces": [face(2)]},
            {"form": "equal", "d": "1 mm", "faces": [face(3), face(4)]}],
            "tolerance": "0.001 mm"});
        let r = resize(v.clone()).unwrap();
        assert!(matches!(r.groups[0].size, BlendSize::Fillet { .. }));
        assert!(matches!(
            r.groups[1].size,
            BlendSize::Chamfer(ChamferForm::Distances { .. })
        ));
        assert_eq!(r.fields(), v);
        let roles: Vec<String> = r.references().into_iter().map(|(k, _)| k).collect();
        assert_eq!(
            roles,
            [
                "groups.0.faces.0",
                "groups.1.faces.0",
                "groups.1.face",
                "groups.2.faces.0",
                "groups.2.faces.1"
            ]
        );
    }

    #[test]
    fn a_resize_refuses_what_it_cant_read() {
        let at = |v: Value| resize(v).unwrap_err().details["pointer"].clone();
        assert_eq!(at(json!({"target": "b", "groups": []})), "/groups");
        assert_eq!(
            at(json!({"target": "b", "groups": [{"faces": [face(1)]}]})),
            "/groups/0"
        );
        assert_eq!(
            at(
                json!({"target": "b", "groups": [{"radius": "1 mm", "form": "equal", "d": "1 mm", "faces": [face(1)]}]})
            ),
            "/groups/0/form"
        );
        assert_eq!(
            at(json!({"target": "b", "groups": [{"radius": "0 mm", "faces": [face(1)]}]})),
            "/groups/0/radius"
        );
        assert_eq!(
            at(json!({"target": "b", "groups": [{"radius": "1 mm", "faces": []}]})),
            "/groups/0/faces"
        );
        // A blend's face in two groups, or twice in one.
        assert_eq!(
            at(
                json!({"target": "b", "groups": [{"radius": "1 mm", "faces": [face(1)]},
                                                  {"radius": "2 mm", "faces": [face(1)]}]})
            ),
            "/groups/1/faces/0"
        );
        assert_eq!(
            at(json!({"target": "b", "groups": [{"radius": "1 mm", "faces": [face(1), face(1)]}]})),
            "/groups/0/faces/1"
        );
    }
}
