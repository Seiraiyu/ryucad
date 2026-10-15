use super::{Point3, UnitVector3, Vector3};
use crate::math;

/// Row-major 3x3 matrix.
type Mat3 = [[f64; 3]; 3];

fn mat_mul(a: &Mat3, b: &Mat3) -> Mat3 {
    let mut m = [[0.0; 3]; 3];
    for (i, row) in m.iter_mut().enumerate() {
        for (j, cell) in row.iter_mut().enumerate() {
            *cell = a[i][0] * b[0][j] + a[i][1] * b[1][j] + a[i][2] * b[2][j];
        }
    }
    m
}

fn mat_vec(m: &Mat3, v: Vector3) -> Vector3 {
    Vector3::new(
        m[0][0] * v.x + m[0][1] * v.y + m[0][2] * v.z,
        m[1][0] * v.x + m[1][1] * v.y + m[1][2] * v.z,
        m[2][0] * v.x + m[2][1] * v.y + m[2][2] * v.z,
    )
}

fn transpose(m: &Mat3) -> Mat3 {
    [
        [m[0][0], m[1][0], m[2][0]],
        [m[0][1], m[1][1], m[2][1]],
        [m[0][2], m[1][2], m[2][2]],
    ]
}

fn row(m: &Mat3, i: usize) -> Vector3 {
    Vector3::new(m[i][0], m[i][1], m[i][2])
}

const IDENTITY3: Mat3 = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

/// A rotation followed by a translation: `p -> R p + t`.
///
/// Preserves lengths and angles, so directions stay unit and analytic
/// surfaces stay analytic. The inverse is exact (`R` transposed).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rigid {
    rot: Mat3,
    trans: Vector3,
}

impl Rigid {
    pub const IDENTITY: Self = Self {
        rot: IDENTITY3,
        trans: Vector3::ZERO,
    };

    pub fn translation(t: Vector3) -> Self {
        Self {
            rot: IDENTITY3,
            trans: t,
        }
    }

    /// Right-handed rotation by `angle` radians about `axis` through the origin.
    pub fn rotation(axis: UnitVector3, angle: f64) -> Self {
        let Vector3 { x, y, z } = axis.get();
        let (s, c) = math::sin_cos(angle);
        let k = 1.0 - c;
        Self {
            rot: [
                [c + x * x * k, x * y * k - z * s, x * z * k + y * s],
                [y * x * k + z * s, c + y * y * k, y * z * k - x * s],
                [z * x * k - y * s, z * y * k + x * s, c + z * z * k],
            ],
            trans: Vector3::ZERO,
        }
    }

    pub fn rotation_matrix(&self) -> [[f64; 3]; 3] {
        self.rot
    }

    pub fn translation_part(&self) -> Vector3 {
        self.trans
    }

    /// `self` applied after `first`: `(self.then_after(first))(p) == self(first(p))`.
    pub fn then_after(&self, first: &Self) -> Self {
        Self {
            rot: mat_mul(&self.rot, &first.rot),
            trans: mat_vec(&self.rot, first.trans) + self.trans,
        }
    }

    pub fn inverse(&self) -> Self {
        let rt = transpose(&self.rot);
        Self {
            rot: rt,
            trans: -mat_vec(&rt, self.trans),
        }
    }

    pub fn apply_point(&self, p: Point3) -> Point3 {
        Point3::ORIGIN + (mat_vec(&self.rot, p.to_vector()) + self.trans)
    }

    pub fn apply_vector(&self, v: Vector3) -> Vector3 {
        mat_vec(&self.rot, v)
    }

    pub fn apply_unit(&self, u: UnitVector3) -> UnitVector3 {
        UnitVector3::new_unchecked(mat_vec(&self.rot, u.get()))
    }

    /// Restores an exactly orthonormal rotation after long composition
    /// chains, by Gram-Schmidt on the rows (x row kept, then y, then z).
    pub fn reorthonormalize(&self) -> Self {
        let x = row(&self.rot, 0)
            .normalize()
            .expect("rotation row is not degenerate");
        let y0 = row(&self.rot, 1);
        let y = (y0 - x.get() * x.get().dot(y0))
            .normalize()
            .expect("rotation row is not degenerate");
        let z = x.get().cross(y.get());
        Self {
            rot: [x.get().to_array(), y.get().to_array(), z.to_array()],
            trans: self.trans,
        }
    }

    /// Largest deviation of `R Rᵀ` from the identity.
    pub fn orthonormality_error(&self) -> f64 {
        let p = mat_mul(&self.rot, &transpose(&self.rot));
        let mut worst: f64 = 0.0;
        for (i, prow) in p.iter().enumerate() {
            for (j, cell) in prow.iter().enumerate() {
                worst = worst.max((cell - IDENTITY3[i][j]).abs());
            }
        }
        worst
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::f64::consts::FRAC_PI_2;

    fn near(a: Point3, b: Point3) -> bool {
        a.distance(b) < 1e-12
    }

    #[test]
    fn rigid_rotation_quarter_turn() {
        let r = Rigid::rotation(UnitVector3::Z, FRAC_PI_2);
        assert!(near(
            r.apply_point(Point3::new(1.0, 0.0, 0.0)),
            Point3::new(0.0, 1.0, 0.0)
        ));
        let v = r.apply_vector(Vector3::new(0.0, 1.0, 0.0));
        assert!((v - Vector3::new(-1.0, 0.0, 0.0)).length() < 1e-15);
    }

    #[test]
    fn rigid_translation_moves_points_not_vectors() {
        let t = Rigid::translation(Vector3::new(1.0, 2.0, 3.0));
        assert_eq!(t.apply_point(Point3::ORIGIN), Point3::new(1.0, 2.0, 3.0));
        assert_eq!(
            t.apply_vector(Vector3::new(1.0, 0.0, 0.0)),
            Vector3::new(1.0, 0.0, 0.0)
        );
    }

    #[test]
    fn rigid_compose_order_and_inverse() {
        let r = Rigid::rotation(UnitVector3::Z, FRAC_PI_2);
        let t = Rigid::translation(Vector3::new(1.0, 0.0, 0.0));
        // Rotate first, then translate.
        let rt = t.then_after(&r);
        assert!(near(
            rt.apply_point(Point3::new(1.0, 0.0, 0.0)),
            Point3::new(1.0, 1.0, 0.0)
        ));
        let back = rt.inverse().apply_point(Point3::new(1.0, 1.0, 0.0));
        assert!(near(back, Point3::new(1.0, 0.0, 0.0)));
    }

    #[test]
    fn rigid_keeps_unit_vectors_unit() {
        let axis = Vector3::new(1.0, 2.0, 3.0).normalize().unwrap();
        let r = Rigid::rotation(axis, 0.7);
        let u = r.apply_unit(UnitVector3::X);
        assert!((u.get().length() - 1.0).abs() < 1e-15);
    }

    #[test]
    fn reorthonormalize_removes_drift() {
        let axis = Vector3::new(1.0, 1.0, 0.0).normalize().unwrap();
        let step = Rigid::rotation(axis, 0.1);
        let mut r = Rigid::IDENTITY;
        for _ in 0..10_000 {
            r = step.then_after(&r);
        }
        let fixed = r.reorthonormalize();
        assert!(fixed.orthonormality_error() <= 4.0 * f64::EPSILON);
        assert!(fixed.orthonormality_error() <= r.orthonormality_error());
    }
}
