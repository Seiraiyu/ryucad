//! What each vertex of a tweaked body becomes: one point, on every new
//! curve meeting there (design 384's Vertices).

use super::Motion;
use super::classify::Class;
use super::motion::apply;
use crate::blend::spine::solve3;
use crate::geom::{Point3, Vector3};
use crate::offset::classify::{Plan, normal};
use crate::offset::edges::{EdgeGeo, NewCurve, project_on};
use crate::offset::vertices::{VertexGeo, end_param, hit, on, one};
use crate::tol::LINEAR_RESOLUTION;
use crate::topo::{EdgeId, FaceId, VertexId};
use crate::work::Meter;
use crate::{Error, TweakError};
use std::collections::BTreeMap;

/// One end of an edge at a vertex: the edge, whether it is the edge's
/// end (not its start), and whether the edge is laminar.
#[derive(Clone, Copy, Debug)]
struct End {
    edge: EdgeId,
    at_end: bool,
    laminar: bool,
}

/// Every vertex's new point, in parallel, in the order of their names.
pub(crate) fn vertices(
    plan: &Plan,
    class: &BTreeMap<EdgeId, Class>,
    geo: &BTreeMap<EdgeId, EdgeGeo>,
    rails: &BTreeMap<EdgeId, NewCurve>,
    meter: &Meter,
) -> Result<BTreeMap<VertexId, VertexGeo>, Error> {
    let body = plan.body;
    let mut vs: Vec<VertexId> = body
        .vertices()
        .filter(|&v| body.vertices[v].edge.is_some())
        .collect();
    vs.sort_by_key(|&v| body.vertices[v].name);
    let out: BTreeMap<VertexId, VertexGeo> = meter
        .par_map(&vs, |meter, &v| {
            vertex(plan, class, geo, rails, v, meter).map(|g| (v, g))
        })
        .into_iter()
        .collect::<Result<_, Error>>()?;
    apart(plan, &out)?;
    Ok(out)
}

/// The curve an end runs on: an inner edge's new curve, a laminar
/// edge's rail.
fn curve_of<'a>(
    geo: &'a BTreeMap<EdgeId, EdgeGeo>,
    rails: &'a BTreeMap<EdgeId, NewCurve>,
    x: End,
) -> &'a NewCurve {
    if x.laminar {
        &rails[&x.edge]
    } else {
        one(geo, x.edge)
    }
}

fn vertex(
    plan: &Plan,
    class: &BTreeMap<EdgeId, Class>,
    geo: &BTreeMap<EdgeId, EdgeGeo>,
    rails: &BTreeMap<EdgeId, NewCurve>,
    v: VertexId,
    meter: &Meter,
) -> Result<VertexGeo, Error> {
    let body = plan.body;
    let at = body.vertices[v].name;
    meter.charge(1).map_err(|_| TweakError::Uncertain { at })?;
    let p = body.vertices[v].point;
    // The ends here, and each face round the vertex with its outward
    // normal there.
    let mut ends: Vec<End> = Vec::new();
    let mut faces: BTreeMap<FaceId, Vector3> = BTreeMap::new();
    for e in body.vertex_edges(v) {
        let edge = &body.edges[e];
        let laminar = body.is_laminar(e);
        for at_end in [false, true] {
            if (if at_end { edge.end } else { edge.start }) != v {
                continue;
            }
            ends.push(End {
                edge: e,
                at_end,
                laminar,
            });
            let t = end_param(plan, e, at_end);
            for fin in body.edge_fins(e) {
                let f = plan.face_of(fin);
                faces.entry(f).or_insert_with(|| normal(body, fin, t));
            }
        }
    }
    ends.sort_by_key(|x| (body.edges[x.edge].name, x.at_end));
    let mut out = VertexGeo {
        kept: true,
        ..VertexGeo::default()
    };
    let put = |out: &mut VertexGeo, x: End, t: f64| {
        let key = (x.edge, x.at_end);
        if x.laminar {
            out.laminar.insert(key, (0, t));
        } else {
            out.one.insert(key, (0, t));
        }
    };
    let motions: Vec<Option<Motion>> = faces.keys().map(|f| plan.motion.get(f).copied()).collect();
    let same = motions.windows(2).all(|w| w[0] == w[1]);
    let own_ends = |out: &mut VertexGeo, x: Point3| {
        out.points = vec![x];
    };
    match (same, motions[0]) {
        (true, None) => {
            own_ends(&mut out, p);
            for &x in &ends {
                put(&mut out, x, end_param(plan, x.edge, x.at_end));
            }
            return Ok(out);
        }
        (true, Some(Motion::Rigid(r))) => {
            own_ends(&mut out, r.apply_point(p));
            for &x in &ends {
                put(&mut out, x, end_param(plan, x.edge, x.at_end));
            }
            return Ok(out);
        }
        (true, Some(Motion::Offset(d)))
            if ends
                .iter()
                .all(|x| x.laminar || class[&x.edge] == Class::Moved) =>
        {
            let n = faces.values().next().copied().unwrap_or(Vector3::ZERO);
            let n = n.normalize().map_or(Vector3::ZERO, |n| n.get());
            own_ends(&mut out, p + n * d);
            for &x in &ends {
                put(&mut out, x, end_param(plan, x.edge, x.at_end));
            }
            return Ok(out);
        }
        _ => {}
    }
    // Where every face's moved tangent plane comes nearest.
    let moved: Vec<(Point3, Vector3)> = faces
        .iter()
        .map(|(&f, &n)| {
            let n = n.normalize().map_or(Vector3::ZERO, |n| n.get());
            apply(
                &plan.motion_of(f),
                &plan.surface[&f],
                plan.sense_of(f),
                p,
                n,
            )
        })
        .collect();
    let x0 = nearest(&moved);
    // The base curve: carried, then moved, then closing.
    let rank = |x: &End| match class.get(&x.edge) {
        Some(Class::Carried) => 0,
        Some(Class::Moved) => 1,
        Some(Class::Closing) => 2,
        _ => 3,
    };
    let base = *ends
        .iter()
        .filter(|x| !x.laminar)
        .min_by_key(|x| (rank(x), body.edges[x.edge].name))
        .ok_or(TweakError::Uncertain { at })?;
    let c = curve_of(geo, rails, base);
    let base_faces: Vec<FaceId> = body.edge_fins(base.edge).map(|f| plan.face_of(f)).collect();
    let mut third: Vec<FaceId> = faces
        .keys()
        .copied()
        .filter(|f| !base_faces.contains(f))
        .collect();
    third.sort_by_key(|&f| body.faces[f].name);
    let (t, x) = match third.first() {
        Some(&f) => hit(c, &plan.surface[&f], x0, at, meter)?.ok_or(TweakError::EdgeVanishes {
            edge: body.edges[base.edge].name,
        })?,
        None => project_on(&c.curve, c.range, x0),
    };
    own_ends(&mut out, x);
    put(&mut out, base, t);
    for &e in &ends {
        if e.edge == base.edge && e.at_end == base.at_end {
            continue;
        }
        let ce = curve_of(geo, rails, e);
        let te = on(ce, x, c.tolerance).ok_or(if faces.len() >= 4 {
            TweakError::VertexSplits { vertex: at }
        } else {
            TweakError::Uncertain { at }
        })?;
        put(&mut out, e, te);
    }
    Ok(out)
}

/// The point nearest every plane `(q, n)` (least squares), pulled to the
/// planes' points' mean by `1e-9` so dependent planes still give one.
fn nearest(planes: &[(Point3, Vector3)]) -> Point3 {
    let k = planes.len().max(1) as f64;
    let mean = planes
        .iter()
        .fold(Vector3::ZERO, |a, (q, _)| a + q.to_vector())
        * (1.0 / k);
    let eps = 1e-9;
    let mut m = [[0.0; 3]; 3];
    let mut r = [0.0; 3];
    for (q, n) in planes {
        let n = [n.x, n.y, n.z];
        let c = n[0] * q.x + n[1] * q.y + n[2] * q.z;
        for i in 0..3 {
            for j in 0..3 {
                m[i][j] += n[i] * n[j];
            }
            r[i] += n[i] * c;
        }
    }
    let mv = [mean.x, mean.y, mean.z];
    for i in 0..3 {
        m[i][i] += eps;
        r[i] += eps * mv[i];
    }
    let rows = m.map(|row| Vector3::new(row[0], row[1], row[2]));
    solve3(rows, r).unwrap_or(Point3::ORIGIN + mean)
}

/// Every open edge's new ends at least ten resolutions apart.
fn apart(plan: &Plan, vg: &BTreeMap<VertexId, VertexGeo>) -> Result<(), Error> {
    let body = plan.body;
    for &e in &plan.edges {
        let edge = &body.edges[e];
        if edge.start == edge.end {
            continue;
        }
        let a = vg[&edge.start].points[0];
        let b = vg[&edge.end].points[0];
        if a.distance(b) < 10.0 * LINEAR_RESOLUTION {
            return Err(TweakError::EdgeVanishes { edge: edge.name }.into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{Point3, Rigid, Vector3};
    use crate::name::{FaceRole, Name, OpKey, Role};
    use crate::primitive;
    use crate::tweak::{classify::classes, curves};
    use crate::work::Meter;

    const K: OpKey = OpKey(6);

    #[test]
    fn a_raised_tops_corners_go_up_and_the_bottoms_stay() {
        let (b, _) = primitive::block(K, 1.0, 0.6, 0.4, &Rigid::IDENTITY).unwrap();
        let n = Name::derive(K, Role::Face(FaceRole::PlusZ), &[]);
        let top = b.faces().find(|&f| b.faces[f].name == n).unwrap();
        let motion = BTreeMap::from([(
            top,
            Motion::Rigid(Rigid::translation(Vector3::new(0.0, 0.0, 0.1))),
        )]);
        let class = classes(&b, &motion).unwrap();
        let plan = curves::plan_for(&b, &motion, &class);
        // unmetered: a unit test.
        let m = Meter::free();
        let geo = curves::curves(&plan, &class, &m).unwrap();
        let rails = curves::rails(&plan, &m).unwrap();
        let vg = vertices(&plan, &class, &geo, &rails, &m).unwrap();
        assert_eq!(vg.len(), 8);
        for (&v, g) in &vg {
            let p = b.vertices[v].point;
            let want = if p.z > 0.2 {
                p + Vector3::new(0.0, 0.0, 0.1)
            } else {
                p
            };
            assert!(
                g.points[0].distance(want) < 1e-9,
                "{:?} {want:?}",
                g.points[0]
            );
            assert!(g.kept);
            assert_eq!(g.one.len(), 3);
        }
        let _ = Point3::ORIGIN;
    }
}
