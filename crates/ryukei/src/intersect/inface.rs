//! Point in face: where a point of a face's surface lies against the
//! face, certified.
//!
//! A point within an edge's (or a vertex's) tolerance of the face's
//! boundary is on it. Otherwise a ray is cast in the face's parameter
//! plane, from the point towards a point of one of its own pcurves, so it
//! must meet the boundary, at the latest where it aims. Every pcurve, at
//! every shift by the surface's periods that the ray can reach, is cut
//! with the ray by certified roots ([`crate::geom::bernstein`]) of the
//! line's equation along its Bézier pieces. The first crossing decides:
//! a loop has its face on its left in the parameter plane when the face
//! uses its surface's natural normal, and on its right otherwise, so the
//! side the loop crosses from says which side the point is on. A ray
//! whose first crossing can't be certified (it meets a vertex, touches a
//! pcurve, or two crossings can't be ordered) is abandoned for one aimed
//! elsewhere; when none settles it, the answer is
//! [`Failure::Uncertain`].

use super::Failure;
use crate::geom::bernstein::{Bernstein, Root};
use crate::geom::chain2::Chain2;
use crate::geom::{Curve, Point2, Point3, Vector2};
use crate::interval::Interval;
use crate::topo::{Body, EdgeId, FaceId, Sense, VertexId};

/// Where a point lies against a face.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    Inside,
    Outside,
    /// Within the tolerance of a vertex of the face.
    OnVertex(VertexId),
    /// Within the tolerance of an edge of the face, away from its
    /// vertices.
    OnEdge(EdgeId),
}

/// Where the point at parameters `uv` of face `face`'s surface lies
/// against the face. Periodic parameters may be given at any shift.
pub fn point_in_face(body: &Body, face: FaceId, uv: Point2) -> Result<Place, Failure> {
    let f = body.face(face).ok_or(Failure::Uncertain)?;
    let surface = body.surface(f.surface).ok_or(Failure::Uncertain)?;
    let p = surface.point(uv.x, uv.y);
    if let Some(on) = on_boundary(body, face, p) {
        return Ok(on);
    }
    let mut chains = Vec::new();
    for l in body.face_loops(face) {
        for fin in body.loop_fins(l) {
            let fi = &body.fins[fin];
            let pc = fi.pcurve.ok_or(Failure::Uncertain)?;
            let e = &body.edges[fi.edge];
            let [s0, s1] = e.interval.map(|t| pc.param(t));
            let (a, b) = match fi.sense {
                Sense::Forward => (s0, s1),
                Sense::Reversed => (s1, s0),
            };
            chains.push(Chain2::new(&body.curves2[pc.curve], a, b));
        }
    }
    if chains.is_empty() {
        // Only lone vertices, or no loops at all: the whole surface.
        return Ok(Place::Inside);
    }
    let (pu, pv) = surface.periods();
    let left_is_inside = f.sense == Sense::Forward;
    let ray = Rays {
        chains: &chains,
        periods: [pu, pv],
    };
    for target in targets(&chains) {
        if let Some(left) = ray.first_crossing(uv, target) {
            return Ok(if left == left_is_inside {
                Place::Inside
            } else {
                Place::Outside
            });
        }
    }
    Err(Failure::Uncertain)
}

/// The vertex or edge of the face within its tolerance of `p`, if any.
fn on_boundary(body: &Body, face: FaceId, p: Point3) -> Option<Place> {
    let mut edges = Vec::new();
    for l in body.face_loops(face) {
        if let Some(v) = body.loops[l].vertex {
            let vx = &body.vertices[v];
            if vx.point.distance(p) <= vx.tolerance.get() {
                return Some(Place::OnVertex(v));
            }
        }
        for fin in body.loop_fins(l) {
            let e = body.fins[fin].edge;
            let edge = &body.edges[e];
            for v in [edge.start, edge.end] {
                let vx = &body.vertices[v];
                if vx.point.distance(p) <= vx.tolerance.get() {
                    return Some(Place::OnVertex(v));
                }
            }
            edges.push(e);
        }
    }
    edges
        .into_iter()
        .find(|&e| {
            let edge = &body.edges[e];
            edge_distance(&body.curves[edge.curve], edge.interval, p) <= edge.tolerance.get()
        })
        .map(Place::OnEdge)
}

/// The distance from `p` to a curve over `[t0, t1]`: its nearest point if
/// that lies in the range, else the nearer end (and a NURBS's local
/// minima in the range).
fn edge_distance(c: &Curve, [t0, t1]: [f64; 2], p: Point3) -> f64 {
    let ends = c.point(t0).distance(p).min(c.point(t1).distance(p));
    let foot = c.project(p);
    let t = match c.period() {
        Some(per) => t0 + (foot.param - t0).rem_euclid(per),
        None => foot.param,
    };
    if t0 <= t && t <= t1 {
        return foot.distance.min(ends);
    }
    match c {
        Curve::Nurbs(n) => n
            .local_minima(p)
            .iter()
            .filter(|m| t0 <= m.param && m.param <= t1)
            .map(|m| m.distance)
            .fold(ends, f64::min),
        _ => ends,
    }
}

/// Points of the pcurves to aim rays at: the middle, then a third and two
/// thirds of the way, of each, in that order.
fn targets(chains: &[Chain2]) -> Vec<Point2> {
    let mut out = Vec::new();
    for frac in [0.5, 1.0 / 3.0, 2.0 / 3.0] {
        for c in chains {
            let n = c.pieces.len() as f64;
            let at = frac * n;
            let i = (at.floor() as usize).min(c.pieces.len() - 1);
            let local = at - i as f64;
            let piece = &c.pieces[i];
            let p = if local <= 0.0 {
                piece.pts[0].p
            } else {
                piece.split(local.min(1.0))[1].pts[0].p
            };
            out.push(Point2::ORIGIN + p);
        }
    }
    out
}

struct Rays<'a> {
    chains: &'a [Chain2],
    periods: [Option<f64>; 2],
}

impl Rays<'_> {
    /// Casts a ray from `p` through `target` (moved by whole periods next
    /// to `p`), a little past it. `Some(left)` when its first crossing is
    /// certified, `left` when the pcurve crosses it moving to its left
    /// (so `p` is on the pcurve's left), `None` when the ray settles
    /// nothing.
    fn first_crossing(&self, p: Point2, target: Point2) -> Option<bool> {
        let shift = |x: f64, near: f64, per: Option<f64>| match per {
            Some(q) => x + ((near - x) / q).round() * q,
            None => x,
        };
        let q = Point2::new(
            shift(target.x, p.x, self.periods[0]),
            shift(target.y, p.y, self.periods[1]),
        );
        let d = q - p;
        let len = d.length();
        if len == 0.0 {
            return None;
        }
        let d = d / len;
        let reach = len * 1.01;
        let n = Vector2::new(-d.y, d.x);
        let end = p + d * reach;
        let (lo, hi) = (
            Vector2::new(p.x.min(end.x), p.y.min(end.y)),
            Vector2::new(p.x.max(end.x), p.y.max(end.y)),
        );
        // (along the ray, as an interval; left), and whether anything
        // left unsettled lies in reach.
        let mut hits: Vec<(Interval, bool)> = Vec::new();
        let mut unsettled: Vec<Interval> = Vec::new();
        for c in self.chains {
            for (du, dv) in self.shifts(c, lo, hi) {
                let off = Vector2::new(du, dv);
                for piece in &c.pieces {
                    let (plo, phi) = piece.hull();
                    let (plo, phi) = (plo + off, phi + off);
                    if phi.x < lo.x || plo.x > hi.x || phi.y < lo.y || plo.y > hi.y {
                        continue;
                    }
                    let pt = Interval::point;
                    let at = |k: usize| {
                        let h = piece.pts[k];
                        let x = pt(h.p.x + off.x) - pt(p.x);
                        let y = pt(h.p.y + off.y) - pt(p.y);
                        (x, y, h.w)
                    };
                    let coeffs: Vec<Interval> = (0..piece.pts.len())
                        .map(|k| {
                            let (x, y, w) = at(k);
                            (x * pt(n.x) + y * pt(n.y)) * pt(w)
                        })
                        .collect();
                    let poly = Bernstein::new(coeffs);
                    for r in poly.roots(1e-12) {
                        let [s0, s1] = r.range();
                        // Where along the ray the bracket lies.
                        let along = |s: f64| {
                            let q = if s <= 0.0 {
                                piece.pts[0].p
                            } else if s >= 1.0 {
                                piece.pts[piece.pts.len() - 1].p
                            } else {
                                piece.split(s)[1].pts[0].p
                            };
                            (q + off - p.to_vector()).dot(d)
                        };
                        let (a, b) = (along(s0), along(s1));
                        let span = Interval::new(a.min(b), a.max(b))?;
                        let span =
                            Interval::new(span.lo() - 1e-12 * reach, span.hi() + 1e-12 * reach)?;
                        if span.hi() < 0.0 || span.lo() > reach {
                            continue;
                        }
                        match r {
                            Root::Simple { rising, .. } if span.lo() > 0.0 => {
                                hits.push((span, rising))
                            }
                            _ => unsettled.push(span),
                        }
                    }
                }
            }
        }
        hits.sort_by(|x, y| x.0.lo().total_cmp(&y.0.lo()));
        let (first, left) = *hits.first()?;
        let clear = |x: &Interval| x.lo() > first.hi();
        if hits[1..].iter().all(|h| clear(&h.0)) && unsettled.iter().all(clear) {
            Some(left)
        } else {
            None
        }
    }

    /// The shifts by whole periods that bring chain `c` into the box.
    fn shifts(&self, c: &Chain2, lo: Vector2, hi: Vector2) -> Vec<(f64, f64)> {
        let range = |per: Option<f64>, clo: f64, chi: f64, lo: f64, hi: f64| -> Vec<f64> {
            match per {
                None => vec![0.0],
                Some(q) => {
                    let first = ((lo - chi) / q).ceil() as i64;
                    let last = ((hi - clo) / q).floor() as i64;
                    (first..=last).map(|k| k as f64 * q).collect()
                }
            }
        };
        let us = range(self.periods[0], c.lo.x, c.hi.x, lo.x, hi.x);
        let vs = range(self.periods[1], c.lo.y, c.hi.y, lo.y, hi.y);
        us.iter()
            .flat_map(|&u| vs.iter().map(move |&v| (u, v)))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::Rigid;
    use crate::name::OpKey;
    use crate::primitive;
    use core::f64::consts::{FRAC_PI_2, PI};

    fn face_on(body: &Body, pred: impl Fn(&crate::geom::Surface) -> bool) -> FaceId {
        body.faces()
            .find(|&f| pred(&body.surfaces[body.faces[f].surface]))
            .expect("a face")
    }

    #[test]
    fn a_block_face() {
        let (b, _) = primitive::block(OpKey(1), 2.0, 3.0, 4.0, &Rigid::IDENTITY).unwrap();
        for f in b.faces().collect::<Vec<_>>() {
            let s = &b.surfaces[b.faces[f].surface];
            // The face's centre is inside, a point well off its side is out.
            let centre = b
                .face_loops(f)
                .flat_map(|l| b.loop_fins(l).collect::<Vec<_>>())
                .map(|fin| b.vertices[b.start_of(fin)].point.to_vector())
                .fold(crate::geom::Vector3::ZERO, |a, v| a + v)
                / 4.0;
            let (u, v) = s.project(Point3::ORIGIN + centre).param;
            assert_eq!(point_in_face(&b, f, Point2::new(u, v)), Ok(Place::Inside));
            assert_eq!(
                point_in_face(&b, f, Point2::new(u + 10.0, v)),
                Ok(Place::Outside)
            );
            assert_eq!(
                point_in_face(&b, f, Point2::new(u, v - 7.0)),
                Ok(Place::Outside)
            );
        }
        // On an edge and at a corner.
        let f = b.faces().next().unwrap();
        let fin = b.loop_fins(b.face_loops(f).next().unwrap()).next().unwrap();
        let e = b.fins[fin].edge;
        let mid = {
            let edge = &b.edges[e];
            b.curves[edge.curve].point((edge.interval[0] + edge.interval[1]) / 2.0)
        };
        let s = &b.surfaces[b.faces[f].surface];
        let (u, v) = s.project(mid).param;
        assert_eq!(
            point_in_face(&b, f, Point2::new(u, v)),
            Ok(Place::OnEdge(e))
        );
        let corner = b.edges[e].start;
        let (u, v) = s.project(b.vertices[corner].point).param;
        assert_eq!(
            point_in_face(&b, f, Point2::new(u, v)),
            Ok(Place::OnVertex(corner))
        );
    }

    #[test]
    fn a_cylinder_side_is_a_band() {
        let (b, _) = primitive::cylinder(OpKey(1), 1.0, 2.0, &Rigid::IDENTITY).unwrap();
        let side = face_on(&b, |s| matches!(s, crate::geom::Surface::Cylinder(_)));
        for u in [0.0, 1.0, 3.0, 6.0, 6.25, -2.0, 20.0] {
            assert_eq!(
                point_in_face(&b, side, Point2::new(u, 1.0)),
                Ok(Place::Inside),
                "{u}"
            );
            assert_eq!(
                point_in_face(&b, side, Point2::new(u, 2.5)),
                Ok(Place::Outside)
            );
            assert_eq!(
                point_in_face(&b, side, Point2::new(u, -0.5)),
                Ok(Place::Outside)
            );
        }
    }

    #[test]
    fn spheres_and_tori_have_no_loops() {
        let (b, _) = primitive::sphere(OpKey(1), 1.0, &Rigid::IDENTITY).unwrap();
        let f = b.faces().next().unwrap();
        assert_eq!(
            point_in_face(&b, f, Point2::new(1.0, FRAC_PI_2 - 0.1)),
            Ok(Place::Inside)
        );
        let (b, _) = primitive::torus(OpKey(1), 3.0, 1.0, &Rigid::IDENTITY).unwrap();
        let f = b.faces().next().unwrap();
        assert_eq!(
            point_in_face(&b, f, Point2::new(1.0, PI)),
            Ok(Place::Inside)
        );
    }

    #[test]
    fn a_cone_tip_face() {
        let (b, _) = primitive::cone(OpKey(1), 1.0, 2.0, &Rigid::IDENTITY).unwrap();
        let side = face_on(&b, |s| matches!(s, crate::geom::Surface::Cone(_)));
        let s = b.surfaces[b.faces[side].surface].clone();
        for (z, want) in [
            (0.5, Place::Inside),
            (1.9, Place::Inside),
            (2.5, Place::Outside),
            (-0.5, Place::Outside),
        ] {
            let (u, v) = s.project(Point3::new(0.0, 1.0, z)).param;
            let got = point_in_face(&b, side, Point2::new(u, v)).unwrap();
            assert!(
                got == want || matches!(got, Place::OnVertex(_)) && z > 1.9,
                "{z}: {got:?}"
            );
        }
    }
}
