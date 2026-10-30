//! Exploded views: `parts/<asm>/explodes/<id>.json`, an assembly's
//! occurrences moved apart step by step for display, in the assembly's
//! frame. An explode never changes a pose, a mate, interference,
//! clearance, mass, a BOM or an export: it is drawn, nothing more. Its
//! quantities are expressions over the assembly's parameters.
//!
//! ```json
//! {
//!   "format": 1,
//!   "kind": "ryucad.explode",
//!   "steps": [
//!     {"instances": ["lid"], "translate": ["0 mm", "0 mm", "40 mm"]},
//!     {"instances": ["p1", "ring:2/pin"],
//!      "rotate": {"angle": "90 deg", "axis": [0, 0, 1]},
//!      "translate": ["gap", "0 mm", "0 mm"]}
//!   ]
//! }
//! ```
//!
//! A step moves the occurrences under each instance path it names (a
//! subassembly's all of them; a pattern's copy `<pattern>:<i>/<seed>`, or
//! a seed's instance and every copy of it), turned by `rotate` about an
//! axis through the middle of their boxes, then moved by `translate`.
//! Steps add up in order.

use super::files::{Obj, header, invalid};
use super::id::{Id, parse_step};
use crate::error::{Code, Error, Result};
use crate::expr::Env;
use crate::units::{Field, Quantity, Unit};
use serde_json::{Value, json};

pub const KIND: &str = "ryucad.explode";

/// The newest format this build reads.
pub const FORMAT: u64 = 1;

/// The folder of an assembly's explodes, inside its folder.
pub const EXPLODES: &str = "explodes";

/// A turn about an axis through the moved occurrences' middle.
#[derive(Clone, Debug, PartialEq)]
pub struct Rotate {
    /// Not of zero length; made unit when applied.
    pub axis: [f64; 3],
    pub angle: Quantity,
}

/// One step: what it moves and how.
#[derive(Clone, Debug, PartialEq)]
pub struct Step {
    /// Instance paths as files write them (`p1`, `p1/left`, `ring:2/pin`),
    /// at least one, each once.
    pub instances: Vec<String>,
    pub translate: Option<[Quantity; 3]>,
    pub rotate: Option<Rotate>,
}

/// An exploded view.
#[derive(Clone, Debug, PartialEq)]
pub struct ExplodeFile {
    pub steps: Vec<Step>,
}

/// An explode's ID: a part ID.
pub fn check_id(text: &str) -> Result<Id> {
    Id::parse(text, "explode")
}

/// Where explode `id` of an assembly lives, relative to its folder.
pub fn explode_file(id: &Id) -> String {
    format!("{EXPLODES}/{id}.json")
}

/// Checks an instance path's text: steps of instance IDs, the first of a
/// copy `<pattern>:<i>[:<j>]`.
pub fn check_path(text: &str, at: &str) -> Result<()> {
    if text.is_empty() {
        return Err(invalid(at, "an instance path names at least one instance"));
    }
    for step in text.split('/') {
        parse_step(step, at)?;
    }
    Ok(())
}

impl Step {
    fn to_json(&self) -> Value {
        let mut v = json!({"instances": self.instances});
        if let Some(t) = &self.translate {
            v["translate"] = json!(t.iter().map(Quantity::text).collect::<Vec<_>>());
        }
        if let Some(r) = &self.rotate {
            v["rotate"] =
                json!({"axis": r.axis.map(super::files::number), "angle": r.angle.text()});
        }
        v
    }

    fn from_json(v: &Value, at: &str, default: Unit, env: &Env) -> Result<Step> {
        let mut o = Obj::new(v, at)?;
        let lookup = |n: &str| env.get(n);
        let here = format!("{at}/instances");
        let list = o.take("instances")?;
        let list = list
            .as_array()
            .filter(|a| !a.is_empty())
            .ok_or_else(|| invalid(&here, "a step moves a list of one or more instance paths"))?;
        let mut instances: Vec<String> = Vec::with_capacity(list.len());
        for (i, p) in list.iter().enumerate() {
            let at = format!("{here}/{i}");
            let text = p
                .as_str()
                .ok_or_else(|| invalid(&at, "an instance path is text"))?;
            check_path(text, &at)?;
            if instances.iter().any(|x| x == text) {
                return Err(invalid(&at, &format!("{text} is listed twice")));
            }
            instances.push(text.to_owned());
        }
        let translate = match o.take_opt("translate") {
            None => None,
            Some(t) => {
                let here = format!("{at}/translate");
                let a = t
                    .as_array()
                    .filter(|a| a.len() == 3)
                    .ok_or_else(|| invalid(&here, "a step's translate is three lengths"))?;
                let mut out = Vec::with_capacity(3);
                for (i, x) in a.iter().enumerate() {
                    out.push(
                        Quantity::from_json_in(x, default, &lookup)
                            .map_err(|e| e.with("pointer", format!("{here}/{i}")))?,
                    );
                }
                let [x, y, z]: [Quantity; 3] = out.try_into().expect("three");
                Some([x, y, z])
            }
        };
        let rotate = match o.take_opt("rotate") {
            None => None,
            Some(r) => {
                let here = format!("{at}/rotate");
                let mut r = Obj::new(&r, &here)?;
                let axis = r.take("axis")?;
                let axis = axis
                    .as_array()
                    .filter(|a| a.len() == 3)
                    .and_then(|a| {
                        let v: Vec<f64> = a.iter().filter_map(Value::as_f64).collect();
                        <[f64; 3]>::try_from(v).ok()
                    })
                    .filter(|a| a.iter().any(|x| *x != 0.0) && a.iter().all(|x| x.is_finite()))
                    .ok_or_else(|| {
                        invalid(
                            &format!("{here}/axis"),
                            "an axis is three numbers, not all 0",
                        )
                    })?;
                let angle = Quantity::from_json_as(&r.take("angle")?, Field::Angle, &lookup)
                    .map_err(|e| e.with("pointer", format!("{here}/angle")))?;
                r.finish()?;
                Some(Rotate { axis, angle })
            }
        };
        o.finish()?;
        if translate.is_none() && rotate.is_none() {
            return Err(invalid(at, "a step moves by translate, rotate or both"));
        }
        Ok(Step {
            instances,
            translate,
            rotate,
        })
    }
}

impl ExplodeFile {
    pub fn to_json(&self) -> Value {
        json!({
            "format": FORMAT,
            "kind": KIND,
            "steps": self.steps.iter().map(Step::to_json).collect::<Vec<_>>(),
        })
    }

    /// Reads an explode: at least one step, each moving a list of
    /// instance paths by lengths (bare numbers in `default`) and a turn,
    /// expressions over `env`, the assembly's parameters.
    pub fn from_json(v: &Value, default: Unit, env: &Env) -> Result<ExplodeFile> {
        let mut o = header(v, KIND, FORMAT)?;
        let steps = o.take("steps")?;
        let steps = steps
            .as_array()
            .filter(|a| !a.is_empty())
            .ok_or_else(|| invalid("/steps", "an explode is a list of one or more steps"))?;
        let steps = steps
            .iter()
            .enumerate()
            .map(|(i, s)| Step::from_json(s, &format!("/steps/{i}"), default, env))
            .collect::<Result<Vec<_>>>()?;
        o.finish()?;
        Ok(ExplodeFile { steps })
    }
}

/// The expression strings an explode's JSON holds, as it is written
/// (each step's lengths and angle), to read their names or rename them.
pub fn expression_strings(v: &mut Value) -> Vec<&mut String> {
    let mut out = Vec::new();
    let Some(Value::Array(steps)) = v.get_mut("steps") else {
        return out;
    };
    for step in steps {
        let Some(step) = step.as_object_mut() else {
            continue;
        };
        for (k, x) in step.iter_mut() {
            match (k.as_str(), x) {
                ("translate", Value::Array(a)) => {
                    out.extend(a.iter_mut().filter_map(|x| match x {
                        Value::String(s) => Some(s),
                        _ => None,
                    }))
                }
                ("rotate", Value::Object(r)) => {
                    if let Some(Value::String(s)) = r.get_mut("angle") {
                        out.push(s);
                    }
                }
                _ => {}
            }
        }
    }
    out
}

/// The parameters an explode's JSON names.
pub fn names(v: &Value) -> std::collections::BTreeSet<String> {
    let mut v = v.clone();
    expression_strings(&mut v)
        .into_iter()
        .filter_map(|s| crate::expr::Expr::parse(s).ok())
        .flat_map(|e| e.names())
        .collect()
}

/// `wrong_part_kind` for an explode in a part that isn't an assembly.
pub fn not_an_assembly(part: &str) -> Error {
    Error::new(
        Code::WrongPartKind,
        format!("{part} is a part: only an assembly has exploded views"),
    )
    .with("part", part)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn explode() -> Value {
        json!({"format": 1, "kind": KIND, "steps": [
            {"instances": ["lid"], "translate": ["0 mm", "0 mm", "40 mm"]},
            {"instances": ["p1", "ring:2/pin"],
             "rotate": {"angle": "90 deg", "axis": [0, 0, 1]},
             "translate": ["2 * gap", "0 mm", "0 mm"]}
        ]})
    }

    fn env() -> Env {
        let p = super::super::files::ParametersFile::from_json(&json!({
            "format": 1, "kind": "ryucad.parameters",
            "parameters": {"gap": "5 mm"}
        }))
        .unwrap();
        p.env()
    }

    fn read(v: &Value) -> Result<ExplodeFile> {
        ExplodeFile::from_json(v, Unit::Mm, &env())
    }

    #[test]
    fn an_explode_reads_and_writes_back() {
        let e = read(&explode()).unwrap();
        assert_eq!(e.steps.len(), 2);
        assert_eq!(e.steps[1].instances, ["p1", "ring:2/pin"]);
        let t = e.steps[1].translate.as_ref().unwrap();
        assert_eq!(t[0].meters(), 0.01);
        assert_eq!(t[0].text(), "2 * gap");
        let r = e.steps[1].rotate.as_ref().unwrap();
        assert_eq!(r.axis, [0.0, 0.0, 1.0]);
        assert!((r.angle.radians() - core::f64::consts::FRAC_PI_2).abs() < 1e-15);
        assert_eq!(e.to_json(), explode());
        // A bare number is a length in the default unit, written so.
        let mut bare = explode();
        bare["steps"][0]["translate"] = json!([0, 0, 40]);
        let b = read(&bare).unwrap();
        assert_eq!(b.steps[0].translate.as_ref().unwrap()[2].meters(), 0.04);
        assert_eq!(b.to_json(), explode());
    }

    #[test]
    fn bad_explodes_say_where() {
        let at = |f: &dyn Fn(&mut Value)| {
            let mut v = explode();
            f(&mut v);
            let e = read(&v).unwrap_err();
            (
                e.code,
                e.details["pointer"].as_str().unwrap_or("").to_owned(),
            )
        };
        let bad = |p: &str| (Code::InvalidDocument, p.to_owned());
        assert_eq!(at(&|v| v["steps"] = json!([])), bad("/steps"));
        assert_eq!(
            at(&|v| v["steps"][0]["instances"] = json!([])),
            bad("/steps/0/instances")
        );
        assert_eq!(
            at(&|v| v["steps"][0]["instances"] = json!(["lid", "lid"])),
            bad("/steps/0/instances/1")
        );
        assert_eq!(
            at(&|v| v["steps"][0]["instances"] = json!(["Lid"])).1,
            "/steps/0/instances/0"
        );
        assert_eq!(
            at(&|v| v["steps"][0]["instances"] = json!(["ring:0/pin"])).1,
            "/steps/0/instances/0"
        );
        assert_eq!(
            at(&|v| {
                v["steps"][0].as_object_mut().unwrap().remove("translate");
            }),
            bad("/steps/0")
        );
        assert_eq!(
            at(&|v| v["steps"][0]["translate"] = json!([1, 2])),
            bad("/steps/0/translate")
        );
        assert_eq!(
            at(&|v| v["steps"][1]["rotate"]["axis"] = json!([0, 0, 0])),
            bad("/steps/1/rotate/axis")
        );
        assert_eq!(
            at(&|v| v["steps"][1]["rotate"]["angle"] = json!("5 mm")).1,
            "/steps/1/rotate/angle"
        );
        assert_eq!(
            at(&|v| v["steps"][1]["translate"][0] = json!("nope")).1,
            "/steps/1/translate/0"
        );
        assert_eq!(
            at(&|v| v["steps"][0]["spin"] = json!(1)),
            bad("/steps/0/spin")
        );
        let mut newer = explode();
        newer["format"] = json!(2);
        assert_eq!(read(&newer).unwrap_err().code, Code::FormatTooNew);
    }

    #[test]
    fn ids_and_files() {
        assert_eq!(check_id("apart").unwrap().as_str(), "apart");
        assert_eq!(check_id("Apart").unwrap_err().code, Code::InvalidId);
        assert_eq!(
            explode_file(&check_id("apart").unwrap()),
            "explodes/apart.json"
        );
        assert_eq!(not_an_assembly("bolt").code, Code::WrongPartKind);
        assert!(check_path("p1/left", "").is_ok());
        assert!(check_path("ring:2:3/pin", "").is_ok());
        assert!(check_path("", "").is_err());
        assert!(check_path("p1//x", "").is_err());
    }
}
