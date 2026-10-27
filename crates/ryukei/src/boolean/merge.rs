//! Merging what a boolean leaves redundant.
//!
//! Two faces either side of an edge on the same surface (one surface, or
//! two that are coincident by the rule of [`crate::intersect`], with
//! outward normals that agree) become one by KEF; an edge left with the
//! same face on both sides of one loop goes by KEMR, and a lone vertex
//! left behind (one an operand didn't have) by KVR. Then a vertex between
//! exactly two edges on one curve goes by JEKV, the second edge first put
//! on the first's curve when they lie on coincident lines or circles.
//! Fins whose pcurves no longer fit their face get exact or fitted ones.
//! Candidates are taken in a fixed order (by entity), and every merge is
//! recorded so names can follow it.

use super::Fail;
use super::table::{GeId, GvId};
use crate::euler;
use crate::geom::{Curve, Point2, Surface};
use crate::name::OpKey;
use crate::tol::{LINEAR_RESOLUTION, Tolerance};
use crate::topo::{Body, EdgeId, FaceId, Pcurve, Sense, VertexId};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// What each surviving entity stands for: kept pieces, global edges,
/// global vertices.
#[derive(Clone, Debug, Default)]
pub(crate) struct Sources {
    pub(crate) faces: BTreeMap<FaceId, Vec<usize>>,
    pub(crate) edges: BTreeMap<EdgeId, Vec<GeId>>,
    pub(crate) vertices: BTreeMap<VertexId, GvId>,
}

/// Merges `body` in place, keeping `sources` up to date. `keep_lone`
/// holds the lone vertices that are an operand's own (a cone's apex).
pub(crate) fn merge(
    body: &mut Body,
    sources: &mut Sources,
    keep_lone: &BTreeSet<VertexId>,
    keys: &mut dyn FnMut() -> OpKey,
) -> Result<(), Fail> {
    let topo = |_| Fail::Inconsistent;
    // Faces on one surface.
    loop {
        let mut done = true;
        let edges: Vec<EdgeId> = body.edges().collect();
        for e in edges {
            if body.edge(e).is_none() {
                continue;
            }
            let fins: Vec<_> = body.edge_fins(e).collect();
            let [a, b] = fins[..] else { continue };
            let fa = body
                .loop_(body.fin(a).expect("live").loop_)
                .expect("live")
                .face;
            let fb = body
                .loop_(body.fin(b).expect("live").loop_)
                .expect("live")
                .face;
            if fa == fb || !same_surface(body, fa, fb) {
                continue;
            }
            let (keep, gone) = (fa.min(fb), fa.max(fb));
            // The merged face goes on a surface holding both faces: the
            // kept face's, the other's, or the plane two flat splines lie
            // on; two splines that only overlap stay apart.
            let Some(onto) = merged_surface(body, keep, gone) else {
                continue;
            };
            euler::kef(body, keys(), e).map_err(topo)?;
            if let Some((surface, sense)) = onto {
                euler::set_surface(body, keep, surface, sense).map_err(topo)?;
            }
            let moved = sources.faces.remove(&gone).unwrap_or_default();
            sources.faces.entry(keep).or_default().extend(moved);
            sources.edges.remove(&e);
            done = false;
        }
        // An edge with one face on both sides, in one loop.
        let edges: Vec<EdgeId> = body.edges().collect();
        for e in edges {
            if body.edge(e).is_none() {
                continue;
            }
            let fins: Vec<_> = body.edge_fins(e).collect();
            let [a, b] = fins[..] else { continue };
            let (la, lb) = (
                body.fin(a).expect("live").loop_,
                body.fin(b).expect("live").loop_,
            );
            if la == lb {
                euler::kemr(body, e).map_err(topo)?;
                sources.edges.remove(&e);
                done = false;
                continue;
            }
            // A closed edge alone in two loops of one face: a circle where a
            // tool only touched a periodic face all round. The second loop
            // leaves as a face of its own, and the edge goes between them.
            let face = body.loop_(la).expect("live").face;
            let alone = |l| body.loop_fins(l).count() == 1;
            if body.loop_(lb).expect("live").face == face && alone(la) && alone(lb) {
                let m = euler::mfkrh(body, keys(), lb).map_err(topo)?.0;
                let kept = euler::kef(body, keys(), e).map_err(topo)?.0.face;
                let gone = if kept == face { m.face } else { face };
                let moved = sources.faces.remove(&gone).unwrap_or_default();
                sources.faces.entry(kept).or_default().extend(moved);
                sources.edges.remove(&e);
                done = false;
            }
        }
        if done {
            break;
        }
    }
    // Lone vertices no operand had.
    let lone: Vec<_> = body
        .loops()
        .filter_map(|l| body.loop_(l).expect("live").vertex.map(|v| (l, v)))
        .collect();
    for (l, v) in lone {
        if keep_lone.contains(&v) {
            continue;
        }
        let face = body.loop_(l).expect("live").face;
        let alone = body.face_loops(face).count() == 1;
        let closed = body
            .surface(body.face(face).expect("live").surface)
            .expect("live")
            .closed_genus()
            .is_some();
        if alone && !closed {
            continue;
        }
        euler::kvr(body, l).map_err(topo)?;
        sources.vertices.remove(&v);
    }
    // Every fin needs a pcurve before vertices merge.
    fill(body)?;
    // Vertices between two edges on one curve.
    let vertices: Vec<VertexId> = body.vertices().collect();
    for v in vertices {
        if body.vertex(v).is_none() {
            continue;
        }
        let edges = body.vertex_edges(v);
        let [x, y] = edges[..] else { continue };
        let (ex, ey) = (body.edge(x).expect("live"), body.edge(y).expect("live"));
        // The edge ending at v, then the one starting there.
        let (e1, e2) = if ex.end == v && ey.start == v {
            (x, y)
        } else if ey.end == v && ex.start == v {
            (y, x)
        } else {
            continue;
        };
        if e1 == e2 || !onto(body, e1, e2)? {
            continue;
        }
        let (k, _) = euler::jekv(body, keys(), v).map_err(topo)?;
        let moved = sources.edges.remove(&e2).unwrap_or_default();
        sources.edges.entry(k.edge).or_default().extend(moved);
        sources.vertices.remove(&v);
    }
    fill(body)
}

/// Where the face `keep` goes when `gone` merges into it: `Some(None)` on
/// its own surface, `Some(Some((surface, sense)))` onto another (with the
/// sense that keeps its outward normal), `None` when no surface holds
/// both.
fn merged_surface(
    body: &mut Body,
    keep: FaceId,
    gone: FaceId,
) -> Option<Option<(crate::topo::SurfaceId, crate::topo::Sense)>> {
    let (fk, fg) = (
        body.face(keep).expect("live").clone(),
        body.face(gone).expect("live").clone(),
    );
    if fk.surface == fg.surface {
        return Some(None);
    }
    let (sk, sg) = (
        body.surface(fk.surface).expect("live").clone(),
        body.surface(fg.surface).expect("live").clone(),
    );
    match crate::intersect::covering(&sk, &sg) {
        Some(true) => Some(None),
        Some(false) => Some(Some((fg.surface, fg.sense))),
        None => {
            let plane = crate::intersect::common(&sk, &sg);
            if !matches!(plane, Surface::Plane(_)) {
                return None;
            }
            let same = crate::intersect::coincident(&sk, &plane)?;
            let sense = if same { fk.sense } else { fk.sense.flipped() };
            let id = body.add_surface(plane);
            Some(Some((id, sense)))
        }
    }
}

/// Whether two faces lie on one surface with outward normals that agree.
fn same_surface(body: &Body, a: FaceId, b: FaceId) -> bool {
    let (fa, fb) = (body.face(a).expect("live"), body.face(b).expect("live"));
    if fa.surface == fb.surface {
        return fa.sense == fb.sense;
    }
    let (sa, sb) = (
        body.surface(fa.surface).expect("live"),
        body.surface(fb.surface).expect("live"),
    );
    match crate::intersect::coincident(sa, sb) {
        Some(same) => same == (fa.sense == fb.sense),
        None => false,
    }
}

/// Puts edge `e2`, which starts where `e1` ends, on `e1`'s curve so JEKV
/// can join them: true when they are on one curve already, or on
/// coincident lines or circles running the same way.
fn onto(body: &mut Body, e1: EdgeId, e2: EdgeId) -> Result<bool, Fail> {
    let (x, y) = (
        body.edge(e1).expect("live").clone(),
        body.edge(e2).expect("live").clone(),
    );
    if x.curve == y.curve {
        if x.interval[1] != y.interval[0] {
            return Ok(false);
        }
        return share_pcurves(body, e1, e2);
    }
    let (cx, cy) = (
        body.curve(x.curve).expect("live").clone(),
        body.curve(y.curve).expect("live").clone(),
    );
    let tol = x.tolerance.get().max(y.tolerance.get());
    let same_line_or_circle = match (&cx, &cy) {
        (Curve::Line(_), Curve::Line(_)) | (Curve::Circle(_), Curve::Circle(_)) => {
            let [t0, t1] = y.interval;
            let n = 4;
            (0..=n).all(|k| {
                let t = t0 + (t1 - t0) * f64::from(k) / f64::from(n);
                cx.distance(cy.point(t)) <= tol
            }) && cx.eval(x.interval[1]).d1.dot(cy.eval(y.interval[0]).d1) > 0.0
        }
        _ => false,
    };
    if !same_line_or_circle {
        return Ok(false);
    }
    // e2 over the same stretch of e1's curve, starting exactly where e1
    // ends.
    let start = x.interval[1];
    let mut end = cx.project(cy.point(y.interval[1])).param;
    if let Some(p) = cx.period() {
        while end <= start {
            end += p;
        }
        while end > start + p {
            end -= p;
        }
    }
    if end <= start {
        return Ok(false);
    }
    let saved = body.clone();
    euler::set_curve(body, e2, x.curve, [start, end]).map_err(|_| Fail::Inconsistent)?;
    if !share_pcurves(body, e1, e2)? {
        *body = saved;
        return Ok(false);
    }
    Ok(true)
}

/// Gives `e2`'s fins the pcurves of the fins of `e1` beside them, where
/// those fit `e2` too (same curve, same face), so JEKV can join them.
/// False, with nothing changed, when they don't.
fn share_pcurves(body: &mut Body, e1: EdgeId, e2: EdgeId) -> Result<bool, Fail> {
    if partners_agree(body, e1, e2) {
        return Ok(in_domain(body, e2));
    }
    let y = body.edge(e2).expect("live").clone();
    let curve = body.curve(y.curve).expect("live").clone();
    let mut changes = Vec::new();
    for g in body.edge_fins(e2).collect::<Vec<_>>() {
        let fin = body.fin(g).expect("live");
        let beside = match fin.sense {
            Sense::Forward => fin.prev,
            Sense::Reversed => fin.next,
        };
        let b = body.fin(beside).expect("live");
        if b.edge != e1 || b.sense != fin.sense {
            return Ok(false);
        }
        let Some(pc) = b.pcurve else {
            return Ok(false);
        };
        let face = body.loop_(fin.loop_).expect("live").face;
        let surface = body
            .surface(body.face(face).expect("live").surface)
            .expect("live");
        let c2 = body.curve2(pc.curve).expect("live");
        let [d0, d1] = c2.domain();
        let fits = (0..=8).all(|k| {
            let t = y.interval[0] + (y.interval[1] - y.interval[0]) * f64::from(k) / 8.0;
            let s = pc.param(t);
            let q = c2.point(s);
            s >= d0 - 1e-12
                && s <= d1 + 1e-12
                && within(surface, q)
                && surface.point(q.x, q.y).distance(curve.point(t)) <= y.tolerance.get()
        });
        if !fits {
            return Ok(false);
        }
        changes.push((g, pc));
    }
    for (g, pc) in changes {
        euler::set_pcurve(body, g, Some(pc)).map_err(|_| Fail::Inconsistent)?;
    }
    Ok(partners_agree(body, e1, e2))
}

/// Whether `q` lies in a surface's domain in each direction that isn't
/// periodic. A pcurve run on past a sphere's pole still maps onto the
/// sphere (over the pole, half a turn round), but no face's parameter
/// plane holds it: two edges either side of a pole can't be joined.
fn within(surface: &Surface, q: Point2) -> bool {
    let domain = surface.domain();
    let (pu, pv) = surface.periods();
    [(q.x, domain[0], pu), (q.y, domain[1], pv)]
        .into_iter()
        .all(|(x, [lo, hi], period)| {
            period.is_some() || {
                let slack = 1e-12 * lo.abs().max(hi.abs()).clamp(1.0, 1e6);
                x >= lo - slack && x <= hi + slack
            }
        })
}

/// Whether every fin of `e` keeps its pcurve within its face's surface's
/// domain (see [`within`]) all along.
fn in_domain(body: &Body, e: EdgeId) -> bool {
    let x = body.edge(e).expect("live");
    body.edge_fins(e).all(|g| {
        let fin = body.fin(g).expect("live");
        let Some(pc) = fin.pcurve else {
            return true;
        };
        let face = body.loop_(fin.loop_).expect("live").face;
        let surface = body
            .surface(body.face(face).expect("live").surface)
            .expect("live");
        let c2 = body.curve2(pc.curve).expect("live");
        (0..=8).all(|k| {
            let t = x.interval[0] + (x.interval[1] - x.interval[0]) * f64::from(k) / 8.0;
            within(surface, c2.point(pc.param(t)))
        })
    })
}

/// Whether every fin of `e2` has a fin of `e1` beside it with the same
/// sense and pcurve, as JEKV needs.
fn partners_agree(body: &Body, e1: EdgeId, e2: EdgeId) -> bool {
    body.edge_fins(e2).all(|g| {
        let fin = body.fin(g).expect("live");
        let beside = match fin.sense {
            Sense::Forward => fin.prev,
            Sense::Reversed => fin.next,
        };
        let b = body.fin(beside).expect("live");
        b.edge == e1 && b.sense == fin.sense && b.pcurve == fin.pcurve
    })
}

/// Gives every fin without a pcurve an exact one, or a fitted one whose
/// bound the edge's tolerance then covers.
fn fill(body: &mut Body) -> Result<(), Fail> {
    let fins: Vec<_> = body
        .fins()
        .filter(|&f| body.fin(f).expect("live").pcurve.is_none())
        .collect();
    for f in fins {
        let fin = body.fin(f).expect("live");
        let e = fin.edge;
        let face = body.loop_(fin.loop_).expect("live").face;
        let edge = body.edge(e).expect("live").clone();
        let surface: Surface = body
            .surface(body.face(face).expect("live").surface)
            .expect("live")
            .clone();
        let curve = body.curve(edge.curve).expect("live").clone();
        let (c2, map) =
            match crate::pcurve::exact(&surface, &curve, edge.interval, edge.tolerance.get()) {
                Some(x) => x,
                None => {
                    let (n, bound) = crate::intersect::fit_pcurve(&surface, &curve, edge.interval)
                        .map_err(|_| Fail::Inconsistent)?;
                    if bound > edge.tolerance.get() {
                        let t = Tolerance::new(bound.max(LINEAR_RESOLUTION))
                            .map_err(|_| Fail::Inconsistent)?;
                        euler::set_edge_tolerance(body, e, t).map_err(|_| Fail::Inconsistent)?;
                    }
                    (crate::geom::Curve2::Nurbs2(Arc::new(n)), [0.0, 1.0])
                }
            };
        let id = body.add_curve2(c2);
        euler::set_pcurve(body, f, Some(Pcurve { curve: id, map }))
            .map_err(|_| Fail::Inconsistent)?;
    }
    Ok(())
}
