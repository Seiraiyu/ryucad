//! A sweep or loft chosen in the tree, as the window draws it (design
//! 323, phase 6), from a `sweep.get` answer: a sweep's profile and the
//! path it follows; a loft's sections in order, each loop with a dot at
//! where it starts and the section's number beside its first loop's
//! start, so a twisted correspondence shows.

use crate::core::model::BodyKey;
use crate::core::nav::Projector;
use crate::core::sketch::{CURVE_WIDTH, Corner, DOT_SIZE, bands};
use ryucad::render::Rgb;
use serde_json::Value;

/// One loop: its polyline, and where it starts (a loft's; a sweep's
/// profile has no start).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LoopDrawing {
    pub points: Vec<[f64; 3]>,
    pub start: Option<[f64; 3]>,
}

/// One section (a sweep's profile is its only one): its number from 1,
/// whether the loft turned it over, its centroid and its loops in the
/// order the loft matches them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SectionDrawing {
    pub number: usize,
    pub turned: bool,
    pub centroid: [f64; 3],
    pub loops: Vec<LoopDrawing>,
}

/// What `sweep.get` answered for a sweep or loft at its part's revision.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SweepDrawing {
    pub feature: BodyKey,
    pub revision: String,
    /// `sweep` or `loft`.
    pub kind: String,
    pub sections: Vec<SectionDrawing>,
    /// A sweep's path.
    pub path: Option<Vec<[f64; 3]>>,
}

fn point(v: &Value) -> Option<[f64; 3]> {
    Some([v[0].as_f64()?, v[1].as_f64()?, v[2].as_f64()?])
}

fn points(v: &Value) -> Vec<[f64; 3]> {
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(point)
        .collect()
}

impl SweepDrawing {
    /// From a `sweep.get` answer for `feature`.
    pub fn from_get(feature: BodyKey, v: &Value) -> SweepDrawing {
        let sections = v["sections"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|s| SectionDrawing {
                number: s["index"].as_u64().unwrap_or(0) as usize + 1,
                turned: s["turned"] == true,
                centroid: point(&s["centroid_m"]).unwrap_or_default(),
                loops: s["loops"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|l| LoopDrawing {
                        points: points(&l["polyline"]),
                        start: point(&l["start_m"]),
                    })
                    .collect(),
            })
            .collect();
        SweepDrawing {
            feature,
            revision: v["revision"].as_str().unwrap_or_default().to_owned(),
            kind: v["type"].as_str().unwrap_or_default().to_owned(),
            sections,
            path: v["path"]["polyline"]
                .is_array()
                .then(|| points(&v["path"]["polyline"])),
        }
    }

    /// Every line it draws: each section's loops, then the path.
    pub fn lines(&self) -> Vec<Vec<[f64; 3]>> {
        self.sections
            .iter()
            .flat_map(|s| s.loops.iter().map(|l| l.points.clone()))
            .chain(self.path.clone())
            .filter(|ps| ps.len() > 1)
            .collect()
    }

    /// Where each loop starts: a loft's dots.
    pub fn dots(&self) -> Vec<[f64; 3]> {
        self.sections
            .iter()
            .flat_map(|s| s.loops.iter().filter_map(|l| l.start))
            .collect()
    }

    /// A loft's section numbers and where each is written: beside its
    /// first loop's start, else at its centroid. A sweep writes none.
    pub fn labels(&self) -> Vec<(String, [f64; 3])> {
        if self.kind != "loft" {
            return Vec::new();
        }
        self.sections
            .iter()
            .map(|s| {
                let at = s.loops.first().and_then(|l| l.start).unwrap_or(s.centroid);
                (s.number.to_string(), at)
            })
            .collect()
    }

    /// Every point it is drawn through: what framing it takes in.
    pub fn points(&self) -> impl Iterator<Item = [f64; 3]> + '_ {
        self.sections
            .iter()
            .flat_map(|s| s.loops.iter().flat_map(|l| l.points.iter().copied()))
            .chain(self.path.iter().flatten().copied())
    }
}

/// What a chosen sweep or loft adds to the scene as `pr` sees it on a
/// viewport `size` pixels big (`scale` pixels to a point), in `color`:
/// its lines as bands twice [`CURVE_WIDTH`] wide and each loop's start as
/// a square twice [`DOT_SIZE`] across, drawn with depth like sketches, so
/// a path inside its solid shows where it leaves it or a section plane
/// cuts it.
pub fn overlay(
    d: &SweepDrawing,
    color: Rgb,
    pr: &Projector,
    size: [f64; 2],
    scale: f64,
) -> Vec<Corner> {
    let mut out = Vec::new();
    let lines = d.lines();
    bands(
        lines.iter().map(|ps| (ps.as_slice(), color, false)),
        pr,
        size,
        scale,
        2.0 * CURVE_WIDTH,
        &mut out,
    );
    let (right, up, _) = pr.camera.basis();
    let r = DOT_SIZE * scale;
    for p in d.dots() {
        let m = pr.pixel(p, size[1]) * r;
        let corner = |sx: f64, sy: f64| [0, 1, 2].map(|i| p[i] + (sx * right[i] + sy * up[i]) * m);
        let c = [
            corner(-1.0, -1.0),
            corner(1.0, -1.0),
            corner(1.0, 1.0),
            corner(-1.0, 1.0),
        ];
        for i in [0, 1, 2, 0, 2, 3] {
            out.push((c[i], color));
        }
    }
    out
}

/// Whether a feature of type `ty` is drawn by `sweep.get`.
pub fn drawn(ty: Option<&str>) -> bool {
    matches!(ty, Some("sweep" | "loft"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn key() -> BodyKey {
        ("p".to_owned(), "l".to_owned())
    }

    #[test]
    fn a_loft_draws_its_sections_with_their_starts_and_numbers() {
        let v = json!({"feature": "l", "type": "loft", "revision": "r1", "closed": false,
            "sections": [
                {"index": 0, "turned": false, "centroid_m": [0.0, 0.0, 0.0],
                 "loops": [{"anchor": "p1", "start": "p1", "start_m": [-0.005, -0.005, 0.0],
                            "polyline": [[-0.005, -0.005, 0.0], [0.005, -0.005, 0.0],
                                         [0.005, 0.005, 0.0], [-0.005, -0.005, 0.0]]}]},
                {"index": 1, "turned": true, "centroid_m": [0.0, 0.0, 0.01],
                 "loops": [{"anchor": "p1", "start": "p4", "start_m": [-0.003, -0.003, 0.01],
                            "polyline": [[-0.003, -0.003, 0.01], [0.003, 0.003, 0.01]]}]}]});
        let d = SweepDrawing::from_get(key(), &v);
        assert_eq!((d.kind.as_str(), d.revision.as_str()), ("loft", "r1"));
        assert_eq!(d.sections.len(), 2);
        assert!(!d.sections[0].turned && d.sections[1].turned);
        assert!(d.path.is_none());
        assert_eq!(d.lines().len(), 2);
        assert_eq!(d.dots(), [[-0.005, -0.005, 0.0], [-0.003, -0.003, 0.01]]);
        assert_eq!(
            d.labels(),
            [
                ("1".to_owned(), [-0.005, -0.005, 0.0]),
                ("2".to_owned(), [-0.003, -0.003, 0.01])
            ]
        );
        assert_eq!(d.points().count(), 6);
    }

    #[test]
    fn a_sweep_draws_its_profile_and_path_with_no_numbers() {
        let v = json!({"feature": "w", "type": "sweep", "revision": "r2",
            "sections": [{"index": 0, "turned": false, "centroid_m": [0.0, 0.0, 0.0],
                          "loops": [{"anchor": "ci",
                                     "polyline": [[0.002, 0.0, 0.0], [0.0, 0.002, 0.0],
                                                  [0.002, 0.0, 0.0]]}]}],
            "path": {"closed": false, "polyline": [[0.0, 0.0, 0.0], [0.0, 0.0, 0.02]]}});
        let d = SweepDrawing::from_get(("p".to_owned(), "w".to_owned()), &v);
        assert_eq!(d.kind, "sweep");
        assert_eq!(
            d.path.as_deref(),
            Some(&[[0.0, 0.0, 0.0], [0.0, 0.0, 0.02]][..])
        );
        assert_eq!(d.lines().len(), 2);
        assert!(d.dots().is_empty() && d.labels().is_empty());
        assert!(drawn(Some("sweep")) && drawn(Some("loft")) && !drawn(Some("extrude")));
    }
}
