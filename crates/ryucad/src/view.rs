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
