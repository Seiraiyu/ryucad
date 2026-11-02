//! What a face meeting a bend is, against the bend's axis.
//!
//! Planes are judged in closed form: square to the axis (a bend's end
//! face, which may run on across flanges), through it (radial), along it
//! but off it (material running past the bend), or tilted. Any other
//! surface is radial when it is made of lines from the axis outwards,
//! that is when its normal is square to the direction away from the axis
//! everywhere: `g = N · r` with `N = S_u × S_v` and `r` the point's offset
//! from the axis made square to it. It is certified cell by cell by the
//! mean value form within `|g| <= s |N| |r|`, `s = tol / √(t² + tol²)`, so
//! across the sheet's thickness `t` the face strays at most `tol` from the
//! radial surface through its edge on the inner cylinder.

use super::cells::{self, Form, Parts, Verdict, add, cross, dot, iv, scale, sub};
use super::shape::{self, flat};
use crate::geom::Frame;
use crate::intersect::face_rect;
use crate::interval::Interval;
use crate::tol::ANGULAR_RESOLUTION;
use crate::topo::{Body, FaceId};

/// What a wall is to a bend.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Wall {
    /// A plane square to the axis.
    Square,
    /// A plane through the axis.
    Through,
    /// A plane along the axis but off it.
    Along,
    /// Any other plane.
    Tilted,
    /// Another surface, certified radial within the tolerance.
    Radial,
    /// Another surface, certified not radial somewhere.
    NotRadial,
    /// Another surface the cells couldn't settle.
    Unsure,
}

/// `v` made square to `a`, over intervals.
fn across(v: cells::IVec, a: cells::IVec) -> cells::IVec {
    sub(v, scale(a, dot(v, a)))
}

pub(super) fn classify(b: &Body, f: FaceId, axis: &Frame, t: f64, tol: f64) -> Wall {
    let eps = tol + shape::face_tolerance(b, f);
    let a = axis.z();
    if let Some(p) = flat(b, f) {
        let n = p.normal;
        if shape::parallel(n, a, ANGULAR_RESOLUTION) {
            return Wall::Square;
        }
        if n.get().dot(a.get()).abs() > ANGULAR_RESOLUTION {
            return Wall::Tilted;
        }
        return if p.height(axis.origin()).abs() <= eps {
            Wall::Through
        } else {
            Wall::Along
        };
    }
    let surface = &b.surfaces[b.faces[f].surface];
    let Some(rect) = face_rect(b, f) else {
        return Wall::Unsure;
    };
    let o = iv(axis.origin().to_vector());
    let ai = iv(a.get());
    let s2 = Interval::point(tol * tol / (t * t + tol * tol));
    let offset = |q: &Parts| across(sub(q.p, o), ai);
    let value = |q: &Parts| dot(cross(q.u, q.v), offset(q));
    let gradient = |q: &Parts| {
        let n = cross(q.u, q.v);
        let r = offset(q);
        let nu = add(cross(q.uu, q.v), cross(q.u, q.uv));
        let nv = add(cross(q.uv, q.v), cross(q.u, q.vv));
        [
            dot(nu, r) + dot(n, across(q.u, ai)),
            dot(nv, r) + dot(n, across(q.v, ai)),
        ]
    };
    let form = Form {
        value: &value,
        gradient: &gradient,
    };
    let verdict = cells::judge(surface, rect, |sheet, cell| {
        let Some((g, over)) = form.over(sheet, cell) else {
            return Verdict::Unsure;
        };
        let n = cross(over.u, over.v);
        let r = offset(&over);
        let den = dot(n, n) * dot(r, r) * s2;
        let g2 = g.square();
        if den.lo() > 0.0 && g2.hi() <= den.lo() {
            Verdict::Yes
        } else if g2.lo() > den.hi() {
            Verdict::No
        } else {
            Verdict::Unsure
        }
    });
    match verdict {
        Verdict::Yes => Wall::Radial,
        Verdict::No => Wall::NotRadial,
        Verdict::Unsure => Wall::Unsure,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::geom::{Rigid, Surface, UnitVector3, Vector3};
    use crate::name::OpKey;
    use crate::primitive;

    /// The block's face whose outward normal is `n`.
    fn face(b: &Body, n: Vector3) -> FaceId {
        b.faces()
            .find(|&f| flat(b, f).is_some_and(|p| (p.normal.get() - n).length() < 1e-12))
            .expect("a face facing that way")
    }

    /// A planar face's surface written as its NURBS form over its reach.
    pub(crate) fn to_spline(b: &mut Body, f: FaceId) {
        let face = &b.faces[f];
        let (surface, sense) = (b.surfaces[face.surface].clone(), face.sense);
        let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
        for e in shape::edges_of(b, f) {
            for p in shape::ends(b, e) {
                let (u, v) = surface.project(p).param;
                for (k, x) in [u, v].into_iter().enumerate() {
                    lo[k] = lo[k].min(x);
                    hi[k] = hi[k].max(x);
                }
            }
        }
        let ranges = [[lo[0] - 1e-4, hi[0] + 1e-4], [lo[1] - 1e-4, hi[1] + 1e-4]];
        let (n, _) = surface.to_nurbs(ranges).unwrap();
        let id = b.add_surface(Surface::Nurbs(std::sync::Arc::new(n)));
        crate::euler::set_surface(b, f, id, sense).unwrap();
        crate::pcurve::fill(b).unwrap();
    }

    #[test]
    fn planes_are_judged_against_the_axis_in_closed_form() {
        let b = primitive::block(OpKey(1), 1.0, 1.0, 1.0, &Rigid::IDENTITY)
            .unwrap()
            .0;
        let axis = Frame::WORLD;
        let at = |n: Vector3| classify(&b, face(&b, n), &axis, 0.1, 1e-7);
        assert_eq!(at(Vector3::new(0.0, 0.0, 1.0)), Wall::Square);
        assert_eq!(at(Vector3::new(-1.0, 0.0, 0.0)), Wall::Through);
        assert_eq!(at(Vector3::new(0.0, -1.0, 0.0)), Wall::Through);
        assert_eq!(at(Vector3::new(1.0, 0.0, 0.0)), Wall::Along);
        let turned = Rigid::rotation(UnitVector3::X, 0.3);
        let c = primitive::block(OpKey(1), 1.0, 1.0, 1.0, &turned)
            .unwrap()
            .0;
        let tilted = c
            .faces()
            .find(|&f| flat(&c, f).is_some_and(|p| p.normal.get().y < -0.5))
            .unwrap();
        assert_eq!(classify(&c, tilted, &axis, 0.1, 1e-7), Wall::Tilted);
    }

    #[test]
    fn other_surfaces_are_radial_by_their_cells() {
        // A block's face through the axis, off it as a bend's wall is,
        // written as a NURBS is radial; a cylinder about the axis is as far
        // from radial as can be.
        let off = Rigid::translation(Vector3::new(0.0, 0.5, 0.0));
        let mut b = primitive::block(OpKey(1), 1.0, 1.0, 1.0, &off).unwrap().0;
        let f = face(&b, Vector3::new(-1.0, 0.0, 0.0));
        to_spline(&mut b, f);
        let axis = Frame::WORLD;
        assert_eq!(classify(&b, f, &axis, 0.1, 1e-7), Wall::Radial);
        let c = primitive::cylinder(OpKey(1), 0.5, 1.0, &Rigid::IDENTITY)
            .unwrap()
            .0;
        let side = c.faces().find(|&f| flat(&c, f).is_none()).unwrap();
        assert_eq!(classify(&c, side, &axis, 0.1, 1e-7), Wall::NotRadial);
    }
}
