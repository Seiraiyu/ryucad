//! Sections (design 326, phase 4): a body cut by a plane, through the
//! boolean, and the faces the cut makes (the cap a drawing hatches).

use crate::Error;
use crate::geom::{Box3, Frame, Plane, Point3, Rigid, Vector3};
use crate::intersect::face_box;
use crate::name::{Name, OpKey};
use crate::primitive;
use crate::tol::LINEAR_RESOLUTION;
use crate::topo::{Body, FaceId};

/// What a section subtracts: a block whose base lies on the plane.
pub(crate) struct HalfSpace {
    pub body: Body,
    /// The name of the face on the plane, before the boolean places it.
    pub base: Name,
}

/// The box of a body's faces (each face's enclosure), or `None` for a
/// body with no faces.
pub(crate) fn extent(body: &Body) -> Option<(Point3, Point3)> {
    let b: Box3 = body
        .faces()
        .map(|f| face_box(body, f))
        .reduce(|a, b| a.union(&b))?;
    Some((b.min(), b.max()))
}

/// Whether every vertex of `f`'s loops lies within resolution of the
/// plane through `p0` square to `n`.
pub(crate) fn on_plane(b: &Body, f: FaceId, p0: Point3, n: Vector3) -> bool {
    b.face_loops(f).all(|l| {
        let lp = &b.loops[l];
        lp.vertex
            .map(|v| ((b.vertices[v].point - p0).dot(n)).abs() <= LINEAR_RESOLUTION)
            .unwrap_or(true)
            && b.loop_fins(l).all(|fin| {
                let v = b.start_of(fin);
                ((b.vertices[v].point - p0).dot(n)).abs() <= LINEAR_RESOLUTION
            })
    })
}

/// The half-space in front of `plane` (along its frame's `z`, the side a
/// section cuts away), as a block a hundredth of the body's diagonal past
/// its box every other way. `None` when nothing of the body's box lies
/// more than resolution in front of the plane: the plane touches the
/// body at most, and the section is the body as it is.
pub(crate) fn half_space(
    key: OpKey,
    body: &Body,
    plane: &Plane,
) -> Result<Option<HalfSpace>, Error> {
    let frame = plane.frame();
    let (p0, n) = (frame.origin(), frame.z().get());
    let Some((lo, hi)) = extent(body) else {
        return Ok(None);
    };
    let mut far = f64::NEG_INFINITY;
    for x in [lo.x, hi.x] {
        for y in [lo.y, hi.y] {
            for z in [lo.z, hi.z] {
                far = far.max((Point3::new(x, y, z) - p0).dot(n));
            }
        }
    }
    if far <= LINEAR_RESOLUTION {
        return Ok(None);
    }
    let diagonal = (hi - lo).length();
    let margin = diagonal.max(1e-6) * 0.01;
    // The box's middle brought onto the plane.
    let middle = Point3::new(
        (lo.x + hi.x) / 2.0,
        (lo.y + hi.y) / 2.0,
        (lo.z + hi.z) / 2.0,
    );
    let foot = middle - n * (middle - p0).dot(n);
    let hint = if n.x.abs() < 0.9 {
        Vector3::new(1.0, 0.0, 0.0)
    } else {
        Vector3::new(0.0, 1.0, 0.0)
    };
    let axes = Frame::new(foot, n, hint)?;
    let w = diagonal / 2.0 + margin;
    let (x, y) = (axes.x().get(), axes.y().get());
    let corner = foot - x * w - y * w;
    let place = Rigid::from_frame(&Frame::new(corner, n, x)?);
    let (tool, _) = primitive::block(key, 2.0 * w, 2.0 * w, far + margin, &place)?;
    let base = tool
        .faces()
        .find(|&f| on_plane(&tool, f, p0, n))
        .map(|f| tool.faces[f].name)
        .expect("a block has a face on its base");
    Ok(Some(HalfSpace { body: tool, base }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{Frame, Plane, Point3, Rigid, Vector3};
    use crate::name::OpKey;
    use crate::primitive;

    fn plane(origin: Point3, normal: Vector3) -> Plane {
        let hint = if normal.x.abs() < 0.9 {
            Vector3::new(1.0, 0.0, 0.0)
        } else {
            Vector3::new(0.0, 1.0, 0.0)
        };
        Plane::new(Frame::new(origin, normal, hint).unwrap())
    }

    #[test]
    fn the_half_space_stands_on_the_plane_in_front_of_it() {
        let (b, _) = primitive::block(OpKey(1), 0.04, 0.03, 0.01, &Rigid::IDENTITY).unwrap();
        let p = plane(Point3::new(0.02, 0.0, 0.0), Vector3::new(1.0, 0.0, 0.0));
        let h = half_space(OpKey(2), &b, &p).unwrap().unwrap();
        // Its base lies on the plane, and every vertex is on it or in
        // front of it.
        let base = h
            .body
            .faces()
            .find(|f| h.body.faces[*f].name == h.base)
            .unwrap();
        assert!(on_plane(
            &h.body,
            base,
            Point3::new(0.02, 0.0, 0.0),
            Vector3::new(1.0, 0.0, 0.0)
        ));
        for (_, v) in h.body.vertices.live() {
            assert!(v.point.x >= 0.02 - 1e-15, "{:?}", v.point);
        }
        // It covers the body's far side.
        let (lo, hi) = extent(&h.body).unwrap();
        assert!(lo.y < 0.0 && hi.y > 0.03 && lo.z < 0.0 && hi.z > 0.01 && hi.x > 0.04);
    }

    #[test]
    fn a_plane_on_a_face_or_past_the_body_needs_no_half_space() {
        let (b, _) = primitive::block(OpKey(1), 0.04, 0.03, 0.01, &Rigid::IDENTITY).unwrap();
        let top = plane(Point3::new(0.0, 0.0, 0.01), Vector3::new(0.0, 0.0, 1.0));
        assert!(half_space(OpKey(2), &b, &top).unwrap().is_none());
        let below = plane(Point3::new(0.0, 0.0, 0.0), Vector3::new(0.0, 0.0, -1.0));
        assert!(half_space(OpKey(2), &b, &below).unwrap().is_none());
        assert!(
            half_space(OpKey(2), &crate::topo::Body::new(), &top)
                .unwrap()
                .is_none()
        );
    }
}
