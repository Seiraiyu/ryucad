//! An assembly's features. An `instance` places a part (or another
//! assembly) of the project, at the revision it names, where its pose
//! puts the part's origin frame.
//!
//! A pose is solver state, as a sketch's positions are: f64 meters and
//! unit-ish axes, written as they are held, with `-0` written as `0`, so
//! a write and a read give the same bits.

use super::files::{Obj, invalid, number};
use super::id::Id;
use super::sketch::clean;
use crate::error::Result;
use serde_json::{Value, json};

/// Which revision of a part an instance places: the part's files as they
/// are (`working`), or a release by its label.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Rev {
    Working,
    Label(String),
}

impl Rev {
    pub fn to_json(&self) -> Value {
        match self {
            Rev::Working => json!("working"),
            Rev::Label(l) => json!({"label": l}),
        }
    }

    /// `"working"` or `{"label": L}`; `at` is its JSON pointer.
    pub fn from_json(v: &Value, at: &str) -> Result<Rev> {
        if v.as_str() == Some("working") {
            return Ok(Rev::Working);
        }
        let mut o = Obj::new(v, at).map_err(|_| {
            invalid(
                at,
                "rev is \"working\" or {\"label\": L} with a release label",
            )
        })?;
        let label = o.take("label")?;
        let label = label
            .as_str()
            .ok_or_else(|| invalid(&format!("{at}/label"), "a label is a string"))?;
        let label = label_text(label).map_err(|e| e.with("pointer", format!("{at}/label")))?;
        o.finish()?;
        Ok(Rev::Label(label))
    }

    /// `working`, or `label B`: as people read it.
    pub fn text(&self) -> String {
        match self {
            Rev::Working => "working".to_owned(),
            Rev::Label(l) => format!("label {l}"),
        }
    }
}

/// A label as a file may give it: 1 to 12 ASCII capital letters and
/// digits. Whether the project's scheme writes it is the release
/// records' business.
pub fn label_text(s: &str) -> Result<String> {
    if s.is_empty()
        || s.len() > 12
        || !s
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
    {
        return Err(invalid(
            "",
            &format!("{s:?} isn't a release label: 1 to 12 capital letters and digits"),
        ));
    }
    Ok(s.to_owned())
}

/// Where an instance puts its part's origin frame: an origin in meters
/// and axes `z` and `x` (`x` made perpendicular to `z` when it is built).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    pub origin_m: [f64; 3],
    pub z: [f64; 3],
    pub x: [f64; 3],
}

/// How far from 0 or ±1 an axis's component may be and still be written
/// as that: a few units in the last place of 1, which is what turning an
/// axis by a quarter turn leaves (`cos(π/2)` is about `6.1e-17`).
const SNAP: f64 = 4.0 * f64::EPSILON;

/// An axis's component as a pose is written: within rounding of 0 or ±1
/// it is that exactly (issue #108), and `-0` is `0` as for any position.
pub fn snap_axis(x: f64) -> f64 {
    for to in [0.0, 1.0, -1.0] {
        if (x - to).abs() <= SNAP {
            return to;
        }
    }
    clean(x)
}

impl Pose {
    /// The assembly's own frame.
    pub const IDENTITY: Pose = Pose {
        origin_m: [0.0, 0.0, 0.0],
        z: [0.0, 0.0, 1.0],
        x: [1.0, 0.0, 0.0],
    };

    /// The pose as it is written: its axes' components snapped
    /// ([`snap_axis`]), its origin's `-0` made `0`.
    pub fn snapped(self) -> Pose {
        Pose {
            origin_m: self.origin_m.map(clean),
            z: self.z.map(snap_axis),
            x: self.x.map(snap_axis),
        }
    }

    pub fn to_json(&self) -> Value {
        json!({
            "origin_m": self.origin_m.map(|x| number(clean(x))),
            "x": self.x.map(|x| number(clean(x))),
            "z": self.z.map(|x| number(clean(x))),
        })
    }

    pub fn from_json(v: &Value, at: &str) -> Result<Pose> {
        let mut o = Obj::new(v, at)?;
        let three = |o: &mut Obj, key: &str| -> Result<[f64; 3]> {
            let here = format!("{at}/{key}");
            let v = o.take(key)?;
            let a = v
                .as_array()
                .filter(|a| a.len() == 3)
                .ok_or_else(|| invalid(&here, "expected three numbers"))?;
            let mut out = [0.0; 3];
            for (i, x) in a.iter().enumerate() {
                out[i] = clean(
                    x.as_f64()
                        .ok_or_else(|| invalid(&format!("{here}/{i}"), "expected a number"))?,
                );
            }
            Ok(out)
        };
        let origin_m = three(&mut o, "origin_m")?;
        let z = three(&mut o, "z")?;
        let x = three(&mut o, "x")?;
        o.finish()?;
        Ok(Pose { origin_m, z, x })
    }
}

/// An `instance` feature: `part` at `rev`, placed by `pose`; `fixed`
/// holds it where it is when mates are solved.
#[derive(Clone, Debug, PartialEq)]
pub struct Instance {
    pub part: Id,
    pub rev: Rev,
    pub pose: Pose,
    pub fixed: bool,
}

impl Instance {
    /// Its fields in a feature file, besides the header; `fixed` is
    /// written only when true.
    pub fn fields(&self) -> Value {
        let mut v = json!({"part": self.part, "pose": self.pose.to_json(),
                           "rev": self.rev.to_json()});
        if self.fixed {
            v["fixed"] = json!(true);
        }
        v
    }

    #[doc(hidden)]
    pub fn from_fields(o: &mut Obj) -> Result<Instance> {
        let part = o.take("part")?;
        let part = part
            .as_str()
            .ok_or_else(|| invalid("/part", "part is a part's ID"))?;
        let part = Id::parse(part, "part").map_err(|e| e.with("pointer", "/part"))?;
        let rev = Rev::from_json(&o.take("rev")?, "/rev")?;
        let pose = Pose::from_json(&o.take("pose")?, "/pose")?;
        let fixed = match o.take_opt("fixed") {
            None => false,
            Some(Value::Bool(b)) => b,
            Some(_) => return Err(invalid("/fixed", "fixed is true or false")),
        };
        Ok(Instance {
            part,
            rev,
            pose,
            fixed,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::files::{Feature, feature_format};
    use crate::error::Code;
    use crate::expr::Env;
    use crate::units::Unit;

    fn read(v: Value) -> Result<Feature> {
        Feature::from_json(&v, Unit::Mm, &Env::empty())
    }

    fn instance(rev: Value) -> Value {
        json!({"format": 4, "kind": "ryucad.feature", "type": "instance", "part": "bracket",
               "rev": rev, "pose": {"origin_m": [0.01, 0, -0.0], "z": [0, 0, 1], "x": [1, 0, 0]}})
    }

    #[test]
    fn instances_read_and_write() {
        let f = read(instance(json!("working"))).unwrap();
        let Feature::Instance(i) = &f else {
            panic!("an instance")
        };
        assert_eq!(i.rev, Rev::Working);
        assert!(!i.fixed);
        // -0 is read as 0, so it is written as 0.
        assert_eq!(i.pose.origin_m[2].to_bits(), 0.0f64.to_bits());
        let v = f.to_json();
        let mut written = instance(json!("working"));
        written["pose"]["origin_m"][2] = json!(0);
        assert_eq!(v, written);
        assert_eq!(f.format(), 4);
        assert_eq!(feature_format(&v), 4);
        assert_eq!(read(v).unwrap(), f);
        let pinned = read(instance(json!({"label": "B"}))).unwrap();
        let Feature::Instance(i) = &pinned else {
            panic!("an instance")
        };
        assert_eq!(i.rev, Rev::Label("B".into()));
        assert_eq!(i.rev.text(), "label B");
        let mut fixed = instance(json!("working"));
        fixed["fixed"] = json!(true);
        let f = read(fixed.clone()).unwrap();
        assert_eq!(f.to_json()["fixed"], true);
    }

    #[test]
    fn instances_are_checked() {
        for (rev, pointer) in [
            (json!("head"), "/rev"),
            (json!({"label": "b"}), "/rev/label"),
            (json!({"label": ""}), "/rev/label"),
            (json!({"label": "ABCDEFGHJKLMN"}), "/rev/label"),
            (json!({"label": "B", "x": 1}), "/rev/x"),
        ] {
            let e = read(instance(rev.clone())).unwrap_err();
            assert_eq!(e.code, Code::InvalidDocument, "{rev}");
            assert_eq!(e.details["pointer"], pointer, "{rev}");
        }
        let mut v = instance(json!("working"));
        v["pose"]["z"] = json!([0, 1]);
        assert_eq!(read(v).unwrap_err().details["pointer"], "/pose/z");
        let mut v = instance(json!("working"));
        v["part"] = json!("Bracket");
        assert_eq!(read(v).unwrap_err().code, Code::InvalidId);
        let mut v = instance(json!("working"));
        v["fixed"] = json!(1);
        assert_eq!(read(v).unwrap_err().details["pointer"], "/fixed");
    }

    #[test]
    fn axes_within_rounding_of_0_or_1_are_written_as_that() {
        let p = Pose {
            origin_m: [0.026000000000000002, -0.0, 1e-17],
            z: [0.0, -1.0 + 2.0 * f64::EPSILON, 6.123233995736766e-17],
            x: [-6.123233995736766e-17, 0.9999999999999999, 1e-9],
        };
        let q = p.snapped();
        // Origins are positions: kept as they are, `-0` aside.
        assert_eq!(q.origin_m, [0.026000000000000002, 0.0, 1e-17]);
        assert_eq!(q.z, [0.0, -1.0, 0.0]);
        assert_eq!(q.x, [0.0, 1.0, 1e-9]);
        assert_eq!(q.z[0].to_bits(), 0.0f64.to_bits());
        assert_eq!(Pose::IDENTITY.snapped(), Pose::IDENTITY);
    }
}
