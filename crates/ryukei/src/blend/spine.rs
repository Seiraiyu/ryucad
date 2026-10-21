//! Spines: where the centre of a fillet's ball runs.
//!
//! A ball of radius `r` touching both faces of an edge has its centre on
//! both faces' offsets by `r` towards the material (inside the body at a
//! convex edge, outside at a concave one). Offsets keep their surfaces'
//! parameters, so the spine's pcurve on each offset is also where the
//! ball touches that face: the rail. The spine is one branch of the
//! offsets' intersection, the one through the centre of the ball that
//! touches both faces in the plane normal to the edge at its middle,
//! turned to run the way the edge does.

use super::convex::{Bend, BendFail, bend};
use super::spec::Spec;
use crate::geom::{Box3, Curve, Curve2, Point3, Surface, Vector3};
use crate::intersect::{self, Failure, SurfaceHit};
use crate::tol::LINEAR_RESOLUTION;
use crate::topo::{Body, FaceId, Sense};
use crate::{BlendError, Error};

/// A fillet's spine.
#[derive(Clone, Debug)]
pub(crate) struct Spine {
    /// The ball's centres, running the way the edge does over `range`.
    pub(crate) curve: Curve,
    pub(crate) range: [f64; 2],
    /// The spine on the offset of each of the edge's faces, in the order
    /// of [`Spec::faces`]: a pcurve and the affine map from the spine's
    /// parameter to its own. The offsets keep their faces' parameters, so
    /// these are the rails' pcurves on the faces.
    pub(crate) pcurves: [(Curve2, [f64; 2]); 2],
    /// How far the spine and its pcurves' images may stray from the
    /// offsets.
    pub(crate) tolerance: f64,
    /// The faces the spine was made from, and their offsets.
    pub(crate) faces: [FaceId; 2],
    pub(crate) offsets: [Surface; 2],
    pub(crate) convex: bool,
    pub(crate) radius: f64,
    /// The spine's parameter at the ball that touches the edge's middle.
    pub(crate) mid: f64,
}

/// The spine of a fillet of `radius` on the resolved edge. Fails with
/// `SmoothEdge`, `ConvexityChanges` or `Uncertain` where the edge's bend
/// can't be given, `RadiusTooLarge` where an offset collapses or no
/// branch passes the ball at the edge's middle, `Unsupported` where the
/// offsets can't be intersected, and `Uncertain` where their
/// intersection isn't certified.
pub(crate) fn spine(body: &Body, spec: &Spec, radius: f64) -> Result<Spine, Error> {
    let at = spec.name;
    let convex = match bend(body, spec.edge) {
        Ok(Bend::Convex) => true,
        Ok(Bend::Concave) => false,
        Ok(Bend::Smooth) => return Err(BlendError::SmoothEdge { edge: at }.into()),
        Err(BendFail::Changes) => return Err(BlendError::ConvexityChanges { edge: at }.into()),
        Err(BendFail::Uncertain) => return Err(BlendError::Uncertain { at }.into()),
    };
    let too_large = || -> Error { BlendError::RadiusTooLarge { edge: at }.into() };
    let mut offsets = Vec::with_capacity(2);
    for &face in &spec.faces {
        let f = &body.faces[face];
        let s = &body.surfaces[f.surface];
        // Towards the material: against the outward normal at a convex
        // edge, along it at a concave one.
        let outward = if f.sense == Sense::Forward { 1.0 } else { -1.0 };
        let d = if convex { -radius } else { radius } * outward;
        let off = s
            .offset(d)
            .ok_or(BlendError::Unsupported { at: f.name })?
            .map_err(|_| too_large())?;
        offsets.push(off);
    }
    let offsets: [Surface; 2] = [offsets[0].clone(), offsets[1].clone()];
    let centre = mid_centre(body, spec, &offsets, radius, convex).ok_or_else(too_large)?;
    let region = region(body, spec, radius);
    let hit = match (&offsets[0], &offsets[1]) {
        (Surface::Pipe(_), _) | (_, Surface::Pipe(_)) => {
            intersect::pipe_surface(&offsets[0], &offsets[1], &region)
        }
        _ => intersect::surface_surface(&offsets[0], &offsets[1], &region),
    };
    let branches = match hit {
        Ok(SurfaceHit::Meet { branches, .. }) => branches,
        Ok(SurfaceHit::Coincident { .. }) => return Err(BlendError::Unsupported { at }.into()),
        Err(Failure::Unsupported) => return Err(BlendError::Unsupported { at }.into()),
        Err(Failure::Uncertain) => return Err(BlendError::Uncertain { at }.into()),
    };
    // The branch through the ball's centre: within ten of its tolerances.
    let mut best: Option<(f64, usize, f64)> = None;
    let mut near = 0;
    for (k, b) in branches.iter().enumerate() {
        let (t, d) = nearest(&b.curve, b.range, centre);
        if d <= 10.0 * b.tolerance.max(LINEAR_RESOLUTION) {
            near += 1;
            if best.is_none_or(|x| d < x.0) {
                best = Some((d, k, t));
            }
        }
    }
    let Some((_, k, mid)) = best else {
        return Err(too_large());
    };
    if near > 1 {
        return Err(BlendError::Uncertain { at }.into());
    }
    let b = branches.into_iter().nth(k).expect("a chosen branch");
    // A closed branch fitted as a clamped spline starts opposite the ball
    // at the edge's middle, so the blend takes one stretch of it.
    let [r0, r1] = b.range;
    let b = super::seam::reseamed(b, r0 + (mid - r0 + (r1 - r0) / 2.0).rem_euclid(r1 - r0));
    let mid = if mid < b.range[0] {
        mid + (r1 - r0)
    } else {
        mid
    };
    let e = &body.edges[spec.edge];
    let [t0, t1] = e.interval;
    let along = body.curves[e.curve].eval((t0 + t1) / 2.0).d1;
    let mut s = Spine {
        curve: b.curve,
        range: b.range,
        pcurves: b.pcurves,
        tolerance: b.tolerance.max(LINEAR_RESOLUTION),
        faces: spec.faces,
        offsets,
        convex,
        radius,
        mid,
    };
    if s.curve.eval(mid).d1.dot(along) < 0.0 {
        s = s.reversed();
    }
    Ok(s)
}

impl Spine {
    /// The same spine run backwards.
    pub(crate) fn reversed(self) -> Self {
        let (curve, c) = self.curve.reversed();
        let flip = |(p, m): (Curve2, [f64; 2])| (p, [m[0] + m[1] * c, -m[1]]);
        let [a, b] = self.pcurves;
        Self {
            curve,
            range: [c - self.range[1], c - self.range[0]],
            pcurves: [flip(a), flip(b)],
            mid: c - self.mid,
            ..self
        }
    }

    /// The point where the ball centred at the spine's parameter `t`
    /// touches face `k` (in the order of [`Spec::faces`]), by the face's
    /// surface at the spine's pcurve on its offset.
    pub(crate) fn foot(&self, surfaces: [&Surface; 2], k: usize, t: f64) -> Point3 {
        let (c, m) = &self.pcurves[k];
        let uv = c.point(m[0] + m[1] * t);
        surfaces[k].point(uv.x, uv.y)
    }
}

/// Whether the ball touching both faces of another resolved edge at its
/// middle is on `spine`: true for edges joined tangentially between the
/// same two faces, whose offsets meet in the same branch.
pub(crate) fn ball_on(body: &Body, spec: &Spec, spine: &Spine) -> bool {
    let Some(offsets) = spine_faces(spine, spec) else {
        return false;
    };
    let Some(c) = mid_centre(body, spec, &offsets, spine.radius, spine.convex) else {
        return false;
    };
    let (_, d) = nearest(&spine.curve, spine.range, c);
    d <= 10.0 * spine.tolerance
}

/// The spine's offsets in the order of `spec`'s faces, when it has the
/// same two.
fn spine_faces(spine: &Spine, spec: &Spec) -> Option<[Surface; 2]> {
    let pick = |f: FaceId| {
        let k = spine.faces.iter().position(|&g| g == f)?;
        Some(spine.offsets[k].clone())
    };
    Some([pick(spec.faces[0])?, pick(spec.faces[1])?])
}

/// The parameter in `range` of the point of `curve` nearest `p`, and its
/// distance.
pub(crate) fn nearest(curve: &Curve, range: [f64; 2], p: Point3) -> (f64, f64) {
    let t = curve.project(p).param;
    let t = match curve.period() {
        Some(period) if !(range[0]..=range[1]).contains(&t) => {
            let k = ((range[0] - t) / period).ceil();
            t + k * period
        }
        _ => t,
    }
    .clamp(range[0], range[1]);
    (t, curve.point(t).distance(p))
}

/// Where the offsets are intersected: both faces' boxes, grown by twice
/// the radius, which holds every ball touching both faces.
fn region(body: &Body, spec: &Spec, radius: f64) -> Box3 {
    let [a, b] = spec.faces.map(|f| intersect::face_box(body, f));
    a.union(&b).grow(2.0 * radius)
}

/// The centre of the ball of `radius` that touches both faces in the plane
/// normal to the edge at its middle: Newton's method on both offsets and
/// that plane, from the centre two flat faces with the edge's normals
/// would give. `None` if it does not settle.
fn mid_centre(
    body: &Body,
    spec: &Spec,
    offsets: &[Surface; 2],
    radius: f64,
    convex: bool,
) -> Option<Point3> {
    let e = &body.edges[spec.edge];
    let [t0, t1] = e.interval;
    let tm = (t0 + t1) / 2.0;
    let ev = body.curves[e.curve].eval(tm);
    let (p, along) = (ev.point, ev.d1.normalize()?.get());
    let normal = |k: usize| -> Option<Vector3> {
        let face = &body.faces[spec.faces[k]];
        let uv = body.fin_uv(spec.fins[k], tm)?;
        let n = body.surfaces[face.surface].normal(uv.x, uv.y)?.get();
        Some(if face.sense == Sense::Forward { n } else { -n })
    };
    let (na, nb) = (normal(0)?, normal(1)?);
    let toward = if convex { -1.0 } else { 1.0 };
    let mut c = p + (na + nb) * (toward * radius / (1.0 + na.dot(nb)).max(1e-3));
    let scale = radius.max(p.to_vector().length()).max(1.0);
    for _ in 0..50 {
        // Each offset's tangent plane at the foot of c, and the edge's
        // normal plane at p.
        let mut rows = [Vector3::ZERO; 3];
        let mut rhs = [0.0; 3];
        for (k, off) in offsets.iter().enumerate() {
            let q = off.project(c);
            let (u, v) = q.param;
            let m = off.normal(u, v)?.get();
            rows[k] = m;
            rhs[k] = m.dot(q.point.to_vector());
        }
        rows[2] = along;
        rhs[2] = along.dot(p.to_vector());
        let next = solve3(rows, rhs)?;
        let step = next.distance(c);
        c = next;
        if step <= 1e-15 * scale {
            break;
        }
    }
    let ok = offsets.iter().all(|o| o.distance(c) <= LINEAR_RESOLUTION);
    ok.then_some(c)
}

/// The point `x` with `rows[k] · x = rhs[k]`, by Cramer's rule; `None`
/// when the rows are nearly dependent.
pub(crate) fn solve3(rows: [Vector3; 3], rhs: [f64; 3]) -> Option<Point3> {
    let [a, b, c] = rows;
    let det = a.dot(b.cross(c));
    if det.abs() <= 1e-12 {
        return None;
    }
    let x = (b.cross(c) * rhs[0] + c.cross(a) * rhs[1] + a.cross(b) * rhs[2]) / det;
    Some(Point3::ORIGIN + x)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blend::spec::resolve;
    use crate::blend::tests::{block, edge_of, slot};
    use crate::blend::{BlendKind, EdgeBlend};
    use crate::geom::Rigid;
    use crate::name::{FaceRole, Name, OpKey};

    fn spine_of(b: &Body, edge: Name, r: f64) -> Result<Spine, Error> {
        let specs = resolve(
            b,
            &[EdgeBlend {
                edge,
                kind: BlendKind::Fillet { radius: r },
            }],
        )?;
        spine(b, &specs[0], r)
    }

    /// The spine's distance from both offsets at a few parameters.
    fn on_offsets(s: &Spine) {
        for i in 0..=8 {
            let t = s.range[0] + (s.range[1] - s.range[0]) * i as f64 / 8.0;
            let p = s.curve.point(t);
            for o in &s.offsets {
                assert!(o.distance(p) <= s.tolerance + 1e-12, "{}", o.distance(p));
            }
        }
    }

    #[test]
    fn a_box_edges_spine_is_a_line_inside() {
        let b = block();
        let e = edge_of(&b, FaceRole::PlusX, FaceRole::PlusZ);
        let s = spine_of(&b, e, 0.2).unwrap();
        assert!(s.convex);
        let Curve::Line(_) = s.curve else {
            panic!("{:?}", s.curve)
        };
        let c = s.curve.point(s.mid);
        assert!((c.x - 0.8).abs() < 1e-12 && (c.z - 0.8).abs() < 1e-12);
        assert!((c.y - 0.5).abs() < 1e-12);
        on_offsets(&s);
        // It runs the way the edge does.
        let n = crate::blend::tests::edge_id(&b, e);
        let ed = b.edge(n).unwrap();
        let d = b.curve(ed.curve).unwrap().eval(ed.interval[0]).d1;
        assert!(s.curve.eval(s.mid).d1.dot(d) > 0.0);
        // The feet are on the faces, a radius from the centre.
        let surfaces = s_faces(&b, &resolve(&b, &[fillet(e, 0.2)]).unwrap()[0]);
        for k in 0..2 {
            let f = s.foot(surfaces, k, s.mid);
            assert!((f.distance(c) - 0.2).abs() < 1e-12);
        }
    }

    fn fillet(edge: Name, radius: f64) -> EdgeBlend {
        EdgeBlend {
            edge,
            kind: BlendKind::Fillet { radius },
        }
    }

    fn s_faces<'a>(b: &'a Body, spec: &Spec) -> [&'a Surface; 2] {
        spec.faces
            .map(|f| b.surface(b.face(f).unwrap().surface).unwrap())
    }

    #[test]
    fn a_rims_spine_is_a_circle_and_a_concave_foots_is_outside() {
        let (cyl, _) = crate::primitive::cylinder(OpKey(1), 1.0, 1.0, &Rigid::IDENTITY).unwrap();
        let rim = cyl.edges().next().unwrap();
        let s = spine_of(&cyl, cyl.edge(rim).unwrap().name, 0.25).unwrap();
        let Curve::Circle(c) = &s.curve else {
            panic!("{:?}", s.curve)
        };
        assert!((c.radius() - 0.75).abs() < 1e-12);
        on_offsets(&s);
        let u = crate::blend::tests::boss();
        let foot = u
            .edges()
            .find(|&e| bend(&u, e) == Ok(Bend::Concave))
            .unwrap();
        let s = spine_of(&u, u.edge(foot).unwrap().name, 0.1).unwrap();
        assert!(!s.convex);
        let Curve::Circle(c) = &s.curve else {
            panic!("{:?}", s.curve)
        };
        // The boss has radius 1 and stands on the plate at z = 1.
        assert!((c.radius() - 1.1).abs() < 1e-12);
        assert!((c.frame().origin().z - 1.1).abs() < 1e-12);
    }

    #[test]
    fn a_slots_arc_spine_is_a_circle_and_its_line_spine_a_line() {
        let b = slot();
        let mut lines = 0;
        let mut circles = 0;
        for e in b.edges() {
            if bend(&b, e) != Ok(Bend::Convex) {
                continue;
            }
            let s = spine_of(&b, b.edge(e).unwrap().name, 0.1).unwrap();
            on_offsets(&s);
            match s.curve {
                Curve::Line(_) => lines += 1,
                Curve::Circle(_) => circles += 1,
                _ => panic!(),
            }
        }
        assert_eq!((lines, circles), (4, 4));
    }

    #[test]
    fn an_oblique_rims_spine_is_an_ellipse() {
        let b = crate::blend::tests::slanted();
        let rim = b
            .edges()
            .find(|&e| matches!(b.curve(b.edge(e).unwrap().curve), Some(Curve::Ellipse(_))))
            .unwrap();
        let s = spine_of(&b, b.edge(rim).unwrap().name, 0.2).unwrap();
        assert!(s.convex);
        assert!(matches!(s.curve, Curve::Ellipse(_)), "{:?}", s.curve);
        on_offsets(&s);
    }

    #[test]
    fn a_boss_on_a_shafts_spine_is_traced() {
        let b = crate::blend::tests::shaft();
        let foot = b
            .edges()
            .find(|&e| bend(&b, e) == Ok(Bend::Concave))
            .unwrap();
        let s = spine_of(&b, b.edge(foot).unwrap().name, 0.1).unwrap();
        assert!(!s.convex);
        assert!(matches!(s.curve, Curve::Nurbs(_)), "{:?}", s.curve);
        assert!(s.curve.is_closed());
        on_offsets(&s);
    }

    #[test]
    fn refusals() {
        let b = block();
        let e = edge_of(&b, FaceRole::PlusX, FaceRole::PlusZ);
        // The faces are a unit apart: a ball of radius 2 still fits
        // between the offsets' intersection, which is a line far inside.
        assert!(spine_of(&b, e, 2.0).is_ok());
        let (cyl, _) = crate::primitive::cylinder(OpKey(1), 1.0, 1.0, &Rigid::IDENTITY).unwrap();
        let rim = cyl.edges().next().unwrap();
        // A ball bigger than the cylinder: the offset collapses.
        assert_eq!(
            spine_of(&cyl, cyl.edge(rim).unwrap().name, 1.5).unwrap_err(),
            BlendError::RadiusTooLarge {
                edge: cyl.edge(rim).unwrap().name
            }
            .into()
        );
        let s = slot();
        let lateral = s
            .edges()
            .find(|&x| bend(&s, x) == Ok(Bend::Smooth))
            .unwrap();
        let n = s.edge(lateral).unwrap().name;
        assert_eq!(
            spine_of(&s, n, 0.1).unwrap_err(),
            BlendError::SmoothEdge { edge: n }.into()
        );
    }
}
