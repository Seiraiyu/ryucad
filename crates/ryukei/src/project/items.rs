//! What a view draws (design 326, phase 3): every edge of every placed
//! body with its class, and every silhouette branch, each with its images
//! cut at folds.

use super::hidden::Class;
use crate::blend::convex::{Bend, BendFail, bend};
use crate::topo::{Body, EdgeId};

/// How a drawing draws `body`'s edge `e`: a sheet's boundary (one fin),
/// smooth where `bend` finds its faces tangent, sharp where it certifies
/// a bend (or both bends: no stretch of it is certified tangent), and
/// uncertain where rounding settles neither, or the edge isn't two fins
/// with pcurves (`bend`'s precondition).
#[allow(dead_code)] // used from Task 11
pub(super) fn class_of(body: &Body, e: EdgeId) -> Class {
    let fins = body.edge_fins(e).count();
    if fins == 1 {
        return Class::Boundary;
    }
    if fins != 2 || body.edge_fins(e).any(|f| body.fins[f].pcurve.is_none()) {
        return Class::Uncertain;
    }
    match bend(body, e) {
        Ok(Bend::Convex | Bend::Concave) | Err(BendFail::Changes) => Class::Sharp,
        Ok(Bend::Smooth) => Class::Smooth,
        Err(BendFail::Uncertain) => Class::Uncertain,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blend::{BlendKind, EdgeBlend, blend};
    use crate::geom::{Curve, Frame, Plane, Point3, Rigid, Surface, Vector3};
    use crate::name::OpKey;
    use crate::{primitive, sheet};

    #[test]
    fn a_blocks_edges_are_sharp_and_a_sheets_are_boundaries() {
        let (b, _) = primitive::block(OpKey(1), 0.01, 0.02, 0.03, &Rigid::IDENTITY).unwrap();
        assert!(b.edges().all(|e| class_of(&b, e) == Class::Sharp));
        let f = Frame::new(
            Point3::ORIGIN,
            Vector3::new(0.0, 0.0, 1.0),
            Vector3::new(1.0, 0.0, 0.0),
        )
        .unwrap();
        let plane = Surface::Plane(Plane::new(f));
        let (s, _) = sheet::from_surface(OpKey(2), &plane, [[0.0, 0.01], [0.0, 0.02]]).unwrap();
        assert_eq!(s.edges().count(), 4);
        assert!(s.edges().all(|e| class_of(&s, e) == Class::Boundary));
    }

    #[test]
    fn a_fillets_rails_are_smooth_and_its_ends_sharp() {
        let (dx, dz) = (0.02, 0.04);
        let (b, _) = primitive::block(OpKey(3), dx, 0.03, dz, &Rigid::IDENTITY).unwrap();
        let on = |p: Point3| (p.x - dx).abs() < 1e-12 && (p.z - dz).abs() < 1e-12;
        let e = b
            .edges()
            .find(|&e| {
                let ed = &b.edges[e];
                on(b.vertices[ed.start].point) && on(b.vertices[ed.end].point)
            })
            .unwrap();
        let fillet = EdgeBlend {
            edge: b.edges[e].name,
            kind: BlendKind::Fillet { radius: 0.004 },
        };
        let (r, _) = blend(OpKey(4), &b, &[fillet]).unwrap();
        let cyl = r
            .faces()
            .find(|&f| matches!(r.surfaces[r.faces[f].surface], Surface::Cylinder(_)))
            .unwrap();
        let mut smooth = 0;
        for e in r.edges() {
            let on_cyl = r.edge_fins(e).any(|f| r.loops[r.fins[f].loop_].face == cyl);
            let line = matches!(r.curves[r.edges[e].curve], Curve::Line(_));
            let want = if on_cyl && line {
                smooth += 1;
                Class::Smooth
            } else {
                Class::Sharp
            };
            assert_eq!(class_of(&r, e), want, "{e:?}");
        }
        assert_eq!(smooth, 2);
    }
}
