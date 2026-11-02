//! Datums, paths and helices as the window draws them (design 297): a
//! datum from its rebuild report's geometry, sized to the box of what its
//! part draws as renders size it ([`ryucad::render::datum_lines`]); a path
//! or helix from a `path.get` answer's polyline. Each is drawn as bands
//! like sketch curves, a plane's square translucent, and picked by its
//! reference, `<part>/<feature>`.

use crate::core::model::Model;
use crate::core::nav::Projector;
use crate::core::sketch::{CURVE_WIDTH, Corner, bands};
use ryucad::render::{DATUM, DATUM_ALPHA, DatumShape, PATH, Rgb};
use serde_json::Value;

/// A translucent triangle corner: a model point and its sRGB colour with
/// its opacity.
pub type GlassCorner = ([f64; 3], [f32; 4]);

/// A path's or helix's polyline, as `path.get` answers it for its part's
/// revision.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PathDrawing {
    pub revision: String,
    pub closed: bool,
    pub points: Vec<[f64; 3]>,
}

impl PathDrawing {
    pub fn from_get(v: &Value) -> PathDrawing {
        let points = v["polyline"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|p| Some([p[0].as_f64()?, p[1].as_f64()?, p[2].as_f64()?]))
            .collect();
        PathDrawing {
            revision: v["revision"].as_str().unwrap_or_default().to_owned(),
            closed: v["closed"] == true,
            points,
        }
    }
}

/// A feature type that draws as a datum, path or helix: `plane`, `axis`,
/// `point`, `path` or `helix`.
pub fn kind_of(ty: &str) -> Option<&'static str> {
    Some(match ty {
        "plane" => "plane",
        "axis" => "axis",
        "point" => "point",
        "path" => "path",
        "helix" => "helix",
        _ => return None,
    })
}

/// One datum, path or helix as drawn: where its lines run (each with
/// whether it is dashed), a plane's square, a point's own place, and where
/// its reference is written.
#[derive(Clone, Debug, PartialEq)]
pub struct Mark {
    /// `<part>/<feature>`, as selections and `visibility.set` name it.
    pub reference: String,
    pub part: String,
    pub feature: String,
    /// `plane`, `axis`, `point`, `path` or `helix`.
    pub kind: &'static str,
    pub lines: Vec<(Vec<[f64; 3]>, bool)>,
    pub square: Option<[[f64; 3]; 4]>,
    /// A point datum's place: picked as a vertex is.
    pub dot: Option<[f64; 3]>,
    pub label: [f64; 3],
}

impl Mark {
    /// Its colour when nothing paints it.
    pub fn color(&self) -> Rgb {
        match self.kind {
            "path" | "helix" => PATH,
            _ => DATUM,
        }
    }

    /// Every point it is drawn through: what framing it takes in.
    pub fn points(&self) -> impl Iterator<Item = [f64; 3]> + '_ {
        self.lines
            .iter()
            .flat_map(|(ps, _)| ps.iter().copied())
            .chain(self.square.into_iter().flatten())
    }
}

/// What the window draws of the datums, paths and helices of the parts it
/// shows and doesn't hide, the datums sized to `bounds` (the box of the
/// bodies and sketches drawn; the origin's 5 mm when there are none), in
/// the order their parts and features are listed. Paths and helices with
/// no drawing yet are left out until `path.get` answers.
pub fn marks(model: &Model, bounds: Option<([f64; 3], [f64; 3])>) -> Vec<Mark> {
    let bounds = bounds.unwrap_or(([0.0; 3], [0.0; 3]));
    let mut out = Vec::new();
    for part in model.shown() {
        let Some(p) = model.parts.get(part) else {
            continue;
        };
        for f in &p.features {
            let Some(kind) = f.ty.as_deref().and_then(kind_of) else {
                continue;
            };
            let reference = format!("{}/{}", p.id, f.id);
            if f.error.is_some() || model.hidden.contains(&reference) {
                continue;
            }
            let (lines, square, dot, label) = match (&f.datum, kind) {
                (Some(shape), _) => {
                    let d = ryucad::render::datum_lines(shape, bounds);
                    let dot = match shape {
                        DatumShape::Point(q) => Some(*q),
                        _ => None,
                    };
                    (d.lines, d.square, dot, d.label)
                }
                (None, "path" | "helix") => {
                    let key = (p.id.clone(), f.id.clone());
                    let Some(d) = model.paths.get(&key).filter(|d| d.points.len() > 1) else {
                        continue;
                    };
                    (vec![(d.points.clone(), false)], None, None, d.points[0])
                }
                _ => continue,
            };
            out.push(Mark {
                reference,
                part: p.id.clone(),
                feature: f.id.clone(),
                kind,
                lines,
                square,
                dot,
                label,
            });
        }
    }
    out
}

/// What `marks` add to the scene as `pr` sees it on a viewport `size`
/// pixels big (`scale` pixels to a point): bands [`CURVE_WIDTH`] wide in
/// [`DATUM`] or [`PATH`] (an axis dashed), and each plane's square as two
/// translucent triangles at [`DATUM_ALPHA`]. What `paint` gives a colour
/// (a selection, the hover, a highlight) is drawn in it, twice as wide, as
/// renders draw a highlighted datum.
pub fn overlay(
    marks: &[Mark],
    paint: impl Fn(&str) -> Option<Rgb>,
    pr: &Projector,
    size: [f64; 2],
    scale: f64,
) -> (Vec<Corner>, Vec<GlassCorner>) {
    let (mut lines, mut glass) = (Vec::new(), Vec::new());
    for m in marks {
        let painted = paint(&m.reference);
        let color = painted.unwrap_or_else(|| m.color());
        let width = if painted.is_some() {
            2.0 * CURVE_WIDTH
        } else {
            CURVE_WIDTH
        };
        bands(
            m.lines
                .iter()
                .map(|(ps, dash)| (ps.as_slice(), color, *dash)),
            pr,
            size,
            scale,
            width,
            &mut lines,
        );
        if let Some(c) = m.square {
            let rgba = [color[0], color[1], color[2], DATUM_ALPHA];
            for i in [0, 1, 2, 0, 2, 3] {
                glass.push((c[i], rgba));
            }
        }
    }
    (lines, glass)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Part `p`: a plane `d1` at 8 mm, an axis `a1` along y, a point
    /// `q1`, a helix `h` and a plane `bad` that failed.
    pub(crate) fn datum_model() -> Model {
        let mut model = Model::default();
        let report = json!({"part": "p", "revision": "r1", "features": [
            {"id": "d1", "type": "plane", "ok": true, "hash": "1",
             "datum": {"kind": "plane", "origin_m": [0.0, 0.0, 0.008],
                       "normal": [0.0, 0.0, 1.0], "x": [1.0, 0.0, 0.0]}},
            {"id": "a1", "type": "axis", "ok": true, "hash": "2",
             "datum": {"kind": "axis", "origin_m": [0.005, 0.0, 0.0],
                       "direction": [0.0, 1.0, 0.0]}},
            {"id": "q1", "type": "point", "ok": true, "hash": "3",
             "datum": {"kind": "point", "origin_m": [0.01, 0.0, 0.0]}},
            {"id": "h", "type": "helix", "ok": true, "hash": "4", "helix": {}},
            {"id": "bad", "type": "plane", "ok": false,
             "error": {"code": "datum_degenerate", "message": "no"}}
        ]});
        model.apply_part(&report);
        model
    }

    #[test]
    fn datums_are_drawn_from_their_reports_and_paths_once_they_come() {
        let mut model = datum_model();
        let bounds = Some(([0.0; 3], [0.02, 0.01, 0.005]));
        let kinds = |m: &Model| -> Vec<(String, &'static str)> {
            marks(m, bounds)
                .into_iter()
                .map(|m| (m.reference, m.kind))
                .collect()
        };
        // The helix waits for its drawing; the failed plane is never drawn.
        assert_eq!(
            kinds(&model),
            [
                ("p/d1".to_owned(), "plane"),
                ("p/a1".to_owned(), "axis"),
                ("p/q1".to_owned(), "point")
            ]
        );
        let ms = marks(&model, bounds);
        assert!(ms[0].square.is_some());
        assert!(ms[1].lines[0].1, "an axis is dashed");
        assert_eq!(ms[2].dot, Some([0.01, 0.0, 0.0]));
        assert_eq!(ms[0].color(), DATUM);
        // The helix's drawing comes, as `path.get` answers it.
        let key = ("p".to_owned(), "h".to_owned());
        assert_eq!(model.ask_paths("p"), vec![(key.clone(), "r1".to_owned())]);
        assert_eq!(model.ask_paths("p"), vec![], "asked once a revision");
        let d = PathDrawing::from_get(&json!({"revision": "r1", "closed": false,
            "polyline": [[0.008, 0.0, 0.0], [0.0, 0.008, 0.001]]}));
        model.path_answered(&key, "r1");
        assert!(model.take_path(&key, d));
        let ms = marks(&model, bounds);
        assert_eq!(ms[3].kind, "helix");
        assert_eq!(ms[3].label, [0.008, 0.0, 0.0]);
        assert_eq!(ms[3].color(), PATH);
        // Hidden, it isn't drawn.
        model.hidden.insert("p/d1".to_owned());
        assert!(!kinds(&model).iter().any(|(r, _)| r == "p/d1"));
    }

    #[test]
    fn a_painted_datum_is_wider_in_its_colour_and_a_plane_is_glass() {
        let model = datum_model();
        let ms = marks(&model, Some(([0.0; 3], [0.02, 0.01, 0.005])));
        let camera = ryucad::view::frame(
            [0.0; 3],
            [0.02, 0.01, 0.01],
            ryucad::view::StandardView::Iso,
            ryucad::view::DEFAULT_PROJECTION,
        );
        let pr = Projector::new(&camera, 1.0, [0.01, 0.005, 0.005], 0.02);
        let size = [400.0, 400.0];
        let red = [1.0, 0.0, 0.0];
        let (plain, glass) = overlay(&ms[..1], |_| None, &pr, size, 1.0);
        let (painted, glass2) = overlay(&ms[..1], |r| (r == "p/d1").then_some(red), &pr, size, 1.0);
        assert!(plain.iter().all(|(_, c)| *c == DATUM));
        assert!(painted.iter().all(|(_, c)| *c == red));
        assert_eq!(plain.len(), painted.len());
        assert_eq!(glass.len(), 6);
        assert!(glass.iter().all(|(_, c)| c[3] == DATUM_ALPHA));
        assert!(glass2.iter().all(|(_, c)| c[..3] == red));
        // Twice as wide: the band's corners lie further from its line.
        let spread = |v: &[Corner]| {
            v.iter()
                .map(|(p, _)| (p[2] - 0.008).abs())
                .fold(0.0f64, f64::max)
        };
        assert!(spread(&painted) > 1.5 * spread(&plain));
    }
}
