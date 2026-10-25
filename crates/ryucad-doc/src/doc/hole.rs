//! Hole features: simple, counterbored or countersunk holes drilled at a
//! sketch's points into the solids they name, against the sketch's
//! normal. A hole always cuts.

use super::files::{Obj, invalid};
use super::id::Id;
use super::reference::Reference;
use super::solid::targets_from_json;
use crate::error::{Code, Error, Result};
use crate::expr::Env;
use crate::units::{Field, Quantity, Unit};
use serde_json::{Value, json};

/// A hole at each of a sketch's points.
#[derive(Clone, Debug, PartialEq)]
pub struct Hole {
    pub sketch: Id,
    /// Points of the sketch; empty means every point no curve uses.
    pub points: Vec<Id>,
    pub form: HoleForm,
    /// The bore's diameter.
    pub diameter: Quantity,
    pub depth: Depth,
    pub bottom: Bottom,
    /// The solids it cuts, by their founders.
    pub targets: Vec<Id>,
}

/// What the hole's mouth is.
#[derive(Clone, Debug, PartialEq)]
pub enum HoleForm {
    Simple,
    /// A wider, flat-bottomed bore of `diameter` to `depth` first.
    Counterbore {
        diameter: Quantity,
        depth: Quantity,
    },
    /// A cone of `diameter` at the mouth and included `angle`.
    Countersink {
        diameter: Quantity,
        angle: Quantity,
    },
}

/// How deep the bore goes, from the sketch's plane.
#[derive(Clone, Debug, PartialEq)]
pub enum Depth {
    /// To the bore's full diameter (a drill point's shoulder).
    Blind(Quantity),
    /// Past everything of its targets.
    ThroughAll,
    /// To a planar face parallel to the sketch, or a vertex, then
    /// `offset` further (a length, of either sign).
    UpTo {
        target: Reference,
        offset: Option<Quantity>,
    },
}

/// The bottom of a blind hole.
#[derive(Clone, Debug, PartialEq)]
pub enum Bottom {
    Flat,
    /// A drill point of this included angle.
    Drill(Quantity),
}

impl HoleForm {
    pub const NAMES: [&str; 3] = ["simple", "counterbore", "countersink"];

    pub fn type_name(&self) -> &'static str {
        match self {
            HoleForm::Simple => "simple",
            HoleForm::Counterbore { .. } => "counterbore",
            HoleForm::Countersink { .. } => "countersink",
        }
    }
}

impl Depth {
    pub fn type_name(&self) -> &'static str {
        match self {
            Depth::Blind(_) => "blind",
            Depth::ThroughAll => "through_all",
            Depth::UpTo { .. } => "up_to",
        }
    }

    pub fn to_json(&self) -> Value {
        match self {
            Depth::Blind(q) => json!({"blind": q.text()}),
            Depth::ThroughAll => json!("through_all"),
            Depth::UpTo { target, offset } => {
                let mut v = json!({"target": target.to_json()});
                if let Some(q) = offset {
                    v["offset"] = json!(q.text());
                }
                json!({ "up_to": v })
            }
        }
    }

    fn from_json(v: &Value, default: Unit, env: &Env) -> Result<Depth> {
        if v == "through_all" {
            return Ok(Depth::ThroughAll);
        }
        let bad = || {
            invalid_extent(
                "/depth",
                "a hole's depth is {\"blind\": length}, \"through_all\" or {\"up_to\": {target, offset}}"
                    .to_owned(),
            )
        };
        let Some(o) = v.as_object().filter(|o| o.len() == 1) else {
            return Err(bad());
        };
        let (k, x) = o.iter().next().expect("one field");
        match k.as_str() {
            "blind" => Ok(Depth::Blind(size(
                x,
                Field::Length(default),
                env,
                "/depth/blind",
                "a hole's depth",
            )?)),
            "up_to" => {
                let mut o = Obj::new(x, "/depth/up_to")?;
                let target = Reference::from_json(&o.take("target")?, "/depth/up_to/target")?;
                let offset = o
                    .take_opt("offset")
                    .map(|v| {
                        Quantity::from_json_in(&v, default, &|n| env.get(n))
                            .map_err(|e| e.with("pointer", "/depth/up_to/offset"))
                    })
                    .transpose()?;
                o.finish()?;
                Ok(Depth::UpTo { target, offset })
            }
            _ => Err(bad()),
        }
    }
}

impl Bottom {
    pub fn to_json(&self) -> Value {
        match self {
            Bottom::Flat => json!("flat"),
            Bottom::Drill(q) => json!({"drill": q.text()}),
        }
    }

    fn from_json(v: &Value, env: &Env) -> Result<Bottom> {
        if v == "flat" {
            return Ok(Bottom::Flat);
        }
        match v
            .as_object()
            .filter(|o| o.len() == 1)
            .and_then(|o| o.get("drill"))
        {
            Some(a) => Ok(Bottom::Drill(angle(
                a,
                env,
                "/bottom/drill",
                "a drill point",
            )?)),
            None => Err(invalid(
                "/bottom",
                "a hole's bottom is \"flat\" or {\"drill\": angle}",
            )),
        }
    }
}

fn invalid_extent(at: &str, msg: String) -> Error {
    Error::new(Code::InvalidExtent, msg).with("pointer", at)
}

/// A length or an angle greater than zero, at `at`.
fn size(v: &Value, field: Field, env: &Env, at: &str, what: &str) -> Result<Quantity> {
    let q = Quantity::from_json_as(v, field, &|n| env.get(n)).map_err(|e| e.with("pointer", at))?;
    let si = match field {
        Field::Length(_) => q.meters(),
        Field::Angle => q.radians(),
    };
    if si <= 0.0 {
        return Err(bad_size(
            at,
            &format!("{what} must be greater than zero"),
            &q,
        ));
    }
    Ok(q)
}

/// An included angle, in `(0, 180 deg)`.
fn angle(v: &Value, env: &Env, at: &str, what: &str) -> Result<Quantity> {
    let q = size(v, Field::Angle, env, at, &format!("{what}'s angle"))?;
    if q.radians() >= core::f64::consts::PI {
        return Err(bad_size(
            at,
            &format!("{what}'s angle is under 180 deg"),
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

impl Hole {
    pub fn fields(&self) -> Value {
        let mut v = json!({
            "sketch": self.sketch,
            "form": self.form.type_name(),
            "diameter": self.diameter.text(),
            "depth": self.depth.to_json(),
            "targets": self.targets,
        });
        if !self.points.is_empty() {
            v["points"] = json!(self.points);
        }
        match &self.form {
            HoleForm::Simple => {}
            HoleForm::Counterbore { diameter, depth } => {
                v["counterbore"] = json!({"diameter": diameter.text(), "depth": depth.text()});
            }
            HoleForm::Countersink { diameter, angle } => {
                v["countersink"] = json!({"diameter": diameter.text(), "angle": angle.text()});
            }
        }
        if self.bottom != Bottom::Flat {
            v["bottom"] = self.bottom.to_json();
        }
        v
    }

    #[doc(hidden)]
    pub fn from_fields(o: &mut Obj, default: Unit, env: &Env) -> Result<Hole> {
        let length = Field::Length(default);
        let sketch = o.take("sketch")?;
        let sketch = sketch
            .as_str()
            .ok_or_else(|| invalid("/sketch", "sketch is a sketch feature's ID"))?;
        let sketch = Id::parse(sketch, "feature").map_err(|e| e.with("pointer", "/sketch"))?;
        let points = points_from_json(o.take_opt("points"))?;
        let diameter = size(
            &o.take("diameter")?,
            length,
            env,
            "/diameter",
            "a hole's diameter",
        )?;
        let form = o.take("form")?;
        let (cbore, csink) = (o.take_opt("counterbore"), o.take_opt("countersink"));
        let form = match (form.as_str(), cbore, csink) {
            (Some("simple"), None, None) => HoleForm::Simple,
            (Some("counterbore"), Some(c), None) => {
                let mut c = Obj::new(&c, "/counterbore")?;
                let d = size(
                    &c.take("diameter")?,
                    length,
                    env,
                    "/counterbore/diameter",
                    "a counterbore's diameter",
                )?;
                let depth = size(
                    &c.take("depth")?,
                    length,
                    env,
                    "/counterbore/depth",
                    "a counterbore's depth",
                )?;
                c.finish()?;
                wider(&d, &diameter, "/counterbore/diameter", "a counterbore")?;
                HoleForm::Counterbore { diameter: d, depth }
            }
            (Some("countersink"), None, Some(c)) => {
                let mut c = Obj::new(&c, "/countersink")?;
                let d = size(
                    &c.take("diameter")?,
                    length,
                    env,
                    "/countersink/diameter",
                    "a countersink's diameter",
                )?;
                let a = angle(
                    &c.take("angle")?,
                    env,
                    "/countersink/angle",
                    "a countersink",
                )?;
                c.finish()?;
                wider(&d, &diameter, "/countersink/diameter", "a countersink")?;
                HoleForm::Countersink {
                    diameter: d,
                    angle: a,
                }
            }
            (Some(f @ ("simple" | "counterbore" | "countersink")), _, _) => {
                let (at, msg) = match f {
                    "counterbore" => (
                        "/counterbore",
                        "a counterbore hole has a counterbore {diameter, depth}, and no countersink",
                    ),
                    "countersink" => (
                        "/countersink",
                        "a countersink hole has a countersink {diameter, angle}, and no counterbore",
                    ),
                    _ => ("/form", "a simple hole has no counterbore or countersink"),
                };
                return Err(invalid(at, msg));
            }
            _ => {
                return Err(invalid(
                    "/form",
                    "a hole's form is simple, counterbore or countersink",
                ));
            }
        };
        let depth = Depth::from_json(&o.take("depth")?, default, env)?;
        let bottom = match o.take_opt("bottom") {
            None => Bottom::Flat,
            Some(b) => Bottom::from_json(&b, env)?,
        };
        if matches!(bottom, Bottom::Drill(_)) && !matches!(depth, Depth::Blind(_)) {
            return Err(invalid_extent(
                "/bottom",
                format!(
                    "a drill point ends a blind hole; a {} hole has no bottom of its own",
                    depth.type_name()
                ),
            ));
        }
        let targets = targets_from_json(&o.take("targets")?, "/targets")?;
        let hole = Hole {
            sketch,
            points,
            form,
            diameter,
            depth,
            bottom,
            targets,
        };
        if let (Depth::Blind(d), Some((mouth, at))) = (&hole.depth, hole.mouth_depth())
            && mouth >= d.meters()
        {
            return Err(invalid_extent(
                at,
                format!(
                    "the hole's {} reaches {mouth} m, past its depth {d}",
                    hole.form.type_name()
                ),
            ));
        }
        Ok(hole)
    }

    /// How deep a counterbore or countersink goes, in meters, and the
    /// pointer of what sets it; `None` for a simple hole.
    pub fn mouth_depth(&self) -> Option<(f64, &'static str)> {
        match &self.form {
            HoleForm::Simple => None,
            HoleForm::Counterbore { depth, .. } => Some((depth.meters(), "/counterbore/depth")),
            HoleForm::Countersink { diameter, angle } => {
                let run = (diameter.meters() - self.diameter.meters()) / 2.0;
                let half = angle.radians() / 2.0;
                Some((run / ryukei::math::tan(half), "/countersink"))
            }
        }
    }

    /// Its references, each with its role: an up-to target's `depth`.
    pub fn references(&self) -> Vec<(String, &Reference)> {
        match &self.depth {
            Depth::UpTo { target, .. } => vec![("depth".to_owned(), target)],
            _ => Vec::new(),
        }
    }
}

/// `inner` is narrower than `outer`, the bore's diameter.
fn wider(outer: &Quantity, inner: &Quantity, at: &str, what: &str) -> Result<()> {
    if outer.meters() <= inner.meters() {
        return Err(bad_size(
            at,
            &format!("{what} is wider than the hole's diameter {inner}"),
            outer,
        ));
    }
    Ok(())
}

/// Reads the optional `points` field: point IDs, each once.
fn points_from_json(v: Option<Value>) -> Result<Vec<Id>> {
    let Some(v) = v else {
        return Ok(Vec::new());
    };
    let a = v
        .as_array()
        .ok_or_else(|| invalid("/points", "points is an array of the sketch's point IDs"))?;
    let mut out: Vec<Id> = Vec::with_capacity(a.len());
    for (i, x) in a.iter().enumerate() {
        let at = format!("/points/{i}");
        let s = x
            .as_str()
            .ok_or_else(|| invalid(&at, "expected a point's ID"))?;
        let id = Id::parse(s, "entity").map_err(|e| e.with("pointer", &at))?;
        if out.contains(&id) {
            return Err(invalid(&at, &format!("{id} is named twice")));
        }
        out.push(id);
    }
    Ok(out)
}

/// The expression strings of a hole: its diameter, its counterbore's or
/// countersink's sizes, a blind depth, an up-to offset and a drill
/// point's angle.
pub fn expression_strings<'a>(k: &str, field: &'a mut Value) -> Vec<&'a mut String> {
    let mut out = Vec::new();
    match (k, field) {
        ("counterbore" | "countersink", Value::Object(o)) => {
            for x in o.values_mut() {
                if let Value::String(s) = x {
                    out.push(s);
                }
            }
        }
        ("depth", Value::Object(o)) => {
            for (k, x) in o.iter_mut() {
                match (k.as_str(), x) {
                    ("blind", Value::String(s)) => out.push(s),
                    ("up_to", Value::Object(u)) => {
                        if let Some(Value::String(s)) = u.get_mut("offset") {
                            out.push(s);
                        }
                    }
                    _ => {}
                }
            }
        }
        ("bottom", Value::Object(o)) => {
            if let Some(Value::String(s)) = o.get_mut("drill") {
                out.push(s);
            }
        }
        _ => {}
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::canonical;
    use crate::doc::files::Feature;

    fn read(v: &Value) -> Result<Feature> {
        let p = [("d".to_owned(), crate::expr::Expr::parse("6 mm").unwrap())].into();
        Feature::from_json(v, Unit::Mm, &Env::new(&p))
    }

    fn hole() -> Value {
        json!({"format": 3, "kind": "ryucad.feature", "type": "hole", "sketch": "s3",
               "points": ["p1", "p2"], "form": "counterbore", "diameter": "d + 0.6 mm",
               "counterbore": {"diameter": "11 mm", "depth": "6.8 mm"},
               "depth": {"blind": "20 mm"}, "bottom": {"drill": "118 deg"},
               "targets": ["box1"]})
    }

    #[test]
    fn holes_round_trip() {
        let f = read(&hole()).unwrap();
        let Feature::Hole(h) = &f else { panic!() };
        assert_eq!(h.diameter.meters(), 0.0066);
        assert_eq!(h.form.type_name(), "counterbore");
        assert_eq!(h.mouth_depth().unwrap().0, 0.0068);
        assert_eq!(h.bottom.to_json(), json!({"drill": "118 deg"}));
        assert_eq!(f.format(), 3);
        assert_eq!(f.targets(), [&Id::parse("box1", "feature").unwrap()]);
        assert_eq!(f.swept().map(Id::as_str), Some("s3"));
        assert!(f.references().is_empty());
        let text = canonical::to_string(&f.to_json());
        assert_eq!(read(&canonical::parse(&text).unwrap()).unwrap(), f);
        assert_eq!(f.to_json()["diameter"], "d + 0.6 mm");
        // Defaults go unwritten: every point, a flat bottom.
        let mut v = hole();
        let o = v.as_object_mut().unwrap();
        o.remove("points");
        o.remove("bottom");
        o.remove("counterbore");
        o.insert("form".into(), json!("countersink"));
        o.insert(
            "countersink".into(),
            json!({"diameter": "12 mm", "angle": 90}),
        );
        let f = read(&v).unwrap();
        let Feature::Hole(h) = &f else { panic!() };
        assert!(h.points.is_empty() && h.bottom == Bottom::Flat);
        // A 90 deg countersink from 12 mm to 6.6 mm is 2.7 mm deep.
        assert!((h.mouth_depth().unwrap().0 - 0.0027).abs() < 1e-15);
        let w = f.to_json();
        assert!(w.get("points").is_none() && w.get("bottom").is_none());
        assert_eq!(w["countersink"]["angle"], "90 deg");
        assert_eq!(read(&w).unwrap(), f);
    }

    #[test]
    fn depths() {
        const N: &str = "0123456789abcdef0123456789abcdef";
        let mut v = hole();
        v.as_object_mut().unwrap().remove("bottom");
        v["depth"] = json!("through_all");
        let f = read(&v).unwrap();
        let Feature::Hole(h) = &f else { panic!() };
        assert_eq!(h.depth, Depth::ThroughAll);
        assert_eq!(read(&f.to_json()).unwrap(), f);
        v["depth"] = json!({"up_to": {"target": {"feature": "box1", "name": N}, "offset": "-d"}});
        let f = read(&v).unwrap();
        let Feature::Hole(h) = &f else { panic!() };
        let Depth::UpTo { target, offset } = &h.depth else {
            panic!()
        };
        assert_eq!(target.feature.as_str(), "box1");
        assert_eq!(offset.as_ref().unwrap().meters(), -0.006);
        assert_eq!(
            f.references()
                .iter()
                .map(|(r, _)| r.as_str())
                .collect::<Vec<_>>(),
            ["depth"]
        );
        assert_eq!(read(&f.to_json()).unwrap(), f);
    }

    #[test]
    fn holes_are_checked() {
        let cases: [(&str, Value, Code, &str); 14] = [
            ("/diameter", json!("0 mm"), Code::InvalidParam, "/diameter"),
            ("/diameter", json!("3 deg"), Code::UnitMismatch, "/diameter"),
            ("/form", json!("tapped"), Code::InvalidDocument, "/form"),
            (
                "/counterbore/diameter",
                json!("5 mm"),
                Code::InvalidParam,
                "/counterbore/diameter",
            ),
            (
                "/counterbore/depth",
                json!("20 mm"),
                Code::InvalidExtent,
                "/counterbore/depth",
            ),
            (
                "/counterbore/glue",
                json!(1),
                Code::InvalidDocument,
                "/counterbore/glue",
            ),
            ("/depth", json!("deep"), Code::InvalidExtent, "/depth"),
            (
                "/depth",
                json!({"blind": "-1 mm"}),
                Code::InvalidParam,
                "/depth/blind",
            ),
            (
                "/depth",
                json!({"up_to": {}}),
                Code::InvalidDocument,
                "/depth/up_to/target",
            ),
            (
                "/bottom",
                json!({"drill": "180 deg"}),
                Code::InvalidParam,
                "/bottom/drill",
            ),
            ("/bottom", json!("round"), Code::InvalidDocument, "/bottom"),
            (
                "/points",
                json!(["p1", "p1"]),
                Code::InvalidDocument,
                "/points/1",
            ),
            ("/targets", json!([]), Code::InvalidDocument, "/targets"),
            ("/sketch", json!("S"), Code::InvalidId, "/sketch"),
        ];
        for (path, value, code, pointer) in cases {
            let mut v = hole();
            let (parent, key) = path.rsplit_once('/').unwrap();
            v.pointer_mut(if parent.is_empty() { "" } else { parent })
                .unwrap()[key] = value;
            let e = read(&v).unwrap_err();
            assert_eq!(
                (e.code, e.details["pointer"].as_str()),
                (code, Some(pointer)),
                "{path}: {e}"
            );
        }
        // A drill point only ends a blind hole.
        let mut v = hole();
        v["depth"] = json!("through_all");
        let e = read(&v).unwrap_err();
        assert_eq!(
            (e.code, e.details["pointer"].as_str()),
            (Code::InvalidExtent, Some("/bottom"))
        );
        // Each form has its own fields, and only those.
        let mut v = hole();
        v["form"] = json!("simple");
        assert_eq!(read(&v).unwrap_err().details["pointer"], "/form");
        v["form"] = json!("countersink");
        assert_eq!(read(&v).unwrap_err().details["pointer"], "/countersink");
        v.as_object_mut().unwrap().remove("targets");
        v["form"] = json!("counterbore");
        assert_eq!(read(&v).unwrap_err().details["pointer"], "/targets");
    }

    #[test]
    fn sizes_read_parameters() {
        let v = hole();
        let f = read(&v).unwrap();
        assert_eq!(
            crate::doc::files::feature_names(&f.to_json())
                .into_iter()
                .collect::<Vec<_>>(),
            ["d"]
        );
        let mut w = f.to_json();
        assert!(crate::doc::files::rename_in_feature(&mut w, "d", "bore"));
        assert_eq!(w["diameter"], "bore + 0.6 mm");
        let mut v = hole();
        v["depth"] = json!({"blind": "2 * d"});
        v["counterbore"]["depth"] = json!("d");
        v["bottom"]["drill"] = json!("d / 1 mm * 20 deg");
        let f = read(&v).unwrap();
        let names = crate::doc::files::feature_names(&f.to_json());
        assert_eq!(names.into_iter().collect::<Vec<_>>(), ["d"]);
        let mut w = f.to_json();
        assert!(crate::doc::files::rename_in_feature(&mut w, "d", "bore"));
        assert_eq!(
            (
                &w["depth"]["blind"],
                &w["counterbore"]["depth"],
                &w["bottom"]["drill"]
            ),
            (
                &json!("2 * bore"),
                &json!("bore"),
                &json!("bore / 1 mm * 20 deg")
            )
        );
    }
}
