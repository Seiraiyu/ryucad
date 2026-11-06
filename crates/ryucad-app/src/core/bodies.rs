//! A shell, offset, mirror or split chosen in the tree, as the window
//! draws it (design 327, phase 7), from its rebuild report: a shell's
//! opening outlined, dashed, round the faces it made; an offset's faces
//! moved by a group painted with its fills; a mirror's plane drawn as a
//! datum plane is; a split's two sides painted in two tints, and its
//! plane. The faces each made are painted by origin, as for any feature.

use crate::core::datums::{GlassCorner, Mark};
use crate::core::nav::Projector;
use crate::core::sketch::{CURVE_WIDTH, Corner, bands};
use ryucad::render::{DatumShape, Rgb};
use serde_json::Value;

/// A plane as a mirror or split resolved it: a point and its unit normal
/// (a split's as flipped), in the part's frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Plane {
    pub origin: [f64; 3],
    pub normal: [f64; 3],
}

impl Plane {
    /// The plane as a datum plane is drawn: its axes the normal's
    /// perpendiculars, `x` across the axis the normal leans from most.
    pub fn shape(&self) -> DatumShape {
        let n = self.normal;
        let least = (0..3)
            .min_by(|&a, &b| n[a].abs().total_cmp(&n[b].abs()))
            .unwrap_or(0);
        let mut a = [0.0; 3];
        a[least] = 1.0;
        let x = unit(cross(a, n));
        let y = cross(n, x);
        DatumShape::Plane {
            origin: self.origin,
            normal: n,
            x,
            y,
        }
    }
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn unit(a: [f64; 3]) -> [f64; 3] {
    let l = (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt();
    if l == 0.0 {
        return a;
    }
    a.map(|x| x / l)
}

/// A split's side: `behind` (the target's, kept by it) or `front` (the
/// solid the split founds), and the solid that holds it.
#[derive(Clone, Debug, PartialEq)]
pub struct Side {
    pub side: String,
    pub solid: String,
}

/// What a shell's, offset's, mirror's or split's report says the window
/// draws.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BodyInfo {
    /// `shell`, `offset`, `mirror` or `split`.
    pub kind: String,
    /// A mirror's or split's plane, once it built.
    pub plane: Option<Plane>,
    /// A split's sides, once it built: behind first.
    pub sides: Vec<Side>,
    /// The faces an offset's groups move, `<part>/<solid>/<name>`.
    pub moved: Vec<String>,
}

fn point(v: &Value) -> Option<[f64; 3]> {
    Some([v[0].as_f64()?, v[1].as_f64()?, v[2].as_f64()?])
}

impl BodyInfo {
    /// From a feature's rebuild report: its `shell`, `offset`, `mirror` or
    /// `split` object; `None` for any other feature.
    pub fn from_report(f: &Value) -> Option<BodyInfo> {
        let (kind, v) = KINDS
            .iter()
            .find_map(|k| f.get(*k).filter(|v| v.is_object()).map(|v| (*k, v)))?;
        let plane = &v["plane"];
        let plane = plane.is_object().then(|| {
            Some(Plane {
                origin: point(&plane["origin_m"])?,
                normal: point(&plane["normal"])?,
            })
        });
        let sides = v["sides"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|s| {
                Some(Side {
                    side: s["side"].as_str()?.to_owned(),
                    solid: s["solid"].as_str()?.to_owned(),
                })
            })
            .collect();
        let moved = if kind == "offset" {
            v["faces"]
                .as_array()
                .into_iter()
                .flatten()
                .flat_map(|g| g["faces"].as_array().into_iter().flatten())
                .filter_map(|f| f.as_str().map(str::to_owned))
                .collect()
        } else {
            Vec::new()
        };
        Some(BodyInfo {
            kind: kind.to_owned(),
            plane: plane.flatten(),
            sides,
            moved,
        })
    }

    /// The plane as a datum plane is drawn, sized to `bounds` (the box of
    /// what the window draws), named `reference`: its outline and its
    /// translucent square.
    pub fn plane_mark(
        &self,
        part: &str,
        feature: &str,
        bounds: Option<([f64; 3], [f64; 3])>,
    ) -> Option<Mark> {
        let shape = self.plane?.shape();
        let d = ryucad::render::datum_lines(&shape, bounds.unwrap_or(([0.0; 3], [0.0; 3])));
        Some(Mark {
            reference: format!("{part}/{feature}"),
            part: part.to_owned(),
            feature: feature.to_owned(),
            kind: "plane",
            lines: d.lines,
            square: d.square,
            dot: None,
            label: d.label,
        })
    }
}

/// The report keys that say a feature is one of these.
const KINDS: [&str; 4] = ["shell", "offset", "mirror", "split"];

/// Whether a feature of type `ty` is drawn here.
pub fn drawn(ty: Option<&str>) -> bool {
    ty.is_some_and(|t| KINDS.contains(&t))
}

/// A split's side's tint: [`crate::ui::style::SPLIT_BEHIND`] behind, else
/// [`crate::ui::style::SPLIT_FRONT`].
pub fn tint(side: &str) -> Rgb {
    if side == "behind" {
        crate::ui::style::SPLIT_BEHIND
    } else {
        crate::ui::style::SPLIT_FRONT
    }
}

/// A shell's opening as `pr` sees it on a viewport `size` pixels big
/// (`scale` pixels to a point): each rim edge's points as a dashed band
/// twice [`CURVE_WIDTH`] wide in `color`, drawn with depth as sketches
/// are.
pub fn rim_overlay(
    rims: &[Vec<[f64; 3]>],
    color: Rgb,
    pr: &Projector,
    size: [f64; 2],
    scale: f64,
) -> Vec<Corner> {
    let mut out = Vec::new();
    bands(
        rims.iter()
            .filter(|ps| ps.len() > 1)
            .map(|ps| (ps.as_slice(), color, true)),
        pr,
        size,
        scale,
        2.0 * CURVE_WIDTH,
        &mut out,
    );
    out
}

/// A mirror's or split's plane as the scene draws a datum plane: its
/// outline in bands and its square as translucent triangles.
pub fn plane_overlay(
    mark: &Mark,
    pr: &Projector,
    size: [f64; 2],
    scale: f64,
) -> (Vec<Corner>, Vec<GlassCorner>) {
    crate::core::datums::overlay(std::slice::from_ref(mark), |_| None, pr, size, scale)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_splits_report_gives_its_plane_and_sides() {
        let f = json!({"id": "sp", "type": "split", "ok": true,
            "split": {"target": "b", "plane": {"origin_m": [0.0, 0.0, 0.004],
                                                "normal": [0.0, 0.0, -1.0]},
                      "flip": true, "keep": "both",
                      "sides": [{"side": "behind", "solid": "b", "lumps": 1},
                                {"side": "front", "solid": "sp", "lumps": 2}]}});
        let b = BodyInfo::from_report(&f).unwrap();
        assert_eq!(b.kind, "split");
        assert_eq!(
            b.plane,
            Some(Plane {
                origin: [0.0, 0.0, 0.004],
                normal: [0.0, 0.0, -1.0]
            })
        );
        assert_eq!(
            b.sides
                .iter()
                .map(|s| (s.side.as_str(), s.solid.as_str()))
                .collect::<Vec<_>>(),
            [("behind", "b"), ("front", "sp")]
        );
        assert_ne!(tint("behind"), tint("front"));
        // Drawn as a datum plane is, a square round the box's middle.
        let m = b
            .plane_mark("p", "sp", Some(([0.0; 3], [0.01, 0.01, 0.008])))
            .unwrap();
        assert_eq!((m.reference.as_str(), m.kind), ("p/sp", "plane"));
        let sq = m.square.unwrap();
        assert!(sq.iter().all(|c| (c[2] - 0.004).abs() < 1e-12), "{sq:?}");
    }

    #[test]
    fn a_failed_split_has_no_plane_and_a_mirrors_plane_has_axes_in_it() {
        // Not built: the plane is the file's text, the sides none.
        let f = json!({"id": "sp", "type": "split", "ok": false,
            "split": {"target": "b", "plane": "xy", "flip": false, "keep": "behind"}});
        let b = BodyInfo::from_report(&f).unwrap();
        assert!(b.plane.is_none() && b.sides.is_empty());
        let f = json!({"id": "m", "type": "mirror", "ok": true,
            "mirror": {"solid": "half", "operation": "join", "targets": ["half"],
                       "plane": {"origin_m": [0.0, 0.0, 0.0], "normal": [1.0, 0.0, 0.0]}}});
        let b = BodyInfo::from_report(&f).unwrap();
        let DatumShape::Plane { x, y, .. } = b.plane.unwrap().shape() else {
            panic!("a plane");
        };
        // The plane's axes lie in it, square to each other.
        let dot = |a: [f64; 3], b: [f64; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
        assert!(dot(x, [1.0, 0.0, 0.0]).abs() < 1e-12 && dot(y, [1.0, 0.0, 0.0]).abs() < 1e-12);
        assert!(dot(x, y).abs() < 1e-12);
        assert!(BodyInfo::from_report(&json!({"id": "e", "type": "extrude"})).is_none());
    }

    #[test]
    fn an_offsets_groups_name_the_faces_it_moves() {
        let f = json!({"id": "o", "type": "offset", "ok": true,
            "offset": {"target": "b", "distance": {"value": "0 mm", "m": 0.0},
                       "faces": [{"distance": {"value": "3 mm", "m": 0.003},
                                  "faces": ["p/b/aa", "p/b/bb"]},
                                 {"distance": {"value": "-1 mm", "m": -0.001},
                                  "faces": ["p/b/cc"]}]}});
        let b = BodyInfo::from_report(&f).unwrap();
        assert_eq!(b.moved, ["p/b/aa", "p/b/bb", "p/b/cc"]);
        assert!(b.plane.is_none());
        // A shell's groups thicken walls; it moves nothing.
        let f = json!({"id": "s", "type": "shell", "ok": true,
            "shell": {"target": "b", "thickness": {"value": "2 mm", "m": 0.002},
                      "direction": "inward", "open": ["p/b/aa"],
                      "faces": [{"thickness": {"value": "4 mm", "m": 0.004},
                                 "faces": ["p/b/bb"]}]}});
        let b = BodyInfo::from_report(&f).unwrap();
        assert!(b.moved.is_empty());
        assert!(drawn(Some("shell")) && drawn(Some("split")) && !drawn(Some("fillet")));
    }

    #[test]
    fn a_shells_rims_are_drawn_as_bands() {
        use ryucad::view::{Camera, Projection};
        let camera = Camera {
            eye: [0.0, -10.0, 0.0],
            target: [0.0; 3],
            up: [0.0, 0.0, 1.0],
            projection: Projection::Orthographic { height: 2.0 },
        };
        let pr = Projector::new(&camera, 1.0, [0.0; 3], 1.0);
        let colour = crate::ui::style::CHOSEN;
        let rim = vec![[-0.5, 0.0, 0.0], [0.0, 0.0, 0.5], [0.5, 0.0, 0.0]];
        let bands = rim_overlay(std::slice::from_ref(&rim), colour, &pr, [200.0, 200.0], 1.0);
        assert!(!bands.is_empty());
        assert_eq!(bands.len() % 3, 0, "triangles");
        assert!(bands.iter().all(|(_, c)| *c == colour));
        // A rim of one point draws nothing, and nothing draws nothing.
        assert!(rim_overlay(&[vec![[0.0; 3]]], colour, &pr, [200.0, 200.0], 1.0).is_empty());
        assert!(rim_overlay(&[], colour, &pr, [200.0, 200.0], 1.0).is_empty());
    }
}
