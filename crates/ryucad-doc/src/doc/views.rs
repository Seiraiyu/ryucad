//! Named views: `parts/<part>/views/<id>.json`, a camera saved with a
//! part or an assembly, in its own frame, so anyone checking out a commit
//! can look at the model as its author meant. Nothing a rebuild makes
//! reads them. Positions are viewer state, as a sketch's are: f64 meters,
//! `-0` written as `0`.
//!
//! ```json
//! {
//!   "eye_m": [0.3, -0.3, 0.3],
//!   "format": 1,
//!   "fov_deg": 30,
//!   "kind": "ryucad.view",
//!   "projection": "perspective",
//!   "target_m": [0, 0, 0.01],
//!   "up": [0, 0, 1]
//! }
//! ```
//!
//! An orthographic view has `"projection": "orthographic"` and `height_m`
//! (how tall the view is at the target) in place of `fov_deg`.
//!
//! Format 2 adds what the view cuts and explodes, written only when it
//! has either, so a plain view stays format 1: `sections`, up to three
//! planes (`origin_m` and a unit `normal` towards the side cut away), and
//! an assembly's `explode` (`{"id", "fraction"}`).

use super::files::{Obj, header, invalid, number};
use super::id::Id;
use super::sketch::clean;
use crate::error::{Code, Error, Result};
use serde_json::{Value, json};

pub const KIND: &str = "ryucad.view";

/// The newest format this build reads.
pub const FORMAT: u64 = 2;

/// The most section planes a view keeps.
pub const MAX_SECTIONS: usize = 3;

/// The folder of a part's named views, inside the part's folder.
pub const VIEWS: &str = "views";

/// The names of the standard views (`ryucad::view::STANDARD_VIEWS`): a
/// named view never takes one, so a name means one view.
pub const RESERVED: [&str; 29] = [
    "front",
    "back",
    "left",
    "right",
    "top",
    "bottom",
    "front_top",
    "front_bottom",
    "front_left",
    "front_right",
    "back_top",
    "back_bottom",
    "back_left",
    "back_right",
    "top_left",
    "top_right",
    "bottom_left",
    "bottom_right",
    "front_top_left",
    "front_top_right",
    "front_bottom_left",
    "front_bottom_right",
    "back_top_left",
    "back_top_right",
    "back_bottom_left",
    "back_bottom_right",
    "iso",
    "dimetric",
    "trimetric",
];

/// How a named view projects.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ViewProjection {
    /// `fov_deg` is the vertical field of view, in `(0, 180)`.
    Perspective { fov_deg: f64 },
    /// `height_m` is the view's height at the target, positive.
    Orthographic { height_m: f64 },
}

/// A section plane a view keeps: what lies on the side `normal` points
/// to is cut away.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewSection {
    pub origin_m: [f64; 3],
    pub normal: [f64; 3],
}

/// An assembly's explode a view shows, and how far through it.
#[derive(Clone, Debug, PartialEq)]
pub struct ViewExplode {
    pub id: Id,
    pub fraction: f64,
}

/// A named view: where the eye is, what it looks at and which way is up,
/// in the part's (or assembly's) frame, and what it cuts and explodes.
#[derive(Clone, Debug, PartialEq)]
pub struct ViewFile {
    pub eye_m: [f64; 3],
    pub target_m: [f64; 3],
    pub up: [f64; 3],
    pub projection: ViewProjection,
    pub sections: Vec<ViewSection>,
    pub explode: Option<ViewExplode>,
}

/// A view's ID: a part ID that isn't a standard view's name.
pub fn check_id(text: &str) -> Result<Id> {
    let id = Id::parse(text, "view")?;
    if RESERVED.contains(&text) {
        return Err(Error::new(
            Code::InvalidId,
            format!("{text} is a standard view's name: give the view another"),
        )
        .with("id", text));
    }
    Ok(id)
}

impl ViewFile {
    pub fn to_json(&self) -> Value {
        let three = |a: [f64; 3]| a.map(|x| number(clean(x)));
        let extras = !self.sections.is_empty() || self.explode.is_some();
        let mut v = json!({
            "eye_m": three(self.eye_m),
            "format": if extras { FORMAT } else { 1 },
            "kind": KIND,
            "target_m": three(self.target_m),
            "up": three(self.up),
        });
        match self.projection {
            ViewProjection::Perspective { fov_deg } => {
                v["projection"] = json!("perspective");
                v["fov_deg"] = number(fov_deg);
            }
            ViewProjection::Orthographic { height_m } => {
                v["projection"] = json!("orthographic");
                v["height_m"] = number(height_m);
            }
        }
        if !self.sections.is_empty() {
            v["sections"] = json!(
                self.sections
                    .iter()
                    .map(|s| json!({"origin_m": three(s.origin_m), "normal": three(s.normal)}))
                    .collect::<Vec<_>>()
            );
        }
        if let Some(e) = &self.explode {
            v["explode"] = json!({"id": e.id, "fraction": number(e.fraction)});
        }
        v
    }

    /// Reads a view: finite points, an eye apart from the target, an up
    /// not along the view, a field of view in `(0, 180)` degrees or a
    /// positive height.
    pub fn from_json(v: &Value) -> Result<ViewFile> {
        let mut o = header(v, KIND, FORMAT)?;
        let three = |o: &mut Obj, key: &str| -> Result<[f64; 3]> {
            let at = format!("/{key}");
            let v = o.take(key)?;
            let a = v
                .as_array()
                .filter(|a| a.len() == 3)
                .ok_or_else(|| invalid(&at, "expected three numbers"))?;
            let mut out = [0.0; 3];
            for (i, x) in a.iter().enumerate() {
                out[i] = clean(
                    x.as_f64()
                        .ok_or_else(|| invalid(&format!("{at}/{i}"), "expected a number"))?,
                );
            }
            Ok(out)
        };
        let eye_m = three(&mut o, "eye_m")?;
        let target_m = three(&mut o, "target_m")?;
        let up = three(&mut o, "up")?;
        let positive = |o: &mut Obj, key: &str| -> Result<f64> {
            o.take(key)?
                .as_f64()
                .filter(|x| *x > 0.0)
                .ok_or_else(|| invalid(&format!("/{key}"), "expected a positive number"))
        };
        let projection = match o.take("projection")?.as_str() {
            Some("perspective") => {
                let fov_deg = positive(&mut o, "fov_deg")?;
                if fov_deg >= 180.0 {
                    return Err(invalid(
                        "/fov_deg",
                        "the field of view must be under 180 degrees",
                    ));
                }
                ViewProjection::Perspective { fov_deg }
            }
            Some("orthographic") => ViewProjection::Orthographic {
                height_m: positive(&mut o, "height_m")?,
            },
            _ => {
                return Err(invalid(
                    "/projection",
                    "expected \"perspective\" or \"orthographic\"",
                ));
            }
        };
        let sections = match o.take_opt("sections") {
            None => Vec::new(),
            Some(list) => {
                let list = list
                    .as_array()
                    .filter(|a| !a.is_empty() && a.len() <= MAX_SECTIONS)
                    .ok_or_else(|| {
                        invalid("/sections", "a view keeps one to three section planes")
                    })?
                    .clone();
                let mut out = Vec::with_capacity(list.len());
                for (i, s) in list.iter().enumerate() {
                    let at = format!("/sections/{i}");
                    let mut so = Obj::new(s, &at)?;
                    let origin_m = three(&mut so, "origin_m").map_err(|e| {
                        let p = e.details["pointer"].as_str().unwrap_or("").to_owned();
                        e.with("pointer", format!("{at}{p}"))
                    })?;
                    let normal = three(&mut so, "normal").map_err(|e| {
                        let p = e.details["pointer"].as_str().unwrap_or("").to_owned();
                        e.with("pointer", format!("{at}{p}"))
                    })?;
                    so.finish()?;
                    let n = normal.iter().map(|x| x * x).sum::<f64>().sqrt();
                    if (n - 1.0).abs() > 1e-9 {
                        return Err(invalid(
                            &format!("{at}/normal"),
                            "a section's normal is unit length",
                        ));
                    }
                    out.push(ViewSection { origin_m, normal });
                }
                out
            }
        };
        let explode = match o.take_opt("explode") {
            None => None,
            Some(e) => {
                let mut eo = Obj::new(&e, "/explode")?;
                let id = eo
                    .take("id")?
                    .as_str()
                    .and_then(|s| Id::parse(s, "explode").ok())
                    .ok_or_else(|| invalid("/explode/id", "an explode's ID"))?;
                let fraction = eo
                    .take("fraction")?
                    .as_f64()
                    .filter(|f| (0.0..=1.0).contains(f))
                    .ok_or_else(|| invalid("/explode/fraction", "a fraction from 0 to 1"))?;
                eo.finish()?;
                Some(ViewExplode { id, fraction })
            }
        };
        o.finish()?;
        let d = [0, 1, 2].map(|k| target_m[k] - eye_m[k]);
        if d == [0.0; 3] {
            return Err(invalid("/target_m", "the eye and the target must differ"));
        }
        let c = [
            d[1] * up[2] - d[2] * up[1],
            d[2] * up[0] - d[0] * up[2],
            d[0] * up[1] - d[1] * up[0],
        ];
        if c == [0.0; 3] {
            return Err(invalid(
                "/up",
                "up must be non-zero and not along the view direction",
            ));
        }
        Ok(ViewFile {
            eye_m,
            target_m,
            up,
            projection,
            sections,
            explode,
        })
    }
}

/// Where view `id` of a part lives, relative to the part's folder.
pub fn view_file(id: &Id) -> String {
    format!("{VIEWS}/{id}.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view() -> Value {
        json!({"eye_m": [0.3, -0.3, 0.3], "format": 1, "fov_deg": 30,
               "kind": KIND, "projection": "perspective",
               "target_m": [0, 0, 0.01], "up": [0, 0, 1]})
    }

    #[test]
    fn a_view_reads_and_writes_back() {
        let v = ViewFile::from_json(&view()).unwrap();
        assert_eq!(v.projection, ViewProjection::Perspective { fov_deg: 30.0 });
        assert_eq!(v.target_m, [0.0, 0.0, 0.01]);
        assert_eq!(v.to_json(), view());
        let mut ortho = view();
        let o = ortho.as_object_mut().unwrap();
        o.remove("fov_deg");
        o.insert("projection".into(), json!("orthographic"));
        o.insert("height_m".into(), json!(0.125));
        let read = ViewFile::from_json(&ortho).unwrap();
        assert_eq!(
            read.projection,
            ViewProjection::Orthographic { height_m: 0.125 }
        );
        assert_eq!(read.to_json(), ortho);
        // -0 is written 0, and reads back with its sign gone.
        let mut neg = view();
        neg["eye_m"][1] = json!(-0.0);
        let n = ViewFile::from_json(&neg).unwrap();
        assert!(n.eye_m[1].is_sign_positive());
        assert_eq!(n.to_json()["eye_m"][1], json!(0));
    }

    #[test]
    fn bad_views_say_where() {
        let at = |f: &dyn Fn(&mut Value)| {
            let mut v = view();
            f(&mut v);
            let e = ViewFile::from_json(&v).unwrap_err();
            (
                e.code,
                e.details["pointer"].as_str().unwrap_or("").to_owned(),
            )
        };
        let bad = |p: &str| (Code::InvalidDocument, p.to_owned());
        assert_eq!(at(&|v| v["eye_m"] = json!([0, 0])), bad("/eye_m"));
        assert_eq!(at(&|v| v["up"][2] = json!("z")), bad("/up/2"));
        assert_eq!(
            at(&|v| v["target_m"] = v["eye_m"].clone()),
            bad("/target_m")
        );
        assert_eq!(
            at(&|v| {
                v["target_m"] = json!([0, 0, 0]);
                v["up"] = json!([0.6, -0.6, 0.6]);
            }),
            bad("/up")
        );
        assert_eq!(at(&|v| v["fov_deg"] = json!(180)), bad("/fov_deg"));
        assert_eq!(at(&|v| v["fov_deg"] = json!(0)), bad("/fov_deg"));
        assert_eq!(
            at(&|v| v["projection"] = json!("fisheye")),
            bad("/projection")
        );
        assert_eq!(at(&|v| v["zoom"] = json!(2)), bad("/zoom"));
        let mut newer = view();
        newer["format"] = json!(3);
        assert_eq!(
            ViewFile::from_json(&newer).unwrap_err().code,
            Code::FormatTooNew
        );
    }

    #[test]
    fn a_view_with_sections_and_an_explode_is_format_2() {
        let mut v = view();
        v["format"] = json!(2);
        v["sections"] = json!([{"normal": [0, 0, 1], "origin_m": [0, 0, 0.005]}]);
        v["explode"] = json!({"fraction": 0.5, "id": "apart"});
        let read = ViewFile::from_json(&v).unwrap();
        assert_eq!(
            read.sections,
            [ViewSection {
                origin_m: [0.0, 0.0, 0.005],
                normal: [0.0, 0.0, 1.0]
            }]
        );
        assert_eq!(read.explode.as_ref().unwrap().fraction, 0.5);
        assert_eq!(read.to_json(), v);
        // Without them, format 1 again.
        let plain = ViewFile {
            sections: Vec::new(),
            explode: None,
            ..read.clone()
        };
        assert_eq!(plain.to_json(), view());
        let at = |f: &dyn Fn(&mut Value)| {
            let mut w = v.clone();
            f(&mut w);
            ViewFile::from_json(&w).unwrap_err().details["pointer"]
                .as_str()
                .unwrap_or("")
                .to_owned()
        };
        assert_eq!(at(&|w| w["sections"] = json!([])), "/sections");
        assert_eq!(
            at(&|w| w["sections"][0]["normal"] = json!([0, 0, 2])),
            "/sections/0/normal"
        );
        assert_eq!(
            at(&|w| w["sections"][0]["origin_m"] = json!([0, 0])),
            "/sections/0/origin_m"
        );
        assert_eq!(
            at(&|w| w["explode"]["fraction"] = json!(2)),
            "/explode/fraction"
        );
        assert_eq!(at(&|w| w["explode"]["id"] = json!("Apart")), "/explode/id");
        let mut four = v.clone();
        four["sections"] = json!([
            v["sections"][0],
            v["sections"][0],
            v["sections"][0],
            v["sections"][0]
        ]);
        assert!(ViewFile::from_json(&four).is_err());
    }

    #[test]
    fn standard_names_are_not_view_ids() {
        assert_eq!(check_id("detail").unwrap().as_str(), "detail");
        for name in RESERVED {
            assert_eq!(check_id(name).unwrap_err().code, Code::InvalidId, "{name}");
        }
        assert_eq!(check_id("Bad Name").unwrap_err().code, Code::InvalidId);
        assert_eq!(view_file(&check_id("detail").unwrap()), "views/detail.json");
    }
}
