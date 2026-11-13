//! Cosmetic threads (ryucad 28.B): a thread on cylindrical faces as data,
//! internal or external by its class (6H or 6g, 2B or 2A), never kernel
//! geometry. It founds and acts on no solid; rebuild makes it
//! `Output::Thread`.

use super::files::{Obj, invalid};
use super::hole::Thread;
use super::reference::Reference;
use crate::error::{Code, Error, Result};
use crate::expr::Env;
use crate::units::{Field, Quantity, Unit};
use serde_json::{Value, json};

/// The feature format a cosmetic thread is written at: ryucad 28.B's,
/// the same number as `hole::GEOMETRY_FORMAT`.
pub const FORMAT: u64 = super::hole::GEOMETRY_FORMAT;

#[derive(Clone, Debug, PartialEq)]
pub struct CosmeticThread {
    /// Cylindrical faces of one body, coaxial, of one radius.
    pub faces: Vec<Reference>,
    /// As a hole's, with no depth and no tap drill.
    pub thread: Thread,
    /// An edge at the end it starts from; needed with a length.
    pub from: Option<Reference>,
    /// None is the faces' whole length.
    pub depth: Option<Quantity>,
}

impl CosmeticThread {
    pub fn fields(&self) -> Value {
        let mut v = json!({
            "faces": self.faces.iter().map(Reference::to_json).collect::<Vec<_>>(),
            "thread": self.thread.to_json(),
        });
        if let Some(r) = &self.from {
            v["from"] = r.to_json();
        }
        if let Some(q) = &self.depth {
            v["depth"] = json!(q.text());
        }
        v
    }

    #[doc(hidden)]
    pub fn from_fields(o: &mut Obj, default: Unit, env: &Env) -> Result<CosmeticThread> {
        let faces = match o.take("faces")? {
            Value::Array(a) if !a.is_empty() => a
                .iter()
                .enumerate()
                .map(|(i, x)| Reference::from_json(x, &format!("/faces/{i}")))
                .collect::<Result<Vec<_>>>()?,
            _ => {
                return Err(invalid(
                    "/faces",
                    "faces is a list of one or more cylindrical faces",
                ));
            }
        };
        let tv = o.take("thread")?;
        for k in ["depth", "drill", "wall"] {
            if tv.get(k).is_some() {
                return Err(invalid(
                    &format!("/thread/{k}"),
                    "a cosmetic thread's thread has no depth, drill or wall: its depth is the feature's own",
                ));
            }
        }
        let thread = Thread::read(&tv, default, env)?;
        let from = o
            .take_opt("from")
            .map(|x| Reference::from_json(&x, "/from"))
            .transpose()?;
        let depth = o
            .take_opt("depth")
            .filter(|d| d != "full")
            .map(|d| {
                Quantity::from_json_as(&d, Field::Length(default), &|n| env.get(n))
                    .map_err(|e| e.with("pointer", "/depth"))
            })
            .transpose()?;
        if let Some(q) = &depth
            && q.meters() <= 0.0
        {
            return Err(invalid(
                "/depth",
                "a thread's depth is more than 0, or \"full\"",
            ));
        }
        if depth.is_some() && from.is_none() {
            return Err(Error::new(
                Code::InvalidParam,
                "a cosmetic thread with a length gives `from`: an edge at the end it starts from",
            )
            .with("pointer", "/from")
            .with("reason", "thread_from_needed"));
        }
        Ok(CosmeticThread {
            faces,
            thread,
            from,
            depth,
        })
    }

    /// Its references, each with its role.
    pub fn references(&self) -> Vec<(String, &Reference)> {
        let mut out: Vec<(String, &Reference)> = self
            .faces
            .iter()
            .enumerate()
            .map(|(i, r)| (format!("faces.{i}"), r))
            .collect();
        if let Some(r) = &self.from {
            out.push(("from".to_owned(), r));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use crate::doc::files::Feature;
    use crate::units::Unit;
    use serde_json::{Value, json};

    fn read(v: &Value) -> crate::error::Result<Feature> {
        Feature::from_json(v, Unit::Mm, &crate::expr::Env::empty())
    }

    fn face(k: u8) -> Value {
        json!({"feature": "shaft", "name": format!("{k:032x}")})
    }

    #[test]
    fn cosmetic_threads_round_trip_and_refuse() {
        let v = json!({"format": 25, "kind": "ryucad.feature", "type": "cosmetic_thread",
            "faces": [face(1)], "thread": {"standard": "iso_metric", "size": "M10", "class": "6g"}});
        let f = read(&v).unwrap();
        assert_eq!(f.to_json(), v);
        assert_eq!(f.format(), 25);
        assert_eq!(crate::doc::files::feature_format(&v), 24);
        let mut w = v.clone();
        w["depth"] = json!("12 mm");
        w["from"] = face(2);
        assert_eq!(read(&w).unwrap().to_json(), w);
        // A length runs from an end: `from` is needed.
        w.as_object_mut().unwrap().remove("from");
        assert_eq!(
            read(&w).unwrap_err().details["reason"],
            "thread_from_needed"
        );
        // Its depth is its own field, never the thread's; no tap drill.
        let mut w = v.clone();
        w["thread"]["depth"] = json!("full");
        assert_eq!(read(&w).unwrap_err().details["pointer"], "/thread/depth");
        let mut w = v.clone();
        w["faces"] = json!([]);
        assert_eq!(read(&w).unwrap_err().details["pointer"], "/faces");
    }
}
