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

#[cfg(test)]
use super::bound::triangle_ok;
use super::bound::{Targets, Verdict, triangle_test};
use super::cdt::{NONE, next, prev};
use super::domain::{Anchor, Domain, Side};
use super::uv::{FaceGeom, U, V};
use crate::geom::{Point2, Point3};
use crate::predicates::{Sign, orient2d};
use crate::tol::TOUCH_TOLERANCE;
use crate::topo::EdgeId;
use crate::work::Meter;
use std::collections::{BTreeMap, VecDeque};

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
    /// Slivers split by their other edges so far (see [`SLIVER_SPLITS`]).
    pub(crate) slivers: u32,
}

/// The most slivers a face's refinement splits by their other edges
/// before it is called stalled. `nist_ctc_05`'s face 68 needs 9 in all at
/// mass's chord; a face that keeps making such slivers (heal_props' fault
/// seed 393 in shell facing: 232,741 in 70 s) would otherwise refine
/// without end where it used to stall at once.
const SLIVER_SPLITS: u32 = 64;

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
/// may have. An inner edge joining the boundary points in `split` (sorted;
/// see [`chords`]) is split however well its triangles fit.
#[cfg(test)]
pub(crate) fn refine(
    d: &mut Domain,
    g: &FaceGeom,
    t: &Targets,
    fixed: &dyn Fn(Anchor) -> Point3,
    cap: u64,
    split: &[Chord],
) -> Refined {
    // unmetered: a test
    refine_in(d, g, t, fixed, &|_| 0.0, cap, split, &Meter::free())
}

/// [`refine`] charging `m` a unit per triangle tested: a halt stalls it.
/// `tube` gives each boundary anchor the tolerance within which a
/// triangle at it is held to looser targets (see [`in_tube`]).
#[allow(clippy::too_many_arguments)]
pub(crate) fn refine_in(
    d: &mut Domain,
    g: &FaceGeom,
    t: &Targets,
    fixed: &dyn Fn(Anchor) -> Point3,
    tube: &dyn Fn(Anchor) -> f64,
    cap: u64,
    split: &[Chord],
    m: &Meter,
) -> Refined {
    let mut out = Refined::default();
    for _ in 0..PINCH_ROUNDS {
        if let Some(stop) = refine_pass(d, g, t, fixed, tube, cap, split, &mut out, m) {
            out.stop = Some(stop);
            return out;
        }
        let pinches = pinched(d);
        if pinches.is_empty() {
            return out;
        }
        let mut flipped = false;
        for (x, a) in pinches {
            if d.cdt.flip_edge(x, a) {
                flipped = true;
                continue;
            }
            // Not convex: split the edge instead, which also takes x off
            // the pole there.
            let Some((tri, i)) = d.cdt.find_edge(x, a).or_else(|| d.cdt.find_edge(a, x)) else {
                continue;
            };
            if d.cdt.is_fixed(tri, i) {
                continue;
            }
            let before = d.cdt.triangle_count();
            if split_inner(d, g, tri, i).is_some() {
                flipped = true;
                for t in before..d.cdt.triangle_count() {
                    let inside = d.inside[d.cdt.origin(t as u32) as usize];
                    d.inside.push(inside);
                }
            }
        }
        if !flipped {
            break;
        }
    }
    out.stop = Some(Stop::Stalled);
    out
}

/// Rounds of unpinching and refining again.
const PINCH_ROUNDS: usize = 16;

/// Which position a vertex will weld to: its body anchor, or its class.
fn weld(d: &Domain, v: u32) -> (u8, u32, u32) {
    match d.classes.anchor(v) {
        Anchor::Vertex(x) => (0, x.index() as u32, 0),
        Anchor::Sample(e, k) => (1, e.index() as u32, k),
        Anchor::Free => (2, d.classes.root(v), 0),
    }
}

/// Edges to flip so that no vertex is pinched. Going round a vertex, the
/// neighbours that weld to any one position must come in one run: two
/// runs would make the welded edge to that position non-manifold. It
/// happens at a pole (whose side welds to one point), where the scaled
/// plane stretches the surface sideways so that Delaunay triangles can
/// reach past a nearer point to the pole twice, and across a seam when
/// triangles are wide enough to touch both of its sides. All but the
/// longest run are flipped away; each flip (or split, where a flip
/// can't be made) takes one edge from that position, so the repair ends.
fn pinched(d: &Domain) -> Vec<(u32, u32)> {
    let mut out = Vec::new();
    for x in 0..d.uv.len() as u32 {
        let (fan, closed) = d.cdt.fan(x);
        // The neighbours counterclockwise, None across outside triangles.
        let mut ring: Vec<Option<u32>> = Vec::new();
        // Each triangle adds the neighbour it shares with the next one; an
        // open fan starts with its first triangle's other neighbour.
        for (k, &(t, i)) in fan.iter().enumerate() {
            let c = d.cdt.corners(t);
            let inside = d.inside[t as usize];
            if !closed && k == 0 {
                ring.push(inside.then_some(c[prev(i)]));
            }
            ring.push(inside.then_some(c[next(i)]));
        }
        let n = ring.len();
        if n == 0 || ring.iter().all(Option::is_none) {
            continue;
        }
        // Neighbours welding to x's own position (along a pole side) don't
        // count.
        let own = weld(d, x);
        let key = |k: usize| ring[k].map(|v| weld(d, v)).filter(|&w| w != own);
        // Start where the key changes, so that no run wraps.
        let start = if closed {
            match (0..n).find(|&k| key(k) != key((k + n - 1) % n)) {
                Some(k) => k,
                None => continue,
            }
        } else {
            0
        };
        // Runs of one key, as index lists.
        let mut runs: BTreeMap<(u8, u32, u32), Vec<Vec<usize>>> = BTreeMap::new();
        let mut j = 0;
        while j < n {
            let k = (start + j) % n;
            let Some(w) = key(k) else {
                j += 1;
                continue;
            };
            let mut run = vec![k];
            while j + 1 < n && key((start + j + 1) % n) == Some(w) {
                j += 1;
                run.push((start + j) % n);
            }
            runs.entry(w).or_default().push(run);
            j += 1;
        }
        for list in runs.values().filter(|l| l.len() > 1) {
            let keep =
                (0..list.len()).fold(0, |b, r| if list[r].len() > list[b].len() { r } else { b });
            for (r, run) in list.iter().enumerate() {
                if r != keep {
                    out.extend(run.iter().map(|&k| (x, ring[k].expect("a neighbour"))));
                }
            }
        }
    }
    out
}

/// The targets a triangle in the tube of a tolerant boundary point is
/// held to, when it is one: its model-space diameter is within the
/// tolerance `tube` gives one of its corners' anchors (past
/// `TOUCH_TOLERANCE`) plus the waiver's length. It is held to the chord
/// plus that tolerance, and its turn is waived as one under the chord is.
/// Where two tolerant edges' pcurves end apart at their vertex, the corner
/// triangle between them keeps its size however finely the edges are
/// sampled (#465), and the surface there is the face's only to within the
/// tube anyway: the mesher's guarantee is the chord plus the local
/// tolerance near tolerant edges. Exact bodies' tolerances never pass
/// `TOUCH_TOLERANCE`, so their meshes don't change.
fn in_tube(
    d: &Domain,
    corners: [u32; 3],
    xyz: [Point3; 3],
    t: &Targets,
    tube: &dyn Fn(Anchor) -> f64,
) -> Option<Targets> {
    let reach = corners
        .iter()
        .map(|&v| tube(d.classes.anchor(v)))
        .fold(0.0, f64::max);
    if reach <= TOUCH_TOLERANCE {
        return None;
    }
    let size = (0..3).fold(0.0f64, |m, i| m.max(xyz[i].distance(xyz[(i + 1) % 3])));
    (size <= reach + t.waive).then_some(Targets {
        chord: t.chord + reach,
        waive: t.waive + reach,
        ..*t
    })
}

/// One pass of refinement over the inside triangles.
#[allow(clippy::too_many_arguments)]
fn refine_pass(
    d: &mut Domain,
    g: &FaceGeom,
    t: &Targets,
    fixed: &dyn Fn(Anchor) -> Point3,
    tube: &dyn Fn(Anchor) -> f64,
    cap: u64,
    split: &[Chord],
    out: &mut Refined,
    m: &Meter,
) -> Option<Stop> {
    let mut stop = None;
    let mut count = d.inside.iter().filter(|&&x| x).count() as u64;
    let mut queue: VecDeque<u32> = (0..d.cdt.triangle_count() as u32)
        .filter(|&t| d.inside[t as usize])
        .collect();
    let mut queued: Vec<bool> = d.inside.clone();
    let mut pos: Vec<Option<Point3>> = vec![None; d.uv.len()];
    // Below this length, both in the scaled plane and in model space, an
    // edge is not split. The scaled plane alone can shrink an edge far
    // below its length: near a cone's apex, scaled by the face's smallest
    // radius, an edge across the face may be many times longer than it
    // looks there.
    let floor = t.waive / 16.0;
    while let Some(tri) = queue.pop_front() {
        // A triangle tested.
        if m.charge(1).is_err() {
            return Some(Stop::Stalled);
        }
        queued[tri as usize] = false;
        let corners = d.cdt.corners(tri);
        pos.resize(d.uv.len(), None);
        let xyz = corners.map(|v| *pos[v as usize].get_or_insert_with(|| position(d, g, v, fixed)));
        let uv = corners.map(|v| d.uv[v as usize]);
        // An inner edge to split whatever the triangle's fit: opposite
        // corner i.
        let chord = (0..3).find(|&i| {
            !split.is_empty()
                && !d.cdt.is_fixed(tri, i)
                && chord_of(d, corners[next(i)], corners[prev(i)])
                    .is_some_and(|c| split.binary_search(&c).is_ok())
        });
        let verdict = match chord {
            Some(_) => Verdict::Long,
            None => match triangle_test(g.surface, g.metric, g.stretched(), uv, xyz, t) {
                Verdict::Pass => Verdict::Pass,
                v => match in_tube(d, corners, xyz, t, tube) {
                    Some(wide) => triangle_test(g.surface, g.metric, g.stretched(), uv, xyz, &wide),
                    None => v,
                },
            },
        };
        if verdict == Verdict::Pass {
            continue;
        }
        if count > cap {
            return Some(Stop::TooLarge);
        }
        let scaled = uv.map(|p| g.metric.scale(p));
        let len = |i: usize| scaled[next(i)].distance(scaled[prev(i)]);
        let i = chord.unwrap_or_else(|| edge_to_split(verdict, uv, g.metric));
        // A shared chord is split once whatever its length: its midpoint
        // is free, so neither half is a chord again. Two faces meeting
        // across a neck thinner than the floor draw such chords too.
        let short = |i: usize| len(i) < floor && xyz[next(i)].distance(xyz[prev(i)]) < floor;
        if chord.is_none() && short(i) {
            stop = Some(Stop::Stalled);
            continue;
        }
        let (b, c) = (corners[next(i)], corners[prev(i)]);
        let before = d.cdt.triangle_count();
        let made = if !d.cdt.is_fixed(tri, i) {
            split_inner(d, g, tri, i).or_else(|| {
                if out.slivers >= SLIVER_SPLITS {
                    return None;
                }
                out.slivers += 1;
                // The edge's midpoint, rounded, is a vertex already: the
                // corner opposite it, a sliver lying along the edge
                // (`nist_ctc_05`'s spline face 68 at mass's chord, #337).
                // Split one of its other inner edges, the longer first,
                // whose midpoints lie off the vertices; the sliver's long
                // edge then has a corner of its own to split at. Only a
                // triangle that would stall its face comes here, so every
                // face that meshed meshes as it did; and at most
                // SLIVER_SPLITS times a face.
                let mut others = [next(i), prev(i)];
                if len(others[1]) > len(others[0]) {
                    others.swap(0, 1);
                }
                others.into_iter().find_map(|j| {
                    (!d.cdt.is_fixed(tri, j) && !short(j))
                        .then(|| split_inner(d, g, tri, j))
                        .flatten()
                })
            })
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
            stop = Some(Stop::Stalled);
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
    stop
}

/// Which edge of a failing triangle to split, by the corner opposite it:
/// the longest in the scaled plane; or, where only the normal's turn
/// fails, the edge it turns most along (`turn_u |Δu| + turn_v |Δv|`). In
/// a stretched plane those can differ, and splitting the longest edge of
/// a triangle that turns too far across it splits it the wrong way, down
/// to the floor (#255). Unbounded rates fall back on the lengths.
fn edge_to_split(verdict: Verdict, uv: [Point2; 3], metric: super::bound::Metric) -> usize {
    let scaled = uv.map(|p| metric.scale(p));
    let len = |i: usize| scaled[next(i)].distance(scaled[prev(i)]);
    let weight = |i: usize| match verdict {
        Verdict::Turned { turn_u, turn_v } => {
            let (p, q) = (uv[next(i)], uv[prev(i)]);
            let w = turn_u * (p.x - q.x).abs() + turn_v * (p.y - q.y).abs();
            if w.is_finite() { w } else { len(i) }
        }
        _ => len(i),
    };
    (0..3).fold(0, |best, i| if weight(i) > weight(best) { i } else { best })
}

/// A boundary point's identity across faces: a body vertex, or an edge
/// sample by edge and index.
pub(crate) type Point = (u8, u32, u32);

/// An inner mesh edge between two boundary points, the lesser first.
pub(crate) type Chord = [Point; 2];

fn point_of(d: &Domain, v: u32) -> Option<Point> {
    match d.classes.anchor(v) {
        Anchor::Vertex(x) => Some((0, x.index() as u32, 0)),
        Anchor::Sample(e, k) => Some((1, e.index() as u32, k)),
        Anchor::Free => None,
    }
}

fn chord_of(d: &Domain, a: u32, b: u32) -> Option<Chord> {
    let (p, q) = (point_of(d, a)?, point_of(d, b)?);
    Some([p.min(q), p.max(q)])
}

/// The face's inside mesh edges that join two boundary points, sorted,
/// each with whether it is a loop segment.
///
/// Where two faces meet in a wedge thinner than the chord (an edge
/// bending into both, or two edges meeting at a shallow angle), each
/// fills the sliver beside the boundary with triangles of boundary points
/// alone, and one may draw an edge across it that the other draws too, as
/// a chord or as a loop segment: the welded mesh then uses it more than
/// twice. Such a chord is split in every face it is a chord of
/// ([`refine`]'s `split`), each putting a point of its own surface on it.
pub(crate) fn chords(d: &Domain) -> Vec<(Chord, bool)> {
    let mut out = Vec::new();
    for tri in 0..d.cdt.triangle_count() as u32 {
        if !d.inside[tri as usize] {
            continue;
        }
        let c = d.cdt.corners(tri);
        for i in 0..3 {
            if let Some(chord) = chord_of(d, c[next(i)], c[prev(i)]) {
                out.push((chord, d.cdt.is_fixed(tri, i)));
            }
        }
    }
    out.sort();
    out.dedup();
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
                let r = refine(&mut d, geom, &t, &fixed, 1_000_000, &[]);
                for tri in 0..d.cdt.triangle_count() as u32 {
                    if !d.inside[tri as usize] {
                        continue;
                    }
                    let c = d.cdt.corners(tri);
                    let xyz = c.map(|v| position(&d, geom, v, &fixed));
                    let uv = c.map(|v| d.uv[v as usize]);
                    assert!(triangle_ok(
                        geom.surface,
                        geom.metric,
                        geom.stretched(),
                        uv,
                        xyz,
                        &t
                    ));
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
    fn a_turn_alone_splits_the_edge_it_turns_along() {
        use crate::mesh::bound::Metric;
        // Long along u in the plane, turning only along v (#255's pipe
        // face, its plane shortened round the tube).
        let uv = [
            Point2::new(0.0, 0.0),
            Point2::new(1.0, 0.1),
            Point2::new(0.4, 0.3),
        ];
        let m = Metric { a: 1.0, b: 1.0 };
        // Too far from the surface: the longest edge, opposite corner 2.
        assert_eq!(edge_to_split(Verdict::Long, uv, m), 2);
        // Turning too far along v alone: the edge that climbs most in v,
        // from (0.4, 0.3) to (0, 0), opposite corner 1.
        let turned = Verdict::Turned {
            turn_u: 0.0,
            turn_v: 1.0,
        };
        assert_eq!(edge_to_split(turned, uv, m), 1);
        // Unbounded rates fall back on the plane's lengths.
        let unbounded = Verdict::Turned {
            turn_u: f64::INFINITY,
            turn_v: 1.0,
        };
        assert_eq!(edge_to_split(unbounded, uv, m), 2);
    }

    #[test]
    fn a_tiny_budget_stops_early() {
        let (b, _) = primitive::sphere(OpKey(1), 1.0, &Rigid::IDENTITY).unwrap();
        let g = geoms(&b);
        let f = b.faces().next().unwrap();
        let geom = g[f.index()].as_ref().unwrap();
        let mut d = Domain::new(geom, &gather(&b, geom, &[])).unwrap();
        let none = |_: Anchor| unreachable!();
        let r = refine(&mut d, geom, &targets(1e-4, 0.2), &none, 50, &[]);
        assert_eq!(r.stop, Some(Stop::TooLarge));
    }
}
