//! What the canvas draws, cut to its chord: each item's outline in sheet
//! millimetres, cut once per scene and zoom band, and its box for culling
//! (design 519: a 500-part sheet pans at 60 frames a second). Headless.

use crate::core::pick::text_box;
use crate::core::scene::{P, Scene, Shape};
use ryu_app::canvas::{Camera, arc_steps, bezier_at, meets};

/// The zoom band a zoom falls in: half an octave wide.
pub fn band(zoom: f64) -> i32 {
    (2.0 * ryu_math::ln(zoom.max(1e-6)) / std::f64::consts::LN_2).floor() as i32
}

/// The points of the arc from `a` through `m` to `c`, no chord further
/// than `chord` from it; a line when they are in a row.
pub fn arc_points(a: P, m: P, c: P, chord: f64) -> Vec<P> {
    let d = 2.0 * (a[0] * (m[1] - c[1]) + m[0] * (c[1] - a[1]) + c[0] * (a[1] - m[1]));
    if d.abs() < 1e-12 {
        return vec![a, c];
    }
    let sq = |p: P| p[0] * p[0] + p[1] * p[1];
    let ux = (sq(a) * (m[1] - c[1]) + sq(m) * (c[1] - a[1]) + sq(c) * (a[1] - m[1])) / d;
    let uy = (sq(a) * (c[0] - m[0]) + sq(m) * (a[0] - c[0]) + sq(c) * (m[0] - a[0])) / d;
    let r = ryu_math::hypot(a[0] - ux, a[1] - uy);
    let ang = |p: P| ryu_math::atan2(p[1] - uy, p[0] - ux);
    let tau = std::f64::consts::TAU;
    let (t0, tm, t1) = (ang(a), ang(m), ang(c));
    let ccw = |from: f64, to: f64| (to - from).rem_euclid(tau);
    // Counterclockwise from a to c passes m, or else clockwise does.
    let sweep = if ccw(t0, tm) <= ccw(t0, t1) {
        ccw(t0, t1)
    } else {
        ccw(t0, t1) - tau
    };
    let n = arc_steps(r, sweep, chord);
    let mut out: Vec<P> = (0..n)
        .map(|i| {
            let t = t0 + sweep * i as f64 / n as f64;
            [ux + r * ryu_math::cos(t), uy + r * ryu_math::sin(t)]
        })
        .collect();
    out.push(c);
    out
}

fn circle(c: P, r: f64, chord: f64) -> Vec<P> {
    let n = arc_steps(r, std::f64::consts::TAU, chord).max(12);
    let mut out: Vec<P> = (0..n)
        .map(|i| {
            let t = std::f64::consts::TAU * i as f64 / n as f64;
            [c[0] + r * ryu_math::cos(t), c[1] + r * ryu_math::sin(t)]
        })
        .collect();
    out.push(out[0]);
    out
}

fn bounds(pts: &[P], grow: f64) -> Option<[P; 2]> {
    let f = *pts.first()?;
    let [lo, hi] = pts.iter().fold([f, f], |[lo, hi], p| {
        [
            [lo[0].min(p[0]), lo[1].min(p[1])],
            [hi[0].max(p[0]), hi[1].max(p[1])],
        ]
    });
    Some([[lo[0] - grow, lo[1] - grow], [hi[0] + grow, hi[1] + grow]])
}

/// A camera that shows box `b` whole in a screen of `size` points, a
/// margin of 4% round it.
pub fn fit(b: [P; 2], size: P) -> Camera {
    let w = (b[1][0] - b[0][0]).max(1.0);
    let h = (b[1][1] - b[0][1]).max(1.0);
    let mut c = Camera::fit([w, h], size);
    c.centre = [(b[0][0] + b[1][0]) / 2.0, (b[0][1] + b[1][1]) / 2.0];
    c.zoom = c.zoom.min(200.0);
    c
}

/// A scene's items cut for one zoom band: outlines and boxes by item.
pub struct Cut {
    pub lines: Vec<Vec<P>>,
    pub boxes: Vec<Option<[P; 2]>>,
}

impl Cut {
    pub fn of(scene: &Scene, zoom: f64) -> Cut {
        // A quarter point at the band's closest zoom.
        let chord = 0.25 / ryu_math::exp(f64::from(band(zoom) + 1) * std::f64::consts::LN_2 / 2.0);
        let mut lines = Vec::with_capacity(scene.items.len());
        let mut boxes = Vec::with_capacity(scene.items.len());
        for it in &scene.items {
            let (pts, b) = match &it.shape {
                Shape::Polyline { points, width, .. } | Shape::Bus { points, width, .. } => {
                    (points.clone(), bounds(points, *width))
                }
                Shape::Circle {
                    center,
                    radius,
                    width,
                    ..
                } => {
                    let pts = circle(*center, *radius, chord);
                    let b = bounds(&pts, *width);
                    (pts, b)
                }
                Shape::Arc {
                    start,
                    mid,
                    end,
                    width,
                    ..
                } => {
                    let pts = arc_points(*start, *mid, *end, chord);
                    let b = bounds(&pts, *width);
                    (pts, b)
                }
                Shape::Bezier { points, width, .. } => {
                    let w = vec![1.0; points.len()];
                    let pts: Vec<P> = (0..=32)
                        .map(|i| bezier_at(points, &w, f64::from(i) / 32.0))
                        .collect();
                    let b = bounds(&pts, *width);
                    (pts, b)
                }
                Shape::Frame {
                    rect,
                    title_tab,
                    width,
                    ..
                } => {
                    let r = *rect;
                    let pts = vec![r[0], [r[1][0], r[0][1]], r[1], [r[0][0], r[1][1]], r[0]];
                    let mut all = pts.clone();
                    if let Some(t) = title_tab {
                        all.extend([t[0], t[1]]);
                    }
                    let b = bounds(&all, *width);
                    (pts, b)
                }
                Shape::Text {
                    at,
                    size,
                    anchor,
                    vertical,
                    width,
                    ..
                } => (
                    Vec::new(),
                    Some(text_box(*at, *size, *width, *anchor, *vertical)),
                ),
            };
            lines.push(pts);
            boxes.push(b);
        }
        Cut { lines, boxes }
    }

    /// The items whose boxes meet `view`.
    pub fn visible(&self, view: [P; 2]) -> impl Iterator<Item = usize> + '_ {
        self.boxes
            .iter()
            .enumerate()
            .filter(move |(_, b)| b.is_some_and(|b| meets(b, view)))
            .map(|(i, _)| i)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::scene::{Scene, tests::small};

    #[test]
    fn curves_are_cut_once_per_zoom_band_and_boxes_cull() {
        let s = Scene::from_json(&small()).unwrap();
        assert_eq!(band(4.2), band(4.6));
        assert_ne!(band(4.2), band(8.5));
        let g = Cut::of(&s, 4.0);
        assert_eq!(g.lines.len(), s.items.len());
        // The junction's circle is a closed ring of many points.
        assert!(g.lines[4].len() > 8);
        assert_eq!(g.lines[4].first(), g.lines[4].last());
        // Every item has a box, and the far corner of the paper sees none.
        assert!(g.boxes.iter().all(Option::is_some));
        let far = [[200.0, 150.0], [210.0, 160.0]];
        assert_eq!(g.visible(far).count(), 0);
        let near = [[-1.0, -1.0], [1.0, 1.0]];
        assert!(g.visible(near).count() >= 3);
    }

    #[test]
    fn arcs_pass_through_their_middle_and_fits_centre_the_paper() {
        let pts = arc_points([1.0, 0.0], [0.0, 1.0], [-1.0, 0.0], 0.01);
        assert_eq!(pts.first(), Some(&[1.0, 0.0]));
        let last = *pts.last().unwrap();
        assert!((last[0] + 1.0).abs() < 1e-9 && last[1].abs() < 1e-9);
        assert!(pts.iter().all(|p| p[1] >= -1e-9), "the upper half");
        let c = fit([[-10.0, -20.0], [287.0, 190.0]], [800.0, 600.0]);
        assert_eq!(c.centre, [138.5, 85.0]);
        let z = fit([[0.0, 0.0], [0.0, 0.0]], [800.0, 600.0]).zoom;
        assert!(z.is_finite() && z <= 200.0);
    }
}
