//! Evaluation results: a point together with its derivatives, and the
//! differential geometry derived from them.

use super::{Point3, UnitVector3, Vector3};

/// A curve's point and its first two derivatives at one parameter.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CurveEval {
    pub point: Point3,
    pub d1: Vector3,
    pub d2: Vector3,
}

impl CurveEval {
    /// The unit tangent in the direction of increasing parameter. Where the
    /// first derivative vanishes (shorter than
    /// [`crate::tol::LINEAR_RESOLUTION`]), the curve leaves the point along
    /// its second derivative, so that is the tangent. `None` when both
    /// vanish.
    pub fn tangent(&self) -> Option<UnitVector3> {
        self.d1.normalize().or_else(|| self.d2.normalize())
    }

    /// `|d1 × d2| / |d1|³`. `None` where the first derivative vanishes.
    pub fn curvature(&self) -> Option<f64> {
        self.d1.normalize()?;
        let speed = self.d1.length();
        Some(self.d1.cross(self.d2).length() / (speed * speed * speed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tangent_is_the_first_derivative_direction() {
        let e = CurveEval {
            point: Point3::ORIGIN,
            d1: Vector3::new(0.0, 0.0, 4.0),
            d2: Vector3::new(1.0, 0.0, 0.0),
        };
        assert_eq!(e.tangent(), Some(UnitVector3::Z));
    }

    #[test]
    fn tangent_falls_back_to_the_second_derivative() {
        let e = CurveEval {
            point: Point3::ORIGIN,
            d1: Vector3::ZERO,
            d2: Vector3::new(0.0, 3.0, 0.0),
        };
        assert_eq!(e.tangent(), Some(UnitVector3::Y));
        assert_eq!(e.curvature(), None);
        let flat = CurveEval {
            d2: Vector3::ZERO,
            ..e
        };
        assert_eq!(flat.tangent(), None);
    }

    #[test]
    fn curvature_of_a_parabola() {
        // (t, t², 0) at t = 0 has curvature 2; at t = 1 it is
        // 2 / (1 + 4)^(3/2).
        let at = |t: f64| CurveEval {
            point: Point3::new(t, t * t, 0.0),
            d1: Vector3::new(1.0, 2.0 * t, 0.0),
            d2: Vector3::new(0.0, 2.0, 0.0),
        };
        assert_eq!(at(0.0).curvature(), Some(2.0));
        let k = at(1.0).curvature().unwrap();
        assert!((k - 2.0 / (5.0 * 5.0f64.sqrt())).abs() <= 1e-16);
    }
}
