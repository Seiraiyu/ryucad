//! Edge discretization: each edge's samples, shared by every face it
//! bounds.
//!
//! An edge is split by bisection in parameter order until every segment
//! passes [`curve_segment_ok`] against the edge's curve and
//! [`face_segment_ok`] against each face it bounds, so that a triangle
//! resting on it can always pass. A segment that crosses a periodic face's
//! seam is split exactly there first, so that a face can cut its loops at
//! the seam without inventing points. The end samples are the edge's
//! vertices themselves. A sample's parameters on a face come from the
//! fin's pcurve; only a fin without one inverts the point.

use super::bound::{Targets, curve_segment_ok, face_segment_ok};
use super::uv::{FaceGeom, U};
use crate::geom::{Point2, Point3};
use crate::tol::LINEAR_RESOLUTION;
use crate::topo::{Body, EdgeId, FaceId, FinId};

/// The deepest a segment is bisected: 2^-48 of the edge's interval.
const MAX_DEPTH: u32 = 48;

/// Bisection steps when looking for a seam crossing.
const SEAM_STEPS: u32 = 64;

/// One edge's samples, from its start vertex to its end vertex.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct EdgeSamples {
    pub(crate) params: Vec<f64>,
    pub(crate) points: Vec<Point3>,
    /// Each face the edge bounds, once, in the order of its fins, with the
    /// parameters of every sample on it (each in its face's canonical
    /// range, see [`FaceGeom::invert`]).
    pub(crate) faces: Vec<(FaceId, Vec<Point2>)>,
    /// Faces the edge lies off, farther than its tolerance.
    pub(crate) off: Vec<FaceId>,
    /// True when a segment reached the depth limit without passing.
    pub(crate) stalled: bool,
}

impl EdgeSamples {
    /// The parameters of every sample on face `f`.
    pub(crate) fn uv_on(&self, f: FaceId) -> Option<&[Point2]> {
        self.faces
            .iter()
            .find(|(g, _)| *g == f)
            .map(|(_, uv)| uv.as_slice())
    }
}

#[derive(Clone, Debug)]
struct Sample {
    t: f64,
    p: Point3,
    uv: Vec<Point2>,
    depth: u32,
}

/// `x` moved by whole periods to be as close as possible to `near`.
pub(crate) fn unwrap(x: f64, near: f64, period: Option<f64>) -> f64 {
    match period {
        Some(p) => x + ((near - x) / p).round() * p,
        None => x,
    }
}

/// The parameters of a segment's ends on a face, ready to measure: the
/// second moved by whole periods next to the first, and an end on a
/// collapsed row given the other end's free parameter.
pub(crate) fn segment_uv(g: &FaceGeom, a: Point2, b: Point2) -> [Point2; 2] {
    let mut b = Point2::new(
        unwrap(b.x, a.x, g.periods[0]),
        unwrap(b.y, a.y, g.periods[1]),
    );
    let mut a = a;
    if let Some((k, _)) = g.pole(a) {
        if k == U {
            a.y = b.y;
        } else {
            a.x = b.x;
        }
    } else if let Some((k, _)) = g.pole(b) {
        if k == U {
            b.y = a.y;
        } else {
            b.x = a.x;
        }
    }
    [a, b]
}

/// The seam line a segment crosses strictly inside, on a face: its
/// direction and its unwrapped value.
fn seam_crossing(g: &FaceGeom, a: Point2, b: Point2) -> Option<(usize, f64)> {
    if g.pole(a).is_some() || g.pole(b).is_some() {
        return None;
    }
    let [a, b] = segment_uv(g, a, b);
    for (k, (xa, xb)) in [(a.x, b.x), (a.y, b.y)].into_iter().enumerate() {
        let (Some(period), Some(seam)) = (g.periods[k], g.seam(k)) else {
            continue;
        };
        let (lo, hi) = (xa.min(xb), xa.max(xb));
        // The first seam line above lo.
        let line = seam + ((lo - seam) / period).floor() * period + period;
        let line = if line - period > lo {
            line - period
        } else {
            line
        };
        if line > lo && line < hi {
            return Some((k, line));
        }
    }
    None
}

/// Samples an edge. `extra` are parameters that must be samples (from a
/// face's split requests).
pub(crate) fn discretize(
    b: &Body,
    e: EdgeId,
    geoms: &[Option<FaceGeom>],
    targets: &Targets,
    extra: &[f64],
) -> EdgeSamples {
    let edge = &b.edges[e];
    let curve = &b.curves[edge.curve];
    let [t0, t1] = edge.interval;
    // Each face once, with the fin that puts the edge on it.
    let mut faces: Vec<FaceId> = Vec::new();
    let mut fins: Vec<FinId> = Vec::new();
    for fin in b.edge_fins(e) {
        let f = b.loops[b.fins[fin].loop_].face;
        if !faces.contains(&f) {
            faces.push(f);
            fins.push(fin);
        }
    }
    let geom = |f: FaceId| geoms[f.index()].as_ref().expect("face geometry");
    // The raw parameters of the edge at `t` on the `i`th face, and how far
    // the surface puts them from `p`: from the pcurve when the fin has one.
    let raw = |i: usize, t: f64, p: Point3| -> (Point2, f64) {
        let g = geom(faces[i]);
        match b.fin_uv(fins[i], t) {
            Some(uv) => (uv, g.surface.point(uv.x, uv.y).distance(p)),
            None => {
                let foot = g.surface.project(p);
                (Point2::new(foot.param.0, foot.param.1), foot.distance)
            }
        }
    };
    let mut off = Vec::new();
    let reach = edge.tolerance.get() + LINEAR_RESOLUTION;
    let mut sample = |t: f64, p: Point3, depth: u32| {
        let uv = faces
            .iter()
            .enumerate()
            .map(|(i, &f)| {
                let (uv, d) = raw(i, t, p);
                if d > reach && !off.contains(&f) {
                    off.push(f);
                }
                geom(f).snap(uv)
            })
            .collect();
        Sample { t, p, uv, depth }
    };
    let start = sample(t0, b.vertices[edge.start].point, 0);
    let end = sample(t1, b.vertices[edge.end].point, 0);
    // The right ends still to reach, nearest last.
    let mut stack = vec![end];
    let mut seeds: Vec<f64> = extra
        .iter()
        .copied()
        .filter(|&t| t > t0 && t < t1)
        .collect();
    if edge.start == edge.end {
        // A closed edge needs a polygon with area.
        seeds.extend([1.0, 2.0].map(|k| t0 + (t1 - t0) * k / 3.0));
    }
    seeds.sort_by(f64::total_cmp);
    seeds.dedup();
    for &t in seeds.iter().rev() {
        stack.push(sample(t, curve.point(t), 0));
    }
    let mut out = vec![start];
    let mut stalled = false;
    while let Some(next) = stack.pop() {
        let cur = out.last().expect("started");
        // A segment may run more than half a period round a face, which its
        // ends alone can't tell from the short way round: when its ends are
        // more than a quarter period apart and going by its middle
        // disagrees with the ends, the middle becomes a sample first.
        let far = faces.iter().enumerate().any(|(i, &f)| {
            let g = geom(f);
            (0..2).any(|k| {
                g.periods[k].is_some_and(|period| {
                    let x = |p: Point2| [p.x, p.y][k];
                    let (a, z) = (x(cur.uv[i]), x(next.uv[i]));
                    (unwrap(z, a, Some(period)) - a).abs() > 0.25 * period
                })
            })
        });
        if (cur.depth == 0 && next.depth == 0) || far {
            let t = 0.5 * (cur.t + next.t);
            let mid = sample(t, curve.point(t), 1);
            let long = faces.iter().enumerate().any(|(i, &f)| {
                let g = geom(f);
                (0..2).any(|k| {
                    let Some(period) = g.periods[k] else {
                        return false;
                    };
                    let x = |p: Point2| [p.x, p.y][k];
                    let step = |a: f64, b: f64| unwrap(b, a, Some(period)) - a;
                    let (a, m, z) = (x(cur.uv[i]), x(mid.uv[i]), x(next.uv[i]));
                    (step(a, m) + step(m, z) - step(a, z)).abs() > 0.5 * period
                })
            });
            if long && t > cur.t && t < next.t {
                stack.push(next);
                stack.push(mid);
                continue;
            }
        }
        // A seam crossing becomes a sample first.
        let crossing = faces
            .iter()
            .enumerate()
            .find_map(|(i, &f)| seam_crossing(geom(f), cur.uv[i], next.uv[i]).map(|c| (i, c)));
        if let Some((i, (k, line))) = crossing {
            let g = geom(faces[i]);
            // Which side of the seam line the edge is at `t`, from its
            // exact (unsnapped) parameters.
            let side = |t: f64| {
                let (uv, _) = raw(i, t, curve.point(t));
                let x = [uv.x, uv.y][k];
                let x = unwrap(x, [cur.uv[i].x, cur.uv[i].y][k], g.periods[k]);
                x < line
            };
            let below = side(cur.t);
            let (mut lo, mut hi) = (cur.t, next.t);
            for _ in 0..SEAM_STEPS {
                let t = 0.5 * (lo + hi);
                if t <= lo || t >= hi {
                    break;
                }
                if side(t) == below {
                    lo = t;
                } else {
                    hi = t;
                }
            }
            let t = 0.5 * (lo + hi);
            // Over a segment more than half a period long, the side test
            // (unwrapped next to its start) flips half a period away
            // rather than at the seam: then the segment is halved first.
            let (uv, _) = raw(i, t, curve.point(t));
            let period = g.periods[k].expect("periodic");
            let x = unwrap([uv.x, uv.y][k], line, Some(period));
            if (x - line).abs() > 1e-6 * period {
                let half = 0.5 * (cur.t + next.t);
                let depth = cur.depth.max(next.depth) + 1;
                if half > cur.t && half < next.t && depth <= MAX_DEPTH {
                    let mut mid = sample(half, curve.point(half), depth);
                    mid.depth = depth;
                    stack.push(next);
                    stack.push(mid);
                    continue;
                }
            }
            let mut mid = sample(t, curve.point(t), 0);
            // Exactly on the seam on that face.
            let seam = g.seam(k).expect("periodic");
            if k == U {
                mid.uv[i].x = seam;
            } else {
                mid.uv[i].y = seam;
            }
            mid.depth = cur.depth.max(next.depth) + 1;
            if mid.t > cur.t && mid.t < next.t && mid.depth <= MAX_DEPTH {
                stack.push(next);
                stack.push(mid);
                continue;
            }
        }
        let ok = curve_segment_ok(curve, [cur.t, next.t], [cur.p, next.p], targets)
            && faces.iter().enumerate().all(|(i, &f)| {
                let g = geom(f);
                let uv = segment_uv(g, cur.uv[i], next.uv[i]);
                face_segment_ok(g.surface, g.metric, uv, [cur.p, next.p], targets)
            });
        let depth = cur.depth.max(next.depth) + 1;
        if ok || depth > MAX_DEPTH {
            stalled |= !ok;
            out.push(next);
        } else {
            let t = 0.5 * (cur.t + next.t);
            let mut mid = sample(t, curve.point(t), depth);
            mid.depth = depth;
            stack.push(next);
            stack.push(mid);
        }
    }
    // Two edges making a loop of their own (a sliver between an edge
    // and a curve close beside it) each within the chord unsampled would
    // make a polygon with no area: such an edge gets its middle.
    if out.len() == 2
        && edge.start != edge.end
        && fins
            .iter()
            .any(|&fin| b.loop_fins(b.fins[fin].loop_).count() == 2)
    {
        let t = 0.5 * (t0 + t1);
        out.insert(1, sample(t, curve.point(t), 1));
    }
    EdgeSamples {
        params: out.iter().map(|s| s.t).collect(),
        points: out.iter().map(|s| s.p).collect(),
        faces: faces
            .iter()
            .enumerate()
            .map(|(i, &f)| (f, out.iter().map(|s| s.uv[i]).collect()))
            .collect(),
        off,
        stalled,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::geom::{Cylinder, Frame, Rigid, Surface, Vector3};
    use crate::name::OpKey;
    use crate::primitive;
    use crate::topo::Sense;
    use core::f64::consts::TAU;

    pub(crate) fn targets(chord: f64, angle: f64) -> Targets {
        Targets {
            chord,
            angle,
            max_edge: f64::INFINITY,
            waive: chord,
        }
    }

    pub(crate) fn geoms(b: &Body) -> Vec<Option<FaceGeom<'_>>> {
        let mut out = vec![None; b.faces.slot_count()];
        for f in b.faces() {
            out[f.index()] = Some(FaceGeom::new(b, f));
        }
        out
    }

    #[test]
    fn a_circle_on_a_cylinder() {
        let (b, _) = primitive::cylinder(OpKey(1), 1.0, 2.0, &Rigid::IDENTITY).unwrap();
        let g = geoms(&b);
        let t = targets(1e-3, 0.5);
        for e in b.edges() {
            let s = discretize(&b, e, &g, &t, &[]);
            assert!(!s.stalled && s.off.is_empty());
            let v = b.vertices[b.edges[e].start].point;
            assert_eq!((s.points[0], *s.points.last().unwrap()), (v, v));
            // The side and one disk.
            assert_eq!(s.faces.len(), 2);
            let n = s.params.len() - 1;
            // Each chord's sagitta 1 - cos(dt / 2) is under half the chord
            // tolerance, and the face pre-refinement halves that again:
            // M h² / 8 <= chord / 4 with M = 1 needs dt <= √(2e-3) / ... .
            for w in s.params.windows(2) {
                let dt = w[1] - w[0];
                assert!(1.0 - crate::math::cos(dt / 2.0) <= t.chord / 2.0);
            }
            assert!((50..400).contains(&n), "{n} segments");
            // On the side, every sample keeps its height.
            let side = s.uv_on(b.faces().next().unwrap()).unwrap();
            assert!(side.iter().all(|p| (p.y - side[0].y).abs() <= 1e-12));
        }
    }

    #[test]
    fn straight_edges_on_planes_need_no_samples() {
        let (b, _) = primitive::block(OpKey(1), 1.0, 2.0, 3.0, &Rigid::IDENTITY).unwrap();
        let g = geoms(&b);
        for e in b.edges() {
            let s = discretize(&b, e, &g, &targets(1e-3, 0.5), &[]);
            assert_eq!(s.params.len(), 2);
            let short = Targets {
                max_edge: 0.3,
                ..targets(1e-3, 0.5)
            };
            let s = discretize(&b, e, &g, &short, &[]);
            let len = b.vertices[b.edges[e].start]
                .point
                .distance(b.vertices[b.edges[e].end].point);
            // Bisection: the least power of two pieces short enough.
            let pieces = (len / 0.3).ceil() as usize;
            assert_eq!(s.params.len() - 1, pieces.next_power_of_two());
            // Requested samples are kept.
            let [t0, t1] = b.edges[e].interval;
            let mid = t0 + (t1 - t0) / 3.0;
            let s = discretize(&b, e, &g, &targets(1.0, 1.5), &[mid]);
            assert_eq!(s.params, vec![t0, mid, t1]);
        }
    }

    /// A cylinder whose side's seam is turned away from its vertices.
    pub(crate) fn turned_seam(turn: f64) -> Body {
        let (mut b, _) = primitive::cylinder(OpKey(1), 1.0, 2.0, &Rigid::IDENTITY).unwrap();
        let side = b.faces().next().unwrap();
        let frame = Frame::new(
            Point3::ORIGIN,
            Vector3::new(0.0, 0.0, 1.0),
            Vector3::new(crate::math::cos(turn), crate::math::sin(turn), 0.0),
        )
        .unwrap();
        let s = b.add_surface(Surface::Cylinder(Cylinder::new(frame, 1.0).unwrap()));
        crate::euler::set_surface(&mut b, side, s, Sense::Forward).unwrap();
        b
    }

    #[test]
    fn seam_crossings_become_samples() {
        let b = turned_seam(1.0);
        let g = geoms(&b);
        let side = b.faces().next().unwrap();
        for e in b.edges() {
            let s = discretize(&b, e, &g, &targets(1e-2, 0.5), &[]);
            let uv = s.uv_on(side).unwrap();
            // The vertex is at angle -1 on the side, the seam at 0.
            assert!((uv[0].x - (TAU - 1.0)).abs() <= 1e-12);
            let on_seam: Vec<_> = uv.iter().filter(|p| p.x == 0.0).collect();
            assert_eq!(on_seam.len(), 1, "one sample exactly on the seam");
            // It is a genuine point of the circle at the seam.
            let k = uv.iter().position(|p| p.x == 0.0).unwrap();
            let p = s.points[k];
            assert!(
                (p - Point3::new(crate::math::cos(1.0), crate::math::sin(1.0), p.z)).length()
                    <= 1e-15
            );
        }
    }

    #[test]
    fn edges_off_a_face_are_reported() {
        let (mut b, _) = primitive::cylinder(OpKey(1), 1.0, 2.0, &Rigid::IDENTITY).unwrap();
        let side = b.faces().next().unwrap();
        let s = b.add_surface(Surface::Cylinder(
            Cylinder::new(Frame::WORLD, 1.01).unwrap(),
        ));
        crate::euler::set_surface(&mut b, side, s, Sense::Forward).unwrap();
        let g = geoms(&b);
        for e in b.edges() {
            let s = discretize(&b, e, &g, &targets(1e-2, 0.5), &[]);
            assert_eq!(s.off, vec![side]);
        }
    }
}
