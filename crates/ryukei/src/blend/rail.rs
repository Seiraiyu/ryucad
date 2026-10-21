//! Rails: where a blend meets the faces it lies on.
//!
//! A fillet's rail on a face is where the ball touches it: the face's
//! surface along the spine's pcurve on the face's offset. Along a line or
//! a circle spine the rails are lines and circles, recognized in closed
//! form and checked; along any other spine they are fitted
//! ([`crate::intersect::fit_along`]), with the fitted pcurve on the face.
//! Every rail is parameterized by an affine function of the spine's
//! parameter, so rails and the blend surface agree on where sections are.

use super::spine::Spine;
use crate::geom::{Circle, Curve, Curve2, Frame, Line, Point3, Surface};
use crate::intersect::{Failure, fit_along};
use crate::tol::LINEAR_RESOLUTION;
use std::sync::Arc;

/// A rail of a blend on one of its faces.
#[derive(Clone, Debug)]
pub(crate) struct Rail {
    pub(crate) curve: Curve,
    /// The rail's parameter at the spine's `t` is `map[0] + map[1] t`.
    pub(crate) map: [f64; 2],
    /// The rail on its face: a pcurve and the affine map from the rail's
    /// parameter to the pcurve's.
    pub(crate) pcurve: (Curve2, [f64; 2]),
    /// How far the rail may stray from its face, its pcurve's image and
    /// the blend surface, certified for a fitted rail.
    pub(crate) tolerance: f64,
    /// Its pcurve on the blend surface, where the rail is known to lie on
    /// a parameter line of it (a ruled chamfer's), by the rail's
    /// parameter.
    pub(crate) on_blend: Option<(Curve2, [f64; 2])>,
}

impl Rail {
    /// The rail's parameter at the spine's `t`.
    pub(crate) fn param(&self, t: f64) -> f64 {
        self.map[0] + self.map[1] * t
    }
}

/// Samples at which a rail recognized in closed form is checked.
const CHECKS: usize = 9;

/// The rail of `spine` on its face `k`, whose surface is `surface`, over
/// the spine's parameters `range`. Fails with `Uncertain` where a fitted
/// rail can't be certified within `10 × LINEAR_RESOLUTION`.
pub(crate) fn rail(
    spine: &Spine,
    surface: &Surface,
    k: usize,
    range: [f64; 2],
) -> Result<Rail, Failure> {
    let (pc, m) = &spine.pcurves[k];
    let foot = |t: f64| {
        let uv = pc.point(m[0] + m[1] * t);
        surface.point(uv.x, uv.y)
    };
    let exact = match &spine.curve {
        Curve::Line(_) => line_rail(&foot, range),
        Curve::Circle(c) => circle_rail(c, &foot, range),
        _ => None,
    };
    if let Some((curve, map, off)) = exact {
        // The pcurve by the rail's parameter `τ = a + b t`.
        let (a, b) = (map[0], map[1]);
        let pmap = [m[0] - m[1] * a / b, m[1] / b];
        return Ok(Rail {
            curve,
            map,
            pcurve: (pc.clone(), pmap),
            tolerance: off.max(LINEAR_RESOLUTION) + spine.tolerance,
            on_blend: None,
        });
    }
    let lifted = fit_along(surface, pc, *m, range)?;
    Ok(Rail {
        curve: Curve::Nurbs(Arc::new(lifted.curve)),
        map: [0.0, 1.0],
        pcurve: (Curve2::Nurbs2(Arc::new(lifted.pcurve)), [0.0, 1.0]),
        tolerance: lifted.tolerance + spine.tolerance,
        on_blend: None,
    })
}

/// The worst distance between the feet and a candidate rail at
/// [`CHECKS`] samples, when every one is within resolution.
fn checked(
    foot: &dyn Fn(f64) -> Point3,
    curve: &Curve,
    map: [f64; 2],
    [t0, t1]: [f64; 2],
) -> Option<f64> {
    let mut worst = 0.0f64;
    for i in 0..CHECKS {
        let t = t0 + (t1 - t0) * i as f64 / (CHECKS - 1) as f64;
        worst = worst.max(curve.point(map[0] + map[1] * t).distance(foot(t)));
    }
    (worst <= LINEAR_RESOLUTION).then_some(worst)
}

/// A straight rail along a line spine: the line through the feet at the
/// ends of the range, by length from the first.
pub(crate) fn line_rail(
    foot: &dyn Fn(f64) -> Point3,
    [t0, t1]: [f64; 2],
) -> Option<(Curve, [f64; 2], f64)> {
    let (a, b) = (foot(t0), foot(t1));
    let line = Curve::Line(Line::through(a, b).ok()?);
    let speed = a.distance(b) / (t1 - t0);
    let map = [-speed * t0, speed];
    let off = checked(foot, &line, map, [t0, t1])?;
    Some((line, map, off))
}

/// A circular rail along a circle spine: coaxial with it, through the
/// foot at the middle of the range, by the same angle.
pub(crate) fn circle_rail(
    spine: &Circle,
    foot: &dyn Fn(f64) -> Point3,
    [t0, t1]: [f64; 2],
) -> Option<(Curve, [f64; 2], f64)> {
    let f = spine.frame();
    let p = foot((t0 + t1) / 2.0);
    let along = (p - f.origin()).dot(f.z().get());
    let centre = f.origin() + f.z().get() * along;
    let radius = p.distance(centre);
    let frame = Frame::new(centre, f.z().get(), f.x().get()).ok()?;
    let circle = Curve::Circle(Circle::new(frame, radius).ok()?);
    let map = [0.0, 1.0];
    let off = checked(foot, &circle, map, [t0, t1])?;
    Some((circle, map, off))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blend::convex::{Bend, bend};
    use crate::blend::spec::{Spec, resolve};
    use crate::blend::spine::spine;
    use crate::blend::tests::{block, edge_of, shaft, slanted, slot};
    use crate::blend::{BlendKind, EdgeBlend};
    use crate::name::{FaceRole, Name};
    use crate::topo::Body;

    fn setup(b: &Body, edge: Name, r: f64) -> (Spec, Spine, [Surface; 2]) {
        let spec = resolve(
            b,
            &[EdgeBlend {
                edge,
                kind: BlendKind::Fillet { radius: r },
            }],
        )
        .unwrap()
        .remove(0);
        let s = spine(b, &spec, r).unwrap();
        let surfaces = spec
            .faces
            .map(|f| b.surface(b.face(f).unwrap().surface).unwrap().clone());
        (spec, s, surfaces)
    }

    /// The rail lies on its surface, its pcurve's image on it, and each
    /// point is a radius from the spine's point at the same section.
    fn sound(s: &Spine, surface: &Surface, rail: &Rail, range: [f64; 2]) {
        for i in 0..=20 {
            let t = range[0] + (range[1] - range[0]) * i as f64 / 20.0;
            let tau = rail.param(t);
            let p = rail.curve.point(tau);
            assert!(
                surface.distance(p) <= rail.tolerance,
                "{}",
                surface.distance(p)
            );
            let (c, m) = &rail.pcurve;
            let uv = c.point(m[0] + m[1] * tau);
            assert!(surface.point(uv.x, uv.y).distance(p) <= rail.tolerance);
            let d = p.distance(s.curve.point(t));
            assert!((d - s.radius).abs() <= rail.tolerance, "{d}");
        }
    }

    #[test]
    fn a_box_edges_rails_are_lines() {
        let b = block();
        let (_, s, surfaces) = setup(&b, edge_of(&b, FaceRole::PlusX, FaceRole::PlusZ), 0.2);
        let range = [s.mid - 0.7, s.mid + 0.7];
        for (k, surface) in surfaces.iter().enumerate() {
            let r = rail(&s, surface, k, range).unwrap();
            assert!(matches!(r.curve, Curve::Line(_)));
            assert!(r.tolerance <= 2.0 * LINEAR_RESOLUTION);
            sound(&s, surface, &r, range);
        }
    }

    #[test]
    fn a_slots_arc_rails_are_circles() {
        let b = slot();
        let e = b
            .edges()
            .find(|&e| {
                bend(&b, e) == Ok(Bend::Convex)
                    && matches!(b.curve(b.edge(e).unwrap().curve), Some(Curve::Circle(_)))
            })
            .unwrap();
        let (_, s, surfaces) = setup(&b, b.edge(e).unwrap().name, 0.1);
        let range = [s.mid - 1.0, s.mid + 1.0];
        for (k, surface) in surfaces.iter().enumerate() {
            let r = rail(&s, surface, k, range).unwrap();
            assert!(matches!(r.curve, Curve::Circle(_)), "{:?}", r.curve);
            sound(&s, surface, &r, range);
        }
    }

    #[test]
    fn rails_along_fitted_spines_are_fitted() {
        for b in [slanted(), shaft()] {
            let e = b
                .edges()
                .find(|&e| {
                    !matches!(b.curve(b.edge(e).unwrap().curve), Some(Curve::Circle(_)))
                        && bend(&b, e).is_ok_and(|x| x != Bend::Smooth)
                })
                .unwrap();
            let (_, s, surfaces) = setup(&b, b.edge(e).unwrap().name, 0.1);
            for (k, surface) in surfaces.iter().enumerate() {
                let r = rail(&s, surface, k, s.range).unwrap();
                assert!(matches!(r.curve, Curve::Nurbs(_)));
                assert!(r.tolerance <= 20.0 * LINEAR_RESOLUTION, "{}", r.tolerance);
                sound(&s, surface, &r, s.range);
            }
        }
    }
}
