//! The surface a fillet lies on: the surface its ball sweeps along the
//! spine. Along a line it is a cylinder, along a circle a torus, both
//! with frames that keep the spine's parameter (and a cylinder's seam
//! opposite the middle of the section); along any other spine a
//! [`Pipe`] anchored on an analytic face, or on any face through its
//! rail's pcurve, so its `θ = 0` is the rail on that face. A closed fitted spine is made periodic first, so a fillet
//! all round a closed edge is a closed tube with no seam edge.

use super::spine::Spine;
use crate::geom::{Curve, Cylinder, Frame, Pipe, Surface, Torus};
use crate::topo::Sense;
use std::sync::Arc;

/// Why a fillet surface could not be made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SurfaceFail {
    /// The ball is too big for the spine's curvature (a torus that would
    /// not be a ring, a pipe that would fold).
    TooLarge,
    /// A closed fitted spine that can't be made periodic.
    Unsupported,
}

/// The fillet surface of `spine` over its parameters `range`, anchored
/// on `anchor` (the surface of the edge's face `k` in the spine's order:
/// on its foot if it is analytic, else on the rail, the spine's pcurve on
/// its offset) where it is a pipe, and the sense of the fillet face on it: its natural
/// normal points away from the spine, out of the body at a convex edge.
/// `feet` are the points where the ball at the spine's middle touches the
/// two faces.
pub(crate) fn fillet_surface(
    spine: &Spine,
    (anchor, k): (&Surface, usize),
    range: [f64; 2],
    feet: [crate::geom::Point3; 2],
) -> Result<(Surface, Sense), SurfaceFail> {
    let pipe = |curve: Curve| {
        let p = if anchor.frame().is_some() {
            Pipe::new(curve, spine.radius, anchor.clone())
        } else {
            let (pc, map) = &spine.pcurves[k];
            Pipe::on_rail(curve, spine.radius, anchor.clone(), pc.clone(), *map)
        };
        p.map(|p| Surface::Pipe(Arc::new(p)))
            .map_err(|_| SurfaceFail::TooLarge)
    };
    let sense = if spine.convex {
        Sense::Forward
    } else {
        Sense::Reversed
    };
    let r = spine.radius;
    let surface = match &spine.curve {
        Curve::Line(l) => {
            // The seam opposite the middle of the section.
            let c = spine.curve.point(spine.mid);
            let middle = (feet[0] - c) + (feet[1] - c);
            let frame = Frame::new(l.origin(), l.dir().get(), -middle)
                .map_err(|_| SurfaceFail::TooLarge)?;
            Surface::Cylinder(Cylinder::new(frame, r).map_err(|_| SurfaceFail::TooLarge)?)
        }
        Curve::Circle(c) => Surface::Torus(
            Torus::new(*c.frame(), c.radius(), r).map_err(|_| SurfaceFail::TooLarge)?,
        ),
        Curve::Nurbs(n) => {
            let [d0, d1] = n.domain();
            let whole = range[0] <= d0 && range[1] >= d1;
            let curve = if whole && spine.curve.is_closed() {
                n.bezier_periodic().ok_or(SurfaceFail::Unsupported)?
            } else {
                n.part([range[0].max(d0), range[1].min(d1)])
                    .map_err(|_| SurfaceFail::Unsupported)?
            };
            pipe(Curve::Nurbs(Arc::new(curve)))?
        }
        other => pipe(other.clone())?,
    };
    Ok((surface, sense))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blend::convex::{Bend, bend};
    use crate::blend::rail::rail;
    use crate::blend::spec::resolve;
    use crate::blend::spine::spine;
    use crate::blend::tests::{block, edge_of, shaft, slanted, slot};
    use crate::blend::{BlendKind, EdgeBlend};
    use crate::name::{FaceRole, Name};
    use crate::topo::Body;

    /// The fillet surface of an edge, checked to hold both rails.
    fn fillet_of(b: &Body, edge: Name, r: f64) -> (Surface, Sense) {
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
        let k = usize::from(matches!(surfaces[0], Surface::Pipe(_)));
        let feet = [0, 1].map(|j| s.foot(j, s.mid));
        let range = if s.curve.is_closed() {
            s.range
        } else {
            [s.mid - 0.3, s.mid + 0.3]
        };
        let (f, sense) = fillet_surface(&s, (&surfaces[k], k), range, feet).unwrap();
        for (j, surface) in surfaces.iter().enumerate() {
            let rl = rail(&s, surface, j, range).unwrap();
            for i in 0..=10 {
                let t = range[0] + (range[1] - range[0]) * i as f64 / 10.0;
                let p = rl.curve.point(rl.param(t));
                assert!(f.distance(p) <= rl.tolerance + 1e-9, "{}", f.distance(p));
            }
        }
        (f, sense)
    }

    #[test]
    fn a_box_edge_is_rounded_by_a_cylinder_seamed_away_from_it() {
        let b = block();
        let (f, sense) = fillet_of(&b, edge_of(&b, FaceRole::PlusX, FaceRole::PlusZ), 0.2);
        assert_eq!(sense, Sense::Forward);
        let Surface::Cylinder(c) = &f else {
            panic!("{f:?}")
        };
        let x = f.frame().unwrap().x().get();
        // The seam points away from the edge, into the body.
        assert!(x.x < -0.7 && x.z < -0.7, "{x:?}");
        assert!((c.radius() - 0.2).abs() < 1e-15);
    }

    #[test]
    fn arcs_and_rims_are_rounded_by_tori() {
        let b = slot();
        let e = b
            .edges()
            .find(|&e| {
                bend(&b, e) == Ok(Bend::Convex)
                    && matches!(b.curve(b.edge(e).unwrap().curve), Some(Curve::Circle(_)))
            })
            .unwrap();
        let (f, _) = fillet_of(&b, b.edge(e).unwrap().name, 0.1);
        assert!(matches!(f, Surface::Torus(_)));
        let u = crate::blend::tests::boss();
        let foot = u
            .edges()
            .find(|&e| bend(&u, e) == Ok(Bend::Concave))
            .unwrap();
        let (f, sense) = fillet_of(&u, u.edge(foot).unwrap().name, 0.1);
        assert!(matches!(f, Surface::Torus(_)));
        assert_eq!(sense, Sense::Reversed);
    }

    #[test]
    fn other_spines_are_rounded_by_pipes() {
        let b = slanted();
        let rim = b
            .edges()
            .find(|&e| matches!(b.curve(b.edge(e).unwrap().curve), Some(Curve::Ellipse(_))))
            .unwrap();
        let (f, sense) = fillet_of(&b, b.edge(rim).unwrap().name, 0.2);
        assert!(matches!(f, Surface::Pipe(_)));
        assert_eq!(sense, Sense::Forward);
        let b = shaft();
        let foot = b
            .edges()
            .find(|&e| bend(&b, e) == Ok(Bend::Concave))
            .unwrap();
        let (f, sense) = fillet_of(&b, b.edge(foot).unwrap().name, 0.1);
        let Surface::Pipe(p) = &f else { panic!() };
        assert!(p.spine().period().is_some());
        assert_eq!(sense, Sense::Reversed);
    }
}
