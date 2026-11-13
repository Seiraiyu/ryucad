//! Motions: `parts/<asm>/motions/<id>.json`, drives an assembly's sweep
//! steps from start to end (design 529). A motion never moves anything
//! a file holds: a sweep, a playback or a render reads it. Its values
//! are expressions over the assembly's parameters.
//!
//! ```json
//! {"format": 1, "kind": "ryucad.motion", "steps": 13,
//!  "drives": [
//!    {"mate": "hinge", "angle": {"from": "-6 deg", "to": "6 deg"}},
//!    {"instances": ["p1", "p2"],
//!     "rotate": {"axis": {"origin": ["0 mm", "0 mm", "0 mm"], "direction": "z"},
//!                "from": "0 deg", "to": "90 deg"}}
//!  ]}
//! ```
//!
//! A **mate drive** takes one of the assembly's own revolute,
//! cylindrical, slider or planar mates' angle or distance from `from` to
//! `to`. A **move drive** turns (`rotate`, about an axis through
//! `origin`, `direction` `x`, `y`, `z` or three numbers) and/or moves
//! (`translate`, along `direction`) a set of top-level instances as one
//! rigid group, held where it puts them. Every drive moves linearly with
//! the same fraction. `steps` is 2 to 1,000, 11 when left out, and is
//! always written.

use super::files::{Obj, header, invalid};
use super::id::Id;
use crate::error::{Code, Error, Result};
use crate::expr::Env;
use crate::units::{Field, Quantity, Unit};
use serde_json::{Value, json};
use std::collections::BTreeSet;

pub const KIND: &str = "ryucad.motion";

/// The newest format this build reads.
pub const FORMAT: u64 = 1;

/// The folder of an assembly's motions, inside its folder.
pub const MOTIONS: &str = "motions";

/// Steps when a motion doesn't say.
pub const STEPS: u32 = 11;

/// The most steps a motion takes.
pub const MAX_STEPS: u32 = 1000;

/// Which of a mate's values a mate drive drives.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Driven {
    Angle,
    Distance,
}

impl Driven {
    /// Its key in a file: `angle` or `distance`.
    pub fn key(self) -> &'static str {
        match self {
            Driven::Angle => "angle",
            Driven::Distance => "distance",
        }
    }
}

/// From where to where a drive goes.
#[derive(Clone, Debug, PartialEq)]
pub struct Range {
    pub from: Quantity,
    pub to: Quantity,
}

/// A direction: an axis of the assembly's frame or three numbers, not
/// all 0.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Direction {
    /// 0, 1, 2 for x, y, z.
    Axis(usize),
    Vector([f64; 3]),
}

impl Direction {
    pub fn to_json(&self) -> Value {
        match self {
            Direction::Axis(k) => json!(["x", "y", "z"][*k]),
            Direction::Vector(d) => json!(d.map(super::files::number)),
        }
    }

    /// Made unit.
    pub fn unit(&self) -> [f64; 3] {
        match self {
            Direction::Axis(k) => {
                let mut d = [0.0; 3];
                d[*k] = 1.0;
                d
            }
            Direction::Vector(d) => {
                let n = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
                d.map(|x| x / n)
            }
        }
    }

    fn from_json(v: &Value, at: &str) -> Result<Direction> {
        let bad = || invalid(at, "a direction is x, y, z or three numbers, not all 0");
        match v {
            Value::String(s) => match s.as_str() {
                "x" => Ok(Direction::Axis(0)),
                "y" => Ok(Direction::Axis(1)),
                "z" => Ok(Direction::Axis(2)),
                _ => Err(bad()),
            },
            Value::Array(a) if a.len() == 3 => {
                let d: Vec<f64> = a.iter().filter_map(Value::as_f64).collect();
                match <[f64; 3]>::try_from(d) {
                    Ok(d) if d.iter().all(|x| x.is_finite()) && d.iter().any(|x| *x != 0.0) => {
                        Ok(Direction::Vector(d))
                    }
                    _ => Err(bad()),
                }
            }
            _ => Err(bad()),
        }
    }
}

/// A move drive's turn: about the axis through `origin` along
/// `direction`, from `range.from` to `range.to`.
#[derive(Clone, Debug, PartialEq)]
pub struct Rotate {
    pub origin: [Quantity; 3],
    pub direction: Direction,
    pub range: Range,
}

/// A move drive's slide along `direction`.
#[derive(Clone, Debug, PartialEq)]
pub struct Translate {
    pub direction: Direction,
    pub range: Range,
}

/// One drive. A move drive is the larger, and the common one in a
/// file is a mate drive, but a motion holds a few: not worth a box.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq)]
pub enum Drive {
    Mate {
        mate: Id,
        driven: Driven,
        range: Range,
    },
    Move {
        /// Top-level instance IDs, at least one, each once.
        instances: Vec<Id>,
        rotate: Option<Rotate>,
        translate: Option<Translate>,
    },
}

/// A motion.
#[derive(Clone, Debug, PartialEq)]
pub struct MotionFile {
    pub steps: u32,
    pub drives: Vec<Drive>,
}

/// A motion's ID: a part ID.
pub fn check_id(text: &str) -> Result<Id> {
    Id::parse(text, "motion")
}

/// Where motion `id` of an assembly lives, relative to its folder.
pub fn motion_file(id: &Id) -> String {
    format!("{MOTIONS}/{id}.json")
}

fn range_json(r: &Range) -> Value {
    json!({"from": r.from.text(), "to": r.to.text()})
}

/// `from` and `to` of `o` for `field`, pointing at `at/from` and `at/to`.
fn range(o: &mut Obj<'_>, at: &str, field: Field, env: &Env) -> Result<Range> {
    let lookup = |n: &str| env.get(n);
    let mut q = |k: &str| -> Result<Quantity> {
        Quantity::from_json_as(&o.take(k)?, field, &lookup)
            .map_err(|e| e.with("pointer", format!("{at}/{k}")))
    };
    let from = q("from")?;
    let to = q("to")?;
    Ok(Range { from, to })
}

impl Drive {
    pub fn to_json(&self) -> Value {
        match self {
            Drive::Mate {
                mate,
                driven,
                range,
            } => {
                let mut v = json!({"mate": mate});
                v[driven.key()] = range_json(range);
                v
            }
            Drive::Move {
                instances,
                rotate,
                translate,
            } => {
                let mut v = json!({"instances": instances});
                if let Some(r) = rotate {
                    let mut j = range_json(&r.range);
                    j["axis"] = json!({
                        "origin": r.origin.iter().map(Quantity::text).collect::<Vec<_>>(),
                        "direction": r.direction.to_json(),
                    });
                    v["rotate"] = j;
                }
                if let Some(t) = translate {
                    let mut j = range_json(&t.range);
                    j["direction"] = t.direction.to_json();
                    v["translate"] = j;
                }
                v
            }
        }
    }

    fn from_json(v: &Value, at: &str, default: Unit, env: &Env) -> Result<Drive> {
        let mut o = Obj::new(v, at)?;
        if let Some(m) = o.take_opt("mate") {
            let here = format!("{at}/mate");
            let mate = m
                .as_str()
                .ok_or_else(|| invalid(&here, "a mate drive names its mate's ID"))
                .and_then(|s| Id::parse(s, "mate").map_err(|e| e.with("pointer", here.clone())))?;
            let (driven, r) = match (o.take_opt("angle"), o.take_opt("distance")) {
                (Some(r), None) => (Driven::Angle, r),
                (None, Some(r)) => (Driven::Distance, r),
                _ => return Err(invalid(at, "a mate drive drives its angle or its distance")),
            };
            o.finish()?;
            let here = format!("{at}/{}", driven.key());
            let field = match driven {
                Driven::Angle => Field::Angle,
                Driven::Distance => Field::Length(default),
            };
            let mut ro = Obj::new(&r, &here)?;
            let range = range(&mut ro, &here, field, env)?;
            ro.finish()?;
            return Ok(Drive::Mate {
                mate,
                driven,
                range,
            });
        }
        let here = format!("{at}/instances");
        let list = o.take("instances")?;
        let list = list.as_array().filter(|a| !a.is_empty()).ok_or_else(|| {
            invalid(
                &here,
                "a drive names a mate, or a list of one or more instances",
            )
        })?;
        let mut instances: Vec<Id> = Vec::with_capacity(list.len());
        for (i, p) in list.iter().enumerate() {
            let at = format!("{here}/{i}");
            let id = p
                .as_str()
                .ok_or_else(|| invalid(&at, "an instance is named by its ID"))
                .and_then(|s| {
                    Id::parse(s, "instance").map_err(|e| {
                        e.with("pointer", at.clone()).with(
                            "hint",
                            "a move drive moves top-level instances: a subassembly as one, a copy with its seed",
                        )
                    })
                })?;
            if instances.contains(&id) {
                return Err(invalid(&at, &format!("{id} is listed twice")));
            }
            instances.push(id);
        }
        let rotate = match o.take_opt("rotate") {
            None => None,
            Some(r) => {
                let here = format!("{at}/rotate");
                let mut ro = Obj::new(&r, &here)?;
                let axis = ro.take("axis")?;
                let ahere = format!("{here}/axis");
                let mut ao = Obj::new(&axis, &ahere)?;
                let origin = ao.take("origin")?;
                let ohere = format!("{ahere}/origin");
                let origin = origin
                    .as_array()
                    .filter(|a| a.len() == 3)
                    .ok_or_else(|| invalid(&ohere, "an axis's origin is three lengths"))?;
                let lookup = |n: &str| env.get(n);
                let mut xs = Vec::with_capacity(3);
                for (i, x) in origin.iter().enumerate() {
                    xs.push(
                        Quantity::from_json_in(x, default, &lookup)
                            .map_err(|e| e.with("pointer", format!("{ohere}/{i}")))?,
                    );
                }
                let [x, y, z]: [Quantity; 3] = xs.try_into().expect("three");
                let direction =
                    Direction::from_json(&ao.take("direction")?, &format!("{ahere}/direction"))?;
                ao.finish()?;
                let range = range(&mut ro, &here, Field::Angle, env)?;
                ro.finish()?;
                Some(Rotate {
                    origin: [x, y, z],
                    direction,
                    range,
                })
            }
        };
        let translate = match o.take_opt("translate") {
            None => None,
            Some(t) => {
                let here = format!("{at}/translate");
                let mut to = Obj::new(&t, &here)?;
                let direction =
                    Direction::from_json(&to.take("direction")?, &format!("{here}/direction"))?;
                let range = range(&mut to, &here, Field::Length(default), env)?;
                to.finish()?;
                Some(Translate { direction, range })
            }
        };
        o.finish()?;
        if rotate.is_none() && translate.is_none() {
            return Err(invalid(at, "a move drive turns, moves or both"));
        }
        Ok(Drive::Move {
            instances,
            rotate,
            translate,
        })
    }
}

impl MotionFile {
    pub fn to_json(&self) -> Value {
        json!({
            "format": FORMAT,
            "kind": KIND,
            "steps": self.steps,
            "drives": self.drives.iter().map(Drive::to_json).collect::<Vec<_>>(),
        })
    }

    /// Reads a motion: its steps and one or more drives, lengths (bare
    /// numbers in `default`) and angles as expressions over `env`, the
    /// assembly's parameters. An instance in two move drives, or one mate
    /// value driven twice, is refused at the second.
    pub fn from_json(v: &Value, default: Unit, env: &Env) -> Result<MotionFile> {
        let mut o = header(v, KIND, FORMAT)?;
        let steps = match o.take_opt("steps") {
            None => STEPS,
            Some(s) => s
                .as_u64()
                .filter(|n| (2..=u64::from(MAX_STEPS)).contains(n))
                .ok_or_else(|| invalid("/steps", "steps is a whole number from 2 to 1000"))?
                as u32,
        };
        let drives = o.take("drives")?;
        let drives = drives
            .as_array()
            .filter(|a| !a.is_empty())
            .ok_or_else(|| invalid("/drives", "a motion is a list of one or more drives"))?;
        let drives = drives
            .iter()
            .enumerate()
            .map(|(i, d)| Drive::from_json(d, &format!("/drives/{i}"), default, env))
            .collect::<Result<Vec<_>>>()?;
        o.finish()?;
        let mut moved: BTreeSet<&Id> = BTreeSet::new();
        let mut driven: BTreeSet<(&Id, Driven)> = BTreeSet::new();
        for (i, d) in drives.iter().enumerate() {
            match d {
                Drive::Mate {
                    mate, driven: v, ..
                } => {
                    if !driven.insert((mate, *v)) {
                        return Err(invalid(
                            &format!("/drives/{i}"),
                            &format!("{mate}'s {} is driven twice", v.key()),
                        ));
                    }
                }
                Drive::Move { instances, .. } => {
                    for (j, inst) in instances.iter().enumerate() {
                        if !moved.insert(inst) {
                            return Err(invalid(
                                &format!("/drives/{i}/instances/{j}"),
                                &format!("{inst} is moved by an earlier drive"),
                            ));
                        }
                    }
                }
            }
        }
        Ok(MotionFile { steps, drives })
    }
}

/// The expression strings a motion's JSON holds, as it is written (each
/// range's ends and each axis's origin), to read their names or rename
/// them.
pub fn expression_strings(v: &mut Value) -> Vec<&mut String> {
    let mut out = Vec::new();
    let Some(Value::Array(drives)) = v.get_mut("drives") else {
        return out;
    };
    fn ends<'a>(o: &'a mut serde_json::Map<String, Value>, out: &mut Vec<&'a mut String>) {
        for (k, x) in o.iter_mut() {
            match (k.as_str(), x) {
                ("from" | "to", Value::String(s)) => out.push(s),
                ("axis", Value::Object(a)) => {
                    if let Some(Value::Array(origin)) = a.get_mut("origin") {
                        out.extend(origin.iter_mut().filter_map(|x| match x {
                            Value::String(s) => Some(s),
                            _ => None,
                        }));
                    }
                }
                _ => {}
            }
        }
    }
    for d in drives {
        let Some(d) = d.as_object_mut() else {
            continue;
        };
        for (k, x) in d.iter_mut() {
            if let ("angle" | "distance" | "rotate" | "translate", Value::Object(o)) =
                (k.as_str(), x)
            {
                ends(o, &mut out);
            }
        }
    }
    out
}

/// The parameters a motion's JSON names.
pub fn names(v: &Value) -> BTreeSet<String> {
    let mut v = v.clone();
    expression_strings(&mut v)
        .into_iter()
        .filter_map(|s| crate::expr::Expr::parse(s).ok())
        .flat_map(|e| e.names())
        .collect()
}

/// `wrong_part_kind` for a motion in a part that isn't an assembly.
pub fn not_an_assembly(part: &str) -> Error {
    Error::new(
        Code::WrongPartKind,
        format!("{part} is a part: only an assembly has motions"),
    )
    .with("part", part)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn motion() -> Value {
        json!({"format": 1, "kind": KIND, "steps": 13, "drives": [
            {"mate": "hinge", "angle": {"from": "-6 deg", "to": "6 deg"}},
            {"mate": "slide", "distance": {"from": "0 mm", "to": "2 * gap"}},
            {"instances": ["p1", "p2"],
             "rotate": {"axis": {"origin": ["0 mm", "0 mm", "gap"], "direction": "z"},
                        "from": "0 deg", "to": "90 deg"},
             "translate": {"direction": [1, 0, 0], "from": "0 mm", "to": "10 mm"}}
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

    fn read(v: &Value) -> Result<MotionFile> {
        MotionFile::from_json(v, Unit::Mm, &env())
    }

    #[test]
    fn a_motion_reads_and_writes_back() {
        let m = read(&motion()).unwrap();
        assert_eq!(m.steps, 13);
        assert_eq!(m.drives.len(), 3);
        let Drive::Mate {
            mate,
            driven,
            range,
        } = &m.drives[1]
        else {
            panic!()
        };
        assert_eq!((mate.as_str(), *driven), ("slide", Driven::Distance));
        assert_eq!(range.to.meters(), 0.01);
        assert_eq!(range.to.text(), "2 * gap");
        let Drive::Move {
            instances,
            rotate,
            translate,
        } = &m.drives[2]
        else {
            panic!()
        };
        assert_eq!(instances.len(), 2);
        let r = rotate.as_ref().unwrap();
        assert_eq!(r.origin[2].meters(), 0.005);
        assert_eq!(r.direction.unit(), [0.0, 0.0, 1.0]);
        assert!((r.range.to.radians() - core::f64::consts::FRAC_PI_2).abs() < 1e-15);
        assert_eq!(
            translate.as_ref().unwrap().direction.unit(),
            [1.0, 0.0, 0.0]
        );
        assert_eq!(m.to_json(), motion());
        // Steps left out are 11, written so.
        let mut bare = motion();
        bare.as_object_mut().unwrap().remove("steps");
        assert_eq!(read(&bare).unwrap().steps, STEPS);
        assert_eq!(read(&bare).unwrap().to_json()["steps"], 11);
        assert_eq!(names(&motion()).into_iter().collect::<Vec<_>>(), ["gap"]);
    }

    #[test]
    fn bad_motions_say_where() {
        let at = |f: &dyn Fn(&mut Value)| {
            let mut v = motion();
            f(&mut v);
            let e = read(&v).unwrap_err();
            e.details["pointer"].as_str().unwrap_or("").to_owned()
        };
        assert_eq!(at(&|v| v["steps"] = json!(1)), "/steps");
        assert_eq!(at(&|v| v["steps"] = json!(1001)), "/steps");
        assert_eq!(at(&|v| v["drives"] = json!([])), "/drives");
        assert_eq!(
            at(&|v| v["drives"][0] = json!({"mate": "hinge"})),
            "/drives/0"
        );
        assert_eq!(
            at(&|v| v["drives"][0]["distance"] = json!({"from": 0, "to": 1})),
            "/drives/0"
        );
        assert_eq!(
            at(&|v| v["drives"][0]["angle"]["to"] = json!("5 mm")),
            "/drives/0/angle/to"
        );
        assert_eq!(
            at(&|v| v["drives"][0]["mate"] = json!("Hinge")),
            "/drives/0/mate"
        );
        assert_eq!(
            at(&|v| v["drives"][2]["instances"] = json!(["p1/left"])),
            "/drives/2/instances/0"
        );
        assert_eq!(
            at(&|v| v["drives"][2]["instances"] = json!(["ring:2"])),
            "/drives/2/instances/0"
        );
        assert_eq!(
            at(&|v| v["drives"][2]["instances"] = json!(["p1", "p1"])),
            "/drives/2/instances/1"
        );
        assert_eq!(
            at(&|v| v["drives"][2]["rotate"]["axis"]["direction"] = json!([0, 0, 0])),
            "/drives/2/rotate/axis/direction"
        );
        assert_eq!(
            at(&|v| v["drives"][2]["rotate"]["axis"]["origin"] = json!([0, 0])),
            "/drives/2/rotate/axis/origin"
        );
        assert_eq!(
            at(&|v| {
                let d = v["drives"][2].as_object_mut().unwrap();
                d.remove("rotate");
                d.remove("translate");
            }),
            "/drives/2"
        );
        // Moved twice, driven twice: refused at the second.
        assert_eq!(
            at(&|v| v["drives"]
                .as_array_mut()
                .unwrap()
                .push(json!({"instances": ["p2"], "translate":
                    {"direction": "x", "from": 0, "to": 1}}))),
            "/drives/3/instances/0"
        );
        assert_eq!(
            at(&|v| v["drives"]
                .as_array_mut()
                .unwrap()
                .push(json!({"mate": "hinge", "angle": {"from": 0, "to": 1}}))),
            "/drives/3"
        );
        assert_eq!(at(&|v| v["drives"][0]["spin"] = json!(1)), "/drives/0/spin");
        let mut newer = motion();
        newer["format"] = json!(2);
        assert_eq!(read(&newer).unwrap_err().code, Code::FormatTooNew);
    }

    #[test]
    fn ids_and_files() {
        assert_eq!(check_id("layer-x1").unwrap().as_str(), "layer-x1");
        assert_eq!(check_id("Layer").unwrap_err().code, Code::InvalidId);
        assert_eq!(
            motion_file(&check_id("swing").unwrap()),
            "motions/swing.json"
        );
        assert_eq!(not_an_assembly("bolt").code, Code::WrongPartKind);
    }

    #[test]
    fn expressions_are_found_where_they_are_written() {
        let mut v = motion();
        let found: Vec<String> = expression_strings(&mut v)
            .into_iter()
            .map(|s| s.clone())
            .collect();
        assert_eq!(
            found,
            [
                "-6 deg", "6 deg", "0 mm", "2 * gap", "0 mm", "0 mm", "gap", "0 deg", "90 deg",
                "0 mm", "10 mm"
            ]
        );
    }
}
