//! Drafted surfaces: the plane, cone or binormal sweep through a hinge.

use super::hinge::{HingeCurve, size_of};
use crate::geom::{
    Cone, Curve, Frame, FrameLaw, Line, Plane, Point3, Surface, Sweep, UnitVector3, Vector3,
};
use crate::intersect::{CurveHit, curve_surface_in, extended_curve, face_box};
use crate::offset::edges::NewCurve;
use crate::tol::LINEAR_RESOLUTION;
use crate::topo::{Body, EdgeId, FaceId, Sense};
use crate::work::Meter;
use crate::{Error, TweakError};
use std::sync::Arc;

/// How far from the pull, at least, a hinge's tangent and a face's
/// normal must turn (radians, as a sine) for the draft to have a
/// direction.
pub(crate) const ALONG: f64 = 0.01;

/// The draft's frame at a hinge point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Turned {
    /// The pull made square to the hinge's tangent, unit.
    pub(crate) across: Vector3,
    /// Square to the tangent and `across`, on the side the face's outward
    /// normal points.
    pub(crate) side: Vector3,
    /// The drafted outward normal: `cos θ side + sin θ across`.
    pub(crate) normal: Vector3,
    /// The ruling: `cos θ across − sin θ side`.
    pub(crate) ruling: Vector3,
}

/// The draft's frame where the hinge's tangent is `t` and the face's
/// outward normal `n`, for `pull` and the signed `angle`: `None` where
/// either runs within [`ALONG`] of the pull.
pub(crate) fn turned(t: Vector3, n: Vector3, pull: Vector3, angle: f64) -> Option<Turned> {
    let t = t.normalize()?.get();
    let n = n.normalize()?.get();
    let pull = pull.normalize()?.get();
    let dp = pull - t * pull.dot(t);
    if dp.length() < ALONG || n.cross(pull).length() < ALONG {
        return None;
    }
    let across = dp.normalize()?.get();
    let e1 = pull.cross(t).normalize()?.get();
    let k = n.dot(e1);
    if k.abs() < ALONG {
        return None;
    }
    let side = if k > 0.0 { e1 } else { -e1 };
    let (s, c) = crate::math::sin_cos(angle);
    Some(Turned {
        across,
        side,
        normal: side * c + across * s,
        ruling: across * c - side * s,
    })
}

/// Face `f`'s outward normal at its foot of `p`.
pub(crate) fn outward_at(body: &Body, f: FaceId, p: Point3) -> Option<Vector3> {
    let face = &body.faces[f];
    let s = &body.surfaces[face.surface];
    let (u, v) = s.project(p).param;
    let sign = if face.sense == Sense::Forward {
        1.0
    } else {
        -1.0
    };
    s.normal(u, v).map(|n| n.get() * sign)
}

/// Samples along a hinge at which its direction is judged.
const SAMPLES: usize = 17;

/// Face `f`'s drafted surface about `hinge` at the signed `angle` from
/// `pull`, and its sense on it (its outward side out): a plane through a
/// line hinge, a cone through a circle about an axis along the pull, else
/// a binormal sweep along the hinge. `HingeAlongPull` where the hinge or
/// the face's normal runs along the pull at any of 17 samples;
/// `FaceCollapses` where the surface can't be made.
pub(crate) fn drafted(
    body: &Body,
    f: FaceId,
    hinge: &HingeCurve,
    pull: UnitVector3,
    angle: f64,
    meter: &Meter,
) -> Result<(Surface, Sense), Error> {
    let at = body.faces[f].name;
    let along = || -> Error { TweakError::HingeAlongPull { face: at }.into() };
    let collapses = || -> Error { TweakError::FaceCollapses { face: at }.into() };
    let [s0, s1] = hinge.range;
    let mut mid = None;
    for i in 0..SAMPLES {
        let s = s0 + (s1 - s0) * i as f64 / (SAMPLES - 1) as f64;
        let p = hinge.curve.point(s);
        let t = hinge.curve.tangent(s).ok_or_else(along)?.get();
        let n = outward_at(body, f, p).ok_or_else(along)?;
        let d = turned(t, n, pull.get(), angle).ok_or_else(along)?;
        if i == SAMPLES / 2 {
            mid = Some((p, t, n, d));
        }
    }
    let (p, t, n, d) = mid.expect("a middle sample");
    match &hinge.curve {
        Curve::Line(l) => {
            let frame = Frame::new(l.origin(), d.normal, l.dir().get()).map_err(|_| collapses())?;
            Ok((Surface::Plane(Plane::new(frame)), Sense::Forward))
        }
        Curve::Circle(k) if k.frame().z().get().cross(pull.get()).length() < 1e-12 => {
            let c = k.frame().origin();
            let out = n.dot(p - c) > 0.0;
            let narrows = out == (angle > 0.0);
            let z = if narrows { -pull.get() } else { pull.get() };
            let frame = Frame::new(c, z, k.frame().x().get()).map_err(|_| collapses())?;
            let cone = Cone::new(frame, k.radius(), angle.abs()).map_err(|_| collapses())?;
            let sense = if out { Sense::Forward } else { Sense::Reversed };
            Ok((Surface::Cone(cone), sense))
        }
        _ => swept(body, f, hinge, pull, angle, (p, t, d), meter),
    }
}

/// How far past its ends a hinge path is carried, and how far the
/// profile reaches each way, as shares of the face's diagonal.
const PATH_MARGIN: f64 = 0.5;
const REACH: f64 = 2.0;

/// The binormal sweep of the ruling along `hinge`: `mid` is the middle
/// sample's point, tangent and draft frame, which fix the side `σ` (the
/// same all along: `turned` found the face on one side at every sample)
/// and the sense.
fn swept(
    body: &Body,
    f: FaceId,
    hinge: &HingeCurve,
    pull: UnitVector3,
    angle: f64,
    (p, t, d): (Point3, Vector3, Turned),
    _meter: &Meter,
) -> Result<(Surface, Sense), Error> {
    let at = body.faces[f].name;
    let collapses = || -> Error { TweakError::FaceCollapses { face: at }.into() };
    let size = size_of(&face_box(body, f));
    let [r0, r1] = hinge.range;
    // A whole period, or a clamped curve whose ends meet: carried no
    // further, which would fold the sweep back over itself.
    let closed = hinge
        .curve
        .period()
        .is_some_and(|q| (r1 - r0 - q).abs() <= 1e-12 * q.max(1.0))
        || hinge.curve.point(r0).distance(hinge.curve.point(r1)) <= LINEAR_RESOLUTION;
    let (path, range) = if closed {
        (hinge.curve.clone(), hinge.range)
    } else {
        let m = hinge.range.map(|s| {
            let v = hinge.curve.eval(s).d1.length();
            if v > 0.0 { PATH_MARGIN * size / v } else { 0.0 }
        });
        extended_curve(&hinge.curve, hinge.range, m).unwrap_or((hinge.curve.clone(), hinge.range))
    };
    let pv = pull.get();
    let sigma = d.side.dot(pv.cross(t)).signum();
    let s0 = range[0];
    let h0 = path.point(s0);
    let t0 = path.tangent(s0).ok_or_else(collapses)?.get();
    let e1 = pv.cross(t0).normalize().ok_or_else(collapses)?.get();
    let e2 = t0.cross(e1);
    let (sn, cs) = crate::math::sin_cos(angle);
    let r = (e2 * cs - e1 * (sigma * sn))
        .normalize()
        .ok_or_else(collapses)?;
    // How far along the ruling the face reaches from the hinge: the
    // farthest corner of its box along the pull, over the ruling's
    // share of it, and a tenth more.
    let bx = face_box(body, f);
    let need = bx
        .corners()
        .iter()
        .map(|c| (*c - h0).dot(pv).abs())
        .fold(0.0, f64::max)
        / r.get().dot(pv).abs().max(ALONG)
        * 1.1
        + 10.0 * LINEAR_RESOLUTION;
    // A long profile along a tightly curved path can't always be
    // certified regular (its enclosures widen with it): halve it, but
    // never below what the face needs.
    let mut reach = REACH * size;
    let profile = Curve::Line(Line::from_unit(h0, r).map_err(|_| collapses())?);
    let start = Frame::new(h0, t0, e1).map_err(|_| collapses())?;
    let sweep = loop {
        match Sweep::new(
            (profile.clone(), [-reach, reach]),
            (path.clone(), range),
            start,
            FrameLaw::Binormal(pull),
            0.0,
            1.0,
        ) {
            Ok(sw) => break sw,
            Err(_) if reach / 2.0 >= need => reach /= 2.0,
            Err(_) => return Err(collapses()),
        }
    };
    let s = Surface::Sweep(Arc::new(sweep));
    let (u, v) = s.project(p).param;
    let n = s.normal(u, v).ok_or_else(collapses)?.get();
    let sense = if n.dot(d.normal) > 0.0 {
        Sense::Forward
    } else {
        Sense::Reversed
    };
    Ok((s, sense))
}

/// A tangent edge between faces drafted alike by `d`: the ruling through
/// where it crosses their hinge, a line running the way the edge ran,
/// reaching past both old ends by half the edge's length. `TangencyBroken`
/// where the edge doesn't cross the hinge.
pub(crate) fn ruling_edge(
    body: &Body,
    e: EdgeId,
    d: &super::Drafted,
    meter: &Meter,
) -> Result<NewCurve, Error> {
    let edge = &body.edges[e];
    let broken = || -> Error { TweakError::TangencyBroken { edge: edge.name }.into() };
    let curve = &body.curves[edge.curve];
    let x = match &d.on {
        super::HingeOn::Plane(plane) => {
            let hits =
                curve_surface_in(curve, edge.interval, plane, 10.0 * LINEAR_RESOLUTION, meter)
                    .map_err(|_| TweakError::Uncertain { at: edge.name })?;
            hits.iter()
                .find_map(|h| match h {
                    CurveHit::Point { point, .. } => Some(*point),
                    CurveHit::On { .. } => None,
                })
                .ok_or_else(broken)?
        }
        super::HingeOn::Edges(on) => {
            let ends = |h: EdgeId| [body.edges[h].start, body.edges[h].end];
            [edge.start, edge.end]
                .into_iter()
                .find(|v| on.iter().any(|&h| ends(h).contains(v)))
                .map(|v| body.vertices[v].point)
                .ok_or_else(broken)?
        }
    };
    let f = crate::tweak::classify::face_pair(body, e)[0];
    let n = outward_at(body, f, x).ok_or_else(broken)?;
    let hs = d.hinge.curve.project(x).param;
    let t = d.hinge.curve.tangent(hs).ok_or_else(broken)?.get();
    let turn = turned(t, n, d.pull.get(), d.angle).ok_or_else(broken)?;
    let [p0, p1] = edge.interval.map(|s| curve.point(s));
    let r = if (p1 - p0).dot(turn.ruling) < 0.0 {
        -turn.ruling
    } else {
        turn.ruling
    };
    let dir = r.normalize().ok_or_else(broken)?;
    let line = Line::from_unit(x, dir).map_err(|_| broken())?;
    let along = |p: Point3| (p - x).dot(dir.get());
    let half = 0.5 * p0.distance(p1) + 10.0 * LINEAR_RESOLUTION;
    let [a, b] = [along(p0), along(p1)];
    Ok(NewCurve {
        curve: Curve::Line(line),
        range: [a.min(b) - half, a.max(b) + half],
        period: None,
        tolerance: edge.tolerance.get(),
        pcurves: [None, None],
        reversed: false,
    })
}

#[cfg(test)]
mod tests {
    use super::super::hinge::neutral;
    use super::*;
    use crate::geom::Rigid;
    use crate::name::{FaceRole, Name, OpKey, Role};
    use crate::primitive;

    const K: OpKey = OpKey(12);

    fn plane_at(z: f64) -> Surface {
        Surface::Plane(Plane::new(
            Frame::new(
                Point3::new(0.0, 0.0, z),
                Vector3::new(0.0, 0.0, 1.0),
                Vector3::new(1.0, 0.0, 0.0),
            )
            .unwrap(),
        ))
    }

    fn up() -> UnitVector3 {
        UnitVector3::exact(Vector3::new(0.0, 0.0, 1.0)).unwrap()
    }

    fn side_of(b: &Body) -> FaceId {
        b.faces()
            .find(|&f| matches!(b.surfaces[b.faces[f].surface], Surface::Cylinder(_)))
            .unwrap()
    }

    #[test]
    fn a_wall_drafts_to_a_plane_through_its_hinge() {
        // unmetered: a unit test.
        let m = Meter::free();
        let (b, _) = primitive::block(K, 1.0, 0.6, 0.4, &Rigid::IDENTITY).unwrap();
        let n = Name::derive(K, Role::Face(FaceRole::PlusX), &[]);
        let wall = b.faces().find(|&f| b.faces[f].name == n).unwrap();
        let h = neutral(&b, wall, &plane_at(0.0), &m).unwrap();
        let th = 0.05;
        let (s, sense) = drafted(&b, wall, &h, up(), th, &m).unwrap();
        assert_eq!(sense, Sense::Forward);
        let Surface::Plane(p) = &s else {
            panic!("{s:?}")
        };
        assert!((p.frame().z().get().z - crate::math::sin(th)).abs() < 1e-15);
        // At the top the wall has moved in by 0.4 tan θ (along its
        // normal, so the foot's x differs by its cosine's share).
        let foot = s.project(Point3::new(1.0, 0.3, 0.4)).point;
        assert!((foot.x - (1.0 - 0.4 * crate::math::tan(th))).abs() < 1e-3);
    }

    #[test]
    fn a_boss_drafts_to_a_cone_narrowing_along_the_pull_and_a_hole_to_one_opening() {
        // unmetered: a unit test.
        let m = Meter::free();
        let (c, _) = primitive::cylinder(K, 0.1, 0.4, &Rigid::IDENTITY).unwrap();
        let side = side_of(&c);
        let h = neutral(&c, side, &plane_at(0.0), &m).unwrap();
        let (s, sense) = drafted(&c, side, &h, up(), 0.1, &m).unwrap();
        assert_eq!(sense, Sense::Forward);
        let Surface::Cone(k) = &s else {
            panic!("{s:?}")
        };
        assert!((k.half_angle() - 0.1).abs() < 1e-15);
        assert!((k.radius() - 0.1).abs() < 1e-15);
        let p = s.project(Point3::new(0.1, 0.0, 0.2)).point;
        assert!((crate::math::hypot(p.x, p.y) - (0.1 - 0.2 * crate::math::tan(0.1))).abs() < 2e-3);
        // A hole: the same cylinder as a face whose outward side faces the
        // axis (its sense reversed) opens along the pull.
        let mut hole = c.clone();
        hole.faces[side].sense = Sense::Reversed;
        let (s, sense) = drafted(&hole, side, &h, up(), 0.1, &m).unwrap();
        assert_eq!(sense, Sense::Reversed);
        assert!(matches!(s, Surface::Cone(_)), "{s:?}");
        assert!(
            s.frame()
                .unwrap()
                .z()
                .get()
                .dot(Vector3::new(0.0, 0.0, 1.0))
                > 0.999
        );
    }

    #[test]
    fn a_top_has_no_draft() {
        // unmetered: a unit test.
        let m = Meter::free();
        let (b, _) = primitive::block(K, 1.0, 0.6, 0.4, &Rigid::IDENTITY).unwrap();
        let n = Name::derive(K, Role::Face(FaceRole::PlusZ), &[]);
        let top = b.faces().find(|&f| b.faces[f].name == n).unwrap();
        // The top's edge as a hinge: the top's normal is the pull.
        let e = b
            .loop_fins(b.face_loops(top).next().unwrap())
            .next()
            .unwrap();
        let edge = &b.edges[b.fins[e].edge];
        let h = HingeCurve {
            curve: b.curves[edge.curve].clone(),
            range: edge.interval,
        };
        assert!(matches!(
            drafted(&b, top, &h, up(), 0.1, &m),
            Err(Error::InvalidTweak(TweakError::HingeAlongPull { .. }))
        ));
    }

    #[test]
    fn a_walls_draft_tilts_its_normal_to_the_pull_and_rules_in_its_plane() {
        let pull = Vector3::new(0.0, 0.0, 1.0);
        let n = Vector3::new(1.0, 0.0, 0.0);
        let th = 3.0_f64.to_radians();
        let (s, c) = crate::math::sin_cos(th);
        for t in [Vector3::new(0.0, 1.0, 0.0), Vector3::new(0.0, -1.0, 0.0)] {
            let d = turned(t, n, pull, th).unwrap();
            assert!((d.normal - Vector3::new(c, 0.0, s)).length() < 1e-15);
            assert!((d.ruling - Vector3::new(-s, 0.0, c)).length() < 1e-15);
            assert!(d.normal.dot(d.ruling).abs() < 1e-15);
            assert!(d.ruling.dot(t).abs() < 1e-15);
        }
        // A hole's wall (normal -x) opens along the pull.
        let d = turned(Vector3::new(0.0, 1.0, 0.0), -n, pull, th).unwrap();
        assert!((d.ruling - Vector3::new(s, 0.0, c)).length() < 1e-15);
        // A face already at 2° drafts to 3°, not 5°.
        let (s2, c2) = crate::math::sin_cos(2.0_f64.to_radians());
        let d = turned(
            Vector3::new(0.0, 1.0, 0.0),
            Vector3::new(c2, 0.0, s2),
            pull,
            th,
        )
        .unwrap();
        assert!((d.normal.dot(pull) - s).abs() < 1e-15);
    }

    #[test]
    fn a_hinge_or_a_normal_along_the_pull_has_no_draft() {
        let pull = Vector3::new(0.0, 0.0, 1.0);
        assert!(turned(pull, Vector3::new(1.0, 0.0, 0.0), pull, 0.05).is_none());
        assert!(turned(Vector3::new(1.0, 0.0, 0.0), pull, pull, 0.05).is_none());
    }

    #[test]
    fn a_circle_hinge_as_a_spline_sweeps_to_the_same_cone() {
        // unmetered: a unit test.
        let m = Meter::free();
        let (c, _) = primitive::cylinder(K, 0.1, 0.4, &Rigid::IDENTITY).unwrap();
        let side = side_of(&c);
        let h = neutral(&c, side, &plane_at(0.2), &m).unwrap();
        let (cone, _) = drafted(&c, side, &h, up(), 0.1, &m).unwrap();
        let (nurbs, _) = h.curve.to_nurbs(h.range).unwrap();
        let [n0, n1] = nurbs.domain();
        let spline = HingeCurve {
            curve: Curve::Nurbs(Arc::new(nurbs)),
            range: [n0, n1],
        };
        let (s, sense) = drafted(&c, side, &spline, up(), 0.1, &m).unwrap();
        assert!(matches!(s, Surface::Sweep(_)), "{s:?}");
        let sign = if sense == Sense::Forward { 1.0 } else { -1.0 };
        let [[u0, u1], [v0, v1]] = s.domain();
        for i in 0..=8 {
            for j in 0..=8 {
                let (u, v) = (
                    u0 + (u1 - u0) * (0.4 + 0.2 * f64::from(i) / 8.0),
                    v0 + (v1 - v0) * f64::from(j) / 8.0,
                );
                let p = s.eval(u, v).point;
                let on = cone.project(p).point;
                assert!(p.distance(on) < 1e-7, "{p:?}");
                let n = s.normal(u, v).unwrap().get() * sign;
                assert!((crate::math::asin(n.z) - 0.1).abs() < 1e-9, "{n:?}");
            }
        }
    }

    #[test]
    fn a_rounded_corners_tangent_edge_becomes_the_ruling_through_its_hinge() {
        // unmetered: a unit test.
        let m = Meter::free();
        let (bx, _) = primitive::block(K, 1.0, 0.6, 0.4, &Rigid::IDENTITY).unwrap();
        let mut f = [
            Name::derive(K, Role::Face(FaceRole::PlusX), &[]),
            Name::derive(K, Role::Face(FaceRole::PlusY), &[]),
        ];
        f.sort();
        let corner = Name::derive(K, Role::Edge, &f);
        let (b, _) = crate::blend::blend(
            OpKey(13),
            &bx,
            &[crate::blend::EdgeBlend {
                edge: corner,
                kind: crate::blend::BlendKind::Fillet { radius: 0.1 },
            }],
        )
        .unwrap();
        // A vertical tangent edge between a wall and the fillet.
        let wall = b
            .faces()
            .find(|&g| b.faces[g].name == f[0] || b.faces[g].name == f[1])
            .unwrap();
        let e = b
            .face_loops(wall)
            .flat_map(|l| b.loop_fins(l).map(|x| b.fins[x].edge).collect::<Vec<_>>())
            .find(|&e| {
                let [t0, t1] = b.edges[e].interval;
                let c = &b.curves[b.edges[e].curve];
                (c.point(t0).z - c.point(t1).z).abs() > 0.3
                    && crate::tweak::classify::tangent(&b, e).unwrap_or(false)
            })
            .unwrap();
        let g = crate::tweak::classify::face_pair(&b, e)[0];
        let plane = plane_at(0.0);
        let h = neutral(&b, g, &plane, &m).unwrap();
        let d = super::super::Drafted {
            hinge: h,
            on: super::super::HingeOn::Plane(plane),
            pull: up(),
            angle: 0.05,
        };
        let c = ruling_edge(&b, e, &d, &m).unwrap();
        let Curve::Line(l) = &c.curve else { panic!() };
        // Through the edge's foot on z = 0, tilted 0.05 rad from z.
        assert!(l.origin().z.abs() < 1e-12);
        assert!((crate::math::acos(l.dir().get().z.abs()) - 0.05).abs() < 1e-12);
    }
}
