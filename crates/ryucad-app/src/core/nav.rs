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
    /// A turn to a view under way: the cube's clicks and arrows,
    /// standard and named views, framing, and a followed camera.
    pub turn: Option<Turn>,
    /// An orbit or pan carrying on after the button is let go.
    pub glide: Option<Glide>,
    /// Whether turns ease and drags glide (`ui.motion`: `animate`); with
    /// `reduce` every move is at once.
    pub animate: bool,
}

/// How long a turn to a view takes, in seconds.
pub const TURN_SECONDS: f64 = 0.25;

/// How long a glide takes to stop, in seconds.
pub const GLIDE_SECONDS: f64 = 0.3;

/// The slowest let-go that glides, in points a second.
pub const GLIDE_FROM: f32 = 60.0;

/// An orbit or pan carrying on after the button is let go: at the speed
/// it was let go at, slowing linearly to rest over [`GLIDE_SECONDS`].
#[derive(Clone, Debug, PartialEq)]
pub struct Glide {
    pub gesture: Gesture,
    /// Points a second, y down, when let go.
    pub velocity: [f32; 2],
    /// The viewport's height in points, as the drag had it.
    pub height: f32,
    pub pivot: Option<[f64; 3]>,
    pub start: f64,
    /// When it last moved.
    pub last: f64,
}

/// A camera turning from `from` to `to`, begun at `start` (seconds, as
/// the frame's input counts them).
#[derive(Clone, Debug, PartialEq)]
pub struct Turn {
    pub from: Camera,
    pub to: Camera,
    pub start: f64,
}

/// A rotation as a unit quaternion `[w, x, y, z]`.
type Quat = [f64; 4];

/// The rotation taking the model's axes to a camera's right, up and back
/// (towards the eye), from the matrix with those columns (Shepperd's
/// method: the largest of the four squares first, so no root is of a
/// small difference).
fn quat(right: [f64; 3], up: [f64; 3], back: [f64; 3]) -> Quat {
    let m = |r: usize, c: usize| [right, up, back][c][r];
    let trace = m(0, 0) + m(1, 1) + m(2, 2);
    let q = if trace > 0.0 {
        let s = (trace + 1.0).sqrt() * 2.0;
        [
            s / 4.0,
            (m(2, 1) - m(1, 2)) / s,
            (m(0, 2) - m(2, 0)) / s,
            (m(1, 0) - m(0, 1)) / s,
        ]
    } else if m(0, 0) > m(1, 1) && m(0, 0) > m(2, 2) {
        let s = (1.0 + m(0, 0) - m(1, 1) - m(2, 2)).sqrt() * 2.0;
        [
            (m(2, 1) - m(1, 2)) / s,
            s / 4.0,
            (m(0, 1) + m(1, 0)) / s,
            (m(0, 2) + m(2, 0)) / s,
        ]
    } else if m(1, 1) > m(2, 2) {
        let s = (1.0 + m(1, 1) - m(0, 0) - m(2, 2)).sqrt() * 2.0;
        [
            (m(0, 2) - m(2, 0)) / s,
            (m(0, 1) + m(1, 0)) / s,
            s / 4.0,
            (m(1, 2) + m(2, 1)) / s,
        ]
    } else {
        let s = (1.0 + m(2, 2) - m(0, 0) - m(1, 1)).sqrt() * 2.0;
        [
            (m(1, 0) - m(0, 1)) / s,
            (m(0, 2) + m(2, 0)) / s,
            (m(1, 2) + m(2, 1)) / s,
            s / 4.0,
        ]
    };
    let n = q.iter().map(|x| x * x).sum::<f64>().sqrt();
    q.map(|x| x / n)
}

/// A unit quaternion's up and back: its matrix's second and third
/// columns.
fn up_back(q: Quat) -> ([f64; 3], [f64; 3]) {
    let [w, x, y, z] = q;
    (
        [
            2.0 * (x * y - w * z),
            1.0 - 2.0 * (x * x + z * z),
            2.0 * (y * z + w * x),
        ],
        [
            2.0 * (x * z + w * y),
            2.0 * (y * z - w * x),
            1.0 - 2.0 * (x * x + y * y),
        ],
    )
}

/// The camera `t` of the way (0 to 1) from `a` to `b`, eased in and out:
/// its turn the shorter way between their orientations (quaternions
/// interpolated and normalized: square roots only, no platform
/// trigonometry), its target moved straight, its distance and
/// orthographic height or field of view in between. Projections of two
/// kinds take `b`'s.
pub fn between(a: &Camera, b: &Camera, t: f64) -> Camera {
    let s = t.clamp(0.0, 1.0);
    let s = s * s * (3.0 - 2.0 * s);
    let q = |c: &Camera| {
        let (r, u, f) = c.basis();
        quat(r, u, f.map(|x| -x))
    };
    let (qa, mut qb) = (q(a), q(b));
    if (0..4).map(|i| qa[i] * qb[i]).sum::<f64>() < 0.0 {
        qb = qb.map(|x| -x);
    }
    let mixed: Quat = [0, 1, 2, 3].map(|i| qa[i] + (qb[i] - qa[i]) * s);
    let n = mixed.iter().map(|x| x * x).sum::<f64>().sqrt();
    let (up, back) = up_back(mixed.map(|x| x / n));
    let lerp = |x: f64, y: f64| x + (y - x) * s;
    let target = [0, 1, 2].map(|k| lerp(a.target[k], b.target[k]));
    let d = lerp(a.distance(), b.distance());
    let projection = match (a.projection, b.projection) {
        (Projection::Perspective { fov_deg: x }, Projection::Perspective { fov_deg: y }) => {
            Projection::Perspective {
                fov_deg: lerp(x, y),
            }
        }
        (Projection::Orthographic { height: x }, Projection::Orthographic { height: y }) => {
            Projection::Orthographic { height: lerp(x, y) }
        }
        (_, p) => p,
    };
    Camera {
        eye: [0, 1, 2].map(|k| target[k] + back[k] * d),
        target,
        up,
        projection,
    }
}

impl Nav {
    pub fn new(camera: Camera) -> Nav {
        Nav {
            camera,
            style: Navigation::default(),
            turn: None,
            glide: None,
            animate: true,
        }
    }

    /// Starts turning to `to` at `now` (seconds); with motion reduced it
    /// is there at once.
    pub fn turn_to(&mut self, to: Camera, now: f64) {
        self.glide = None;
        if !self.animate {
            self.camera = to;
            self.turn = None;
            return;
        }
        self.turn = Some(Turn {
            from: self.camera.clone(),
            to,
            start: now,
        });
    }

    /// Whether a turn or a glide is under way.
    pub fn moving(&self) -> bool {
        self.turn.is_some() || self.glide.is_some()
    }

    /// A drag let go at `velocity` (points a second): an orbit or pan
    /// glides on, if it was moving fast enough and motion isn't reduced.
    pub fn let_go(
        &mut self,
        gesture: Gesture,
        velocity: [f32; 2],
        height: f32,
        pivot: Option<[f64; 3]>,
        now: f64,
    ) {
        let speed = (velocity[0] * velocity[0] + velocity[1] * velocity[1]).sqrt();
        if !self.animate || gesture == Gesture::None || speed < GLIDE_FROM {
            return;
        }
        self.glide = Some(Glide {
            gesture,
            velocity,
            height,
            pivot,
            start: now,
            last: now,
        });
    }

    /// Moves a glide on to `now`: by the distance a linearly slowing
    /// speed covers since its last step. Whether it is still under way.
    fn glide_on(&mut self, now: f64) -> bool {
        let Some(g) = self.glide.clone() else {
            return false;
        };
        // The speed falls from 1 to 0 over the glide; the distance since
        // `start` is the integral, t - t² / 2T.
        let covered = |at: f64| {
            let t = (at - g.start).clamp(0.0, GLIDE_SECONDS);
            t - t * t / (2.0 * GLIDE_SECONDS)
        };
        let k = (covered(now) - covered(g.last)) as f32;
        let delta = [g.velocity[0] * k, g.velocity[1] * k];
        self.drag(g.gesture, delta, g.height, g.pivot);
        // `drag` is a hand's and stops the glide; this step is the glide's.
        let done = now - g.start >= GLIDE_SECONDS;
        self.glide = (!done).then_some(Glide { last: now, ..g });
        !done
    }

    /// Moves a turn or a glide under way on to `now`; whether it is still
    /// under way. A turn ends exactly at its camera.
    pub fn step(&mut self, now: f64) -> bool {
        if self.turn.is_none() {
            return self.glide_on(now);
        }
        let Some(turn) = &self.turn else {
            return false;
        };
        let t = (now - turn.start) / TURN_SECONDS;
        if t >= 1.0 {
            self.camera = turn.to.clone();
            self.turn = None;
            return false;
        }
        self.camera = between(&turn.from, &turn.to, t);
        true
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
        if gesture != Gesture::None {
            // The person's hand stops a turn or a glide where it is.
            self.turn = None;
            self.glide = None;
        }
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
        self.turn = None;
        self.glide = None;
        self.camera = view::zoom(&self.camera, factor, toward);
    }

    /// A pinch by `zoom` (above 1 is closer) zooms towards `toward`, the
    /// point under the mouse, or the target.
    pub fn pinch(&mut self, zoom: f32, toward: Option<[f64; 3]>) {
        self.turn = None;
        self.glide = None;
        self.camera = view::zoom(&self.camera, 1.0 / f64::from(zoom.max(1e-3)), toward);
    }

    /// A standard view framing `points` (not empty) in a viewport of
    /// `aspect`.
    pub fn standard(&mut self, v: StandardView, points: &[[f64; 3]], aspect: f64) {
        self.camera = view::frame_points(points, v, self.camera.projection, aspect);
    }

    /// The same direction, framing `points` (not empty) in a viewport of
    /// `aspect`, at once.
    pub fn fit(&mut self, points: &[[f64; 3]], aspect: f64) {
        self.camera = view::refit_points(&self.camera, points, aspect);
    }

    /// The same, turning there from `now`: the direction a turn under
    /// way is going to, kept.
    pub fn fit_to(&mut self, points: &[[f64; 3]], aspect: f64, now: f64) {
        let from = self.turn.as_ref().map_or(&self.camera, |t| &t.to).clone();
        let to = view::refit_points(&from, points, aspect);
        self.turn_to(to, now);
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
        self.cull().sight(lo, hi) != Sight::Out
    }

    /// What [`Projector::sees_box`] needs, worked out once for many boxes.
    pub fn cull(&self) -> Cull {
        let (r, u, f) = self.camera.basis();
        let (hw, hh) = self.half();
        Cull {
            r,
            u,
            f,
            eye: self.camera.eye,
            hw,
            hh,
            perspective: matches!(self.camera.projection, Projection::Perspective { .. }),
        }
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

/// How much of a box is in view ([`Cull::sight`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sight {
    /// All of it lies beyond one side of the view, or behind the eye.
    Out,
    /// Some of it may be in view.
    Partly,
    /// All of it is in view, in front of the eye.
    In,
}

/// A view's sides, for judging many boxes ([`Projector::cull`]).
pub struct Cull {
    r: [f64; 3],
    u: [f64; 3],
    f: [f64; 3],
    eye: [f64; 3],
    hw: f64,
    hh: f64,
    perspective: bool,
}

impl Cull {
    /// How much of the box `lo`..`hi` is in view: [`Sight::Out`] only
    /// when all of it lies beyond one side, or behind the eye; [`Sight::In`]
    /// when every corner is inside every side. The depth range is made
    /// around the model, so it culls nothing.
    pub fn sight(&self, lo: [f64; 3], hi: [f64; 3]) -> Sight {
        // Each corner's offsets past the four sides (positive is outside)
        // and in front of the eye.
        let mut inside = [false; 5];
        let mut all = true;
        for c in 0..8 {
            let p = [0, 1, 2].map(|k| if c >> k & 1 == 0 { lo[k] } else { hi[k] });
            let rel = sub(p, self.eye);
            let (x, y, z) = (dot(rel, self.r), dot(rel, self.u), dot(rel, self.f));
            let s = if self.perspective { z } else { 1.0 };
            let sides = [
                x - self.hw * s,
                -x - self.hw * s,
                y - self.hh * s,
                -y - self.hh * s,
            ];
            for (i, d) in sides.iter().enumerate() {
                inside[i] |= *d <= 0.0;
                all &= *d <= 0.0;
            }
            let front = !self.perspective || z > 0.0;
            inside[4] |= front;
            all &= front;
        }
        if !inside.iter().all(|x| *x) {
            Sight::Out
        } else if all {
            Sight::In
        } else {
            Sight::Partly
        }
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
    fn a_turn_eases_from_one_view_to_another_and_ends_on_it() {
        let points = [[0.0; 3], [0.02, 0.01, 0.005]];
        let from = view::frame_points(&points, StandardView::Front, view::DEFAULT_PROJECTION, 1.5);
        for v in view::STANDARD_VIEWS.map(|(_, v)| v) {
            let to = view::frame_points(&points, v, view::DEFAULT_PROJECTION, 1.5);
            let mut nav = Nav::new(from.clone());
            nav.turn_to(to.clone(), 10.0);
            assert!(nav.step(10.0));
            // It starts where it was, within rounding.
            assert!(dot(sub(nav.camera.eye, from.eye), sub(nav.camera.eye, from.eye)) < 1e-24);
            assert!(nav.step(10.0 + TURN_SECONDS / 2.0));
            // Midway it is a camera, its up across the view.
            let mid = nav.camera.clone();
            assert!(mid.check().is_ok(), "{v:?}");
            let (_, _, f) = mid.basis();
            assert!(dot(mid.up, f).abs() < 1e-9, "{v:?}");
            assert!(!nav.step(10.0 + TURN_SECONDS));
            assert_eq!(nav.camera, to);
            assert_eq!(nav.turn, None);
        }
        // A hand on the mouse stops a turn.
        let mut nav = Nav::new(from.clone());
        nav.turn_to(iso(), 0.0);
        nav.drag(Gesture::Orbit, [10.0, 0.0], 500.0, None);
        assert_eq!(nav.turn, None);
        // The orientation turns the shorter way: front to right is a
        // quarter turn, so midway looks along the diagonal.
        let right = view::frame_points(&points, StandardView::Right, view::DEFAULT_PROJECTION, 1.5);
        let mid = between(&from, &right, 0.5);
        let (_, _, f) = mid.basis();
        let s = 0.5f64.sqrt();
        assert!(dot(f, [-s, s, 0.0]) > 1.0 - 1e-12, "{f:?}");
        // A switch of projection takes the new one at once.
        let ortho = Camera {
            projection: Projection::Orthographic { height: 0.03 },
            ..from.clone()
        };
        assert_eq!(between(&from, &ortho, 0.1).projection, ortho.projection);
    }

    #[test]
    fn a_let_go_glides_and_comes_to_rest() {
        let mut nav = Nav::new(iso());
        // Too slow to glide.
        nav.let_go(Gesture::Orbit, [10.0, 0.0], 500.0, None, 0.0);
        assert!(!nav.moving());
        // A quick orbit carries on, each step less, and stops.
        nav.let_go(Gesture::Orbit, [600.0, 0.0], 500.0, None, 0.0);
        let mut moved = Vec::new();
        let mut last = nav.camera.clone();
        let mut t = 0.0;
        while nav.moving() {
            t += 0.05;
            nav.step(t);
            let d = sub(nav.camera.eye, last.eye);
            moved.push(dot(d, d).sqrt());
            last = nav.camera.clone();
        }
        assert!(t <= GLIDE_SECONDS + 0.05 + 1e-9, "{t}");
        assert!(moved.len() >= 5, "{moved:?}");
        assert!(moved.windows(2).all(|w| w[1] <= w[0] + 1e-15), "{moved:?}");
        // The whole glide is what the linear slowing covers: half the
        // let-go speed for its time, so a drag of 600 × 0.15 points.
        let mut drag = Nav::new(iso());
        drag.drag(Gesture::Orbit, [90.0, 0.0], 500.0, None);
        let e = sub(nav.camera.eye, drag.camera.eye);
        assert!(dot(e, e) < 1e-18, "{e:?}");
        // A hand stops it.
        nav.let_go(Gesture::Pan, [0.0, 800.0], 500.0, None, 1.0);
        nav.scroll(5.0, None);
        assert!(!nav.moving());
        // Motion reduced: no glide, and turns land at once.
        nav.animate = false;
        nav.let_go(Gesture::Orbit, [600.0, 0.0], 500.0, None, 2.0);
        assert!(!nav.moving());
        let to = Camera {
            eye: [1.0, 2.0, 3.0],
            ..iso()
        };
        nav.turn_to(to.clone(), 2.0);
        assert!(!nav.moving());
        assert_eq!(nav.camera, to);
    }

    #[test]
    fn framing_turns_there() {
        let points = [[0.0; 3], [0.02, 0.01, 0.005]];
        let mut nav = Nav::new(iso());
        nav.fit_to(&points, 1.5, 5.0);
        let to = nav.turn.as_ref().unwrap().to.clone();
        assert_eq!(to, view::refit_points(&iso(), &points, 1.5));
        // Framed again while turning: from where it is going.
        nav.step(5.1);
        nav.fit_to(&points, 1.5, 5.1);
        let again = nav.turn.as_ref().unwrap().to.clone();
        let d = sub(again.eye, to.eye);
        assert!(dot(d, d) < 1e-30, "{d:?}");
        assert!(!nav.step(5.1 + TURN_SECONDS));
        assert_eq!(nav.camera, again);
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
