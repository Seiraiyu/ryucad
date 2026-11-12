//! A hole's sizes from what it says it is: `derive(hole, tables)` is
//! pure, reading only the hole's fields and the standards (ryu-standards'
//! built-ins and the project's `hole_standards.json`). Every size comes
//! back with its source. The file never holds a derived size; a written
//! one overrides (`source: override`).
//!
//! Lengths from tables are exact nanometres until here, and become f64
//! metres by one division each.

use crate::doc::hole::{Hole, HoleForm};
use crate::doc::hole_standards::HoleStandards;
use crate::error::{Code, Error, Result};
use crate::units::Quantity;
use serde_json::{Map, Value, json};

/// Where a size came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// A built-in value: a formula's, a public-domain document's, a fact.
    Standard(ryu_standards::Source),
    /// ryucad's tap-drill rule.
    Rule,
    /// The project's `hole_standards.json`.
    Project,
    /// Written in the hole's file.
    Override,
    /// ISO 286.
    Fit,
}

impl Source {
    pub fn kind(self) -> &'static str {
        match self {
            Source::Standard(s) => s.kind(),
            Source::Rule => "rule",
            Source::Project => "project",
            Source::Override => "override",
            Source::Fit => "fit",
        }
    }
}

/// One size: in metres (or radians for an angle), its exact text, and
/// its source.
#[derive(Clone, Debug, PartialEq)]
pub struct Size {
    pub si: f64,
    pub text: String,
    pub angle: bool,
    pub source: Source,
}

impl Size {
    /// A table's length: exact nanometres, written in inches when `inch`
    /// and exact there, else millimetres.
    pub fn nm(nm: i64, inch: bool, source: Source) -> Size {
        let text = match inch.then(|| ryu_standards::in_decimal(nm)).flatten() {
            Some(t) => format!("{t} in"),
            None => format!("{} mm", ryu_standards::mm_decimal(nm)),
        };
        Size {
            si: nm as f64 / 1e9,
            text,
            angle: false,
            source,
        }
    }

    /// A written quantity: a length, or an angle.
    pub fn quantity(q: &Quantity, source: Source) -> Size {
        let angle = q.dim() == crate::units::Dim::ANGLE;
        Size {
            si: q.si(),
            text: q.text(),
            angle,
            source,
        }
    }

    /// The nanometres of a length, rounded: exact for any decimal written
    /// to the nanometre.
    pub fn to_nm(&self) -> i64 {
        (self.si * 1e9).round() as i64
    }

    pub fn to_json(&self) -> Value {
        let key = if self.angle { "rad" } else { "m" };
        let mut v = json!({"value": self.text, "source": self.source.kind()});
        v[key] = json!(self.si);
        if let Source::Standard(s) = self.source {
            v["from"] = json!(s.text());
        }
        v
    }
}

/// A thread's derived sizes (Task 20).
#[derive(Clone, Debug, PartialEq)]
pub struct ThreadSizes {
    pub standard: String,
    pub size: String,
    /// `M6x1.0-6H`, `1/4-20 UNC-2B`: the spec without depth.
    pub designation: String,
    pub major: Option<Size>,
    pub pitch: Size,
    /// Threads per inch, for inch and pipe threads.
    pub tpi: Option<String>,
    pub lead: Size,
    pub class: Option<String>,
    pub minor: Option<[Size; 2]>,
    pub major_min: Option<Size>,
    pub left: bool,
    pub starts: u32,
    /// None is full depth.
    pub depth: Option<Size>,
    pub taper: Option<(u32, u32)>,
}

/// The bore's tolerance, worked out.
#[derive(Clone, Debug, PartialEq)]
pub struct FitSizes {
    /// The ISO 286 class, when one.
    pub class: Option<String>,
    pub upper: Size,
    pub lower: Size,
    /// Least and greatest diameter.
    pub limits: [Size; 2],
}

/// Every size a hole is built from.
#[derive(Clone, Debug, PartialEq)]
pub struct Sizes {
    pub drill: Size,
    /// Whether the drill is a standard drill (a tap-drill rule's computed
    /// size when none qualifies is not).
    pub drill_standard: bool,
    /// Diameter and depth.
    pub counterbore: Option<(Size, Size)>,
    /// Diameter and included angle.
    pub countersink: Option<(Size, Size)>,
    pub thread: Option<ThreadSizes>,
    pub fit: Option<FitSizes>,
}

impl Sizes {
    /// How deep the mouth goes, in metres; none for a simple hole.
    pub fn mouth_depth(&self) -> Option<f64> {
        if let Some((_, depth)) = &self.counterbore {
            return Some(depth.si);
        }
        let (d, a) = self.countersink.as_ref()?;
        let run = (d.si - self.drill.si) / 2.0;
        Some(run / ryukei::math::tan(a.si / 2.0))
    }

    pub fn to_json(&self) -> Value {
        let mut v = Map::new();
        v.insert("drill".into(), self.drill.to_json());
        if !self.drill_standard {
            v.insert("drill_standard".into(), json!(false));
        }
        if let Some((d, h)) = &self.counterbore {
            v.insert(
                "counterbore".into(),
                json!({"diameter": d.to_json(), "depth": h.to_json()}),
            );
        }
        if let Some((d, a)) = &self.countersink {
            v.insert(
                "countersink".into(),
                json!({"diameter": d.to_json(), "angle": a.to_json()}),
            );
        }
        if let Some(t) = &self.thread {
            let mut x = json!({
                "standard": t.standard, "size": t.size, "designation": t.designation,
                "pitch": t.pitch.to_json(), "lead": t.lead.to_json(),
                "hand": if t.left { "left" } else { "right" }, "starts": t.starts,
            });
            if let Some(m) = &t.major {
                x["major"] = m.to_json();
            }
            if let Some(n) = &t.tpi {
                x["tpi"] = json!(n);
            }
            if let Some(c) = &t.class {
                x["class"] = json!(c);
            }
            let mut limits = Map::new();
            if let Some(m) = &t.major_min {
                limits.insert("major".into(), json!([m.to_json(), null]));
            }
            if let Some([lo, hi]) = &t.minor {
                limits.insert("minor".into(), json!([lo.to_json(), hi.to_json()]));
            }
            if !limits.is_empty() {
                x["limits"] = Value::Object(limits);
            }
            x["depth"] = t.depth.as_ref().map_or(json!("full"), Size::to_json);
            if let Some((a, b)) = t.taper {
                x["taper"] = json!(format!("{a}:{b}"));
            }
            v.insert("thread".into(), x);
        }
        if let Some(f) = &self.fit {
            let mut x = json!({"upper": f.upper.to_json(), "lower": f.lower.to_json(),
                               "limits": [f.limits[0].to_json(), f.limits[1].to_json()]});
            if let Some(c) = &f.class {
                x["class"] = json!(c);
            }
            v.insert("fit".into(), x);
        }
        Value::Object(v)
    }
}

/// The project's tables, as a part sees them.
pub type Tables<'a> = Option<&'a std::result::Result<HoleStandards, Error>>;

/// A hole's sizes.
pub fn derive(h: &Hole, tables: Tables) -> Result<Sizes> {
    if h.thread.is_some() || h.fastener.is_some() {
        // Tasks 20 and 21 derive these.
        let _ = tables;
        return Err(Error::new(
            Code::InvalidParam,
            "threads and fasteners are derived from Task 20 on",
        ));
    }
    let drill = Size::quantity(
        h.diameter
            .as_ref()
            .expect("a plain hole writes its diameter"),
        Source::Override,
    );
    let written = |q: &Option<Quantity>| q.as_ref().map(|q| Size::quantity(q, Source::Override));
    let (counterbore, countersink) = match &h.form {
        HoleForm::Simple => (None, None),
        HoleForm::Counterbore { diameter, depth } => (written(diameter).zip(written(depth)), None),
        HoleForm::Countersink { diameter, angle } => (None, written(diameter).zip(written(angle))),
    };
    let _ = &h.bore; // Task 21
    Ok(Sizes {
        drill,
        drill_standard: true,
        counterbore,
        countersink,
        thread: None,
        fit: None,
    })
}

/// A refusal at write time as a rebuild reports it: a project entry
/// that was there when the hole was written and is gone now is
/// `not_found`, naming the file.
pub fn at_rebuild(e: Error) -> Error {
    if e.details.get("reason").and_then(Value::as_str) == Some("needs_project_table") {
        let mut e = e;
        e.code = Code::NotFound;
        return e;
    }
    e
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::files::Feature;
    use crate::expr::Env;
    use crate::units::Unit;
    use serde_json::json;

    pub(crate) fn hole(fields: Value) -> Hole {
        let mut v = json!({"format": 3, "kind": "ryucad.feature", "type": "hole",
                           "sketch": "s", "form": "simple", "depth": "through_all",
                           "targets": ["b"]});
        for (k, x) in fields.as_object().unwrap() {
            v[k] = x.clone();
        }
        match Feature::from_json(&v, Unit::Mm, &Env::empty()).unwrap() {
            Feature::Hole(h) => *h,
            _ => unreachable!(),
        }
    }

    #[test]
    fn a_plain_hole_is_built_from_what_it_writes() {
        let h = hole(json!({"form": "counterbore", "diameter": "6.6 mm",
                            "counterbore": {"diameter": "11 mm", "depth": "6.8 mm"}}));
        let s = derive(&h, None).unwrap();
        assert_eq!(s.drill.si, 0.0066);
        assert_eq!(s.drill.source, Source::Override);
        let (d, depth) = s.counterbore.as_ref().unwrap();
        assert_eq!((d.si, depth.si), (0.011, 0.0068));
        assert_eq!(s.mouth_depth(), Some(0.0068));
        assert_eq!(
            s.to_json()["drill"],
            json!({"value": "6.6 mm", "m": 0.0066, "source": "override"})
        );
    }
}
