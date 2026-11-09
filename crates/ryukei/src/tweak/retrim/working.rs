//! Working faces' surfaces: each region face's surface (its new one, or
//! its own) carried past its domain where a bounded surface needs to
//! reach the region box, and the parameter box covering the box.

use crate::geom::{Box3, Surface};
use crate::intersect::{carried_by, face_rect};
use crate::topo::{Body, FaceId, Sense};
use crate::{Error, TweakError};

/// One working face's surface, parameter box and sense.
#[derive(Clone, Debug)]
pub(crate) struct Working {
    pub(crate) surface: Surface,
    pub(crate) rect: [[f64; 2]; 2],
    pub(crate) sense: Sense,
    /// Whether the surface could be carried where the region needs it: a
    /// piece reaching the side of a face that couldn't is
    /// `ExtensionUnsupported`, not `RegionTooSmall`.
    pub(crate) extendable: bool,
}

/// Whether `s` is a bounded surface `carried_by` takes.
fn carriable(s: &Surface) -> bool {
    matches!(
        s,
        Surface::Nurbs(_) | Surface::Extrusion(_) | Surface::Revolution(_)
    )
}

/// Face `f`'s working surface: `new` where the tweak gives one, else its
/// own, under `sense`. `same_params` says the surface keeps the old one's
/// parameters (no new surface, a rigid motion or an offset), so the face's
/// own parameter box can be read from its pcurves. The parameter box holds
/// that box and the feet of `region`'s corners, grown by a sixteenth each
/// way, within the domain; a periodic direction is a whole period. A
/// bounded surface is first carried past each end of its domain its face
/// comes near, by three times the region's diagonal.
pub(crate) fn working(
    body: &Body,
    f: FaceId,
    new: Option<&Surface>,
    sense: Sense,
    same_params: bool,
    region: &Box3,
) -> Result<Working, Error> {
    let name = body.faces[f].name;
    let mut surface = new
        .cloned()
        .unwrap_or_else(|| body.surfaces[body.faces[f].surface].clone());
    let base = if same_params {
        face_rect(body, f)
    } else {
        None
    };
    let diagonal = region.min().distance(region.max());
    let mut extendable = true;
    if carriable(&surface) {
        let domain = surface.domain();
        let (pu, pv) = surface.periods();
        let rect0 = base.unwrap_or(domain);
        let mid = surface.eval(
            (rect0[0][0] + rect0[0][1]) / 2.0,
            (rect0[1][0] + rect0[1][1]) / 2.0,
        );
        let speed = [mid.du.length(), mid.dv.length()];
        let m = crate::tweak::extend::margins(rect0, domain, [pu, pv], speed, diagonal);
        if m != [[0.0; 2]; 2] {
            match carried_by(&surface, m) {
                Some(c) => surface = c,
                None => extendable = false,
            }
        }
    } else if !matches!(
        surface,
        Surface::Plane(_)
            | Surface::Cylinder(_)
            | Surface::Cone(_)
            | Surface::Sphere(_)
            | Surface::Torus(_)
    ) {
        // Pipes, sweeps, offsets and bent surfaces aren't carried.
        extendable = false;
    }
    let domain = surface.domain();
    let (pu, pv) = surface.periods();
    let periods = [pu, pv];
    let mut want = base.unwrap_or([[f64::INFINITY, f64::NEG_INFINITY]; 2]);
    for c in region.corners() {
        let (u, v) = surface.project(c).param;
        want[0] = [want[0][0].min(u), want[0][1].max(u)];
        want[1] = [want[1][0].min(v), want[1][1].max(v)];
    }
    let mut rect = [[0.0; 2]; 2];
    for k in 0..2 {
        if let Some(p) = periods[k] {
            rect[k] = [domain[k][0], domain[k][0] + p];
            continue;
        }
        let [a, b] = want[k];
        if !(a.is_finite() && b.is_finite() && a < b) {
            return Err(TweakError::ExtensionUnsupported { face: name }.into());
        }
        let g = (b - a) / 16.0;
        rect[k] = [(a - g).max(domain[k][0]), (b + g).min(domain[k][1])];
    }
    Ok(Working {
        surface,
        rect,
        sense,
        extendable,
    })
}

/// Whether a face moved by `motion` keeps its surface's parameters.
pub(crate) fn same_params(motion: Option<&crate::tweak::Motion>) -> bool {
    matches!(
        motion,
        None | Some(crate::tweak::Motion::Rigid(_)) | Some(crate::tweak::Motion::Offset(_))
    )
}

#[cfg(test)]
mod tests {
    use super::super::tests::{K, block, face_of};
    use super::*;
    use crate::geom::Point3;
    use crate::name::FaceRole;

    fn region_box() -> Box3 {
        Box3::new(Point3::new(-0.5, -0.5, -0.5), Point3::new(1.5, 1.1, 0.9))
    }

    #[test]
    fn a_plane_covers_the_box_as_it_sees_it() {
        let b = block();
        let top = face_of(&b, FaceRole::PlusZ);
        let w = working(&b, top, None, b.faces[top].sense, true, &region_box()).unwrap();
        assert!(w.extendable);
        // Every corner of the box projects inside the rect.
        for c in region_box().corners() {
            let (u, v) = w.surface.project(c).param;
            assert!(u >= w.rect[0][0] && u <= w.rect[0][1], "{u} {:?}", w.rect);
            assert!(v >= w.rect[1][0] && v <= w.rect[1][1], "{v} {:?}", w.rect);
        }
    }

    #[test]
    fn a_cylinder_takes_its_whole_turn() {
        let (c, _) = crate::primitive::cylinder(
            super::super::tests::K,
            0.1,
            0.3,
            &crate::geom::Rigid::IDENTITY,
        )
        .unwrap();
        let side = c
            .faces()
            .find(|&f| matches!(c.surfaces[c.faces[f].surface], Surface::Cylinder(_)))
            .unwrap();
        let w = working(&c, side, None, c.faces[side].sense, true, &region_box()).unwrap();
        let p = core::f64::consts::TAU;
        assert!((w.rect[0][1] - w.rect[0][0] - p).abs() < 1e-12);
        assert!(w.rect[1][0] <= -0.5 && w.rect[1][1] >= 0.9);
    }

    #[test]
    fn a_spline_top_is_carried_past_its_domain() {
        // A spline patch of the block's top plane, as a sheet of one face.
        let b = block();
        let top = face_of(&b, FaceRole::PlusZ);
        let plane = &b.surfaces[b.faces[top].surface];
        let (n, _) = plane.to_nurbs([[0.1, 0.5], [0.1, 0.3]]).unwrap();
        let s = Surface::Nurbs(std::sync::Arc::new(n));
        let (sheet, _) = crate::sheet::from_surface(K, &s, s.domain()).unwrap();
        let f = sheet.faces().next().unwrap();
        let before = sheet.surfaces[sheet.faces[f].surface].domain();
        let w = working(&sheet, f, None, sheet.faces[f].sense, true, &region_box()).unwrap();
        let after = w.surface.domain();
        assert!(w.extendable);
        assert!(after[0][0] < before[0][0] && after[0][1] > before[0][1]);
        assert!(after[1][0] < before[1][0] && after[1][1] > before[1][1]);
    }
}
