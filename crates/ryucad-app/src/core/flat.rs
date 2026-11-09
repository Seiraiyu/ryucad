//! The flat view (design 346, The app; question 116): View > Flat
//! pattern shows the chosen sheet metal solid's flat body in the viewport
//! where its fixed flange lies, picked as the folded body is (every name
//! is kept), with its bend lines dashed over it in `render::BEND_UP` and
//! `BEND_DOWN` and their notes beside them, as `render --flat` draws it.
//! The window reads the bend lines from `flat.get` and the body from
//! `mesh.get` with `flat`.

use crate::core::nav::Projector;
use crate::core::sketch::{CURVE_WIDTH, Corner, bands};
use ryucad::render::Rgb;
use serde_json::Value;

/// One bend line of a flat, in the part's frame.
#[derive(Clone, Debug, PartialEq)]
pub struct BendLine {
    /// Its bend, by the least inner face (`<part>/<solid>/<name>`).
    pub bend: String,
    pub from: [f64; 3],
    pub to: [f64; 3],
    /// `up` or `down`.
    pub direction: String,
    /// `UP 90° R1 K0.44`, as the DXF writes it.
    pub note: String,
    /// The flat body's edges along it.
    pub edges: Vec<String>,
}

impl BendLine {
    /// Its colour: the DXF's yellow folding up, cyan down.
    pub fn colour(&self) -> Rgb {
        if self.direction == "down" {
            ryucad::render::BEND_DOWN
        } else {
            ryucad::render::BEND_UP
        }
    }

    /// Its middle, where its note goes.
    pub fn middle(&self) -> [f64; 3] {
        [0, 1, 2].map(|i| 0.5 * (self.from[i] + self.to[i]))
    }
}

/// A flat's bend lines as `flat.get` answers them: its frame's plane
/// carried into the part's frame.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FlatLines {
    pub part: String,
    pub solid: String,
    /// `whole` (unfolded in memory) or `unfold` (an unfold in force).
    pub from: String,
    /// The unfold in force, when `from` is `unfold`.
    pub unfold: Option<String>,
    pub lines: Vec<BendLine>,
    /// The pattern's frame: its `z` towards its viewer side (where the
    /// bend lines lie), its `y` up, as `render --flat` looks at it.
    pub towards: [f64; 3],
    pub up: [f64; 3],
}

fn triple(v: &Value) -> [f64; 3] {
    [0, 1, 2].map(|i| v[i].as_f64().unwrap_or(0.0))
}

impl FlatLines {
    /// From `flat.get`'s answer: each bend line's ends, in millimetres in
    /// the pattern's frame, lifted into the part's frame in meters.
    pub fn from_answer(v: &Value) -> FlatLines {
        let f = &v["frame"];
        let (o, x, y) = (triple(&f["origin_m"]), triple(&f["x"]), triple(&f["y"]));
        let lift = |p: &Value| {
            let (u, w) = (
                p[0].as_f64().unwrap_or(0.0) / 1e3,
                p[1].as_f64().unwrap_or(0.0) / 1e3,
            );
            [0, 1, 2].map(|i| o[i] + x[i] * u + y[i] * w)
        };
        let lines = v["bend_lines"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|l| BendLine {
                bend: l["bend"].as_str().unwrap_or_default().to_owned(),
                from: lift(&l["from_mm"]),
                to: lift(&l["to_mm"]),
                direction: l["direction"].as_str().unwrap_or("up").to_owned(),
                note: l["note"].as_str().unwrap_or_default().to_owned(),
                edges: l["edges"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|e| e.as_str().map(str::to_owned))
                    .collect(),
            })
            .collect();
        FlatLines {
            part: v["part"].as_str().unwrap_or_default().to_owned(),
            solid: v["solid"].as_str().unwrap_or_default().to_owned(),
            from: v["from"].as_str().unwrap_or("whole").to_owned(),
            unfold: v["unfold"].as_str().map(str::to_owned),
            lines,
            towards: triple(&f["z"]),
            up: y,
        }
    }

    /// The camera `render --flat` takes, framing `points` (the flat's,
    /// as the window has them) at `aspect`: along the pattern's `z`, its
    /// `y` up.
    pub fn camera(&self, points: &[[f64; 3]], aspect: f64) -> ryucad::view::Camera {
        ryucad::view::frame_points_along(
            points,
            self.towards,
            self.up,
            ryucad::view::DEFAULT_PROJECTION,
            aspect,
        )
    }
}

/// The flat view: which solid it lays flat, and what the window has of
/// it so far.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FlatView {
    pub part: String,
    /// The solid asked for; `""` for the part's one sheet metal solid,
    /// filled in from the answer.
    pub solid: String,
    /// The part revision it was read at: the view reads it again when
    /// the part's moves.
    pub revision: String,
    pub lines: Option<FlatLines>,
    /// Why the solid can't be shown flat.
    pub error: Option<ryucad::error::Error>,
    /// Whether the camera has turned to look at it, as `render --flat`
    /// does: once, when its body and its lines have both come.
    pub turned: bool,
}

/// The bend lines as `pr` sees them on a viewport `size` pixels big
/// (`scale` pixels to a point): dashed bands twice [`CURVE_WIDTH`] wide in
/// their direction's colour, drawn with depth as sketches are, pulled a
/// little towards the eye so the flat's faces don't hide them.
pub fn overlay(lines: &FlatLines, pr: &Projector, size: [f64; 2], scale: f64) -> Vec<Corner> {
    let segments: Vec<([[f64; 3]; 2], Rgb)> = lines
        .lines
        .iter()
        .map(|l| ([l.from, l.to], l.colour()))
        .collect();
    let mut out = Vec::new();
    bands(
        segments.iter().map(|(ps, c)| (ps.as_slice(), *c, true)),
        pr,
        size,
        scale,
        2.0 * CURVE_WIDTH,
        &mut out,
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn bend_lines_are_lifted_from_the_patterns_frame() {
        let v = json!({"schema": 1, "part": "p", "solid": "plate", "from": "unfold",
            "unfold": "u",
            "frame": {"origin_m": [0.0, 0.0, 0.0015], "x": [0, 1, 0], "y": [-1, 0, 0],
                      "z": [0, 0, 1]},
            "bend_lines": [{"bend": "p/plate/a1", "from_mm": [10.0, 0.0], "to_mm": [10.0, 20.0],
                            "direction": "down", "note": "DOWN 90° R1 K0.44",
                            "edges": ["p/plate/e1"]}]});
        let f = FlatLines::from_answer(&v);
        assert_eq!(
            (f.from.as_str(), f.unfold.as_deref()),
            ("unfold", Some("u"))
        );
        let l = &f.lines[0];
        assert_eq!(l.from, [0.0, 0.01, 0.0015]);
        assert_eq!(l.to, [-0.02, 0.01, 0.0015]);
        assert_eq!(l.middle(), [-0.01, 0.01, 0.0015]);
        assert_eq!(l.colour(), ryucad::render::BEND_DOWN);
        assert_eq!(l.edges, ["p/plate/e1"]);
        assert_eq!((f.towards, f.up), ([0.0, 0.0, 1.0], [-1.0, 0.0, 0.0]));
        // Looked at as render --flat looks: from its viewer side.
        let c = f.camera(&[l.from, l.to], 1.0);
        assert!(c.eye[2] > c.target[2] && c.up == [-1.0, 0.0, 0.0], "{c:?}");
    }
}
