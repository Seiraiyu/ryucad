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
fn frame_along(
    lo: [f64; 3],
    hi: [f64; 3],
    back: [f64; 3],
    up: [f64; 3],
    projection: Projection,
) -> Camera {
    let target = [0, 1, 2].map(|k| lo[k] + (hi[k] - lo[k]) / 2.0);
    let r = (norm(sub(hi, lo)) / 2.0).max(1e-6) * MARGIN;
    let b = norm(back);
    let back = back.map(|x| x / b);
    let (distance, projection) = match projection {
        Projection::Perspective { fov_deg } => {
            let half = fov_deg * core::f64::consts::PI / 360.0;
            (r / ryukei::math::sin(half), projection)
        }
        Projection::Orthographic { .. } => (2.0 * r, Projection::Orthographic { height: 2.0 * r }),
    };
    Camera {
        eye: [0, 1, 2].map(|k| target[k] + back[k] * distance),
        target,
        up,
        projection,
    }
}

/// A camera on the box `lo`..`hi` from a standard view.
pub fn frame(lo: [f64; 3], hi: [f64; 3], view: StandardView, projection: Projection) -> Camera {
    let (back, up) = view.axes();
    frame_along(lo, hi, back, up, projection)
}

/// `camera` moved to frame the box, keeping its direction, up and kind of
/// projection.
pub fn refit(camera: &Camera, lo: [f64; 3], hi: [f64; 3]) -> Camera {
    let back = sub(camera.eye, camera.target);
    frame_along(lo, hi, back, camera.up, camera.projection)
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
        assert_eq!(c.target, [1.0, 1.0, 1.0]);
        assert!(c.eye[1] < 0.0 && c.eye[0] == 1.0 && c.eye[2] == 1.0);
        assert!(c.check().is_ok());
        // The bounding sphere fits the field of view.
        let d = norm(sub(c.eye, c.target));
        let r = 3f64.sqrt() * MARGIN;
        assert!((ryukei::math::sin(15f64.to_radians()) * d - r).abs() < 1e-12);
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
        assert_eq!(moved.target, [10.5; 3]);
        assert_eq!(moved.up, c.up);
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
