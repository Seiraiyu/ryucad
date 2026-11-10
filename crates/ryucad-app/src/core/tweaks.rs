//! A draft, move face or replace face chosen in the tree, as the window
//! draws it (design 400, phase 6), from its rebuild report: the faces it
//! modified painted in the chosen colour (read from `inspect` of the
//! feature, the tweak making nothing a face's origin could name); a
//! draft's neutral plane drawn as a datum plane is, or its parting-line
//! edges in the chosen colour, and its pull as an arrow in the datums'
//! colour from the plane's middle; a move's direction or axis drawn as a
//! datum axis is. A tweak keeps every name, so what it modified is found
//! by name in the solids that show it.

use crate::core::bodies::Plane;
use crate::core::datums::Mark;
use crate::core::nav::Projector;
use crate::core::sketch::{CURVE_WIDTH, Corner, bands};
use ryucad::render::{DatumShape, Rgb};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

/// The report keys, which are the feature types, that say a feature is a
/// tweak.
pub const KINDS: [&str; 3] = ["draft", "move_face", "replace_face"];

/// Whether a feature of type `ty` is drawn here.
pub fn drawn(ty: Option<&str>) -> bool {
    ty.is_some_and(|t| KINDS.contains(&t))
}

/// One drafted face's certified angle from its pull, as the report gives
/// it: `2 deg` both ends where it is exact.
#[derive(Clone, Debug, PartialEq)]
pub struct Angle {
    /// `<part>/<solid>/<name>`.
    pub face: String,
    pub least: String,
    pub greatest: String,
    /// The two are the same bits: the kernel's closed form.
    pub exact: bool,
    /// Past a millidegree from the angle asked, or spread wider: what the
    /// rebuild warns `draft_angle_varies` of.
    pub varies: bool,
}

impl Angle {
    /// As the form shows it: `2 deg`, or `1.9 deg to 2.1 deg`.
    pub fn text(&self) -> String {
        if self.exact || self.least == self.greatest {
            self.least.clone()
        } else {
            format!("{} to {}", self.least, self.greatest)
        }
    }
}

/// A line a move goes along or turns about, in the part's frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Line {
    pub origin: [f64; 3],
    pub direction: [f64; 3],
}

/// What a tweak's report says the window draws.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TweakInfo {
    /// `draft`, `move_face` or `replace_face`.
    pub kind: String,
    /// A move's `translate`, `rotate` or `offset`.
    pub by: Option<String>,
    /// A draft's neutral plane, its normal the pull, once it built.
    pub neutral: Option<Plane>,
    /// A draft's pull, once it built.
    pub pull: Option<[f64; 3]>,
    /// A parting-line draft's edges, `<part>/<solid>/<name>`.
    pub parting: Vec<String>,
    /// A translation's or rotation's line, once it built.
    pub line: Option<Line>,
    /// A draft's faces' angles, once it built.
    pub angles: Vec<Angle>,
}

fn point(v: &Value) -> Option<[f64; 3]> {
    Some([v[0].as_f64()?, v[1].as_f64()?, v[2].as_f64()?])
}

impl TweakInfo {
    /// From a feature's rebuild report: its `draft`, `move_face` or
    /// `replace_face` object; `None` for any other feature.
    pub fn from_report(f: &Value) -> Option<TweakInfo> {
        let (kind, v) = KINDS
            .iter()
            .find_map(|k| f.get(*k).filter(|v| v.is_object()).map(|v| (*k, v)))?;
        let neutral = &v["neutral"];
        let neutral = neutral.is_object().then(|| {
            Some(Plane {
                origin: point(&neutral["origin_m"])?,
                normal: point(&neutral["normal"])?,
            })
        });
        let line = &v["line"];
        let line = line.is_object().then(|| {
            Some(Line {
                origin: point(&line["origin_m"])?,
                direction: point(&line["direction"])?,
            })
        });
        let angles = v["angles"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|a| {
                Some(Angle {
                    face: a["face"].as_str()?.to_owned(),
                    least: a["least"]["value"].as_str()?.to_owned(),
                    greatest: a["greatest"]["value"].as_str()?.to_owned(),
                    exact: a["exact"] == true,
                    varies: a["varies"] == true,
                })
            })
            .collect();
        Some(TweakInfo {
            kind: kind.to_owned(),
            by: v["by"].as_str().map(str::to_owned),
            neutral: neutral.flatten(),
            pull: point(&v["pull"]),
            parting: v["parting"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|e| e.as_str().map(str::to_owned))
                .collect(),
            line: line.flatten(),
            angles,
        })
    }

    /// Whether any drafted face's angle varies: the tree's warning badge.
    pub fn varies(&self) -> bool {
        self.angles.iter().any(|a| a.varies)
    }

    /// A draft's neutral plane as a datum plane is drawn, sized to
    /// `bounds` (the box of what the window draws), named
    /// `<part>/<feature>`.
    pub fn neutral_mark(
        &self,
        part: &str,
        feature: &str,
        bounds: Option<([f64; 3], [f64; 3])>,
    ) -> Option<Mark> {
        let shape = self.neutral?.shape();
        Some(mark(part, feature, "plane", &shape, bounds))
    }

    /// A move's line as a datum axis is drawn, across `bounds`.
    pub fn line_mark(
        &self,
        part: &str,
        feature: &str,
        bounds: Option<([f64; 3], [f64; 3])>,
    ) -> Option<Mark> {
        let l = self.line?;
        let shape = DatumShape::Axis {
            origin: l.origin,
            direction: l.direction,
        };
        Some(mark(part, feature, "axis", &shape, bounds))
    }
}

fn mark(
    part: &str,
    feature: &str,
    kind: &'static str,
    shape: &DatumShape,
    bounds: Option<([f64; 3], [f64; 3])>,
) -> Mark {
    let d = ryucad::render::datum_lines(shape, bounds.unwrap_or(([0.0; 3], [0.0; 3])));
    Mark {
        reference: format!("{part}/{feature}"),
        part: part.to_owned(),
        feature: feature.to_owned(),
        kind,
        lines: d.lines,
        square: d.square,
        dot: None,
        label: d.label,
    }
}

/// The cosine and sine of 25°, an arrow head's half angle (the same bits
/// everywhere: no platform trigonometry).
const HEAD_COS: f64 = 0.906_307_787_036_649_9;
const HEAD_SIN: f64 = 0.422_618_261_740_699_44;

/// An arrow `length` long from `from` along unit `dir`: its shaft and the
/// two strokes of its head, a fifth of its length back at 25° either side
/// in a plane holding `dir`.
pub fn arrow(from: [f64; 3], dir: [f64; 3], length: f64) -> Vec<Vec<[f64; 3]>> {
    let tip = [0, 1, 2].map(|k| from[k] + length * dir[k]);
    // A direction square to `dir`: across the axis it leans from least.
    let least = (0..3)
        .min_by(|&a, &b| dir[a].abs().total_cmp(&dir[b].abs()))
        .unwrap_or(0);
    let mut a = [0.0; 3];
    a[least] = 1.0;
    let side = unit(cross(dir, a));
    let (back, across) = (0.2 * length * HEAD_COS, 0.2 * length * HEAD_SIN);
    let head = |s: f64| [0, 1, 2].map(|k| tip[k] - back * dir[k] + s * across * side[k]);
    vec![vec![from, tip], vec![head(1.0), tip, head(-1.0)]]
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

/// Lines as `pr` sees them on a viewport `size` pixels big (`scale`
/// pixels to a point), twice [`CURVE_WIDTH`] wide in `color`, unbroken,
/// drawn with depth as sketches are: a parting line, a pull's arrow.
pub fn lines_overlay(
    lines: &[Vec<[f64; 3]>],
    color: Rgb,
    pr: &Projector,
    size: [f64; 2],
    scale: f64,
) -> Vec<Corner> {
    let mut out = Vec::new();
    bands(
        lines
            .iter()
            .filter(|ps| ps.len() > 1)
            .map(|ps| (ps.as_slice(), color, false)),
        pr,
        size,
        scale,
        2.0 * CURVE_WIDTH,
        &mut out,
    );
    out
}

/// The faces a tweak modified, by solid, from `inspect` of the feature
/// (`v`): the references its `kind` object's `modified` lists that name a
/// face of the solid as the tweak left it.
pub fn modified(v: &Value, kind: &str) -> BTreeMap<(String, String), BTreeSet<String>> {
    let faces: BTreeSet<(String, String)> = v["bodies"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|b| {
            let solid = b["solid"].as_str().unwrap_or_default().to_owned();
            b["faces"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(move |f| Some((solid.clone(), f["name"].as_str()?.to_owned())))
        })
        .collect();
    let mut out: BTreeMap<(String, String), BTreeSet<String>> = BTreeMap::new();
    for r in v[kind]["modified"]["references"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        let mut it = r.splitn(3, '/');
        let (Some(part), Some(solid), Some(name)) = (it.next(), it.next(), it.next()) else {
            continue;
        };
        if faces.contains(&(solid.to_owned(), name.to_owned())) {
            out.entry((part.to_owned(), solid.to_owned()))
                .or_default()
                .insert(name.to_owned());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_drafts_report_gives_its_plane_pull_and_angles() {
        let f = json!({"id": "d", "type": "draft", "ok": true,
            "draft": {"target": "b", "angle": {"value": "2 deg", "rad": 0.0349},
                      "neutral": "xy", "reverse": false, "tangent": true,
                      "faces": ["p/b/e1", "p/b/e2"],
                      "pull": [0.0, 0.0, 1.0],
                      "angles": [
                          {"face": "p/b/e1", "least": {"value": "2 deg", "rad": 0.0349},
                           "greatest": {"value": "2 deg", "rad": 0.0349},
                           "exact": true, "varies": false},
                          {"face": "p/b/e2", "least": {"value": "1.9 deg", "rad": 0.0331},
                           "greatest": {"value": "2.1 deg", "rad": 0.0366},
                           "exact": false, "varies": true}],
                      "modified": {"faces": 2, "edges": 6, "vertices": 4}}});
        // Built, the report's neutral plane is resolved over its text.
        let mut f = f;
        f["draft"]["neutral"] = json!({"origin_m": [0.0, 0.0, 0.0], "normal": [0.0, 0.0, 1.0]});
        let t = TweakInfo::from_report(&f).unwrap();
        assert_eq!(t.kind, "draft");
        assert_eq!(
            t.neutral,
            Some(Plane {
                origin: [0.0; 3],
                normal: [0.0, 0.0, 1.0]
            })
        );
        assert_eq!(t.pull, Some([0.0, 0.0, 1.0]));
        assert_eq!(
            t.angles.iter().map(Angle::text).collect::<Vec<_>>(),
            ["2 deg", "1.9 deg to 2.1 deg"]
        );
        assert!(t.varies());
        // Drawn as a datum plane is: a square in z = 0 about the origin.
        let m = t
            .neutral_mark("p", "d", Some(([0.0; 3], [0.04, 0.03, 0.02])))
            .unwrap();
        assert_eq!((m.reference.as_str(), m.kind), ("p/d", "plane"));
        assert!(m.square.unwrap().iter().all(|c| c[2].abs() < 1e-12));
        assert!(t.line_mark("p", "d", None).is_none());
    }

    #[test]
    fn a_failed_draft_and_a_parting_line_draw_what_they_can() {
        // Not built: the neutral plane is the file's text; nothing drawn.
        let f = json!({"id": "d", "type": "draft", "ok": false,
            "draft": {"target": "b", "neutral": "xy", "faces": ["p/b/e1"]}});
        let t = TweakInfo::from_report(&f).unwrap();
        assert!(t.neutral.is_none() && t.pull.is_none() && !t.varies());
        // A parting line names its edges whether or not it built.
        let f = json!({"id": "d", "type": "draft", "ok": true,
            "draft": {"target": "b", "parting": ["p/b/k1", "p/b/k2"], "pull": [0.0, 0.0, 1.0]}});
        let t = TweakInfo::from_report(&f).unwrap();
        assert_eq!(t.parting, ["p/b/k1", "p/b/k2"]);
        assert!(t.neutral_mark("p", "d", None).is_none());
        assert!(TweakInfo::from_report(&json!({"id": "e", "type": "extrude"})).is_none());
        assert!(drawn(Some("move_face")) && !drawn(Some("offset")) && !drawn(None));
    }

    #[test]
    fn a_moves_line_is_drawn_as_a_datum_axis() {
        let f = json!({"id": "m", "type": "move_face", "ok": true,
            "move_face": {"target": "b", "by": "rotate", "axis": "x",
                          "line": {"origin_m": [0.0, 0.0, 0.02], "direction": [1.0, 0.0, 0.0]},
                          "modified": {"faces": 1, "edges": 4, "vertices": 4}}});
        let t = TweakInfo::from_report(&f).unwrap();
        assert_eq!(t.by.as_deref(), Some("rotate"));
        let m = t
            .line_mark("p", "m", Some(([0.0; 3], [0.04, 0.03, 0.02])))
            .unwrap();
        assert_eq!(m.kind, "axis");
        // Across the box, along x, at y = 0 and z = 0.02.
        let ps: Vec<[f64; 3]> = m.lines.iter().flat_map(|(ps, _)| ps.clone()).collect();
        assert!(
            ps.iter()
                .all(|p| p[1].abs() < 1e-12 && (p[2] - 0.02).abs() < 1e-12)
        );
        assert!(ps.iter().any(|p| p[0] < 0.0) && ps.iter().any(|p| p[0] > 0.04));
    }

    #[test]
    fn the_head_is_25_degrees() {
        let a = 25f64 * core::f64::consts::PI / 180.0;
        assert!((ryu_math::cos(a) - HEAD_COS).abs() < 1e-16);
        assert!((ryu_math::sin(a) - HEAD_SIN).abs() < 1e-16);
    }

    #[test]
    fn an_arrow_runs_its_length_and_its_head_comes_back() {
        let a = arrow([0.0, 0.0, 0.0], [0.0, 0.0, 1.0], 0.01);
        assert_eq!(a[0], vec![[0.0; 3], [0.0, 0.0, 0.01]]);
        let head = &a[1];
        assert_eq!(head[1], [0.0, 0.0, 0.01]);
        for p in [head[0], head[2]] {
            // A fifth back along the shaft, either side of it.
            assert!((p[2] - (0.01 - 0.002 * HEAD_COS)).abs() < 1e-15);
            assert!(((p[0] * p[0] + p[1] * p[1]).sqrt() - 0.002 * HEAD_SIN).abs() < 1e-15);
        }
    }

    #[test]
    fn modified_faces_are_the_references_that_name_faces() {
        let v = json!({"part": "p", "bodies": [{"feature": "m", "solid": "b",
            "faces": [{"name": "f1"}, {"name": "f2"}], "edges": [{"name": "e1"}]}],
            "move_face": {"modified": {"faces": 1, "edges": 1, "vertices": 0,
                "references": ["p/b/e1", "p/b/f2"]}}});
        let m = modified(&v, "move_face");
        assert_eq!(
            m,
            BTreeMap::from([(
                ("p".to_owned(), "b".to_owned()),
                BTreeSet::from(["f2".to_owned()])
            )])
        );
        assert!(modified(&v, "draft").is_empty());
    }
}
