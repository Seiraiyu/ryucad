//! Cameras: how a participant looks at the model. A camera lives in the
//! live session (each participant has its own) and drives renders.

use crate::error::{Code, Error, Result};
use crate::units::{Quantity, Unit};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A camera in model space, in meters.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Camera {
    pub eye: [f64; 3],
    pub target: [f64; 3],
    pub up: [f64; 3],
    pub projection: Projection,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Projection {
    /// `fov_deg` is the vertical field of view.
    Perspective { fov_deg: f64 },
    /// `height` is the height of the view at the target, in meters.
    Orthographic { height: f64 },
}

/// A camera as a request gives it: points are quantities (strings with
/// units, or bare numbers in the project's unit), `up` is a direction.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CameraArg {
    pub eye: [Value; 3],
    pub target: [Value; 3],
    pub up: [f64; 3],
    pub projection: ProjectionArg,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum ProjectionArg {
    Perspective { fov_deg: f64 },
    Orthographic { height: Value },
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn norm(a: [f64; 3]) -> f64 {
    (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt()
}

fn bad(why: &str) -> Error {
    Error::new(Code::InvalidParam, format!("bad camera: {why}"))
}

impl CameraArg {
    /// The camera in meters, checked.
    pub fn resolve(&self, unit: Unit) -> Result<Camera> {
        let point = |q: &[Value; 3], what: &str| -> Result<[f64; 3]> {
            let mut out = [0.0; 3];
            for (i, v) in q.iter().enumerate() {
                out[i] = Quantity::from_json(v, unit)
                    .map_err(|e| e.with("pointer", format!("/camera/{what}/{i}")))?
                    .meters();
            }
            Ok(out)
        };
        let projection = match &self.projection {
            ProjectionArg::Perspective { fov_deg } => Projection::Perspective { fov_deg: *fov_deg },
            ProjectionArg::Orthographic { height } => Projection::Orthographic {
                height: Quantity::from_json(height, unit)
                    .map_err(|e| e.with("pointer", "/camera/projection/orthographic/height"))?
                    .meters(),
            },
        };
        let c = Camera {
            eye: point(&self.eye, "eye")?,
            target: point(&self.target, "target")?,
            up: self.up,
            projection,
        };
        c.check()?;
        Ok(c)
    }
}

impl Camera {
    /// Refuses a camera that can't define a view.
    pub fn check(&self) -> Result<()> {
        let finite = |a: [f64; 3]| a.iter().all(|x| x.is_finite());
        if !(finite(self.eye) && finite(self.target) && finite(self.up)) {
            return Err(bad("every coordinate must be finite"));
        }
        let dir = sub(self.target, self.eye);
        if norm(dir) <= ryukei::tol::LINEAR_RESOLUTION {
            return Err(bad("the eye and the target must differ"));
        }
        let (d, u) = (norm(dir), norm(self.up));
        if u == 0.0 || norm(cross(dir, self.up)) <= ryukei::tol::ANGULAR_RESOLUTION * d * u {
            return Err(bad("up must be non-zero and not along the view direction"));
        }
        match self.projection {
            Projection::Perspective { fov_deg } if !(fov_deg > 0.0 && fov_deg < 180.0) => {
                Err(bad("the field of view must be between 0 and 180 degrees"))
            }
            Projection::Orthographic { height } if !(height.is_finite() && height > 0.0) => {
                Err(bad("the orthographic height must be positive"))
            }
            _ => Ok(()),
        }
    }
}

/// A named direction to look from. Z is up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StandardView {
    Front,
    Back,
    Left,
    Right,
    Top,
    Bottom,
    Iso,
}

impl StandardView {
    pub fn parse(s: &str) -> Result<StandardView> {
        Ok(match s {
            "front" => StandardView::Front,
            "back" => StandardView::Back,
            "left" => StandardView::Left,
            "right" => StandardView::Right,
            "top" => StandardView::Top,
            "bottom" => StandardView::Bottom,
            "iso" => StandardView::Iso,
            _ => {
                return Err(Error::new(
                    Code::InvalidParam,
                    format!("unknown view {s:?}; use front, back, left, right, top, bottom or iso"),
                ));
            }
        })
    }

    /// From the target towards the eye, and up.
    fn axes(self) -> ([f64; 3], [f64; 3]) {
        match self {
            StandardView::Front => ([0.0, -1.0, 0.0], [0.0, 0.0, 1.0]),
            StandardView::Back => ([0.0, 1.0, 0.0], [0.0, 0.0, 1.0]),
            StandardView::Left => ([-1.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
            StandardView::Right => ([1.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
            StandardView::Top => ([0.0, 0.0, 1.0], [0.0, 1.0, 0.0]),
            StandardView::Bottom => ([0.0, 0.0, -1.0], [0.0, 1.0, 0.0]),
            StandardView::Iso => ([1.0, -1.0, 1.0], [0.0, 0.0, 1.0]),
        }
    }
}

/// The projection a framed view uses when there is none to keep.
pub const DEFAULT_PROJECTION: Projection = Projection::Perspective { fov_deg: 30.0 };

/// How much room a framed view leaves around the box.
const MARGIN: f64 = 1.1;

/// A camera looking along `dir` (from the eye towards the target, as
/// `-back`) with `up`, framing the box `lo`..`hi` so its bounding sphere
/// fits the view.
/// The corners of the box `lo`..`hi`.
fn corners(lo: [f64; 3], hi: [f64; 3]) -> [[f64; 3]; 8] {
    core::array::from_fn(|i| [0, 1, 2].map(|k| if i >> k & 1 == 1 { hi[k] } else { lo[k] }))
}

/// A camera looking along `-back` at `points` (not empty), centred on
/// them and as close as lets them all fit a view of `aspect` (width over
/// height) with the margin. Fitting the points, not their box, frames real
/// models tightly: a box's corners are mostly empty.
fn frame_along(
    points: &[[f64; 3]],
    back: [f64; 3],
    up: [f64; 3],
    projection: Projection,
    aspect: f64,
) -> Camera {
    let b = norm(back);
    let back = back.map(|x| x / b);
    // The view's axes, as `Camera::basis` will have them.
    let forward = back.map(|x| -x);
    let right = unit(cross(forward, up));
    let up_ = cross(right, forward);
    // The middle of the points' box, moved across the view to the middle
    // of the points as the view sees them (no move for a symmetric model
    // in orthographic, so its target stays exact; perspective moves it
    // again below).
    let (mut lo, mut hi) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
    let (mut across_lo, mut across_hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
    for p in points {
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
        for (k, axis) in [right, up_].into_iter().enumerate() {
            across_lo[k] = across_lo[k].min(dot(*p, axis));
            across_hi[k] = across_hi[k].max(dot(*p, axis));
        }
    }
    let centre = [0, 1, 2].map(|k| lo[k] + (hi[k] - lo[k]) / 2.0);
    let shift = |k: usize, axis: [f64; 3]| {
        let by = (across_lo[k] + across_hi[k]) / 2.0 - dot(centre, axis);
        // Below the kernel's resolution it is rounding, not asymmetry.
        let by = if by.abs() < ryukei::tol::LINEAR_RESOLUTION {
            0.0
        } else {
            by
        };
        scale(axis, by)
    };
    let mut target = add(centre, add(shift(0, right), shift(1, up_)));
    let aspect = if aspect.is_finite() && aspect > 0.0 {
        aspect
    } else {
        1.0
    };
    let floor = 1e-6;
    let fit = |target: [f64; 3]| {
        // Each point around the target: how far across the view it reaches
        // (its width scaled to height) and how far towards the eye it
        // stands.
        let around = points.iter().map(|p| {
            let q = sub(*p, target);
            let across = (dot(q, right).abs() / aspect).max(dot(q, up_).abs());
            (across, dot(q, back))
        });
        match projection {
            // Back far enough that every point is inside the field of view,
            // with the margin: a point `toward` the eye is that much nearer.
            Projection::Perspective { fov_deg } => {
                let tan = ryukei::math::tan(fov_deg * core::f64::consts::PI / 360.0);
                let d = around
                    .map(|(across, toward)| across.max(floor) * MARGIN / tan + toward)
                    .fold(floor, f64::max);
                (d, projection)
            }
            Projection::Orthographic { .. } => {
                let (across, toward) =
                    around.fold((floor, 0.0f64), |(a, t), (x, y)| (a.max(x), t.max(y)));
                let height = 2.0 * across * MARGIN;
                // In front of the nearest point, as far again as the view is
                // high.
                (toward + height, Projection::Orthographic { height })
            }
        }
    };
    let (mut distance, mut projection) = fit(target);
    // In perspective nearer points look bigger, so the middle of what the
    // view sees isn't the middle across: move the target until it is (the
    // extremes on each side seen at equal angles), then step back to fit.
    if let Projection::Perspective { .. } = projection {
        for _ in 0..16 {
            let mut moved = 0.0f64;
            for axis in [right, up_] {
                // The points seen furthest each way: across, and depth.
                let seen = |p: &[f64; 3]| {
                    let q = sub(*p, target);
                    let (a, z) = (dot(q, axis), distance - dot(q, back));
                    (a / z, a, z)
                };
                let (mut lo, mut hi) = ((f64::INFINITY, 0.0, 1.0), (f64::NEG_INFINITY, 0.0, 1.0));
                for s in points.iter().map(seen) {
                    if s.0 < lo.0 {
                        lo = s;
                    }
                    if s.0 > hi.0 {
                        hi = s;
                    }
                }
                // The move `by` with (hi.a - by) / hi.z = -(lo.a - by) / lo.z.
                let by = (hi.1 / hi.2 + lo.1 / lo.2) / (1.0 / hi.2 + 1.0 / lo.2);
                if by.abs() >= ryukei::tol::LINEAR_RESOLUTION {
                    target = add(target, scale(axis, by));
                    moved = moved.max(by.abs());
                }
            }
            (distance, projection) = fit(target);
            if moved < ryukei::tol::LINEAR_RESOLUTION {
                break;
            }
        }
    }
    Camera {
        eye: [0, 1, 2].map(|k| target[k] + back[k] * distance),
        target,
        up,
        projection,
    }
}

/// A camera on the box `lo`..`hi` from a standard view.
pub fn frame(lo: [f64; 3], hi: [f64; 3], view: StandardView, projection: Projection) -> Camera {
    frame_points(&corners(lo, hi), view, projection, 1.0)
}

/// A camera on `points` (not empty) from a standard view, for a view of
/// `aspect` (width over height): the model's mesh points frame it far more
/// tightly than its box.
pub fn frame_points(
    points: &[[f64; 3]],
    view: StandardView,
    projection: Projection,
    aspect: f64,
) -> Camera {
    let (back, up) = view.axes();
    frame_along(points, back, up, projection, aspect)
}

/// `camera` moved to frame the box, keeping its direction, up and kind of
/// projection.
pub fn refit(camera: &Camera, lo: [f64; 3], hi: [f64; 3]) -> Camera {
    refit_points(camera, &corners(lo, hi), 1.0)
}

/// `camera` moved to frame `points` (not empty) in a view of `aspect`,
/// keeping its direction, up and kind of projection.
pub fn refit_points(camera: &Camera, points: &[[f64; 3]], aspect: f64) -> Camera {
    let back = sub(camera.eye, camera.target);
    frame_along(points, back, camera.up, camera.projection, aspect)
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn unit(a: [f64; 3]) -> [f64; 3] {
    let n = norm(a);
    a.map(|x| x / n)
}

fn add(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn scale(a: [f64; 3], k: f64) -> [f64; 3] {
    a.map(|x| x * k)
}

/// `v` turned by `angle` radians about the unit `axis` (Rodrigues).
fn rotate(v: [f64; 3], axis: [f64; 3], angle: f64) -> [f64; 3] {
    let (s, c) = ryukei::math::sin_cos(angle);
    let along = scale(axis, dot(axis, v) * (1.0 - c));
    add(add(scale(v, c), scale(cross(axis, v), s)), along)
}

impl Camera {
    /// The view's unit axes: right, up and forward (from the eye towards
    /// the target).
    pub fn basis(&self) -> ([f64; 3], [f64; 3], [f64; 3]) {
        let forward = unit(sub(self.target, self.eye));
        let right = unit(cross(forward, self.up));
        (right, cross(right, forward), forward)
    }

    /// Half the view's height at `depth` along the view direction (at any
    /// depth, for an orthographic view).
    pub fn half_height(&self, depth: f64) -> f64 {
        match self.projection {
            Projection::Perspective { fov_deg } => {
                ryukei::math::tan(fov_deg * core::f64::consts::PI / 360.0) * depth
            }
            Projection::Orthographic { height } => height / 2.0,
        }
    }

    /// The distance from the eye to the target.
    pub fn distance(&self) -> f64 {
        norm(sub(self.target, self.eye))
    }
}

/// `camera` turned about `pivot`: `yaw` radians about the model's Z axis,
/// then `pitch` radians about the view's right axis. Up turns with it, so
/// the view can go over the top without flipping.
pub fn orbit(camera: &Camera, pivot: [f64; 3], yaw: f64, pitch: f64) -> Camera {
    let z = [0.0, 0.0, 1.0];
    let (eye, target, up) = (sub(camera.eye, pivot), sub(camera.target, pivot), camera.up);
    let (eye, target, up) = (
        rotate(eye, z, yaw),
        rotate(target, z, yaw),
        rotate(up, z, yaw),
    );
    let right = unit(cross(sub(target, eye), up));
    let (eye, target, up) = (
        rotate(eye, right, pitch),
        rotate(target, right, pitch),
        rotate(up, right, pitch),
    );
    Camera {
        eye: add(eye, pivot),
        target: add(target, pivot),
        up: unit(up),
        projection: camera.projection,
    }
}

/// `camera` moved in the view plane by `dx` (right) and `dy` (up), as
/// fractions of the view's height at the target.
pub fn pan(camera: &Camera, dx: f64, dy: f64) -> Camera {
    let (right, up, _) = camera.basis();
    let h = 2.0 * camera.half_height(camera.distance());
    let shift = add(scale(right, dx * h), scale(up, dy * h));
    Camera {
        eye: add(camera.eye, shift),
        target: add(camera.target, shift),
        ..camera.clone()
    }
}

/// `camera` zoomed by `factor` (below 1 is closer) towards `toward`, a
/// point that stays where it is on screen; the target when `None`.
pub fn zoom(camera: &Camera, factor: f64, toward: Option<[f64; 3]>) -> Camera {
    let p = toward.unwrap_or(camera.target);
    match camera.projection {
        // Along the ray through `p`, which therefore stays put.
        Projection::Perspective { .. } => {
            let step = scale(sub(p, camera.eye), 1.0 - factor);
            let eye = add(camera.eye, step);
            // Towards a point, the target moves to the point's depth on
            // the same axis, so later zooms and orbits keep their scale
            // and the target stays by the model; towards the target, it
            // stays.
            let target = if toward.is_some() {
                let (_, _, forward) = camera.basis();
                add(eye, scale(forward, dot(sub(p, eye), forward)))
            } else {
                camera.target
            };
            Camera {
                eye,
                target,
                ..camera.clone()
            }
        }
        // Scaled about `p` in the view plane.
        Projection::Orthographic { height } => {
            let (_, _, forward) = camera.basis();
            let rel = sub(camera.target, p);
            let in_plane = sub(rel, scale(forward, dot(rel, forward)));
            let shift = scale(in_plane, factor - 1.0);
            Camera {
                eye: add(camera.eye, shift),
                target: add(camera.target, shift),
                projection: Projection::Orthographic {
                    height: height * factor,
                },
                ..camera.clone()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn arg(v: Value) -> Result<Camera> {
        serde_json::from_value::<CameraArg>(v)
            .unwrap()
            .resolve(Unit::Mm)
    }

    #[test]
    fn quantities_become_meters() {
        let c = arg(
            json!({"eye": ["100 mm", 0, "1 m"], "target": [0, 0, 0], "up": [0, 0, 1],
                           "projection": {"orthographic": {"height": "50"}}}),
        )
        .unwrap();
        assert_eq!(c.eye, [0.1, 0.0, 1.0]);
        assert_eq!(c.projection, Projection::Orthographic { height: 0.05 });
        assert_eq!(
            json!(c),
            json!({"eye": [0.1, 0.0, 1.0], "target": [0.0, 0.0, 0.0], "up": [0.0, 0.0, 1.0],
                   "projection": {"orthographic": {"height": 0.05}}})
        );
    }

    #[test]
    fn standard_views_frame_the_box() {
        let (lo, hi) = ([0.0, 0.0, 0.0], [2.0, 2.0, 2.0]);
        let c = frame(lo, hi, StandardView::Front, DEFAULT_PROJECTION);
        assert!(close(c.target, [1.0, 1.0, 1.0]), "{:?}", c.target);
        assert!(c.eye[1] < 0.0 && c.eye[0] == 1.0 && c.eye[2] == 1.0);
        assert!(c.check().is_ok());
        let r = MARGIN;
        let top = frame(
            lo,
            hi,
            StandardView::Top,
            Projection::Orthographic { height: 1.0 },
        );
        assert_eq!(top.up, [0.0, 1.0, 0.0]);
        assert!(top.eye[2] > 2.0);
        assert_eq!(top.projection, Projection::Orthographic { height: 2.0 * r });
        for v in ["front", "back", "left", "right", "top", "bottom", "iso"] {
            let c = frame(lo, hi, StandardView::parse(v).unwrap(), DEFAULT_PROJECTION);
            assert!(c.check().is_ok(), "{v}");
        }
        assert_eq!(
            StandardView::parse("side").unwrap_err().code,
            Code::InvalidParam
        );
        // Refitting keeps the direction and moves to the new box.
        let moved = refit(&c, [10.0; 3], [11.0; 3]);
        assert!(close(moved.target, [10.5; 3]), "{:?}", moved.target);
        assert_eq!(moved.up, c.up);
    }

    /// The largest of a box's corners' distances from the middle of the
    /// view, as a fraction of the half-height there (1 at the edge).
    fn reach(c: &Camera, lo: [f64; 3], hi: [f64; 3]) -> f64 {
        let (r, u, f) = c.basis();
        let mut most: f64 = 0.0;
        for i in 0..8 {
            let p = [0, 1, 2].map(|k| if i >> k & 1 == 1 { hi[k] } else { lo[k] });
            let q = sub(p, c.eye);
            let depth = dot(q, f);
            assert!(depth > 0.0, "a corner behind the eye");
            let h = c.half_height(depth);
            most = most.max(dot(q, r).abs() / h).max(dot(q, u).abs() / h);
        }
        most
    }

    #[test]
    fn framing_is_tight_on_long_flat_models() {
        let (lo, hi) = ([0.0, 0.0, 0.0], [0.2, 0.02, 0.01]);
        for projection in [DEFAULT_PROJECTION, Projection::Orthographic { height: 1.0 }] {
            for v in [StandardView::Front, StandardView::Top, StandardView::Iso] {
                let c = frame(lo, hi, v, projection);
                let k = reach(&c, lo, hi);
                assert!(k <= 1.0 + 1e-9, "{v:?} {projection:?}: out of view ({k})");
                assert!(
                    k >= 1.0 / MARGIN - 1e-9,
                    "{v:?} {projection:?}: loose ({k})"
                );
            }
        }
    }

    #[test]
    fn points_frame_tighter_than_their_box() {
        // A ring in the ground plane, seen from the front: its box's near
        // corners stand out towards the eye, the ring doesn't.
        let pts: Vec<[f64; 3]> = (0..64)
            .map(|i| {
                let (s, c) = ryukei::math::sin_cos(f64::from(i) * core::f64::consts::PI / 32.0);
                [c, s, 0.0]
            })
            .collect();
        let tight = frame_points(&pts, StandardView::Front, DEFAULT_PROJECTION, 1.0);
        let boxed = frame(
            [-1.0, -1.0, 0.0],
            [1.0, 1.0, 0.0],
            StandardView::Front,
            DEFAULT_PROJECTION,
        );
        assert!(tight.distance() < boxed.distance());
        // Every point in view, the outermost near the margin.
        let (r, u, f) = tight.basis();
        let mut most: f64 = 0.0;
        for p in &pts {
            let q = sub(*p, tight.eye);
            let h = tight.half_height(dot(q, f));
            most = most.max(dot(q, r).abs() / h).max(dot(q, u).abs() / h);
        }
        assert!((1.0 / MARGIN - 1e-9..=1.0 + 1e-9).contains(&most), "{most}");
    }

    #[test]
    fn a_wide_view_frames_a_wide_model_closer() {
        let pts = [[-1.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 0.2]];
        let square = frame_points(&pts, StandardView::Front, DEFAULT_PROJECTION, 1.0);
        let wide = frame_points(&pts, StandardView::Front, DEFAULT_PROJECTION, 2.0);
        assert!(wide.distance() < 0.6 * square.distance());
    }

    fn close(a: [f64; 3], b: [f64; 3]) -> bool {
        norm(sub(a, b)) < 1e-9
    }

    #[test]
    fn orbiting_keeps_the_distance_and_comes_round() {
        let c = frame([0.0; 3], [1.0; 3], StandardView::Front, DEFAULT_PROJECTION);
        let pivot = [0.5; 3];
        let turned = orbit(&c, pivot, 0.7, -0.4);
        assert!((norm(sub(turned.eye, pivot)) - norm(sub(c.eye, pivot))).abs() < 1e-12);
        assert!(turned.check().is_ok());
        assert!((dot(turned.up, sub(turned.target, turned.eye))).abs() < 1e-9);
        let pi = core::f64::consts::PI;
        let round = orbit(&orbit(&c, pivot, pi, 0.0), pivot, pi, 0.0);
        assert!(close(round.eye, c.eye) && close(round.up, c.up));
        // Over the top and back, with no flip.
        let over = orbit(&orbit(&c, pivot, 0.0, 2.0), pivot, 0.0, -2.0);
        assert!(close(over.eye, c.eye) && close(over.up, c.up));
        // A yaw from the front view moves the eye round Z.
        let side = orbit(&c, c.target, pi / 2.0, 0.0);
        assert!(side.eye[0] > c.target[0] && (side.eye[1] - c.target[1]).abs() < 1e-9);
    }

    #[test]
    fn panning_moves_the_view_plane() {
        let c = frame([0.0; 3], [1.0; 3], StandardView::Front, DEFAULT_PROJECTION);
        let h = 2.0 * c.half_height(c.distance());
        let moved = pan(&c, 0.5, -0.25);
        let d = sub(moved.target, c.target);
        assert!(close(d, [0.5 * h, 0.0, -0.25 * h]));
        assert!(close(sub(moved.eye, c.eye), d));
    }

    #[test]
    fn zooming_keeps_the_point_in_place() {
        let c = frame([0.0; 3], [1.0; 3], StandardView::Iso, DEFAULT_PROJECTION);
        let closer = zoom(&c, 0.5, None);
        assert!((closer.distance() - c.distance() / 2.0).abs() < 1e-12);
        assert_eq!(closer.target, c.target);
        let p = [1.0, 0.0, 1.0];
        let ortho = Camera {
            projection: Projection::Orthographic { height: 3.0 },
            ..c.clone()
        };
        for cam in [c, ortho] {
            let before = crate::render::View::new(&cam, 400, 300).pixel(p).unwrap();
            let z = zoom(&cam, 0.8, Some(p));
            assert!(z.check().is_ok());
            let after = crate::render::View::new(&z, 400, 300).pixel(p).unwrap();
            assert!((before.0 - after.0).abs() < 1e-3 && (before.1 - after.1).abs() < 1e-3);
        }
    }

    #[test]
    fn perspective_framing_centres_what_the_view_sees() {
        // Tall, so its near top looks bigger than its far bottom.
        let points = corners([0.0; 3], [1.0, 1.0, 3.0]);
        for aspect in [1.0, 1.6] {
            let c = frame_points(&points, StandardView::Iso, DEFAULT_PROJECTION, aspect);
            let (r, u, f) = c.basis();
            let seen = |axis: [f64; 3]| {
                let s: Vec<f64> = points
                    .iter()
                    .map(|p| {
                        let q = sub(*p, c.eye);
                        dot(q, axis) / dot(q, f)
                    })
                    .collect();
                let lo = s.iter().copied().fold(f64::INFINITY, f64::min);
                let hi = s.iter().copied().fold(f64::NEG_INFINITY, f64::max);
                (lo, hi)
            };
            for axis in [r, u] {
                let (lo, hi) = seen(axis);
                assert!((lo + hi).abs() < 1e-6 * (hi - lo), "{lo} .. {hi}");
            }
        }
    }

    #[test]
    fn zooming_at_a_point_keeps_the_target_by_the_model() {
        let c = frame([0.0; 3], [1.0; 3], StandardView::Iso, DEFAULT_PROJECTION);
        let (_, _, forward) = c.basis();
        let p = [1.0, 0.0, 1.0];
        let start = dot(sub(p, c.target), sub(p, c.target)).sqrt();
        let mut cam = c.clone();
        for factor in [0.8; 6].into_iter().chain([1.25; 6]) {
            cam = zoom(&cam, factor, Some(p));
            // On the same axis, at the depth of the point zoomed at.
            let (_, _, f) = cam.basis();
            assert!(close(f, forward));
            let depth = |q: [f64; 3]| dot(sub(q, cam.eye), forward);
            assert!((depth(cam.target) - depth(p)).abs() < 1e-9);
            // So never farther from the point than it began.
            assert!(dot(sub(p, cam.target), sub(p, cam.target)).sqrt() <= start + 1e-9);
        }
    }

    #[test]
    fn refuses_cameras_with_no_view() {
        let base = json!({"eye": [1, 2, 3], "target": [0, 0, 0], "up": [0, 0, 1],
                          "projection": {"perspective": {"fov_deg": 45}}});
        assert!(arg(base.clone()).is_ok());
        let with = |k: &str, v: Value| {
            let mut c = base.clone();
            c[k] = v;
            arg(c).unwrap_err()
        };
        assert_eq!(with("target", json!([1, 2, 3])).code, Code::InvalidParam);
        assert_eq!(with("up", json!([0, 0, 0])).code, Code::InvalidParam);
        assert_eq!(with("up", json!([2, 4, 6])).code, Code::InvalidParam);
        assert_eq!(
            with("projection", json!({"perspective": {"fov_deg": 180}})).code,
            Code::InvalidParam
        );
        assert_eq!(
            with("projection", json!({"orthographic": {"height": "0 mm"}})).code,
            Code::InvalidParam
        );
        let e = with("eye", json!(["x", 0, 0]));
        assert_eq!(
            (e.code, &e.details["pointer"]),
            (Code::InvalidQuantity, &json!("/camera/eye/0"))
        );
    }
}
