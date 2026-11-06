//! Where a point of a changed face goes.

use crate::geom::{Point3, Rigid, Surface, Vector3};
use crate::topo::Sense;

/// How a changed face moves: what its new surface is, and where each of
/// its points goes, which chooses the branch of every edge that closes
/// and how far a search may reach.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Motion {
    /// Along its outward normal by the distance (the offset's own).
    Offset(f64),
    /// By a rigid motion.
    Rigid(Rigid),
    /// To its foot on its new surface.
    Replace,
}

impl Motion {
    /// The distance an offset moves by; zero for any other motion (the
    /// offset's code reads distances only through offsets).
    pub(crate) fn distance(&self) -> f64 {
        match self {
            Self::Offset(d) => *d,
            Self::Rigid(_) | Self::Replace => 0.0,
        }
    }
}

/// Where the point `p` of a face with outward normal `n` goes under `m`,
/// and its outward normal there: `new` is the face's new surface and
/// `sense` the face's sense on it (read only by a replacement).
pub(crate) fn apply(
    m: &Motion,
    new: &Surface,
    sense: Sense,
    p: Point3,
    n: Vector3,
) -> (Point3, Vector3) {
    match m {
        Motion::Offset(d) => (p + n * *d, n),
        Motion::Rigid(r) => (r.apply_point(p), r.apply_vector(n)),
        Motion::Replace => {
            let foot = new.project(p);
            let (u, v) = foot.param;
            let sign = if sense == Sense::Forward { 1.0 } else { -1.0 };
            let m = new.normal(u, v).map_or(n, |x| x.get() * sign);
            (foot.point, m)
        }
    }
}

/// The point on both planes (through `q[k]`, unit normal `n[k]`) nearest
/// their points' mean; where the planes are within `1e-6` of parallel,
/// the mean itself.
pub(crate) fn on_both_planes(q: [Point3; 2], n: [Vector3; 2]) -> Point3 {
    let m = q[0] + (q[1] - q[0]) * 0.5;
    let c = n[0].dot(n[1]);
    let det = 1.0 - c * c;
    if det < 1e-12 {
        return m;
    }
    let r = [n[0].dot(q[0] - m), n[1].dot(q[1] - m)];
    let alpha = (r[0] - c * r[1]) / det;
    let beta = (r[1] - c * r[0]) / det;
    m + n[0] * alpha + n[1] * beta
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{Frame, Plane, UnitVector3};

    fn unit(x: f64, y: f64, z: f64) -> UnitVector3 {
        Vector3::new(x, y, z).normalize().unwrap()
    }

    #[test]
    fn offsets_move_along_the_normal_and_rigid_motions_carry_it() {
        let p = Point3::new(1.0, 2.0, 3.0);
        let n = Vector3::new(0.0, 0.0, 1.0);
        let plane = Surface::Plane(Plane::new(Frame::WORLD));
        let (q, m) = apply(&Motion::Offset(0.25), &plane, Sense::Forward, p, n);
        assert_eq!((q, m), (Point3::new(1.0, 2.0, 3.25), n));
        let r = Rigid::rotation(unit(1.0, 0.0, 0.0), core::f64::consts::FRAC_PI_2);
        let (q, m) = apply(&Motion::Rigid(r), &plane, Sense::Forward, p, n);
        assert!(q.distance(Point3::new(1.0, -3.0, 2.0)) < 1e-12);
        assert!((m - Vector3::new(0.0, -1.0, 0.0)).length() < 1e-12);
    }

    #[test]
    fn a_replacement_takes_the_foot_and_its_normal_by_the_sense() {
        let top = Surface::Plane(Plane::new(
            Frame::new(
                Point3::new(0.0, 0.0, 0.5),
                Vector3::new(0.0, 0.0, 1.0),
                Vector3::new(1.0, 0.0, 0.0),
            )
            .unwrap(),
        ));
        let p = Point3::new(0.3, 0.2, 0.4);
        let n = Vector3::new(0.0, 0.0, 1.0);
        let (q, m) = apply(&Motion::Replace, &top, Sense::Forward, p, n);
        assert!(q.distance(Point3::new(0.3, 0.2, 0.5)) < 1e-15);
        assert_eq!(m, n);
        let (_, m) = apply(&Motion::Replace, &top, Sense::Reversed, p, n);
        assert_eq!(m, -n);
    }

    #[test]
    fn two_moved_planes_meet_nearest_their_points() {
        // A box's top raised by 0.1 and its side kept: the edge's middle
        // goes straight up.
        let q = [Point3::new(1.0, 0.3, 0.5), Point3::new(1.0, 0.3, 0.4)];
        let n = [Vector3::new(0.0, 0.0, 1.0), Vector3::new(1.0, 0.0, 0.0)];
        let x = on_both_planes(q, n);
        assert!(x.distance(Point3::new(1.0, 0.3, 0.5)) < 1e-15);
        // Parallel planes: their points' mean.
        let x = on_both_planes(q, [n[0], n[0]]);
        assert!(x.distance(Point3::new(1.0, 0.3, 0.45)) < 1e-15);
    }
}
