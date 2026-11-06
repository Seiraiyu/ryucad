//! What a view draws (design 326, phase 3): every edge of every placed
//! body with its class, and every silhouette branch, each with its images
//! cut at folds.

use super::hidden::{Class, Source};
use super::{ImageCurve, Silhouette, Silhouettes, View, project_curve};
use crate::blend::convex::{Bend, BendFail, bend};
use crate::geom::Curve;
use crate::intersect::face_box;
use crate::math;
use crate::tol::{LINEAR_RESOLUTION, touch_reach};
use crate::topo::{Body, EdgeId, FaceId};
use crate::work::Meter;
use crate::{Error, ViewError};

/// How a drawing draws `body`'s edge `e`: a sheet's boundary (one fin),
/// smooth where `bend` finds its faces tangent, sharp where it certifies
/// a bend (or both bends: no stretch of it is certified tangent), and
/// uncertain where rounding settles neither, or the edge isn't two fins
/// with pcurves (`bend`'s precondition).
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

/// One 3D curve a view draws, with its images cut at folds.
#[derive(Clone, Debug)]
pub(super) struct Item {
    pub(super) source: Source,
    pub(super) curve: Curve,
    pub(super) images: Vec<ImageCurve>,
    /// The curve's tolerance, at least resolution.
    pub(super) tolerance: f64,
    pub(super) class: Class,
    pub(super) on_silhouette: bool,
    /// The placed body it lies on, and the faces of that body it lies on
    /// (an edge's faces, a silhouette's face), sorted.
    pub(super) body: usize,
    pub(super) own: Vec<FaceId>,
    /// How far along a ray from the curve hits on its own faces are
    /// skipped.
    pub(super) skip_own: f64,
}

/// Everything `bodies` (placed, with `originals` as given) draw in
/// `view`: per body, its edges in ID order, then its silhouette branches
/// as `silhouettes` holds them. An edge whose image can't be certified is
/// `ViewError::Image` naming it; a silhouette's, `ViewError::Uncertain`
/// naming its face.
pub(super) fn items(
    bodies: &[&Body],
    originals: &[&Body],
    silhouettes: &[Silhouettes],
    view: &View,
    m: &Meter,
) -> Result<Vec<Item>, Error> {
    let mut out = Vec::new();
    for (b, &body) in bodies.iter().enumerate() {
        let sil = &silhouettes[b];
        let edges: Vec<EdgeId> = body.edges().collect();
        let per = m.par_map(&edges, |m, &e| {
            edge_item(body, originals[b], b, e, sil, view, m)
        });
        for r in per {
            out.push(r?);
        }
        for s in &sil.branches {
            out.push(silhouette_item(body, b, s, view, m)?);
        }
    }
    Ok(out)
}

fn edge_item(
    body: &Body,
    original: &Body,
    b: usize,
    e: EdgeId,
    sil: &Silhouettes,
    view: &View,
    m: &Meter,
) -> Result<Item, Error> {
    let edge = &body.edges[e];
    let fail = || Error::InvalidView(ViewError::Image { at: edge.name });
    // An edge projected. A halt is reported by `work::run`, whatever error
    // carries it out.
    m.charge(1).map_err(|_| fail())?;
    let curve = body.curves[edge.curve].clone();
    let images = project_curve(&curve, edge.interval, view).map_err(|_| fail())?;
    let tolerance = edge.tolerance.get().max(LINEAR_RESOLUTION);
    let mut own: Vec<FaceId> = body
        .edge_fins(e)
        .map(|f| body.loops[body.fins[f].loop_].face)
        .collect();
    own.sort();
    own.dedup();
    Ok(Item {
        source: Source::Edge {
            body: b as u32,
            edge: e,
            name: edge.name,
        },
        curve,
        images,
        tolerance,
        class: class_of(original, e),
        on_silhouette: sil.on_edges.binary_search(&e).is_ok(),
        body: b,
        own,
        skip_own: touch_reach(tolerance),
    })
}

fn silhouette_item(
    body: &Body,
    b: usize,
    s: &Silhouette,
    view: &View,
    m: &Meter,
) -> Result<Item, Error> {
    let fail = || Error::InvalidView(ViewError::Uncertain { at: s.face_name });
    // A silhouette branch projected.
    m.charge(1).map_err(|_| fail())?;
    let images = project_curve(&s.curve, s.range, view).map_err(|_| fail())?;
    let tolerance = s.tolerance.max(LINEAR_RESOLUTION);
    let bx = face_box(body, s.face);
    let size = (bx.max() - bx.min()).length();
    let reach = touch_reach(tolerance);
    Ok(Item {
        source: Source::Silhouette {
            body: b as u32,
            face: s.face,
            face_name: s.face_name,
            ordinal: s.ordinal,
        },
        curve: s.curve.clone(),
        images,
        tolerance,
        class: Class::Silhouette,
        on_silhouette: false,
        body: b,
        own: vec![s.face],
        skip_own: math::sqrt(2.0 * size * reach).max(reach),
    })
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

    use crate::project::{View, silhouettes};
    use crate::work::Meter;

    fn gather(b: &Body, v: &View) -> Vec<Item> {
        let s = silhouettes(b, v).unwrap();
        // unmetered: a unit test
        items(&[b], &[b], &[s], v, &Meter::free()).unwrap()
    }

    #[test]
    fn a_blocks_items_are_its_twelve_edges_seen_once_each() {
        let (b, _) = primitive::block(OpKey(5), 0.01, 0.02, 0.03, &Rigid::IDENTITY).unwrap();
        let iso = View::new(Vector3::new(1.0, -1.0, 1.0), Vector3::new(0.0, 0.0, 1.0)).unwrap();
        let it = gather(&b, &iso);
        assert_eq!(it.len(), 12);
        for i in &it {
            assert!(matches!(i.source, Source::Edge { body: 0, .. }));
            assert_eq!(i.images.len(), 1);
            assert_eq!(i.own.len(), 2);
            assert_eq!(i.class, Class::Sharp);
        }
    }

    #[test]
    fn a_cylinder_side_on_draws_two_rims_folded_in_halves_and_two_rulings() {
        let (c, _) = primitive::cylinder(OpKey(6), 0.01, 0.03, &Rigid::IDENTITY).unwrap();
        let front = View::new(Vector3::new(0.0, -1.0, 0.0), Vector3::new(0.0, 0.0, 1.0)).unwrap();
        let it = gather(&c, &front);
        let (edges, sil): (Vec<&Item>, Vec<&Item>) = it
            .iter()
            .partition(|i| matches!(i.source, Source::Edge { .. }));
        assert_eq!(edges.len(), 2);
        // Each rim is edge-on, cut where it folds: its frame's x is the
        // view's, so the folds fall at 0 and π and each rim is two halves.
        assert!(edges.iter().all(|i| i.images.len() == 2));
        assert_eq!(sil.len(), 2);
        for s in sil {
            assert_eq!(s.class, Class::Silhouette);
            assert_eq!(s.own.len(), 1);
            assert!(s.skip_own > crate::tol::touch_reach(s.tolerance));
        }
    }
}
