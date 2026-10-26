//! Looking around: how mouse input moves the window's camera in each
//! navigation style, and the projection that turns the camera into pixels
//! and pixels back into rays. Cameras are in meters, as in
//! `ryucad::view`; matrices are made relative to a reference point near
//! the model so f32 positions on the GPU stay precise.

use ryucad::view::{self, Camera, Projection, StandardView};

/// The `ui.navigation` preference: which mouse buttons orbit and pan.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Navigation {
    /// Left-drag orbits, right-drag pans (and Shift+left-drag).
    #[default]
    Ryucad,
    /// Middle-drag orbits, Ctrl+middle-drag pans.
    Solidworks,
    /// Right-drag orbits, middle-drag pans.
    Onshape,
}

impl Navigation {
    pub fn parse(s: &str) -> Navigation {
        match s {
            "solidworks" => Navigation::Solidworks,
            "onshape" => Navigation::Onshape,
            _ => Navigation::Ryucad,
        }
    }

    /// What a drag with these buttons and modifiers does.
    pub fn gesture(self, b: Buttons, m: Modifiers) -> Gesture {
        match self {
            Navigation::Ryucad if b.primary && m.shift => Gesture::Pan,
            Navigation::Ryucad if b.primary => Gesture::Orbit,
            Navigation::Ryucad if b.secondary || b.middle => Gesture::Pan,
            Navigation::Solidworks if b.middle && m.ctrl => Gesture::Pan,
            Navigation::Solidworks if b.middle => Gesture::Orbit,
            Navigation::Onshape if b.secondary => Gesture::Orbit,
            Navigation::Onshape if b.middle => Gesture::Pan,
            _ => Gesture::None,
        }
    }
}

/// The mouse buttons held during a drag.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Buttons {
    pub primary: bool,
    pub secondary: bool,
    pub middle: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub shift: bool,
    pub ctrl: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gesture {
    Orbit,
    Pan,
    None,
}

/// How far a drag across the viewport's height turns the view: half a
/// turn.
const ORBIT_PER_HEIGHT: f64 = core::f64::consts::PI;

/// How much one point of scrolling zooms.
const ZOOM_PER_POINT: f32 = 0.002;

/// The window's camera, and how it moves.
#[derive(Clone, Debug, PartialEq)]
pub struct Nav {
    pub camera: Camera,
    pub style: Navigation,
}

impl Nav {
    pub fn new(camera: Camera) -> Nav {
        Nav {
            camera,
            style: Navigation::default(),
        }
    }

    /// A drag of `delta` (in points, y down) across a viewport `height`
    /// points tall. Orbits turn about `pivot` (the point under the mouse
    /// when the drag began), else the target.
    pub fn drag(
        &mut self,
        gesture: Gesture,
        delta: [f32; 2],
        height: f32,
        pivot: Option<[f64; 3]>,
    ) {
        let (dx, dy) = (f64::from(delta[0] / height), f64::from(delta[1] / height));
        self.camera = match gesture {
            Gesture::Orbit => {
                let pivot = pivot.unwrap_or(self.camera.target);
                view::orbit(
                    &self.camera,
                    pivot,
                    -dx * ORBIT_PER_HEIGHT,
                    -dy * ORBIT_PER_HEIGHT,
                )
            }
            // The model follows the mouse.
            Gesture::Pan => view::pan(&self.camera, -dx, dy),
            Gesture::None => return,
        };
    }

    /// Scrolling by `points` (up is positive) zooms in towards `toward`,
    /// the point under the mouse, or the target.
    pub fn scroll(&mut self, points: f32, toward: Option<[f64; 3]>) {
        let factor = f64::from((-points * ZOOM_PER_POINT).exp());
        self.camera = view::zoom(&self.camera, factor, toward);
    }

    /// A pinch by `zoom` (above 1 is closer) zooms towards `toward`, the
    /// point under the mouse, or the target.
    pub fn pinch(&mut self, zoom: f32, toward: Option<[f64; 3]>) {
        self.camera = view::zoom(&self.camera, 1.0 / f64::from(zoom.max(1e-3)), toward);
    }

    /// A standard view framing `points` (not empty) in a viewport of
    /// `aspect`.
    pub fn standard(&mut self, v: StandardView, points: &[[f64; 3]], aspect: f64) {
        self.camera = view::frame_points(points, v, self.camera.projection, aspect);
    }

    /// The same direction, framing `points` (not empty) in a viewport of
    /// `aspect`.
    pub fn fit(&mut self, points: &[[f64; 3]], aspect: f64) {
        self.camera = view::refit_points(&self.camera, points, aspect);
    }
}

/// A 4×4 matrix in columns, as WGSL wants it.
pub type Mat4 = [[f32; 4]; 4];

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn along(p: [f64; 3], d: [f64; 3], k: f64) -> [f64; 3] {
    [p[0] + d[0] * k, p[1] + d[1] * k, p[2] + d[2] * k]
}

/// The camera on a viewport: how model points become pixels.
#[derive(Clone, Debug, PartialEq)]
pub struct Projector {
    pub camera: Camera,
    /// Width over height.
    pub aspect: f64,
    /// Depth range along the view direction, around the model.
    pub near: f64,
    pub far: f64,
}

impl Projector {
    /// The camera on a viewport of `aspect`, with the depth range around
    /// the model's bounding sphere (`centre`, `radius`).
    pub fn new(camera: &Camera, aspect: f64, centre: [f64; 3], radius: f64) -> Projector {
        let (_, _, forward) = camera.basis();
        let d = dot(sub(centre, camera.eye), forward);
        let r = radius.max(1e-6) * 1.05;
        let far = (d + r).max(1e-6);
        let near = match camera.projection {
            // Not too close: depth precision falls with near/far, and lines
            // are pulled in front of faces by a fraction of their distance.
            Projection::Perspective { .. } => (d - r).max(far * 1e-3),
            Projection::Orthographic { .. } => d - r,
        };
        Projector {
            camera: camera.clone(),
            aspect,
            near,
            far,
        }
    }

    /// Half the view's width and height at unit depth (perspective) or
    /// everywhere (orthographic).
    fn half(&self) -> (f64, f64) {
        let hh = match self.camera.projection {
            Projection::Perspective { .. } => self.camera.half_height(1.0),
            Projection::Orthographic { .. } => self.camera.half_height(0.0),
        };
        (hh * self.aspect, hh)
    }

    /// For pulling lines towards the eye (see the scene's line shader): 1
    /// in perspective, and the eye's own clip z there, which the
    /// projection gives the view-space origin.
    pub fn line_pull(&self) -> [f32; 2] {
        match self.camera.projection {
            Projection::Perspective { .. } => {
                let (n, fa) = (self.near, self.far);
                [1.0, (n * fa / (n - fa)) as f32]
            }
            Projection::Orthographic { .. } => [0.0, 0.0],
        }
    }

    /// Projection times view, for positions relative to `reference`.
    pub fn matrix(&self, reference: [f64; 3]) -> Mat4 {
        let (r, u, f) = self.camera.basis();
        let eye = sub(self.camera.eye, reference);
        // View: right, up and backwards are x, y and z.
        let view = [
            [r[0], u[0], -f[0], 0.0],
            [r[1], u[1], -f[1], 0.0],
            [r[2], u[2], -f[2], 0.0],
            [-dot(r, eye), -dot(u, eye), dot(f, eye), 1.0],
        ];
        let (hw, hh) = self.half();
        let (n, fa) = (self.near, self.far);
        // Depth 0 at near, 1 at far, as wgpu has it.
        let proj = match self.camera.projection {
            Projection::Perspective { .. } => [
                [1.0 / hw, 0.0, 0.0, 0.0],
                [0.0, 1.0 / hh, 0.0, 0.0],
                [0.0, 0.0, fa / (n - fa), -1.0],
                [0.0, 0.0, n * fa / (n - fa), 0.0],
            ],
            Projection::Orthographic { .. } => [
                [1.0 / hw, 0.0, 0.0, 0.0],
                [0.0, 1.0 / hh, 0.0, 0.0],
                [0.0, 0.0, 1.0 / (n - fa), 0.0],
                [0.0, 0.0, n / (n - fa), 1.0],
            ],
        };
        let mut out = [[0f32; 4]; 4];
        for (c, col) in out.iter_mut().enumerate() {
            for (row, x) in col.iter_mut().enumerate() {
                *x = (0..4).map(|k| proj[k][row] * view[c][k]).sum::<f64>() as f32;
            }
        }
        out
    }

    /// A model point in normalized device coordinates (x right, y up,
    /// both -1 to 1 across the viewport), or `None` behind the eye.
    pub fn ndc(&self, p: [f64; 3]) -> Option<[f64; 2]> {
        let (r, u, f) = self.camera.basis();
        let rel = sub(p, self.camera.eye);
        let (x, y, z) = (dot(rel, r), dot(rel, u), dot(rel, f));
        let (hw, hh) = self.half();
        match self.camera.projection {
            Projection::Perspective { .. } if z <= 0.0 => None,
            Projection::Perspective { .. } => Some([x / (hw * z), y / (hh * z)]),
            Projection::Orthographic { .. } => Some([x / hw, y / hh]),
        }
    }

    /// How far one pixel spans at `p`, in meters, on a viewport `height`
    /// pixels high: what marks a few pixels wide are made of.
    pub fn pixel(&self, p: [f64; 3], height: f64) -> f64 {
        let (_, hh) = self.half();
        let depth = match self.camera.projection {
            Projection::Perspective { .. } => {
                let (_, _, f) = self.camera.basis();
                dot(sub(p, self.camera.eye), f).max(0.0)
            }
            Projection::Orthographic { .. } => 1.0,
        };
        2.0 * hh * depth / height
    }

    /// Whether any of the box `lo`..`hi` may be in view: `false` only
    /// when all of it lies beyond one side of the view, or behind the eye.
    /// The depth range is made around the model, so it culls nothing.
    pub fn sees_box(&self, lo: [f64; 3], hi: [f64; 3]) -> bool {
        let (r, u, f) = self.camera.basis();
        let (hw, hh) = self.half();
        let perspective = matches!(self.camera.projection, Projection::Perspective { .. });
        // Each corner's offsets past the four sides (positive is outside)
        // and in front of the eye.
        let mut inside = [false; 5];
        for c in 0..8 {
            let p = [0, 1, 2].map(|k| if c >> k & 1 == 0 { lo[k] } else { hi[k] });
            let rel = sub(p, self.camera.eye);
            let (x, y, z) = (dot(rel, r), dot(rel, u), dot(rel, f));
            let s = if perspective { z } else { 1.0 };
            let sides = [x - hw * s, -x - hw * s, y - hh * s, -y - hh * s];
            for (i, d) in sides.iter().enumerate() {
                inside[i] |= *d <= 0.0;
            }
            inside[4] |= !perspective || z > 0.0;
        }
        inside.iter().all(|x| *x)
    }

    /// How many pixels the box's diagonal spans at its middle, on a
    /// viewport `height` pixels high: what chooses its level of detail.
    pub fn box_pixels(&self, lo: [f64; 3], hi: [f64; 3], height: f64) -> f64 {
        let d = sub(hi, lo);
        let middle = [0, 1, 2].map(|k| (lo[k] + hi[k]) / 2.0);
        let px = self.pixel(middle, height);
        if px > 0.0 {
            dot(d, d).sqrt() / px
        } else {
            f64::INFINITY
        }
    }

    /// The ray through a point in normalized device coordinates: its
    /// origin and unit direction.
    pub fn ray(&self, ndc: [f64; 2]) -> ([f64; 3], [f64; 3]) {
        let (r, u, f) = self.camera.basis();
        let (hw, hh) = self.half();
        let (sx, sy) = (ndc[0] * hw, ndc[1] * hh);
        match self.camera.projection {
            Projection::Perspective { .. } => {
                let d = along(along(f, r, sx), u, sy);
                let n = dot(d, d).sqrt();
                (self.camera.eye, d.map(|x| x / n))
            }
            Projection::Orthographic { .. } => {
                let o = along(along(self.camera.eye, r, sx), u, sy);
                (o, f)
            }
        }
    }

    /// The point on the ray through `ndc` as deep in the view as `point`:
    /// what the mouse is over, at the model's depth, when it is over
    /// nothing.
    pub fn at_depth(&self, ndc: [f64; 2], point: [f64; 3]) -> [f64; 3] {
        let (_, _, f) = self.camera.basis();
        let (o, d) = self.ray(ndc);
        let k = dot(sub(point, o), f) / dot(d, f);
        along(o, d, k)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn iso() -> Camera {
        view::frame(
            [0.0; 3],
            [0.02, 0.01, 0.005],
            StandardView::Iso,
            view::DEFAULT_PROJECTION,
        )
    }

    fn mul(m: &Mat4, p: [f64; 3]) -> [f64; 4] {
        let v = [p[0], p[1], p[2], 1.0];
        let mut out = [0.0; 4];
        for (row, o) in out.iter_mut().enumerate() {
            *o = (0..4).map(|c| f64::from(m[c][row]) * v[c]).sum();
        }
        out
    }

    #[test]
    fn each_style_has_its_buttons() {
        let left = Buttons {
            primary: true,
            ..Default::default()
        };
        let right = Buttons {
            secondary: true,
            ..Default::default()
        };
        let middle = Buttons {
            middle: true,
            ..Default::default()
        };
        let none = Modifiers::default();
        let shift = Modifiers {
            shift: true,
            ..Default::default()
        };
        let ctrl = Modifiers {
            ctrl: true,
            ..Default::default()
        };
        let table = [
            (Navigation::Ryucad, left, none, Gesture::Orbit),
            (Navigation::Ryucad, left, shift, Gesture::Pan),
            (Navigation::Ryucad, right, none, Gesture::Pan),
            (Navigation::Solidworks, middle, none, Gesture::Orbit),
            (Navigation::Solidworks, middle, ctrl, Gesture::Pan),
            (Navigation::Solidworks, left, none, Gesture::None),
            (Navigation::Onshape, right, none, Gesture::Orbit),
            (Navigation::Onshape, middle, none, Gesture::Pan),
            (Navigation::Onshape, left, none, Gesture::None),
        ];
        for (style, b, m, want) in table {
            assert_eq!(style.gesture(b, m), want, "{style:?} {b:?} {m:?}");
        }
        assert_eq!(Navigation::parse("onshape"), Navigation::Onshape);
        assert_eq!(Navigation::parse("anything else"), Navigation::Ryucad);
    }

    #[test]
    fn drags_and_scrolls_move_the_camera() {
        let mut nav = Nav::new(iso());
        let d = nav.camera.distance();
        nav.drag(Gesture::Orbit, [100.0, 0.0], 500.0, None);
        assert!((nav.camera.distance() - d).abs() < 1e-12);
        assert_ne!(nav.camera.eye, iso().eye);
        let before = nav.camera.target;
        nav.drag(Gesture::Pan, [50.0, 0.0], 500.0, None);
        assert_ne!(nav.camera.target, before);
        nav.scroll(100.0, None);
        assert!(nav.camera.distance() < d);
        let same = nav.clone();
        nav.drag(Gesture::None, [10.0, 10.0], 500.0, None);
        assert_eq!(nav, same);
        // From above, at the points' middle depth (moved across, since the
        // nearer one looks bigger).
        nav.standard(StandardView::Top, &[[0.0; 3], [1.0; 3]], 1.0);
        assert_eq!(nav.camera.basis().2, [0.0, 0.0, -1.0]);
        assert_eq!(nav.camera.target[2], 0.5);
    }

    #[test]
    fn the_matrix_agrees_with_the_projection_and_the_rays() {
        let ortho = Camera {
            projection: Projection::Orthographic { height: 0.03 },
            ..iso()
        };
        for camera in [iso(), ortho] {
            let pr = Projector::new(&camera, 1.5, [0.01, 0.005, 0.0025], 0.012);
            assert!(pr.near < pr.far);
            let reference = [0.01, 0.0, 0.0];
            let m = pr.matrix(reference);
            for p in [[0.0, 0.0, 0.0], [0.02, 0.01, 0.005], [0.003, 0.007, 0.001]] {
                let c = mul(&m, sub(p, reference));
                let ndc = pr.ndc(p).unwrap();
                assert!((c[0] / c[3] - ndc[0]).abs() < 1e-5, "{p:?}");
                assert!((c[1] / c[3] - ndc[1]).abs() < 1e-5, "{p:?}");
                let depth = c[2] / c[3];
                assert!((0.0..=1.0).contains(&depth), "{depth}");
                // The ray through the point's pixel passes through it.
                let (o, d) = pr.ray(ndc);
                let t = dot(sub(p, o), d);
                let miss = sub(along(o, d, t), p);
                assert!(dot(miss, miss).sqrt() < 1e-9);
            }
            // The target is in the middle.
            let mid = pr.ndc(camera.target).unwrap();
            assert!(mid[0].abs() < 1e-12 && mid[1].abs() < 1e-12);
        }
        let pr = Projector::new(&iso(), 1.0, [0.0; 3], 0.01);
        let behind = along(iso().eye, sub(iso().eye, iso().target), 1.0);
        assert_eq!(pr.ndc(behind), None);
    }

    #[test]
    fn a_point_on_a_ray_at_a_depth() {
        let model = [0.003, -0.002, 0.004];
        for projection in [
            view::DEFAULT_PROJECTION,
            Projection::Orthographic { height: 0.05 },
        ] {
            let cam = Camera {
                projection,
                ..iso()
            };
            let pr = Projector::new(&cam, 1.5, [0.0; 3], 0.01);
            let at = [0.4, -0.7];
            let p = pr.at_depth(at, model);
            let n = pr.ndc(p).unwrap();
            assert!((n[0] - at[0]).abs() < 1e-9 && (n[1] - at[1]).abs() < 1e-9);
            let (_, _, f) = cam.basis();
            let depth = |q: [f64; 3]| dot(sub(q, cam.eye), f);
            assert!((depth(p) - depth(model)).abs() < 1e-12);
        }
    }
}
