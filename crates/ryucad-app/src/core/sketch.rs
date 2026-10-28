//! A sketch as the window draws it, read from a `sketch.get` answer: its
//! curves as polylines and its points, each in its solve state, and its
//! dimensions' labels with their values as written.

use crate::core::nav::Projector;
use ryucad::sketch_view::{ConstraintState, EntityState};
use serde_json::Value;

/// Sketch curves' width, dots' size, and construction curves' dashes and
/// gaps, in points.
pub const CURVE_WIDTH: f64 = 2.0;
pub const DOT_SIZE: f64 = 5.0;
pub const DASH: [f64; 2] = [6.0, 4.0];

/// A triangle corner of the overlay: a model point and its sRGB colour.
pub type Corner = ([f64; 3], [f32; 3]);

/// A curve: a polyline in model space.
#[derive(Clone, Debug, PartialEq)]
pub struct Curve {
    pub entity: String,
    /// `line`, `arc` or `circle`.
    pub ty: &'static str,
    pub construction: bool,
    pub state: EntityState,
    pub points: Vec<[f64; 3]>,
}

/// A point.
#[derive(Clone, Debug, PartialEq)]
pub struct Dot {
    pub entity: String,
    pub state: EntityState,
    pub at: [f64; 3],
}

/// A dimension's label, and its value as written, for editing.
#[derive(Clone, Debug, PartialEq)]
pub struct Label {
    pub constraint: String,
    /// `length`, `radius` and so on.
    pub ty: String,
    pub state: ConstraintState,
    pub at: [f64; 3],
    pub text: String,
    pub value: String,
}

/// A sketch as drawn, for the part revision it was read at.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Drawing {
    pub revision: String,
    pub curves: Vec<Curve>,
    pub dots: Vec<Dot>,
    pub labels: Vec<Label>,
}

fn point(v: &Value) -> Option<[f64; 3]> {
    let a = v.as_array()?;
    Some([
        a.first()?.as_f64()?,
        a.get(1)?.as_f64()?,
        a.get(2)?.as_f64()?,
    ])
}

fn constraint_state(s: &str) -> ConstraintState {
    match s {
        "redundant" => ConstraintState::Redundant,
        "conflicting" => ConstraintState::Conflicting,
        _ => ConstraintState::Ok,
    }
}

impl Drawing {
    /// The drawing in a `sketch.get` answer. What it can't read is left
    /// out.
    pub fn from_get(v: &Value) -> Drawing {
        let mut d = Drawing {
            revision: v["revision"].as_str().unwrap_or_default().to_owned(),
            ..Drawing::default()
        };
        for (id, e) in v["entities"].as_object().into_iter().flatten() {
            let state = e["state"]
                .as_str()
                .and_then(EntityState::parse)
                .unwrap_or(EntityState::Under);
            if let Some(at) = point(&e["at"]) {
                d.dots.push(Dot {
                    entity: id.clone(),
                    state,
                    at,
                });
            }
            if let Some(ps) = e["polyline"].as_array() {
                d.curves.push(Curve {
                    entity: id.clone(),
                    ty: match e["type"].as_str() {
                        Some("arc") => "arc",
                        Some("circle") => "circle",
                        _ => "line",
                    },
                    construction: e["construction"] == true,
                    state,
                    points: ps.iter().filter_map(point).collect(),
                });
            }
        }
        for (id, c) in v["constraints"].as_object().into_iter().flatten() {
            let Some(at) = point(&c["label"]["at"]) else {
                continue;
            };
            d.labels.push(Label {
                constraint: id.clone(),
                ty: c["type"].as_str().unwrap_or_default().to_owned(),
                state: constraint_state(c["state"].as_str().unwrap_or_default()),
                at,
                text: c["label"]["text"].as_str().unwrap_or_default().to_owned(),
                value: c["value"]["value"].as_str().unwrap_or_default().to_owned(),
            });
        }
        d
    }

    /// Every point it is drawn through: what frames it.
    pub fn points(&self) -> impl Iterator<Item = [f64; 3]> + '_ {
        self.curves
            .iter()
            .flat_map(|c| c.points.iter().copied())
            .chain(self.dots.iter().map(|d| d.at))
    }
}

/// The pieces of the segment `a`..`b` (`sa`..`sb` on screen) to draw
/// with dashes of `on` pixels and gaps of `off`, from `phase` into the
/// pattern; returns them as fractions along it, and moves `phase` on.
/// Only what lies within `margin` of a viewport `size` big is dashed.
fn dashed(
    sa: [f64; 2],
    sb: [f64; 2],
    [on, off]: [f64; 2],
    size: [f64; 2],
    margin: f64,
    phase: &mut f64,
) -> Vec<(f64, f64)> {
    let (dx, dy) = (sb[0] - sa[0], sb[1] - sa[1]);
    let full = (dx * dx + dy * dy).sqrt();
    let mut out = Vec::new();
    let Some((t0, t1)) = ryucad::sketch_view::on_screen(sa, sb, size, margin) else {
        *phase = (*phase + full) % (on + off);
        return out;
    };
    // The pattern moves on over what isn't shown, before and after.
    *phase = (*phase + t0 * full) % (on + off);
    let len = (t1 - t0) * full;
    let mut s = 0.0;
    while s < len {
        let (drawn, left) = if *phase < on {
            (true, on - *phase)
        } else {
            (false, on + off - *phase)
        };
        let step = left.min(len - s);
        if drawn {
            out.push((t0 + s / full, t0 + (s + step) / full));
        }
        s += step;
        *phase = (*phase + step) % (on + off);
    }
    *phase = (*phase + (1.0 - t1) * full) % (on + off);
    out
}

fn lerp(a: [f64; 3], b: [f64; 3], t: f64) -> [f64; 3] {
    [0, 1, 2].map(|k| a[k] + (b[k] - a[k]) * t)
}

/// Bands `width` points wide along polylines in model space, as `pr`
/// sees them on a viewport `size` pixels big (`scale` pixels to a point),
/// each `(points, colour, dashed)`: triangles added to `out`, drawn with
/// depth like edges.
pub fn bands<'a>(
    lines: impl IntoIterator<Item = (&'a [[f64; 3]], [f32; 3], bool)>,
    pr: &Projector,
    size: [f64; 2],
    scale: f64,
    width: f64,
    out: &mut Vec<Corner>,
) {
    let (right, up, _) = pr.camera.basis();
    let screen = |p: [f64; 3]| {
        pr.ndc(p)
            .map(|n| [(n[0] + 1.0) / 2.0 * size[0], (1.0 - n[1]) / 2.0 * size[1]])
    };
    // A model offset `k` pixels along screen direction `d` (y down) at `p`.
    let offset = |p: [f64; 3], d: [f64; 2], k: f64| {
        let m = pr.pixel(p, size[1]) * k;
        [0, 1, 2].map(|i| (d[0] * right[i] - d[1] * up[i]) * m)
    };
    let add = |p: [f64; 3], o: [f64; 3], sign: f64| [0, 1, 2].map(|i| p[i] + sign * o[i]);
    let half = width * scale / 2.0;
    let [on, off] = DASH.map(|d| d * scale);
    for (points, color, dash) in lines {
        let mut phase = 0.0;
        for w in points.windows(2) {
            let (Some(sa), Some(sb)) = (screen(w[0]), screen(w[1])) else {
                continue;
            };
            let (dx, dy) = (sb[0] - sa[0], sb[1] - sa[1]);
            let len = (dx * dx + dy * dy).sqrt();
            if len == 0.0 {
                continue;
            }
            let n = [-dy / len, dx / len];
            let pieces = if dash {
                dashed(sa, sb, [on, off], size, half + 1.0, &mut phase)
            } else {
                vec![(0.0, 1.0)]
            };
            for (t0, t1) in pieces {
                let (a, b) = (lerp(w[0], w[1], t0), lerp(w[0], w[1], t1));
                let (oa, ob) = (offset(a, n, half), offset(b, n, half));
                let c = [
                    add(a, oa, 1.0),
                    add(b, ob, 1.0),
                    add(b, ob, -1.0),
                    add(a, oa, -1.0),
                ];
                for i in [0, 1, 2, 0, 2, 3] {
                    out.push((c[i], color));
                }
            }
        }
    }
}

/// What sketches add to the scene as `pr` sees it on a viewport `size`
/// pixels big (`scale` pixels to a point): triangles in model space, each
/// curve a band [`CURVE_WIDTH`] wide in its state's colour (construction
/// curves dashed), each point a square [`DOT_SIZE`] across. Drawn with
/// depth like edges, so they hide behind solids.
pub fn overlay<'a>(
    drawings: impl IntoIterator<Item = &'a Drawing>,
    pr: &Projector,
    size: [f64; 2],
    scale: f64,
) -> Vec<Corner> {
    let (right, up, _) = pr.camera.basis();
    // A model offset `k` pixels along screen direction `d` (y down) at `p`.
    let offset = |p: [f64; 3], d: [f64; 2], k: f64| {
        let m = pr.pixel(p, size[1]) * k;
        [0, 1, 2].map(|i| (d[0] * right[i] - d[1] * up[i]) * m)
    };
    let mut out = Vec::new();
    for d in drawings {
        bands(
            d.curves.iter().map(|c| {
                (
                    c.points.as_slice(),
                    ryucad::render::sketch_color(c.state),
                    c.construction,
                )
            }),
            pr,
            size,
            scale,
            CURVE_WIDTH,
            &mut out,
        );
        let r = DOT_SIZE * scale / 2.0;
        for p in &d.dots {
            let color = ryucad::render::sketch_color(p.state);
            let (x, y) = (offset(p.at, [1.0, 0.0], r), offset(p.at, [0.0, 1.0], r));
            let corner = |sx: f64, sy: f64| [0, 1, 2].map(|i| p.at[i] + sx * x[i] + sy * y[i]);
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
    }
    out
}

/// The origin's axes and planes as bands in the scene (`display.origin`):
/// `size` long, the axes [`CURVE_WIDTH`] wide in the triad's colours, the
/// planes dashed and half as wide, as `ryucad render --origin` draws them.
pub fn origin(pr: &Projector, size: [f64; 2], scale: f64, length: f64) -> Vec<Corner> {
    let (positions, lines) = ryucad::render::origin_lines(length);
    let polylines: Vec<(Vec<[f64; 3]>, [f32; 3], bool)> = lines
        .into_iter()
        .map(|(idx, color, dash)| {
            (
                idx.into_iter().map(|i| positions[i as usize]).collect(),
                color,
                dash,
            )
        })
        .collect();
    let mut out = Vec::new();
    for (points, color, dash) in &polylines {
        let width = if *dash {
            CURVE_WIDTH / 2.0
        } else {
            CURVE_WIDTH
        };
        bands(
            [(points.as_slice(), *color, *dash)],
            pr,
            size,
            scale,
            width,
            &mut out,
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_drawing_from_sketch_get() {
        let v = json!({
            "revision": "r1",
            "entities": {
                "l1": {"type": "line", "start": "p1", "end": "p2", "state": "well",
                       "polyline": [[0, 0, 0], [1, 0, 0]]},
                "l2": {"type": "line", "construction": true, "start": "p1", "end": "p2",
                       "state": "over", "polyline": [[0, 0, 0], [1, 0, 0]]},
                "p1": {"type": "point", "m": [0, 0], "at": [0, 0, 0], "state": "under"},
                "p2": {"type": "point", "m": [1, 0], "at": [1, 0, 0], "state": "failing"}
            },
            "constraints": {
                "c1": {"type": "horizontal", "a": "l1", "state": "ok"},
                "d1": {"type": "length", "a": "l1", "state": "redundant",
                       "value": {"value": "2 * w", "m": 1.0, "expr": true},
                       "label": {"at": [0.5, 0.1, 0], "text": "2 * w = 1000 mm"}}
            }
        });
        let d = Drawing::from_get(&v);
        assert_eq!(d.revision, "r1");
        assert_eq!(d.curves.len(), 2);
        assert_eq!(d.curves[0].state, EntityState::Well);
        assert!(d.curves[1].construction && !d.curves[0].construction);
        assert_eq!(d.curves[0].points, [[0.0; 3], [1.0, 0.0, 0.0]]);
        assert_eq!(d.dots[1].state, EntityState::Failing);
        assert_eq!(
            d.labels,
            [Label {
                constraint: "d1".into(),
                ty: "length".into(),
                state: ConstraintState::Redundant,
                at: [0.5, 0.1, 0.0],
                text: "2 * w = 1000 mm".into(),
                value: "2 * w".into(),
            }]
        );
        assert_eq!(d.points().count(), 6);
    }

    fn top() -> Projector {
        // Looking down at the xy plane, 0.1 m high on a 100 pixel view:
        // a pixel is a millimetre.
        let camera = ryucad::view::Camera {
            eye: [0.0, 0.0, 1.0],
            target: [0.0; 3],
            up: [0.0, 1.0, 0.0],
            projection: ryucad::view::Projection::Orthographic { height: 0.1 },
        };
        Projector::new(&camera, 1.0, [0.0; 3], 0.1)
    }

    #[test]
    fn curves_are_bands_and_points_squares() {
        let d = Drawing {
            curves: vec![Curve {
                entity: "l1".into(),
                ty: "line",
                construction: false,
                state: EntityState::Well,
                points: vec![[0.0, 0.0, 0.0], [0.02, 0.0, 0.0]],
            }],
            dots: vec![Dot {
                entity: "p1".into(),
                state: EntityState::Under,
                at: [0.0; 3],
            }],
            ..Drawing::default()
        };
        let o = overlay([&d], &top(), [100.0, 100.0], 1.0);
        assert_eq!(o.len(), 12);
        let well = ryucad::render::SKETCH_WELL;
        assert!(o[..6].iter().all(|c| c.1 == well));
        // Two pixels wide: a millimetre either side of the line.
        let ys: Vec<f64> = o[..6].iter().map(|c| c.0[1]).collect();
        let span = ys.iter().fold(f64::NEG_INFINITY, |m, y| m.max(*y))
            - ys.iter().fold(f64::INFINITY, |m, y| m.min(*y));
        assert!((span - 0.002).abs() < 1e-12, "{span}");
        // The dot: five pixels square.
        let xs: Vec<f64> = o[6..].iter().map(|c| c.0[0]).collect();
        let w = xs.iter().fold(f64::NEG_INFINITY, |m, x| m.max(*x))
            - xs.iter().fold(f64::INFINITY, |m, x| m.min(*x));
        assert!((w - 0.005).abs() < 1e-12, "{w}");
        assert!(o[6..].iter().all(|c| c.1 == ryucad::render::SKETCH_UNDER));
    }

    #[test]
    fn the_origin_is_three_axes_and_three_dashed_planes() {
        // From above, 20 mm axes: x and y show as bands, z is end on (no
        // length on screen, so no band); the xy plane's square shows dashed,
        // the others edge on.
        let o = origin(&top(), [100.0, 100.0], 1.0, 0.02);
        let of = |c: [f32; 3]| o.iter().filter(|x| x.1 == c).count();
        assert_eq!(of(ryucad::render::AXIS_X), 6);
        assert_eq!(of(ryucad::render::AXIS_Y), 6);
        assert_eq!(of(ryucad::render::AXIS_Z), 0);
        let planes = of(ryucad::render::ORIGIN_PLANE);
        assert!(planes > 6 && planes % 6 == 0, "{planes}");
        // The x axis runs 20 mm from the origin.
        let xs: Vec<f64> = o
            .iter()
            .filter(|x| x.1 == ryucad::render::AXIS_X)
            .map(|x| x.0[0])
            .collect();
        let (lo, hi) = (
            xs.iter().copied().fold(f64::INFINITY, f64::min),
            xs.iter().copied().fold(f64::NEG_INFINITY, f64::max),
        );
        assert!(lo.abs() < 1e-12 && (hi - 0.02).abs() < 1e-12, "{lo} {hi}");
    }

    #[test]
    fn construction_curves_are_dashed() {
        let (size, mut phase) = ([100.0, 100.0], 0.0);
        let d = dashed([0.0, 0.0], [25.0, 0.0], [6.0, 4.0], size, 1.0, &mut phase);
        assert_eq!(d, [(0.0, 0.24), (0.4, 0.64), (0.8, 1.0)]);
        // The pattern carries on into the next segment.
        assert_eq!(phase, 5.0);
        let d = dashed([0.0, 0.0], [10.0, 0.0], [6.0, 4.0], size, 1.0, &mut phase);
        assert_eq!(d, [(0.0, 0.1), (0.5, 1.0)]);
        // A line from just in front of the eye runs far off screen: only
        // what shows is dashed.
        let mut phase = 0.0;
        let d = dashed(
            [-1e12, 50.0],
            [1e12, 50.0],
            [6.0, 4.0],
            size,
            1.0,
            &mut phase,
        );
        assert!((10..=12).contains(&d.len()), "{}", d.len());
    }
}
