//! Direct edits (design 445, ryucad 22): a face tweak's `topology` and
//! `blends` ([`Retrim`]), and `delete_face`: faces of one solid, its
//! `target`, taken out and each gap they leave healed, by a cap or by its
//! neighbours growing. Faces are references into the target as the
//! feature sees it; a seed is a face of a hole, boss or pocket, which is
//! deleted whole, its faces found from the seed on every rebuild.

use super::body::{refs, refs_json, solid_id};
use super::files::{Obj, invalid};
use super::id::Id;
use super::reference::Reference;
use crate::error::{Code, Error, Result};
use serde_json::{Value, json};

/// The feature format direct edits are new in: `delete_face`, and a face
/// tweak with `topology: change` or `blends: rebuild`. Taken in the order
/// stages merge (19 ryucad 18.C's splits by a tool): if another stage
/// holds 20 when this merges, move this constant to the next free number;
/// nothing else names it.
pub const FORMAT: u64 = 20;

/// The feature types this module reads.
pub const TYPES: [&str; 1] = ["delete_face"];

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
}
