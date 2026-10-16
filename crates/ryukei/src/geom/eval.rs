//! Evaluation results: a point together with its derivatives, and the
//! differential geometry derived from them.

use super::{Point3, UnitVector3, Vector3};
use crate::tol::{ANGULAR_RESOLUTION, LINEAR_RESOLUTION};

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

    /// The same derivatives against the parameter `t / s`: `d1` times `s`,
    /// `d2` times `s²`. Tangent and curvature don't change, but the
    /// vanishing tests then measure what moving across a parameter range
    /// of length `s` does, rather than depending on the parameter's units.
    pub fn scaled(&self, s: f64) -> Self {
        Self {
            point: self.point,
            d1: self.d1 * s,
            d2: self.d2 * (s * s),
        }
    }
}

/// A surface's point and its first and second partial derivatives at one
/// `(u, v)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SurfaceEval {
    pub point: Point3,
    pub du: Vector3,
    pub dv: Vector3,
    pub duu: Vector3,
    pub duv: Vector3,
    pub dvv: Vector3,
}

/// The principal curvatures at a point and their directions, measured
/// against a given unit normal `n`: a curvature is positive where the
/// surface bends towards `n`. `k1 >= k2`, and `(dir1, dir2, n)` is
/// right-handed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PrincipalCurvatures {
    pub k1: f64,
    pub k2: f64,
    pub dir1: UnitVector3,
    pub dir2: UnitVector3,
}

impl SurfaceEval {
    /// The direction of `du × dv`, or `None` where either derivative
    /// vanishes (is no longer than [`LINEAR_RESOLUTION`]) or they are
    /// parallel within [`ANGULAR_RESOLUTION`].
    pub fn normal(&self) -> Option<UnitVector3> {
        let (a, b) = (self.du.length(), self.dv.length());
        let n = self.du.cross(self.dv);
        let len = n.length();
        if a <= LINEAR_RESOLUTION || b <= LINEAR_RESOLUTION || len <= ANGULAR_RESOLUTION * a * b {
            return None;
        }
        Some(UnitVector3::new_unchecked(n / len))
    }

    /// The same derivatives against the parameters `(u / su, v / sv)`; see
    /// [`CurveEval::scaled`].
    pub fn scaled(&self, su: f64, sv: f64) -> Self {
        Self {
            point: self.point,
            du: self.du * su,
            dv: self.dv * sv,
            duu: self.duu * (su * su),
            duv: self.duv * (su * sv),
            dvv: self.dvv * (sv * sv),
        }
    }

    /// The principal curvatures against the unit normal `n`, from the first
    /// and second fundamental forms. `None` where `du × dv` vanishes.
    ///
    /// At an umbilic (the curvatures agree to 1e-9 of their size) every
    /// direction is principal: `dir1` is then the direction of `du`.
    pub fn principal_curvatures(&self, n: UnitVector3) -> Option<PrincipalCurvatures> {
        self.normal()?;
        let nv = n.get();
        let (e, f, g) = (
            self.du.dot(self.du),
            self.du.dot(self.dv),
            self.dv.dot(self.dv),
        );
        let (l, m, nn) = (self.duu.dot(nv), self.duv.dot(nv), self.dvv.dot(nv));
        // The shape operator, I⁻¹ II. Its eigenvalues are the curvatures;
        // taking them from the matrix rather than from the mean and Gauss
        // curvatures keeps umbilics free of cancellation.
        let det = e * g - f * f;
        let s11 = (g * l - f * m) / det;
        let s12 = (g * m - f * nn) / det;
        let s21 = (e * m - f * l) / det;
        let s22 = (e * nn - f * m) / det;
        let mean = (s11 + s22) / 2.0;
        let half = (s11 - s22) / 2.0;
        let spread = crate::math::sqrt((half * half + s12 * s21).max(0.0));
        let (k1, k2) = (mean + spread, mean - spread);
        let umbilic = spread <= 1e-9 * k1.abs().max(k2.abs());
        let dir1 = if umbilic {
            None
        } else {
            // An eigenvector for k1, from whichever row of S - k1 gives
            // the longer tangent vector.
            let a = self.du * -s12 + self.dv * (s11 - k1);
            let b = self.du * (s22 - k1) + self.dv * -s21;
            let d = if a.length_squared() >= b.length_squared() {
                a
            } else {
                b
            };
            d.normalize()
        };
        let dir1 = dir1.or_else(|| self.du.normalize())?;
        // Fix the sign: a non-negative component along du, then dv.
        let along = (dir1.get().dot(self.du), dir1.get().dot(self.dv));
        let dir1 = if along < (0.0, 0.0) { -dir1 } else { dir1 };
        let dir2 = nv.cross(dir1.get()).normalize()?;
        Some(PrincipalCurvatures { k1, k2, dir1, dir2 })
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

    /// The saddle `(u, v, uv)`.
    fn saddle(u: f64, v: f64) -> SurfaceEval {
        SurfaceEval {
            point: Point3::new(u, v, u * v),
            du: Vector3::new(1.0, 0.0, v),
            dv: Vector3::new(0.0, 1.0, u),
            duu: Vector3::ZERO,
            duv: Vector3::new(0.0, 0.0, 1.0),
            dvv: Vector3::ZERO,
        }
    }

    fn close(a: Vector3, b: Vector3) -> bool {
        (a - b).length() <= 1e-15
    }

    #[test]
    fn surface_normal_is_du_cross_dv() {
        assert_eq!(saddle(0.0, 0.0).normal(), Some(UnitVector3::Z));
        let pinched = SurfaceEval {
            du: Vector3::ZERO,
            ..saddle(0.0, 0.0)
        };
        assert_eq!(pinched.normal(), None);
        assert_eq!(pinched.principal_curvatures(UnitVector3::Z), None);
    }

    #[test]
    fn surface_normal_needs_two_real_directions() {
        // A tiny du vanishes even though du × dv is long.
        let tiny = SurfaceEval {
            du: Vector3::new(1e-9, 0.0, 0.0),
            dv: Vector3::new(0.0, 1e9, 0.0),
            ..saddle(0.0, 0.0)
        };
        assert_eq!(tiny.normal(), None);
        // Parallel within the angular resolution.
        let parallel = SurfaceEval {
            du: Vector3::new(1.0, 0.0, 0.0),
            dv: Vector3::new(1.0, 1e-12, 0.0),
            ..saddle(0.0, 0.0)
        };
        assert_eq!(parallel.normal(), None);
    }

    #[test]
    fn scaling_the_parameters_scales_the_derivatives() {
        let e = saddle(0.5, 2.0).scaled(2.0, 3.0);
        assert_eq!(e.point, saddle(0.5, 2.0).point);
        assert_eq!(e.du, Vector3::new(2.0, 0.0, 4.0));
        assert_eq!(e.dv, Vector3::new(0.0, 3.0, 1.5));
        assert_eq!(e.duv, Vector3::new(0.0, 0.0, 6.0));
        let c = CurveEval {
            point: Point3::ORIGIN,
            d1: Vector3::new(1.0, 0.0, 0.0),
            d2: Vector3::new(0.0, 1.0, 0.0),
        }
        .scaled(3.0);
        assert_eq!((c.d1.x, c.d2.y), (3.0, 9.0));
    }

    #[test]
    fn principal_curvatures_of_a_saddle() {
        let k = saddle(0.0, 0.0)
            .principal_curvatures(UnitVector3::Z)
            .unwrap();
        assert_eq!((k.k1, k.k2), (1.0, -1.0));
        let h = core::f64::consts::FRAC_1_SQRT_2;
        assert!(close(k.dir1.get(), Vector3::new(h, h, 0.0)));
        assert!(close(k.dir2.get(), Vector3::new(-h, h, 0.0)));
        // Against the reversed normal the curvatures change sign.
        let r = saddle(0.0, 0.0)
            .principal_curvatures(-UnitVector3::Z)
            .unwrap();
        assert_eq!((r.k1, r.k2), (1.0, -1.0));
        assert!(close(r.dir1.get(), Vector3::new(h, -h, 0.0)));
    }

    #[test]
    fn umbilic_points_take_du_as_the_first_direction() {
        let flat = SurfaceEval {
            duv: Vector3::ZERO,
            ..saddle(0.0, 0.0)
        };
        let k = flat.principal_curvatures(UnitVector3::Z).unwrap();
        assert_eq!((k.k1, k.k2), (0.0, 0.0));
        assert_eq!(k.dir1, UnitVector3::X);
        assert_eq!(k.dir2, UnitVector3::Y);
    }
}
