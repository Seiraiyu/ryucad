//! What each edge of a tweaked body becomes: one curve, by its class.

use super::Motion;
use super::classify::{Class, TANGENT};
use crate::intersect::extended_curve;
use crate::offset::classify::{self as oc, Plan, fins, normal};
use crate::offset::edges::{self, EdgeGeo, NewCurve};
use crate::offset::vertices::end_param;
use crate::tol::LINEAR_RESOLUTION;
use crate::topo::{Body, EdgeId, FaceId, VertexId};
use crate::work::Meter;
use crate::{Error, TweakCase, TweakError};
use std::collections::BTreeMap;

/// The offset's class for each edge, which its kept, smooth and closing
/// paths read: a tweak's kept and carried edges are kept, its edges moved
/// by one offset smooth (with how far their faces' offsets lie apart, as
/// `offset::classify` measures it), its closing edges closing.
pub(crate) fn offset_classes(
    body: &Body,
    motion: &BTreeMap<FaceId, Motion>,
    class: &BTreeMap<EdgeId, Class>,
) -> BTreeMap<EdgeId, (oc::Class, f64)> {
    class
        .iter()
        .map(|(&e, &c)| {
            let to = match c {
                Class::Kept | Class::Carried | Class::Hinge => (oc::Class::Kept, 0.0),
                Class::Closing => (oc::Class::Closing, 0.0),
                Class::Moved => {
                    let f = body.loops[body.fins[fins(body, e)[0]].loop_].face;
                    match motion[&f] {
                        Motion::Offset(d) => (oc::Class::Smooth, gap(body, e, d)),
                        Motion::Rigid(_) | Motion::Replace | Motion::Draft { .. } => {
                            (oc::Class::Kept, 0.0)
                        }
                    }
                }
            };
            (e, to)
        })
        .collect()
}

/// How far apart the offsets by `d` of a tangent edge's faces lie along
/// it: `|d|` times the largest angle between their normals at 17 samples,
/// as `offset::classify` measures a smooth edge's gap.
fn gap(body: &Body, e: EdgeId, d: f64) -> f64 {
    let [fa, fb] = fins(body, e);
    let [t0, t1] = body.edges[e].interval;
    (0..17)
        .map(|i| {
            let t = t0 + (t1 - t0) * f64::from(i) / 16.0;
            let (a, b) = (normal(body, fa, t), normal(body, fb, t));
            crate::math::atan2(a.cross(b).length(), a.dot(b)).min(TANGENT) * d.abs()
        })
        .fold(0.0, f64::max)
}

/// The tweak's plan over `body`: each changed face's motion and new
/// surface (the rest their own), for unit tests.
#[cfg(test)]
pub(crate) fn plan_for<'a>(
    body: &'a Body,
    motion: &BTreeMap<FaceId, Motion>,
    class: &BTreeMap<EdgeId, Class>,
) -> Plan<'a> {
    let surface = body
        .faces()
        .map(|f| {
            let s = &body.surfaces[body.faces[f].surface];
            let new = match motion.get(&f) {
                Some(Motion::Rigid(r)) => s.transformed(r),
                _ => s.clone(),
            };
            (f, new)
        })
        .collect();
    Plan::for_tweak(
        body,
        motion.clone(),
        surface,
        BTreeMap::new(),
        offset_classes(body, motion, class),
    )
}

/// How far the vertex goes: the furthest any edge at it with a changed
/// face takes its end.
pub(crate) fn vertex_reach(plan: &Plan, v: VertexId) -> f64 {
    let body = plan.body;
    let p = body.vertices[v].point;
    body.vertex_edges(v)
        .into_iter()
        .filter(|&e| !body.is_laminar(e))
        .filter(|&e| {
            plan.fins(e)
                .iter()
                .any(|&f| plan.motion.contains_key(&plan.face_of(f)))
        })
        .map(|e| {
            let at_end = body.edges[e].end == v;
            let t = end_param(plan, e, at_end);
            plan.goes(plan.fins(e), p, t).distance(p)
        })
        .fold(0.0, f64::max)
}

/// A kept edge's curve carried past both ends to reach where its
/// vertices may go.
fn carried(plan: &Plan, e: EdgeId) -> Result<NewCurve, Error> {
    let body = plan.body;
    let edge = &body.edges[e];
    let curve = &body.curves[edge.curve];
    let [t0, t1] = edge.interval;
    let reach = 3.0 * vertex_reach(plan, edge.start).max(vertex_reach(plan, edge.end))
        + 10.0 * LINEAR_RESOLUTION;
    let margin = |t: f64| {
        let speed = curve.eval(t).d1.length();
        if speed > 0.0 { reach / speed } else { 0.0 }
    };
    let mut m = [margin(t0), margin(t1)];
    if let Some(p) = curve.period() {
        let room = (p - (t1 - t0)) / 2.0 * 0.9;
        m = m.map(|x| x.min(room));
    }
    let unsupported = || -> Error {
        TweakError::Unsupported {
            at: edge.name,
            case: TweakCase::CarriedCurve,
        }
        .into()
    };
    let (c, range) = extended_curve(curve, edge.interval, m).ok_or_else(unsupported)?;
    Ok(NewCurve {
        curve: c,
        range,
        period: None,
        tolerance: edge.tolerance.get(),
        pcurves: [None, None],
        reversed: false,
    })
}

/// An edge both of whose faces move by `r`: its curve transformed, its
/// pcurves kept.
fn moved_rigidly(plan: &Plan, e: EdgeId, r: &crate::geom::Rigid) -> NewCurve {
    let body = plan.body;
    let edge = &body.edges[e];
    let pcurve = |f| {
        body.fins[f]
            .pcurve
            .map(|p| (body.curves2[p.curve].clone(), p.map))
    };
    let [fa, fb] = plan.fins(e);
    NewCurve {
        curve: body.curves[edge.curve].transformed(r),
        range: edge.interval,
        period: None,
        tolerance: edge.tolerance.get(),
        pcurves: [pcurve(fa), pcurve(fb)],
        reversed: false,
    }
}

/// Every inner edge's new curve, in parallel.
pub(crate) fn curves(
    plan: &Plan,
    class: &BTreeMap<EdgeId, Class>,
    meter: &Meter,
) -> Result<BTreeMap<EdgeId, EdgeGeo>, Error> {
    meter
        .par_map(&plan.edges, |meter, &e| {
            meter.charge(1).map_err(|_| TweakError::Uncertain {
                at: plan.body.edges[e].name,
            })?;
            let g = match class[&e] {
                Class::Carried | Class::Hinge => EdgeGeo::One(carried(plan, e)?),
                Class::Moved => {
                    let f = plan.face_of(plan.fins(e)[0]);
                    match plan.motion[&f] {
                        Motion::Rigid(r) => EdgeGeo::One(moved_rigidly(plan, e, &r)),
                        _ => edges::edge(plan, e, meter)?,
                    }
                }
                Class::Kept | Class::Closing => edges::edge(plan, e, meter)?,
            };
            Ok::<_, Error>((e, g))
        })
        .into_iter()
        .collect()
}

/// Every laminar edge's rail: the offset's (the edge itself on an
/// unchanged face, moved along an offset face's normal), but on a face
/// moved rigidly the edge transformed, its pcurve kept, and on an
/// unchanged face whose edge's vertex moves the edge carried past its
/// ends, as a carried inner edge is.
pub(crate) fn rails(plan: &Plan, meter: &Meter) -> Result<BTreeMap<EdgeId, NewCurve>, Error> {
    let mut out = edges::rails(plan, meter)?;
    for &e in &plan.laminar {
        let fin = plan.laminar_fin(e);
        let face = plan.face_of(fin);
        let edge = &plan.body.edges[e];
        if !plan.motion.contains_key(&face)
            && (vertex_reach(plan, edge.start) > 0.0 || vertex_reach(plan, edge.end) > 0.0)
        {
            out.insert(e, carried(plan, e)?);
            continue;
        }
        if let Some(Motion::Rigid(r)) = plan.motion.get(&face) {
            let body = plan.body;
            let edge = &body.edges[e];
            let pc = body.fins[fin]
                .pcurve
                .map(|p| (body.curves2[p.curve].clone(), p.map));
            out.insert(
                e,
                NewCurve {
                    curve: body.curves[edge.curve].transformed(r),
                    range: edge.interval,
                    period: None,
                    tolerance: edge.tolerance.get(),
                    pcurves: [pc, None],
                    reversed: false,
                },
            );
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{Rigid, Vector3};
    use crate::name::{FaceRole, Name, OpKey, Role};
    use crate::primitive;
    use crate::work::Meter;

    const K: OpKey = OpKey(5);

    fn raised(b: &Body, by: f64) -> BTreeMap<FaceId, Motion> {
        let n = Name::derive(K, Role::Face(FaceRole::PlusZ), &[]);
        let top = b.faces().find(|&f| b.faces[f].name == n).unwrap();
        BTreeMap::from([(
            top,
            Motion::Rigid(Rigid::translation(Vector3::new(0.0, 0.0, by))),
        )])
    }

    #[test]
    fn a_raised_tops_edges_close_on_its_plane_and_the_sides_are_carried_up() {
        let (b, _) = primitive::block(K, 1.0, 0.6, 0.4, &Rigid::IDENTITY).unwrap();
        let motion = raised(&b, 0.1);
        let class = super::super::classify::classes(&b, &motion).unwrap();
        let plan = plan_for(&b, &motion, &class);
        // unmetered: a unit test.
        let geo = curves(&plan, &class, &Meter::free()).unwrap();
        for (&e, g) in &geo {
            let EdgeGeo::One(c) = g else {
                panic!("an edge opened")
            };
            match class[&e] {
                Class::Kept => assert_eq!(c.curve, b.curves[b.edges[e].curve]),
                Class::Carried => {
                    let [t0, t1] = b.edges[e].interval;
                    assert!(
                        c.range[0] < t0 - 0.29 && c.range[1] > t1 + 0.29,
                        "{:?}",
                        c.range
                    );
                }
                Class::Closing => {
                    let p = c.curve.point((c.range[0] + c.range[1]) / 2.0);
                    assert!((p.z - 0.5).abs() < 1e-9, "{p:?}");
                }
                Class::Moved | Class::Hinge => unreachable!(),
            }
        }
    }

    #[test]
    fn a_moved_body_transforms_its_curves_keeping_their_parameters() {
        let (b, _) = primitive::block(K, 1.0, 0.6, 0.4, &Rigid::IDENTITY).unwrap();
        let r = Rigid::translation(Vector3::new(0.3, 0.0, 0.0));
        let motion: BTreeMap<_, _> = b.faces().map(|f| (f, Motion::Rigid(r))).collect();
        let class = super::super::classify::classes(&b, &motion).unwrap();
        let plan = plan_for(&b, &motion, &class);
        // unmetered: a unit test.
        let geo = curves(&plan, &class, &Meter::free()).unwrap();
        for (&e, g) in &geo {
            let EdgeGeo::One(c) = g else { panic!() };
            assert_eq!(c.range, b.edges[e].interval);
            assert_eq!(c.curve, b.curves[b.edges[e].curve].transformed(&r));
        }
    }
}
