use super::{Frame, Point3, UnitVector3, Vector3};
use crate::Error;
use crate::math;
use crate::tol::ANGULAR_RESOLUTION;

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

    /// The placement that carries the world frame to `frame`: its axes
    /// become the rotation's columns and its origin the translation. The
    /// inverse of [`Frame::from_rigid`]; exact for axis-aligned frames.
    pub fn from_frame(frame: &Frame) -> Self {
        let (x, y, z) = (frame.x().get(), frame.y().get(), frame.z().get());
        Self {
            rot: [[x.x, y.x, z.x], [x.y, y.y, z.y], [x.z, y.z, z.z]],
            trans: frame.origin().to_vector(),
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

/// A reflection in a plane: `p -> p - 2 ((p - o) · n) n`, for a point `o`
/// of the plane and its unit normal `n`.
///
/// It keeps lengths and angles but turns a right-handed frame
/// left-handed, so a curve or surface it reflects sits in a right-handed
/// frame of its own: [`Mirror::frame`] keeps angles running as they did,
/// [`Mirror::frame_reversed`] runs them the other way.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mirror {
    origin: Point3,
    normal: UnitVector3,
}

impl Mirror {
    /// The reflection in the plane through `origin` normal to `normal`.
    pub fn new(origin: Point3, normal: UnitVector3) -> Self {
        Self { origin, normal }
    }

    /// The reflection in the plane of `frame`'s `x` and `y`.
    pub fn in_plane_of(frame: &Frame) -> Self {
        Self::new(frame.origin(), frame.z())
    }

    pub fn apply_point(&self, p: Point3) -> Point3 {
        let n = self.normal.get();
        p - n * (2.0 * (p - self.origin).dot(n))
    }

    pub fn apply_vector(&self, v: Vector3) -> Vector3 {
        let n = self.normal.get();
        v - n * (2.0 * v.dot(n))
    }

    /// A reflection keeps lengths, so a unit vector stays unit to
    /// rounding.
    pub fn apply_unit(&self, u: UnitVector3) -> UnitVector3 {
        UnitVector3::new_unchecked(self.apply_vector(u.get()))
    }

    /// The right-handed frame with `frame`'s origin, `x` and `y`
    /// reflected; its `z` is `frame`'s reflected and reversed. Angles
    /// about it run as `frame`'s did, so a point at an angle in `frame`
    /// reflects to the point at the same angle in this one. Fails with
    /// `OutsideSizeBox` when the reflected origin leaves the size box.
    pub fn frame(&self, frame: &Frame) -> Result<Frame, Error> {
        Frame::new(
            self.apply_point(frame.origin()),
            -self.apply_vector(frame.z().get()),
            self.apply_vector(frame.x().get()),
        )
    }

    /// The right-handed frame with `frame`'s origin, `x` and `z`
    /// reflected; its `y` is `frame`'s reflected and reversed, so angles
    /// about it run the other way: the point at angle `a` in `frame`
    /// reflects to the point at `-a` in this one, and heights along `z`
    /// stay as they were. Fails as [`Mirror::frame`] does.
    pub fn frame_reversed(&self, frame: &Frame) -> Result<Frame, Error> {
        Frame::new(
            self.apply_point(frame.origin()),
            self.apply_vector(frame.z().get()),
            self.apply_vector(frame.x().get()),
        )
    }
}

/// A general affine map `p -> M p + t`: scaling, mirroring, shear.
///
/// Directions are not preserved in length, so there is no `apply_unit`.
/// A [`Rigid`] converts into an `Affine`, never the reverse.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Affine {
    lin: Mat3,
    trans: Vector3,
}

impl Affine {
    pub const IDENTITY: Self = Self {
        lin: IDENTITY3,
        trans: Vector3::ZERO,
    };

    /// From a row-major 3x3 linear part and a translation.
    pub fn new(linear: [[f64; 3]; 3], translation: Vector3) -> Self {
        Self {
            lin: linear,
            trans: translation,
        }
    }

    /// The checked constructor for matrices from outside the kernel.
    pub fn try_new(linear: [[f64; 3]; 3], translation: Vector3) -> Result<Self, Error> {
        let finite = linear.iter().flatten().all(|x| x.is_finite());
        if finite && Vector3::try_new(translation.x, translation.y, translation.z).is_ok() {
            Ok(Self::new(linear, translation))
        } else {
            Err(Error::NonFinite)
        }
    }

    /// Scale by `s.x`, `s.y`, `s.z` about the origin. Negative factors mirror.
    pub fn scale(s: Vector3) -> Self {
        Self::new(
            [[s.x, 0.0, 0.0], [0.0, s.y, 0.0], [0.0, 0.0, s.z]],
            Vector3::ZERO,
        )
    }

    pub fn linear(&self) -> [[f64; 3]; 3] {
        self.lin
    }

    pub fn translation_part(&self) -> Vector3 {
        self.trans
    }

    pub fn determinant(&self) -> f64 {
        let m = &self.lin;
        m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
            - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
            + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
    }

    /// `self` applied after `first`.
    pub fn then_after(&self, first: &Self) -> Self {
        Self {
            lin: mat_mul(&self.lin, &first.lin),
            trans: mat_vec(&self.lin, first.trans) + self.trans,
        }
    }

    /// `None` when the linear part is singular: its determinant is at most
    /// [`ANGULAR_RESOLUTION`] times the product of its column lengths, i.e.
    /// the columns are within angular resolution of being coplanar.
    pub fn inverse(&self) -> Option<Self> {
        let m = &self.lin;
        let cols = transpose(m);
        let scale = row(&cols, 0).length() * row(&cols, 1).length() * row(&cols, 2).length();
        let det = self.determinant();
        if det.abs() <= ANGULAR_RESOLUTION * scale {
            return None;
        }
        let cof = |r0: usize, c0: usize, r1: usize, c1: usize| {
            m[r0][c0] * m[r1][c1] - m[r0][c1] * m[r1][c0]
        };
        let adj = [
            [cof(1, 1, 2, 2), -cof(0, 1, 2, 2), cof(0, 1, 1, 2)],
            [-cof(1, 0, 2, 2), cof(0, 0, 2, 2), -cof(0, 0, 1, 2)],
            [cof(1, 0, 2, 1), -cof(0, 0, 2, 1), cof(0, 0, 1, 1)],
        ];
        let inv = adj.map(|r| r.map(|x| x / det));
        Some(Self {
            lin: inv,
            trans: -mat_vec(&inv, self.trans),
        })
    }

    pub fn apply_point(&self, p: Point3) -> Point3 {
        Point3::ORIGIN + (mat_vec(&self.lin, p.to_vector()) + self.trans)
    }

    pub fn apply_vector(&self, v: Vector3) -> Vector3 {
        mat_vec(&self.lin, v)
    }
}

impl From<Rigid> for Affine {
    fn from(r: Rigid) -> Self {
        Self {
            lin: r.rot,
            trans: r.trans,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_frame_inverts_from_rigid() {
        assert_eq!(Rigid::from_frame(&Frame::WORLD), Rigid::IDENTITY);
        // Axis-aligned: exact. x' = world y, z' = world -x, so y' = world -z.
        let f = Frame::new(
            Point3::new(1.0, 2.0, 3.0),
            Vector3::new(-1.0, 0.0, 0.0),
            Vector3::new(0.0, 1.0, 0.0),
        )
        .unwrap();
        let r = Rigid::from_frame(&f);
        assert_eq!(r.apply_point(Point3::ORIGIN), Point3::new(1.0, 2.0, 3.0));
        assert_eq!(
            r.apply_vector(Vector3::new(1.0, 0.0, 0.0)),
            Vector3::new(0.0, 1.0, 0.0)
        );
        assert_eq!(
            r.apply_vector(Vector3::new(0.0, 1.0, 0.0)),
            Vector3::new(0.0, 0.0, -1.0)
        );
        assert_eq!(
            r.apply_vector(Vector3::new(0.0, 0.0, 1.0)),
            Vector3::new(-1.0, 0.0, 0.0)
        );
        assert_eq!(Frame::from_rigid(&r).unwrap(), f);
        // Oblique: within rounding.
        let axis = Vector3::new(1.0, 2.0, 2.0).normalize().unwrap();
        let r = Rigid::translation(Vector3::new(0.5, -1.0, 2.0))
            .then_after(&Rigid::rotation(axis, 0.7));
        let back = Rigid::from_frame(&Frame::from_rigid(&r).unwrap());
        for (a, b) in back
            .rotation_matrix()
            .iter()
            .flatten()
            .zip(r.rotation_matrix().iter().flatten())
        {
            assert!((a - b).abs() < 4.0 * f64::EPSILON, "{a} {b}");
        }
        assert_eq!(back.translation_part(), r.translation_part());
    }
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

    #[test]
    fn affine_scale_and_inverse() {
        let s = Affine::scale(Vector3::new(2.0, 3.0, -1.0));
        assert_eq!(
            s.apply_point(Point3::new(1.0, 1.0, 1.0)),
            Point3::new(2.0, 3.0, -1.0)
        );
        assert_eq!(s.determinant(), -6.0);
        let inv = s.inverse().unwrap();
        assert_eq!(
            inv.apply_point(Point3::new(2.0, 3.0, -1.0)),
            Point3::new(1.0, 1.0, 1.0)
        );
    }

    #[test]
    fn affine_singular_has_no_inverse() {
        assert!(
            Affine::scale(Vector3::new(1.0, 1.0, 0.0))
                .inverse()
                .is_none()
        );
        // Third column within angular resolution of the plane of the first two.
        let m = [[1.0, 0.0, 1.0], [0.0, 1.0, 1.0], [0.0, 0.0, 1e-12]];
        assert!(Affine::new(m, Vector3::ZERO).inverse().is_none());
    }

    #[test]
    fn affine_try_new_rejects_non_finite() {
        let mut m = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
        assert!(Affine::try_new(m, Vector3::ZERO).is_ok());
        m[1][2] = f64::NAN;
        assert_eq!(Affine::try_new(m, Vector3::ZERO), Err(Error::NonFinite));
        let inf = Vector3::new(f64::INFINITY, 0.0, 0.0);
        assert_eq!(Affine::try_new(IDENTITY3, inf), Err(Error::NonFinite));
    }

    #[test]
    fn affine_from_rigid_agrees() {
        let r = Rigid::translation(Vector3::new(0.5, 0.0, 0.0))
            .then_after(&Rigid::rotation(UnitVector3::Y, 1.0));
        let a = Affine::from(r);
        let p = Point3::new(1.0, 2.0, 3.0);
        assert_eq!(a.apply_point(p), r.apply_point(p));
        let ab = a.then_after(&Affine::scale(Vector3::new(2.0, 2.0, 2.0)));
        assert!(near(
            ab.apply_point(p),
            r.apply_point(Point3::new(2.0, 4.0, 6.0))
        ));
    }
}
