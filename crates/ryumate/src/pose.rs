//! Poses: where a body's frame sits, as a rotation and an origin. The
//! rotation's columns are the frame's x, y and z axes; its rows are what
//! `ryukei::geom::Rigid::rotation_matrix` gives, so a caller hands its
//! placements over as they are.

/// A rigid placement: `p ↦ rotation · p + origin`, meters. `rotation` is
/// row by row; its columns are the placed frame's axes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    pub rotation: [[f64; 3]; 3],
    pub origin: [f64; 3],
}

impl Pose {
    pub const IDENTITY: Pose = Pose {
        rotation: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
        origin: [0.0, 0.0, 0.0],
    };

    /// The pose with axes `x`, `y` and `z` (columns) at `origin`.
    pub fn from_axes(origin: [f64; 3], x: [f64; 3], y: [f64; 3], z: [f64; 3]) -> Pose {
        Pose {
            rotation: [[x[0], y[0], z[0]], [x[1], y[1], z[1]], [x[2], y[2], z[2]]],
            origin,
        }
    }

    /// Axis `k` (0 x, 1 y, 2 z): column `k` of the rotation.
    pub fn axis(&self, k: usize) -> [f64; 3] {
        [
            self.rotation[0][k],
            self.rotation[1][k],
            self.rotation[2][k],
        ]
    }

    /// `p` placed.
    pub fn apply(&self, p: [f64; 3]) -> [f64; 3] {
        add(self.turn(p), self.origin)
    }

    /// Vector `v` turned (no translation).
    pub fn turn(&self, v: [f64; 3]) -> [f64; 3] {
        let r = &self.rotation;
        [
            r[0][0] * v[0] + r[0][1] * v[1] + r[0][2] * v[2],
            r[1][0] * v[0] + r[1][1] * v[1] + r[1][2] * v[2],
            r[2][0] * v[0] + r[2][1] * v[1] + r[2][2] * v[2],
        ]
    }

    /// `self` after `inner`: `p ↦ self(inner(p))`.
    pub fn then_after(&self, inner: &Pose) -> Pose {
        let mut rotation = [[0.0; 3]; 3];
        for (i, row) in rotation.iter_mut().enumerate() {
            for (j, x) in row.iter_mut().enumerate() {
                *x = self.rotation[i][0] * inner.rotation[0][j]
                    + self.rotation[i][1] * inner.rotation[1][j]
                    + self.rotation[i][2] * inner.rotation[2][j];
            }
        }
        Pose {
            rotation,
            origin: self.apply(inner.origin),
        }
    }

    /// The inverse placement, for a rotation that is orthonormal.
    pub fn inverse(&self) -> Pose {
        let r = &self.rotation;
        let t = [
            [r[0][0], r[1][0], r[2][0]],
            [r[0][1], r[1][1], r[2][1]],
            [r[0][2], r[1][2], r[2][2]],
        ];
        let inv = Pose {
            rotation: t,
            origin: [0.0; 3],
        };
        let o = inv.turn(self.origin);
        Pose {
            rotation: t,
            origin: [-o[0], -o[1], -o[2]],
        }
    }

    /// How far the rotation is from orthonormal: the largest entry of
    /// `RᵀR - I`.
    pub fn skew(&self) -> f64 {
        let mut worst: f64 = 0.0;
        for i in 0..3 {
            for j in 0..3 {
                let d = dot(self.axis(i), self.axis(j)) - if i == j { 1.0 } else { 0.0 };
                worst = worst.max(d.abs());
            }
        }
        worst
    }

    pub(crate) fn finite(&self) -> bool {
        self.rotation.iter().flatten().all(|x| x.is_finite())
            && self.origin.iter().all(|x| x.is_finite())
    }
}

pub(crate) fn add(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

pub(crate) fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

pub(crate) fn scale(a: [f64; 3], k: f64) -> [f64; 3] {
    [a[0] * k, a[1] * k, a[2] * k]
}

pub(crate) fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

pub(crate) fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

pub(crate) fn norm(a: [f64; 3]) -> f64 {
    ryukei::math::sqrt(dot(a, a))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turn_z(a: f64) -> Pose {
        let (s, c) = ryukei::math::sin_cos(a);
        Pose {
            rotation: [[c, -s, 0.0], [s, c, 0.0], [0.0, 0.0, 1.0]],
            origin: [1.0, 2.0, 3.0],
        }
    }

    #[test]
    fn poses_compose_and_invert() {
        let p = turn_z(0.3);
        let back = p.inverse().then_after(&p);
        for i in 0..3 {
            for j in 0..3 {
                let want = if i == j { 1.0 } else { 0.0 };
                assert!((back.rotation[i][j] - want).abs() < 1e-15);
            }
            assert!(back.origin[i].abs() < 1e-15);
        }
        let q = Pose::from_axes([0.0; 3], [0.0, 1.0, 0.0], [-1.0, 0.0, 0.0], [0.0, 0.0, 1.0]);
        assert_eq!(q.apply([1.0, 0.0, 0.0]), [0.0, 1.0, 0.0]);
        assert_eq!(q.axis(1), [-1.0, 0.0, 0.0]);
        assert!(p.skew() < 1e-15);
        assert_eq!(Pose::IDENTITY.then_after(&p), p);
    }
}
