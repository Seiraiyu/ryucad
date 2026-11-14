//! A 2D canvas over a sheet (design 519, The app's home): its camera, the
//! wheel and buttons that move it, curves cut to the screen's chord, hit
//! tests over boxes and polylines, and culling to what the screen shows.
//! Sheet units are the product's (ryucad's drawings: millimetres; ryueda
//! converts its nanometres to them at the edge). Moved from ryucad-app's
//! Drawings tile by ryueda 3.C.

/// A point on the sheet, y up.
pub type P = [f64; 2];

/// How many pieces an arc of `radius` turning `sweep` takes so no chord
/// strays more than `chord` from it: from 1 to 1,024.
pub fn arc_steps(radius: f64, sweep: f64, chord: f64) -> usize {
    let r = radius.abs().max(1e-9);
    let c = (chord / r).min(1.0);
    let step = 2.0 * ryu_math::acos(1.0 - c);
    if step <= 0.0 || !step.is_finite() {
        return 1024;
    }
    ((sweep.abs() / step).ceil() as usize).clamp(1, 1024)
}

/// A rational Bézier's point at `t` (de Casteljau, homogeneous).
pub fn bezier_at(ctrl: &[P], w: &[f64], t: f64) -> P {
    let mut q: Vec<[f64; 3]> = ctrl
        .iter()
        .zip(w)
        .map(|(p, w)| [p[0] * w, p[1] * w, *w])
        .collect();
    let n = q.len();
    for k in 1..n {
        for i in 0..n - k {
            let next = q[i + 1];
            for (c, x) in q[i].iter_mut().enumerate() {
                *x = (1.0 - t) * *x + t * next[c];
            }
        }
    }
    [q[0][0] / q[0][2], q[0][1] / q[0][2]]
}

/// The 2D camera over a sheet: the sheet point at the screen's middle and
/// how many screen points a sheet unit takes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    pub centre: P,
    pub zoom: f64,
}

impl Default for Camera {
    fn default() -> Camera {
        Camera {
            centre: [0.0, 0.0],
            zoom: 1.0,
        }
    }
}

impl Camera {
    /// The whole sheet in a screen of `size` points, a margin of 4% round
    /// it.
    pub fn fit(paper: P, size: P) -> Camera {
        let z = (size[0] / paper[0]).min(size[1] / paper[1]) * 0.96;
        Camera {
            centre: [paper[0] / 2.0, paper[1] / 2.0],
            zoom: if z.is_finite() && z > 0.0 { z } else { 1.0 },
        }
    }

    /// A sheet point on a screen of `size` points (y down, its origin the
    /// screen's top left).
    pub fn to_screen(&self, p: P, size: P) -> P {
        [
            size[0] / 2.0 + (p[0] - self.centre[0]) * self.zoom,
            size[1] / 2.0 - (p[1] - self.centre[1]) * self.zoom,
        ]
    }

    pub fn to_sheet(&self, q: P, size: P) -> P {
        [
            self.centre[0] + (q[0] - size[0] / 2.0) / self.zoom,
            self.centre[1] - (q[1] - size[1] / 2.0) / self.zoom,
        ]
    }

    /// Moved by a drag of `d` screen points.
    pub fn pan(&mut self, d: P) {
        self.centre = [
            self.centre[0] - d[0] / self.zoom,
            self.centre[1] + d[1] / self.zoom,
        ];
    }

    /// Zoomed by `factor` about screen point `at`, which stays where it
    /// is; from a twentieth to 200 points a unit.
    pub fn zoom_at(&mut self, factor: f64, at: P, size: P) {
        let before = self.to_sheet(at, size);
        self.zoom = (self.zoom * factor).clamp(0.05, 200.0);
        let after = self.to_sheet(at, size);
        self.centre = [
            self.centre[0] + before[0] - after[0],
            self.centre[1] + before[1] - after[1],
        ];
    }

    /// The chord curves are cut to: a quarter of a screen point.
    pub fn chord(&self) -> f64 {
        0.25 / self.zoom
    }
}

/// The distance from `p` to the segment from `a` to `b`.
pub fn segment_distance(p: P, a: P, b: P) -> f64 {
    let d = [b[0] - a[0], b[1] - a[1]];
    let l2 = d[0] * d[0] + d[1] * d[1];
    let t = if l2 > 0.0 {
        (((p[0] - a[0]) * d[0] + (p[1] - a[1]) * d[1]) / l2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    ryu_math::hypot(p[0] - a[0] - t * d[0], p[1] - a[1] - t * d[1])
}

/// Whether `p` is in box `b` grown by `grow`.
pub fn inside(p: P, b: [P; 2], grow: f64) -> bool {
    p[0] >= b[0][0] - grow
        && p[0] <= b[1][0] + grow
        && p[1] >= b[0][1] - grow
        && p[1] <= b[1][1] + grow
}

/// The least distance from `p` to a polyline's segments; infinite for
/// none, a point's distance for one.
pub fn polyline_distance(p: P, pts: &[P]) -> f64 {
    match pts {
        [] => f64::INFINITY,
        [a] => segment_distance(p, *a, *a),
        _ => pts
            .windows(2)
            .map(|w| segment_distance(p, w[0], w[1]))
            .fold(f64::INFINITY, f64::min),
    }
}

/// The sheet box a screen of `size` points shows: `[low, high]`.
pub fn visible(cam: &Camera, size: P) -> [P; 2] {
    let top_left = cam.to_sheet([0.0, 0.0], size);
    let bottom_right = cam.to_sheet(size, size);
    [
        [top_left[0], bottom_right[1]],
        [bottom_right[0], top_left[1]],
    ]
}

/// Whether two boxes share a point: what to draw of what is visible.
pub fn meets(a: [P; 2], b: [P; 2]) -> bool {
    a[0][0] <= b[1][0] && b[0][0] <= a[1][0] && a[0][1] <= b[1][1] && b[0][1] <= a[1][1]
}

/// The wheel zooms at the pointer, the middle or right button pans: what
/// every canvas does with its response `r` over `rect`.
pub fn navigate(ui: &egui::Ui, r: &egui::Response, rect: egui::Rect, cam: &mut Camera) {
    let px = [f64::from(rect.width()), f64::from(rect.height())];
    if r.hovered() {
        let (scroll, at) = ui.input(|i| (i.smooth_scroll_delta.y, i.pointer.hover_pos()));
        if scroll != 0.0
            && let Some(at) = at
        {
            let f = ryu_math::exp(f64::from(scroll) / 200.0);
            cam.zoom_at(
                f,
                [f64::from(at.x - rect.left()), f64::from(at.y - rect.top())],
                px,
            );
        }
    }
    if r.dragged_by(egui::PointerButton::Middle) || r.dragged_by(egui::PointerButton::Secondary) {
        let d = r.drag_delta();
        cam.pan([f64::from(d.x), f64::from(d.y)]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_camera_fits_zooms_about_the_pointer_and_pans() {
        let size = [800.0, 600.0];
        let mut c = Camera::fit([297.0, 210.0], size);
        assert_eq!(c.centre, [148.5, 105.0]);
        let at = [100.0, 100.0];
        let before = c.to_sheet(at, size);
        c.zoom_at(2.0, at, size);
        let after = c.to_sheet(at, size);
        assert!((before[0] - after[0]).abs() < 1e-9 && (before[1] - after[1]).abs() < 1e-9);
        let p = c.to_screen([10.0, 20.0], size);
        let q = c.to_sheet(p, size);
        assert!((q[0] - 10.0).abs() < 1e-9 && (q[1] - 20.0).abs() < 1e-9);
        c.pan([c.zoom, 0.0]);
        assert!((c.to_sheet(at, size)[0] - (after[0] - 1.0)).abs() < 1e-9);
    }

    #[test]
    fn hit_tests_and_culling() {
        assert_eq!(segment_distance([0.0, 1.0], [-1.0, 0.0], [1.0, 0.0]), 1.0);
        assert_eq!(
            polyline_distance([3.0, 0.0], &[[0.0, 0.0], [1.0, 0.0], [1.0, 5.0]]),
            2.0
        );
        assert_eq!(polyline_distance([3.0, 0.0], &[]), f64::INFINITY);
        assert!(inside([1.0, 1.0], [[0.0, 0.0], [2.0, 2.0]], 0.0));
        assert!(inside([2.5, 1.0], [[0.0, 0.0], [2.0, 2.0]], 0.5));
        let c = Camera {
            centre: [50.0, 50.0],
            zoom: 1.0,
        };
        let v = visible(&c, [20.0, 10.0]);
        assert_eq!(v, [[40.0, 45.0], [60.0, 55.0]]);
        assert!(meets(v, [[59.0, 54.0], [70.0, 70.0]]));
        assert!(!meets(v, [[61.0, 0.0], [70.0, 70.0]]));
        assert_eq!(arc_steps(10.0, 0.0, 0.01), 1);
        assert!(arc_steps(10.0, std::f64::consts::TAU, 0.01) > 50);
        let mid = bezier_at(&[[0.0, 0.0], [1.0, 2.0], [2.0, 0.0]], &[1.0, 1.0, 1.0], 0.5);
        assert_eq!(mid, [1.0, 1.0]);
    }
}
