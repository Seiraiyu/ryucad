//! Section planes: what a participant's window cuts away, session state
//! like its camera and hidden set, never in Git (a named view keeps the
//! planes it was saved with: [`crate::views`]).
//!
//! A plane is a point and a unit normal in the shown model's frame
//! (a part's, or an assembly's), the normal pointing to the side cut
//! away. A participant has up to [`MAX_PLANES`]; what is kept is what lies
//! on the inner side of all of them. A request names a plane as an origin
//! plane (`xy` keeps z ≤ 0, `yz` x ≤ 0, `zx` y ≤ 0), a planar face (its
//! plane, the normal its outward normal, so what lies outside the face
//! goes; an occurrence's face is placed by its pose), or an explicit
//! origin and normal; `flip` turns the normal round, then `offset` moves
//! the plane along it.

use crate::error::{Code, Error, Result};
use crate::session::Session;
use crate::units::Quantity;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The most planes a participant cuts with.
pub const MAX_PLANES: usize = 3;

/// One section plane, in meters: what lies on the side `normal` points
/// to is cut away.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Plane {
    pub origin_m: [f64; 3],
    /// Unit length.
    pub normal: [f64; 3],
}

impl Plane {
    /// How far `p` is on the cut side: positive is cut away.
    pub fn distance(&self, p: [f64; 3]) -> f64 {
        (0..3)
            .map(|k| (p[k] - self.origin_m[k]) * self.normal[k])
            .sum()
    }

    /// Whether `p` is kept by every plane.
    pub fn keeps(planes: &[Plane], p: [f64; 3]) -> bool {
        planes.iter().all(|s| s.distance(p) <= 0.0)
    }

    /// The plane turned round: what it cut is kept, and the other way.
    pub fn flipped(&self) -> Plane {
        Plane {
            origin_m: self.origin_m,
            normal: self.normal.map(|x| -x),
        }
    }

    /// The plane moved `d` meters along its normal.
    pub fn moved(&self, d: f64) -> Plane {
        Plane {
            origin_m: [0, 1, 2].map(|k| self.origin_m[k] + d * self.normal[k]),
            normal: self.normal,
        }
    }

    /// A plane through `origin` with `normal`, made unit length; `None`
    /// for a normal of no length, or anything not finite.
    pub fn new(origin_m: [f64; 3], normal: [f64; 3]) -> Option<Plane> {
        let n = normal.iter().map(|x| x * x).sum::<f64>().sqrt();
        if !(n > 0.0 && n.is_finite()) || origin_m.iter().any(|x| !x.is_finite()) {
            return None;
        }
        // A normal along an axis stays exactly that axis.
        Some(Plane {
            origin_m,
            normal: normal.map(|x| x / n),
        })
    }
}

/// A plane as a request names it: `plane` (`xy`, `yz`, `zx` or a planar
/// face's reference) or `origin` and `normal`; then `flip` and `offset`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlaneArg {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plane: Option<String>,
    /// Quantities: bare numbers in the project's default unit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<[Value; 3]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub normal: Option<[f64; 3]>,
    /// A length along the normal (after `flip`): bare in the default unit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<Value>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub flip: bool,
}

fn bad(what: impl Into<String>, pointer: String) -> Error {
    Error::new(Code::InvalidParam, what).with("pointer", pointer)
}

impl PlaneArg {
    /// The plane in meters; `at` is its JSON pointer in the request, for
    /// errors.
    pub fn resolve(&self, session: &mut Session, at: &str) -> Result<Plane> {
        let unit = session.project().manifest.default_unit;
        let length = |v: &Value, pointer: String| -> Result<f64> {
            Ok(Quantity::from_json(v, unit)
                .map_err(|e| e.with("pointer", pointer))?
                .meters())
        };
        let plane = match (&self.plane, &self.origin, &self.normal) {
            (Some(p), None, None) => named(session, p, at)?,
            (None, Some(o), Some(n)) => {
                let mut origin = [0.0; 3];
                for (i, q) in o.iter().enumerate() {
                    origin[i] = length(q, format!("{at}/origin/{i}"))?;
                }
                Plane::new(origin, *n).ok_or_else(|| {
                    bad(
                        "a section's normal must have a length",
                        format!("{at}/normal"),
                    )
                })?
            }
            _ => {
                return Err(bad(
                    "name a plane (xy, yz, zx or a planar face), or give an origin and a normal",
                    at.to_owned(),
                ));
            }
        };
        let plane = if self.flip { plane.flipped() } else { plane };
        Ok(match &self.offset {
            Some(d) => plane.moved(length(d, format!("{at}/offset"))?),
            None => plane,
        })
    }
}

/// An origin plane, or the plane of the face `reference` names.
fn named(session: &mut Session, reference: &str, at: &str) -> Result<Plane> {
    let axis = |k: usize| {
        let mut n = [0.0; 3];
        n[k] = 1.0;
        Plane {
            origin_m: [0.0; 3],
            normal: n,
        }
    };
    match reference {
        "xy" => return Ok(axis(2)),
        "yz" => return Ok(axis(0)),
        "zx" => return Ok(axis(1)),
        _ => {}
    }
    if !reference.contains('/') {
        return Err(bad(
            format!("{reference} names no plane: give xy, yz, zx or a planar face"),
            format!("{at}/plane"),
        ));
    }
    face_plane(session, reference)
}

/// The plane of a planar face: through the middle of its vertices (the
/// surface's own origin for a face with none), along its outward normal,
/// placed by its occurrence's pose.
fn face_plane(session: &mut Session, reference: &str) -> Result<Plane> {
    use ryukei::geom::{Point3, Surface, Vector3};
    use ryukei::measure::Entity;
    let located = crate::measure::locate(session, reference)?;
    let not_planar = || {
        Error::new(
            Code::InvalidParam,
            format!("{reference} isn't a planar face: a section lies on a plane"),
        )
        .with("entity", reference)
    };
    let [l] = located.as_slice() else {
        return Err(not_planar());
    };
    let Entity::Face(f) = l.entity else {
        return Err(not_planar());
    };
    let body = &l.built.body;
    let face = body.face(f).ok_or_else(not_planar)?;
    let Some(Surface::Plane(p)) = body.surface(face.surface) else {
        return Err(not_planar());
    };
    let z = p.frame().z().get().to_array();
    let normal = match face.sense {
        ryukei::topo::Sense::Forward => z,
        ryukei::topo::Sense::Reversed => z.map(|x| -x),
    };
    let mut sum = [0.0; 3];
    let mut n = 0.0;
    for lp in body.face_loops(f) {
        for fin in body.loop_fins(lp) {
            let Some(v) = body
                .fin(fin)
                .and_then(|x| body.edge(x.edge))
                .and_then(|e| body.vertex(e.start))
            else {
                continue;
            };
            let q = v.point.to_array();
            for k in 0..3 {
                sum[k] += q[k];
            }
            n += 1.0;
        }
    }
    let origin = if n > 0.0 {
        sum.map(|x| x / n)
    } else {
        p.frame().origin().to_array()
    };
    // On the plane exactly, whatever the vertices' tolerances.
    let o = p.frame().origin().to_array();
    let off: f64 = (0..3).map(|k| (origin[k] - o[k]) * z[k]).sum();
    let origin = [0, 1, 2].map(|k| origin[k] - off * z[k]);
    let at = l
        .pose
        .apply_point(Point3::new(origin[0], origin[1], origin[2]));
    let n = l
        .pose
        .apply_vector(Vector3::new(normal[0], normal[1], normal[2]));
    Plane::new(at.to_array(), n.to_array()).ok_or_else(not_planar)
}

/// `planes` after a request: replaced by `new`, or with `add` new ones
/// after them; more than [`MAX_PLANES`] is `invalid_param`.
pub fn apply(planes: &[Plane], new: Vec<Plane>, add: bool) -> Result<Vec<Plane>> {
    let mut out = if add { planes.to_vec() } else { Vec::new() };
    out.extend(new);
    if out.len() > MAX_PLANES {
        return Err(Error::new(
            Code::InvalidParam,
            format!(
                "a window cuts with at most {MAX_PLANES} section planes; this would make {}",
                out.len()
            ),
        )
        .with("planes", out.len()));
    }
    Ok(out)
}

/// The planes `args` name, in order.
pub fn resolve_all(session: &mut Session, args: &[PlaneArg]) -> Result<Vec<Plane>> {
    args.iter()
        .enumerate()
        .map(|(i, a)| a.resolve(session, &format!("/planes/{i}")))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::tests::{call, fresh};
    use serde_json::json;

    fn arg(v: Value) -> PlaneArg {
        serde_json::from_value(v).unwrap()
    }

    /// Part `p`: a 10 × 20 × 30 mm box, and a cylinder above it.
    fn project() -> (tempfile::TempDir, Session) {
        let (dir, mut s) = fresh();
        call(&mut s, "part.create", json!({"part": "p"})).unwrap();
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "box", "id": "b",
                   "params": {"x": 10, "y": 20, "z": 30}}),
        )
        .unwrap();
        (dir, s)
    }

    #[test]
    fn origin_planes_flips_and_offsets() {
        let (_dir, mut s) = project();
        let xy = arg(json!({"plane": "xy"})).resolve(&mut s, "").unwrap();
        assert_eq!(xy.normal, [0.0, 0.0, 1.0]);
        assert_eq!(xy.distance([1.0, 2.0, 0.5]), 0.5);
        assert!(Plane::keeps(&[xy], [0.0, 0.0, -1.0]));
        assert!(!Plane::keeps(&[xy], [0.0, 0.0, 1.0]));
        let zx = arg(json!({"plane": "zx", "flip": true, "offset": 5}))
            .resolve(&mut s, "")
            .unwrap();
        assert_eq!(zx.normal, [0.0, -1.0, 0.0]);
        assert_eq!(zx.origin_m, [0.0, -0.005, 0.0]);
        let yz = arg(json!({"plane": "yz", "offset": "1 in"}))
            .resolve(&mut s, "")
            .unwrap();
        assert_eq!(yz.origin_m, [0.0254, 0.0, 0.0]);
        let free = arg(json!({"origin": [1, "2 mm", 0], "normal": [0, 3, 4]}))
            .resolve(&mut s, "")
            .unwrap();
        assert_eq!(free.origin_m, [0.001, 0.002, 0.0]);
        assert_eq!(free.normal, [0.0, 0.6, 0.8]);
    }

    #[test]
    fn a_face_s_plane_points_out_of_it() {
        let (_dir, mut s) = project();
        let inspect = call(&mut s, "inspect", json!({"part": "p"})).unwrap()["bodies"][0].clone();
        // The box's top: its plane at z = 30 mm, outward up.
        let faces: Vec<String> = inspect["faces"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| format!("p/b/{}", f["name"].as_str().unwrap()))
            .collect();
        let r = faces
            .iter()
            .find(|r| {
                arg(json!({"plane": r})).resolve(&mut s, "").unwrap().normal == [0.0, 0.0, 1.0]
            })
            .unwrap()
            .clone();
        let plane = arg(json!({"plane": r})).resolve(&mut s, "").unwrap();
        assert_eq!(plane.normal, [0.0, 0.0, 1.0]);
        assert!((plane.origin_m[2] - 0.03).abs() < 1e-12, "{plane:?}");
        // The middle of its corners.
        assert!((plane.origin_m[0] - 0.005).abs() < 1e-12, "{plane:?}");
        let flipped = arg(json!({"plane": r, "flip": true, "offset": 10}))
            .resolve(&mut s, "")
            .unwrap();
        assert_eq!(flipped.normal, [0.0, 0.0, -1.0]);
        assert!((flipped.origin_m[2] - 0.02).abs() < 1e-12);
    }

    #[test]
    fn bad_planes_are_refused() {
        let (_dir, mut s) = project();
        let code = |s: &mut Session, v: Value| arg(v).resolve(s, "/planes/0").unwrap_err();
        let e = code(&mut s, json!({"plane": "xz"}));
        assert_eq!(
            (e.code, &e.details["pointer"]),
            (Code::InvalidParam, &json!("/planes/0/plane"))
        );
        let e = code(&mut s, json!({}));
        assert_eq!(e.code, Code::InvalidParam);
        let e = code(&mut s, json!({"plane": "xy", "normal": [0, 0, 1]}));
        assert_eq!(e.code, Code::InvalidParam);
        let e = code(&mut s, json!({"origin": [0, 0, 0], "normal": [0, 0, 0]}));
        assert_eq!(e.details["pointer"], "/planes/0/normal");
        let e = code(&mut s, json!({"origin": [0, "x", 0], "normal": [0, 0, 1]}));
        assert_eq!(e.details["pointer"], "/planes/0/origin/1");
        // A box's edge isn't a face; a part alone is nothing to cut by.
        let inspect = call(&mut s, "inspect", json!({"part": "p"})).unwrap()["bodies"][0].clone();
        let edge = inspect["edges"][0]["name"].as_str().unwrap().to_owned();
        let e = code(&mut s, json!({"plane": format!("p/b/{edge}")}));
        assert_eq!(e.code, Code::InvalidParam);
        assert!(serde_json::from_value::<PlaneArg>(json!({"plane": "xy", "tilt": 3})).is_err());
    }

    #[test]
    fn at_most_three_planes() {
        let p = Plane::new([0.0; 3], [0.0, 0.0, 1.0]).unwrap();
        let two = apply(&[], vec![p, p.flipped()], false).unwrap();
        let three = apply(&two, vec![p], true).unwrap();
        assert_eq!(three.len(), 3);
        assert_eq!(apply(&three, vec![p], false).unwrap(), vec![p]);
        let e = apply(&three, vec![p], true).unwrap_err();
        assert_eq!(e.code, Code::InvalidParam);
        assert_eq!(
            apply(&three, Vec::new(), false).unwrap(),
            Vec::<Plane>::new()
        );
    }

    #[test]
    fn a_participants_window_cuts_what_it_is_told() {
        use crate::live::{Kind, Live};
        use crate::server::tests::call_live;
        let (_dir, mut s) = project();
        let mut live = Live::new();
        let set =
            |s: &mut Session, live: &mut Live, v: Value| call_live(s, live, None, "section.set", v);
        // With no human, the caller's own window.
        let v = set(
            &mut s,
            &mut live,
            json!({"planes": [{"plane": "xy", "offset": 10}]}),
        )
        .unwrap();
        assert_eq!(
            v["sections"],
            json!([{"origin_m": [0.0, 0.0, 0.01], "normal": [0.0, 0.0, 1.0]}])
        );
        let agent = v["participant"].as_str().unwrap().to_owned();
        // With one, the human's: an agent shows a person a section.
        let human = live.hello(Kind::Human, Some("ann"), None).unwrap();
        let v = set(&mut s, &mut live, json!({"planes": [{"plane": "yz"}]})).unwrap();
        assert_eq!(v["participant"], json!(human));
        let v = set(
            &mut s,
            &mut live,
            json!({"planes": [{"plane": "zx"}, {"plane": "xy", "flip": true}], "add": true}),
        )
        .unwrap();
        assert_eq!(v["sections"].as_array().unwrap().len(), 3);
        let got = call_live(&mut s, &mut live, None, "section.get", json!({})).unwrap();
        assert_eq!(got["sections"], v["sections"]);
        assert_eq!(
            live.get(&human).unwrap().to_json()["sections"],
            v["sections"]
        );
        // A fourth is refused, and changes nothing.
        let e = set(
            &mut s,
            &mut live,
            json!({"planes": [{"plane": "xy"}], "add": true}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::InvalidParam);
        assert_eq!(live.get(&human).unwrap().view.sections.len(), 3);
        // The agent's own planes stay its own.
        let mine = call_live(
            &mut s,
            &mut live,
            None,
            "section.get",
            json!({"participant": agent}),
        )
        .unwrap();
        assert_eq!(mine["sections"].as_array().unwrap().len(), 1);
        let v = set(&mut s, &mut live, json!({"off": true})).unwrap();
        assert_eq!(v["sections"], json!([]));
        let e = set(
            &mut s,
            &mut live,
            json!({"off": true, "planes": [{"plane": "xy"}]}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::Usage);
        assert_eq!(
            set(&mut s, &mut live, json!({})).unwrap_err().code,
            Code::Usage
        );
        let e = call(&mut s, "section.set", json!({"planes": [{"plane": "xy"}]})).unwrap_err();
        assert_eq!(e.code, Code::NoLiveSession);
    }
}
