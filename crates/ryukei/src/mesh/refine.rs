//! Refinement: splitting a face's triangles until each passes the
//! certified tests.
//!
//! A failing triangle has its longest edge (in the scaled plane) split at
//! the midpoint, followed by Delaunay flips. Halving edges always makes
//! progress, so refinement ends; flips keep the triangles from thinning
//! out. An edge on a seam side is split together with its partner on the
//! opposite side, and the two new points share one position; an edge on a
//! pole side is split like any other, its new point at the pole. An edge
//! that is a loop segment belongs to a body edge shared with another face,
//! so it is not split here: the face asks for a sample there instead, and
//! is meshed again once the edge has it.

use super::bound::{Targets, triangle_ok};
use super::cdt::{NONE, next, prev};
use super::domain::{Anchor, Domain, Side};
use super::uv::{FaceGeom, U, V};
use crate::geom::{Point2, Point3};
use crate::predicates::{Sign, orient2d};
use crate::topo::EdgeId;
use std::collections::VecDeque;

/// Why refinement stopped short of the targets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Stop {
    /// A triangle fails even below the smallest size refinement goes to.
    Stalled,
    /// The face passed the triangle budget.
    TooLarge,
}

/// The outcome of refining one face.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Refined {
    /// Samples the face needs on its edges: each edge and parameter once,
    /// in the order found.
    pub(crate) requests: Vec<(EdgeId, f64)>,
    pub(crate) stop: Option<Stop>,
}

/// Each vertex's position: its class's anchor, or the surface at the
/// class root's parameters.
pub(crate) fn position(
    d: &Domain,
    g: &FaceGeom,
    v: u32,
    fixed: &dyn Fn(Anchor) -> Point3,
) -> Point3 {
    match d.classes.anchor(v) {
        Anchor::Free => {
            let p = d.uv[d.classes.root(v) as usize];
            g.surface.point(p.x, p.y)
        }
        a => fixed(a),
    }
}

/// Which rectangle side an edge between two vertices lies along, if any.
fn side_of(d: &Domain, a: u32, b: u32) -> Option<usize> {
    let (p, q) = (d.uv[a as usize], d.uv[b as usize]);
    let on = |s: usize| match s {
        0 => p.y == d.rect[V][0] && q.y == d.rect[V][0],
        1 => p.x == d.rect[U][1] && q.x == d.rect[U][1],
        2 => p.y == d.rect[V][1] && q.y == d.rect[V][1],
        _ => p.x == d.rect[U][0] && q.x == d.rect[U][0],
    };
    (0..4).find(|&s| on(s))
}

/// Refines the inside triangles of a face's domain. `fixed` gives the
/// positions of body anchors; `cap` is the most inside triangles the face
/// may have.
pub(crate) fn refine(
    d: &mut Domain,
    g: &FaceGeom,
    t: &Targets,
    fixed: &dyn Fn(Anchor) -> Point3,
    cap: u64,
) -> Refined {
    let mut out = Refined::default();
    let mut count = d.inside.iter().filter(|&&x| x).count() as u64;
    let mut queue: VecDeque<u32> = (0..d.cdt.triangle_count() as u32)
        .filter(|&t| d.inside[t as usize])
        .collect();
    let mut queued: Vec<bool> = d.inside.clone();
    let mut pos: Vec<Option<Point3>> = vec![None; d.uv.len()];
    // Below this length in the scaled plane a triangle is not split.
    let floor = t.waive / 16.0;
    while let Some(tri) = queue.pop_front() {
        queued[tri as usize] = false;
        let corners = d.cdt.corners(tri);
        pos.resize(d.uv.len(), None);
        let xyz = corners.map(|v| *pos[v as usize].get_or_insert_with(|| position(d, g, v, fixed)));
        let uv = corners.map(|v| d.uv[v as usize]);
        if triangle_ok(g.surface, g.metric, uv, xyz, t) {
            continue;
        }
        if count > cap {
            out.stop = Some(Stop::TooLarge);
            return out;
        }
        // The longest edge in the scaled plane: opposite corner i.
        let scaled = uv.map(|p| g.metric.scale(p));
        let len = |i: usize| scaled[next(i)].distance(scaled[prev(i)]);
        let i = (0..3).fold(0, |best, i| if len(i) > len(best) { i } else { best });
        if len(i) < floor {
            out.stop = Some(Stop::Stalled);
            continue;
        }
        let (b, c) = (corners[next(i)], corners[prev(i)]);
        let before = d.cdt.triangle_count();
        let made = if !d.cdt.is_fixed(tri, i) {
            split_inner(d, g, tri, i)
        } else if d.cdt.neighbour(tri, i) == NONE {
            split_side(d, g, tri, i, b, c)
        } else {
            // A loop segment: ask its edge for a sample in the middle.
            let key = [b.min(c), b.max(c)];
            if let Some(&(e, ta, tb)) = d.segments.get(&key) {
                let r = (e, 0.5 * (ta + tb));
                if !out.requests.contains(&r) {
                    out.requests.push(r);
                }
            }
            continue;
        };
        let Some(made) = made else {
            out.stop = Some(Stop::Stalled);
            continue;
        };
        for t in before..d.cdt.triangle_count() {
            let inside = d.inside[d.cdt.origin(t as u32) as usize];
            d.inside.push(inside);
            count += u64::from(inside);
        }
        queued.resize(d.inside.len(), false);
        for v in made {
            for (t, _) in d.cdt.around(v) {
                if d.inside[t as usize] && !queued[t as usize] {
                    queued[t as usize] = true;
                    queue.push_back(t);
                }
            }
        }
    }
    out
}

/// Splits an inner edge at its midpoint. The new vertex is free.
fn split_inner(d: &mut Domain, g: &FaceGeom, tri: u32, i: usize) -> Option<Vec<u32>> {
    let corners = d.cdt.corners(tri);
    let (b, c) = (corners[next(i)], corners[prev(i)]);
    let (pb, pc) = (d.cdt.point(b), d.cdt.point(c));
    let m = Point2::new(0.5 * (pb.x + pc.x), 0.5 * (pb.y + pc.y));
    let n = d.cdt.point_count();
    let v = if orient2d(pb, pc, m) == Sign::Zero {
        d.cdt.split_edge(tri, i, m)
    } else {
        // Rounding moved the midpoint off the edge: insert it where it is.
        d.cdt.insert_from(tri, m)?
    };
    if (v as usize) < n {
        return None;
    }
    d.grow(v, g.metric.unscale(m));
    Some(vec![v])
}

/// Splits an edge on a side of the rectangle: a seam edge with its
/// partner, a pole edge alone. An outside side is never split.
fn split_side(
    d: &mut Domain,
    g: &FaceGeom,
    tri: u32,
    i: usize,
    b: u32,
    c: u32,
) -> Option<Vec<u32>> {
    let s = side_of(d, b, c)?;
    let (ub, uc) = (d.uv[b as usize], d.uv[c as usize]);
    let mid = Point2::new(0.5 * (ub.x + uc.x), 0.5 * (ub.y + uc.y));
    // Exactly on the side.
    let mid = match s {
        0 => Point2::new(mid.x, d.rect[V][0]),
        1 => Point2::new(d.rect[U][1], mid.y),
        2 => Point2::new(mid.x, d.rect[V][1]),
        _ => Point2::new(d.rect[U][0], mid.y),
    };
    match d.sides[s] {
        Side::Outside => None,
        Side::Pole => {
            let v = d.cdt.split_edge(tri, i, g.metric.scale(mid));
            d.grow(v, mid);
            d.join_poles(v).ok()?;
            Some(vec![v])
        }
        Side::Seam => {
            // Left and right sides pair across u; bottom and top across v.
            let k = if s % 2 == 1 { U } else { V };
            let (pb, pc) = (d.partner[k][b as usize], d.partner[k][c as usize]);
            if pb == NONE || pc == NONE {
                return None;
            }
            let (t2, i2) = d
                .cdt
                .find_edge(pb, pc)
                .or_else(|| d.cdt.find_edge(pc, pb))?;
            let mut twin = mid;
            if k == U {
                twin.x = if mid.x == d.rect[U][0] {
                    d.rect[U][1]
                } else {
                    d.rect[U][0]
                };
            } else {
                twin.y = if mid.y == d.rect[V][0] {
                    d.rect[V][1]
                } else {
                    d.rect[V][0]
                };
            }
            let v = d.cdt.split_edge(tri, i, g.metric.scale(mid));
            d.grow(v, mid);
            // The split may have flipped the partner edge's triangle, but
            // not the edge itself: it is fixed.
            let (t2, i2) = d
                .cdt
                .find_edge(pb, pc)
                .or_else(|| d.cdt.find_edge(pc, pb))
                .unwrap_or((t2, i2));
            let w = d.cdt.split_edge(t2, i2, g.metric.scale(twin));
            d.grow(w, twin);
            d.pair(k, v, w).ok()?;
            d.join_poles(v).ok()?;
            d.join_poles(w).ok()?;
            Some(vec![v, w])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::Rigid;
    use crate::mesh::domain::gather;
    use crate::mesh::edge::discretize;
    use crate::mesh::edge::tests::{geoms, targets, turned_seam};
    use crate::name::OpKey;
    use crate::primitive;
    use crate::topo::Body;

    /// Refines every face of a body and checks that every inside triangle
    /// passes and that the faces asked for no samples.
    fn refine_all(b: &Body, chord: f64, angle: f64) -> Vec<(Domain, Refined)> {
        let g = geoms(b);
        let t = targets(chord, angle);
        let mut samples = vec![None; b.edges.slot_count()];
        for e in b.edges() {
            samples[e.index()] = Some(discretize(b, e, &g, &t, &[]));
        }
        let fixed = |a: Anchor| match a {
            Anchor::Vertex(v) => b.vertices[v].point,
            Anchor::Sample(e, k) => samples[e.index()].as_ref().unwrap().points[k as usize],
            Anchor::Free => unreachable!(),
        };
        b.faces()
            .map(|f| {
                let geom = g[f.index()].as_ref().unwrap();
                let mut d = Domain::new(geom, &gather(b, geom, &samples)).unwrap();
                let r = refine(&mut d, geom, &t, &fixed, 1_000_000);
                for tri in 0..d.cdt.triangle_count() as u32 {
                    if !d.inside[tri as usize] {
                        continue;
                    }
                    let c = d.cdt.corners(tri);
                    let xyz = c.map(|v| position(&d, geom, v, &fixed));
                    let uv = c.map(|v| d.uv[v as usize]);
                    assert!(triangle_ok(geom.surface, geom.metric, uv, xyz, &t));
                }
                d.cdt.validate();
                (d, r)
            })
            .collect()
    }

    #[test]
    fn primitives_refine_without_requests() {
        let bodies = [
            primitive::block(OpKey(1), 1.0, 2.0, 3.0, &Rigid::IDENTITY)
                .unwrap()
                .0,
            primitive::cylinder(OpKey(1), 1.0, 2.0, &Rigid::IDENTITY)
                .unwrap()
                .0,
            primitive::cone(OpKey(1), 1.0, 2.0, &Rigid::IDENTITY)
                .unwrap()
                .0,
            primitive::sphere(OpKey(1), 1.0, &Rigid::IDENTITY)
                .unwrap()
                .0,
            primitive::torus(OpKey(1), 2.0, 0.5, &Rigid::IDENTITY)
                .unwrap()
                .0,
            turned_seam(1.0),
        ];
        for b in &bodies {
            for (_, r) in refine_all(b, 1e-3, 0.3) {
                assert_eq!(r, Refined::default());
            }
        }
    }

    #[test]
    fn seam_points_come_in_pairs() {
        let (b, _) = primitive::sphere(OpKey(1), 1.0, &Rigid::IDENTITY).unwrap();
        let (d, _) = refine_all(&b, 1e-4, 0.2).remove(0);
        let mut left = 0;
        for v in 0..d.uv.len() as u32 {
            let p = d.uv[v as usize];
            if p.x == d.rect[U][0] && p.y > d.rect[V][0] && p.y < d.rect[V][1] {
                left += 1;
                let w = d.partner[U][v as usize];
                assert_eq!(d.uv[w as usize].y, p.y);
                assert_eq!(d.classes.root(v), d.classes.root(w));
            }
            // Every point on a pole side is the pole.
            if p.y == d.rect[V][0] {
                assert_eq!(d.classes.root(v), d.classes.root(0));
            }
        }
        assert!(left > 10, "{left}");
    }

    #[test]
    fn a_tiny_budget_stops_early() {
        let (b, _) = primitive::sphere(OpKey(1), 1.0, &Rigid::IDENTITY).unwrap();
        let g = geoms(&b);
        let f = b.faces().next().unwrap();
        let geom = g[f.index()].as_ref().unwrap();
        let mut d = Domain::new(geom, &gather(&b, geom, &[])).unwrap();
        let none = |_: Anchor| unreachable!();
        let r = refine(&mut d, geom, &targets(1e-4, 0.2), &none, 50);
        assert_eq!(r.stop, Some(Stop::TooLarge));
    }
}
