//! Faces beside each other: which side of a surface a face lies on, or
//! that it keeps off it, away from the curves where the two touch.
//!
//! Where two faces meet tangentially along an edge they share (a fillet
//! and the face it rolls on, along its rail; two blends across a section)
//! their surfaces' intersection is singular all along it, and no trace
//! can follow it. What the self-intersection check needs instead is that
//! the faces meet nowhere else: that one face, parameterized by a
//! [`Sheet`], has one certified sign in the other's implicit form `f`
//! everywhere except a strip round the touching curves. `F = f ∘ P` is
//! enclosed over cells of the face's parameter box (see
//! [`super::pipe::Field`]), halved where it can't be signed, and a cell
//! wholly within the strip is left out. A surface with no implicit form
//! has no side to be on: each cell is certified apart from it instead,
//! every cell of it near the face's cell excluded from that cell as pairs
//! are in a trace ([`super::general::apart`]).

use super::Failure;
use super::fit::first_bound;
use super::implicit::Implicit;
use super::inface::{Place, point_in_face};
use super::pipe::{Cell, Field, signed};
use crate::geom::chain2::Chain2;
use crate::geom::sheet::Sheet;
use crate::geom::{Box2, Curve2, Point2, Point3, Surface, Vector2};
use crate::predicates::Sign;
use crate::topo::{Body, EdgeId, FaceId, Pcurve, VertexId};
use std::collections::VecDeque;

/// The most cells [`beside`] may examine.
const CELLS: usize = 1 << 15;

/// The most cells of another surface with no implicit form [`beside`] may
/// examine, in all, while certifying parts apart from it.
const APART_CELLS: usize = 1 << 20;

/// The deepest [`beside`] halves a cell, one parameter at a time.
const DEPTH: u32 = 64;

/// How many pieces each touching curve's pcurve is cut into first.
const PIECES: u32 = 256;

/// How many times each first piece may be halved, where it lies across
/// both parameters wider than half the strip.
const PIECE_DEPTH: u32 = 8;

/// Which side of the surface `other` a face of `surface` lies on, away
/// from curves where the two touch.
///
/// `cell` is the face's box in `surface`'s parameter plane, and each of
/// `rails` a touching curve's pcurve on `surface` over its parameter
/// range. Every part of the box lying wholly within `strip` (a length in
/// model space) of a rail is left out; on the rest, `F = f ∘ P` must have
/// one certified sign. With `Lu` and `Lv` bounds on `|P_u|` and `|P_v|`
/// over the box (closed forms, a pipe's from its frame along the spine),
/// a part is within the strip when the rail's pieces (256 of them, each
/// halved while its box is wider across both parameters than half the
/// strip there, by their boxes) whose every `v` lies within `strip / Lv` of
/// every `v` of the part cover its `u` range between them: at every `u`
/// of the part a rail passes (each piece is continuous) at a `v` the
/// part's points are within `Lv |Δv| ≤ strip` of; or the same with `u`
/// and `v` exchanged. A periodic direction's pieces count moved by whole
/// periods too, since pcurves unwrap across seams. The parts are halved along
/// whichever parameter adds more to `F`'s enclosure, so the strip needs
/// narrow parts across it only.
///
/// `Ok(Some(sign))` is that sign (`Positive` on the side `other`'s
/// implicit form is positive: outside a cylinder, sphere or torus), or
/// `Zero` when the whole box is within the strip, or when `other` has no
/// implicit form and every part is certified apart from it; `Ok(None)`
/// when parts of both signs are certified, so the face crosses `other`
/// away from the strip. Fails with `Uncertain` where a part can't be
/// signed or certified apart.
pub fn beside(
    surface: &Surface,
    other: &Surface,
    cell: Cell,
    rails: &[(&Curve2, [f64; 2])],
    strip: f64,
) -> Result<Option<Sign>, Failure> {
    Ok(
        match run(surface, other, cell, (rails, &[]), strip, &|_| true, true)? {
            [false, true] => Some(Sign::Positive),
            [true, false] => Some(Sign::Negative),
            [false, false] => Some(Sign::Zero),
            [true, true] => None,
        },
    )
}

/// [`beside`] for a face of a body, touching `other` along `edges` of
/// its own and at `vertices` (the strip round a vertex is the ball of its
/// radius): the box is the face's in its surface's parameter plane, the
/// rails are the edges' pcurves on it, and parts of the box certified
/// outside the face are left out too. A part is outside when the face's
/// boundary doesn't pass through it (each fin's pcurve, by the boxes of
/// its halves down to where they miss the part, moved by whole periods)
/// and its middle is outside the face ([`point_in_face`]).
///
/// Every other part is signed, so `Ok` certifies that the face meets
/// `other` nowhere outside the strip; it gives which signs were seen
/// (`[negative, positive]`), each part's sign holding over all of it,
/// though a part across the boundary may lie partly outside the face.
/// Fails with `Uncertain` where a fin has no pcurve or a part can't be
/// signed.
pub(crate) fn face_beside(
    body: &Body,
    face: FaceId,
    other: &Surface,
    edges: &[EdgeId],
    vertices: &[VertexId],
    strip: f64,
) -> Result<[bool; 2], Failure> {
    let surface = &body.surfaces[body.faces[face].surface];
    let fins: Vec<(EdgeId, Option<Pcurve>)> = body
        .face_loops(face)
        .flat_map(|l| {
            body.loop_fins(l)
                .map(|fin| (body.fins[fin].edge, body.fins[fin].pcurve))
                .collect::<Vec<_>>()
        })
        .collect();
    let mut rails = Vec::new();
    let mut boundary = Vec::new();
    for &(e, pc) in &fins {
        let pc = pc.ok_or(Failure::Uncertain)?;
        let [s0, s1] = body.edges[e].interval.map(|t| pc.param(t));
        let c = (&body.curves2[pc.curve], [s0.min(s1), s0.max(s1)]);
        if edges.contains(&e) {
            rails.push(c);
        }
        boundary.push(c);
    }
    if rails.is_empty() && vertices.is_empty() {
        return Err(Failure::Uncertain);
    }
    let points: Vec<Point3> = vertices.iter().map(|&v| body.vertices[v].point).collect();
    let (pu, pv) = surface.periods();
    let keep = |c: Cell| {
        let crossed = boundary
            .iter()
            .any(|&(curve, range)| meets(curve, range, c, (pu, pv), BOUNDARY_DEPTH));
        let [[u0, u1], [v0, v1]] = c;
        let middle = Point2::new((u0 + u1) / 2.0, (v0 + v1) / 2.0);
        crossed || !matches!(point_in_face(body, face, middle), Ok(Place::Outside))
    };
    run(
        surface,
        other,
        parameter_box(body, face),
        (&rails, &points),
        strip,
        &keep,
        false,
    )
}

/// A face's box in its surface's parameter plane: its pcurves', or the
/// surface's domain for a face with no loops. Along a periodic direction
/// it is the shortest stretch holding every pcurve's range moved by whole
/// periods, so a face across the seam (a quarter of a torus's tube about
/// `v = 0`) isn't taken round the whole period.
pub(crate) fn parameter_box(body: &Body, f: FaceId) -> Cell {
    let surface = &body.surfaces[body.faces[f].surface];
    let mut ranges: [Vec<[f64; 2]>; 2] = [Vec::new(), Vec::new()];
    for l in body.face_loops(f) {
        if let Some(x) = body.loops[l].vertex {
            let (pu, pv) = surface.project(body.vertices[x].point).param;
            ranges[0].push([pu, pu]);
            ranges[1].push([pv, pv]);
        }
        for fin in body.loop_fins(l) {
            if let Some(pc) = body.fins[fin].pcurve {
                let [s0, s1] = body.edges[body.fins[fin].edge]
                    .interval
                    .map(|t| pc.param(t));
                let b = body.curves2[pc.curve].bounds([s0.min(s1), s0.max(s1)]);
                ranges[0].push([b.min().x, b.max().x]);
                ranges[1].push([b.min().y, b.max().y]);
            }
        }
    }
    if ranges[0].is_empty() {
        return surface.domain();
    }
    let (pu, pv) = surface.periods();
    [circular_hull(&ranges[0], pu), circular_hull(&ranges[1], pv)]
}

/// The shortest stretch holding every range, each moved by whole periods
/// where there is a period.
fn circular_hull(ranges: &[[f64; 2]], period: Option<f64>) -> [f64; 2] {
    let plain = ranges
        .iter()
        .fold([f64::INFINITY, f64::NEG_INFINITY], |[a, b], r| {
            [a.min(r[0]), b.max(r[1])]
        });
    let Some(p) = period else {
        return plain;
    };
    if ranges.iter().any(|r| r[1] - r[0] >= p) {
        return plain;
    }
    // Each range by its start in [0, p), merged where they overlap; the
    // hull runs round from the end of the widest gap.
    let mut arcs: Vec<[f64; 2]> = ranges
        .iter()
        .map(|r| {
            let a = r[0].rem_euclid(p);
            [a, a + (r[1] - r[0])]
        })
        .collect();
    arcs.sort_by(|a, b| a[0].total_cmp(&b[0]));
    let mut merged: Vec<[f64; 2]> = Vec::new();
    for a in arcs {
        match merged.last_mut() {
            Some(m) if a[0] <= m[1] => m[1] = m[1].max(a[1]),
            _ => merged.push(a),
        }
    }
    let n = merged.len();
    let (mut best, mut start) = (f64::NEG_INFINITY, merged[0][0]);
    for i in 0..n {
        let next = if i + 1 < n {
            merged[i + 1][0]
        } else {
            merged[0][0] + p
        };
        let gap = next - merged[i][1];
        if gap > best {
            best = gap;
            start = next;
        }
    }
    if best <= 0.0 {
        return plain;
    }
    let lo = start;
    let hi = start + p - best;
    // In the period the first range lies in, so the box meets the face's
    // own pcurves.
    let shift = ((plain[0] - lo) / p).round() * p;
    [lo + shift, hi + shift]
}

/// The box of a plane curve's exact Bézier pieces' control hulls over a
/// range (not the whole curve's, as [`Curve2::bounds`] gives a circle),
/// grown by `1e-12` of its size so that the boxes of neighbouring ranges,
/// whose hulls are rounded apart, meet.
fn piece_box(c: &Curve2, [a, b]: [f64; 2]) -> Box2 {
    let (lo, hi) = if a == b {
        let p = c.point(a).to_vector();
        (p, p)
    } else {
        let chain = Chain2::new(c, a, b);
        (chain.lo, chain.hi)
    };
    let k = 1e-12 * (1.0 + lo.x.abs().max(lo.y.abs()).max(hi.x.abs()).max(hi.y.abs()));
    let g = Vector2::new(k, k);
    Box2::new(Point2::ORIGIN + (lo - g), Point2::ORIGIN + (hi + g))
}

/// How many times a piece of a face's boundary is halved, at most, to
/// find whether it meets a part of the box.
const BOUNDARY_DEPTH: u32 = 16;

/// Whether `curve` over `range` may meet `cell`, moved by any whole
/// number of periods: its box over the range does, and so does the box
/// of a half of it, down to `depth` halvings.
fn meets(
    curve: &Curve2,
    [r0, r1]: [f64; 2],
    cell: Cell,
    (pu, pv): (Option<f64>, Option<f64>),
    depth: u32,
) -> bool {
    let b = piece_box(curve, [r0, r1]);
    let hit = |p: Option<f64>, [lo, hi]: [f64; 2], [c0, c1]: [f64; 2]| match p {
        None => hi >= c0 && lo <= c1,
        // Some whole number of periods brings [lo, hi] onto [c0, c1].
        Some(p) => ((c0 - hi) / p).ceil() <= ((c1 - lo) / p).floor(),
    };
    let met = hit(pu, [b.min().x, b.max().x], cell[0]) && hit(pv, [b.min().y, b.max().y], cell[1]);
    if !met || depth == 0 {
        return met;
    }
    let m = (r0 + r1) / 2.0;
    meets(curve, [r0, m], cell, (pu, pv), depth - 1)
        || meets(curve, [m, r1], cell, (pu, pv), depth - 1)
}

/// The boxes of `n` pieces of each curve over its range, each halved
/// while it is wider than half of `w` across both parameters, moved by whole
/// periods along each periodic direction to every place they meet
/// `cell` (pcurves unwrap across seams, so a curve may lie any number of
/// periods from the face's box).
fn pieces_of(
    curves: &[(&Curve2, [f64; 2])],
    n: u32,
    (pu, pv): (Option<f64>, Option<f64>),
    cell: Cell,
    w: [f64; 2],
) -> Vec<Cell> {
    let shifts = |p: Option<f64>, [lo, hi]: [f64; 2], [c0, c1]: [f64; 2]| -> Vec<f64> {
        match p {
            None => vec![0.0],
            Some(p) => {
                let first = ((c0 - hi) / p).floor() as i64;
                let last = ((c1 - lo) / p).ceil() as i64;
                (first..=last).map(|k| k as f64 * p).collect()
            }
        }
    };
    let mut out = Vec::new();
    for &(c, [r0, r1]) in curves {
        let mut stack: Vec<([f64; 2], u32)> = (0..n)
            .rev()
            .map(|i| {
                let at = |k: u32| r0 + (r1 - r0) * f64::from(k) / f64::from(n);
                ([at(i), at(i + 1)], 0)
            })
            .collect();
        while let Some(([a, b], depth)) = stack.pop() {
            let bx = piece_box(c, [a, b]);
            let (x, y) = ([bx.min().x, bx.max().x], [bx.min().y, bx.max().y]);
            // A piece wider across both directions than half the strip
            // can't cover a part beside it either way: it is halved, up
            // to [`PIECE_DEPTH`] times.
            let wide = w[0] > 0.0
                && w[1] > 0.0
                && (x[1] - x[0]) > w[0] / 2.0
                && (y[1] - y[0]) > w[1] / 2.0;
            let m = (a + b) / 2.0;
            if wide && depth < PIECE_DEPTH && a < m && m < b {
                stack.push(([m, b], depth + 1));
                stack.push(([a, m], depth + 1));
                continue;
            }
            for du in shifts(pu, x, cell[0]) {
                for dv in shifts(pv, y, cell[1]) {
                    out.push([[x[0] + du, x[1] + du], [y[0] + dv, y[1] + dv]]);
                }
            }
        }
    }
    out
}

/// The signs `F` takes (`[negative, positive]`) over the parts of `cell`
/// outside the strip round `rails` and `points` that `keep` doesn't rule
/// out, every one signed; `stop` returns as soon as both are seen.
fn run(
    surface: &Surface,
    other: &Surface,
    cell: Cell,
    (rails, points): (&[(&Curve2, [f64; 2])], &[Point3]),
    strip: f64,
    keep: &dyn Fn(Cell) -> bool,
    stop: bool,
) -> Result<[bool; 2], Failure> {
    let imp = Implicit::of(other);
    let sheet = Sheet::of(surface);
    let field = imp.as_ref().map(|imp| Field { sheet, imp });
    let (lu, lv) = first_bound(surface, cell);
    let w = [strip / lu, strip / lv];
    let pieces = pieces_of(rails, PIECES, surface.periods(), cell, w);
    // A part near a rail's end, which coverage along it can't reach.
    let ends: Vec<Point3> = rails
        .iter()
        .flat_map(|&(c, [r0, r1])| [r0, r1].map(|t| surface.point_at(c, t)))
        .chain(points.iter().copied())
        .collect();
    let near_end = |c: Cell| {
        let Some(p) = sheet.jet_box(c) else {
            return false;
        };
        ends.iter().any(|q| {
            let far = |k: usize, x: f64| (p[k].v.lo() - x).abs().max((p[k].v.hi() - x).abs());
            let q = q.to_array();
            let d2: f64 = (0..3).map(|k| far(k, q[k]) * far(k, q[k])).sum();
            d2 <= strip * strip
        })
    };
    let within =
        |c: Cell| covered(&pieces, c, 0, w[1]) || covered(&pieces, c, 1, w[0]) || near_end(c);
    // A surface with no implicit form has no side to be on: every part is
    // certified apart from it instead.
    if field.is_none() {
        let skip = |c: Cell| within(c) || !keep(c);
        return match super::general::apart(surface, cell, other, &skip, APART_CELLS) {
            Some(true) => Ok([false, false]),
            _ => Err(Failure::Uncertain),
        };
    }
    // Breadth first, so that where the face crosses `other`, both signs
    // turn up at coarse parts, before the parts along the crossing (which
    // never sign) use up the budget.
    let mut signs = [false; 2];
    let mut budget = CELLS;
    let mut queue = VecDeque::from([(cell, 0u32)]);
    while let Some((c, depth)) = queue.pop_front() {
        budget = budget.checked_sub(1).ok_or(Failure::Uncertain)?;
        if within(c) || !keep(c) {
            continue;
        }
        let e = field.as_ref().and_then(|f| f.enclose(c));
        if let Some(e) = e
            && signed(e.v)
        {
            signs[usize::from(e.v.lo() > 0.0)] = true;
            if stop && signs == [true, true] {
                return Ok(signs);
            }
            continue;
        }
        if depth == DEPTH {
            return Err(Failure::Uncertain);
        }
        let [[u0, u1], [v0, v1]] = c;
        // A part already narrower than the strip across one direction is
        // halved along whichever is wider next to the strip, towards the
        // rails' ends; otherwise along whichever adds more to the
        // enclosure.
        let (ru, rv) = ((u1 - u0) / w[0], (v1 - v0) / w[1]);
        let along_u = if ru <= 1.0 || rv <= 1.0 {
            ru >= rv
        } else {
            match e {
                Some(e) => e.d[0].mag() * (u1 - u0) >= e.d[1].mag() * (v1 - v0),
                None => u1 - u0 >= v1 - v0,
            }
        };
        if along_u {
            let m = (u0 + u1) / 2.0;
            queue.push_back(([[u0, m], [v0, v1]], depth + 1));
            queue.push_back(([[m, u1], [v0, v1]], depth + 1));
        } else {
            let m = (v0 + v1) / 2.0;
            queue.push_back(([[u0, u1], [v0, m]], depth + 1));
            queue.push_back(([[u0, u1], [m, v1]], depth + 1));
        }
    }
    Ok(signs)
}

/// Whether the pieces lying within `w` across direction `along` of every
/// point of `cell` (along `v` when `along` is 0) cover its range along
/// `along` between them.
fn covered(pieces: &[Cell], cell: Cell, along: usize, w: f64) -> bool {
    let across = 1 - along;
    let [a0, a1] = cell[along];
    let [c0, c1] = cell[across];
    let mut runs: Vec<[f64; 2]> = pieces
        .iter()
        .filter(|p| {
            p[along][1] >= a0
                && p[along][0] <= a1
                && p[across][1] - w <= c0
                && c1 <= p[across][0] + w
        })
        .map(|p| p[along])
        .collect();
    runs.sort_by(|a, b| a[0].total_cmp(&b[0]));
    let mut reach = a0;
    for [a, b] in runs {
        if a > reach {
            break;
        }
        reach = reach.max(b);
    }
    reach >= a1
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::pipe::tests::on_plane;
    use crate::geom::{Frame, Line2, Plane, Point2, Point3, Vector2, Vector3};
    use core::f64::consts::PI;
    use std::sync::Arc;

    #[test]
    fn a_pipe_beside_its_rail() {
        // The tube on the plane z = 0 touches it along the spine's shadow,
        // the line θ = 0: the rest of the tube is above it.
        let rail =
            Curve2::Line2(Line2::new(Point2::new(0.0, 0.0), Vector2::new(1.0, 0.0)).unwrap());
        let ps = Surface::Pipe(Arc::new(on_plane()));
        let plane = Surface::Plane(Plane::new(Frame::WORLD));
        let whole = [[0.0, 2.0], [-PI, PI]];
        let rails = [(&rail, [-0.1, 2.1])];
        assert_eq!(
            beside(&ps, &plane, whole, &rails, 1e-3),
            Ok(Some(Sign::Positive))
        );
        // A plane a little above cuts the tube outside the strip.
        let higher = Surface::Plane(Plane::new(
            Frame::new(
                Point3::new(0.0, 0.0, 0.01),
                Vector3::new(0.0, 0.0, 1.0),
                Vector3::new(1.0, 0.0, 0.0),
            )
            .unwrap(),
        ));
        assert_eq!(beside(&ps, &higher, whole, &rails, 1e-3), Ok(None));
        // With no strip to leave out, the touch can't be signed.
        assert_eq!(
            beside(&ps, &plane, whole, &rails, 0.0),
            Err(Failure::Uncertain)
        );
    }

    /// A plane beside a tube, which has no implicit form: certified apart
    /// part by part (slow in a debug build).
    #[test]
    #[cfg_attr(debug_assertions, ignore = "slow in a debug build: run it in release")]
    fn a_plane_beside_a_tube() {
        let rail =
            Curve2::Line2(Line2::new(Point2::new(0.0, 0.0), Vector2::new(1.0, 0.0)).unwrap());
        let ps = Surface::Pipe(Arc::new(on_plane()));
        let plane = Surface::Plane(Plane::new(Frame::WORLD));
        let whole = [[0.0, 2.0], [-PI, PI]];
        let rails = [(&rail, [-0.1, 2.1])];
        // A tube along a straight spine keeps the plane off it outside a
        // strip round its contact line, the plane's line y = 0: the tube
        // has no side, but every part of the plane away from the strip is
        // certified apart from it.
        let straight = crate::geom::NurbsCurve::new(
            crate::geom::KnotVector::clamped(3, &[0.0, 1.0, 2.0], &[4, 1, 4]).unwrap(),
            (0..5)
                .map(|i| Point3::new(f64::from(i), 0.0, crate::geom::pipe::tests::R))
                .collect(),
            None,
        )
        .unwrap();
        let tube = Surface::Pipe(Arc::new(
            crate::geom::Pipe::new(
                crate::geom::Curve::Nurbs(Arc::new(straight)),
                crate::geom::pipe::tests::R,
                plane.clone(),
            )
            .unwrap(),
        ));
        let line = Curve2::Line2(Line2::new(Point2::ORIGIN, Vector2::new(1.0, 0.0)).unwrap());
        let face = [[0.2, 3.8], [-1.0, 1.5]];
        assert_eq!(
            beside(&plane, &tube, face, &[(&line, [0.0, 4.0])], 1e-2),
            Ok(Some(Sign::Zero))
        );
        // Round the line y = 0, the curved tube's touch outside the strip
        // can't be.
        assert_eq!(
            beside(&plane, &ps, whole, &rails, 1e-3),
            Err(Failure::Uncertain)
        );
    }

    #[test]
    fn a_cylinder_beside_a_tangent_plane() {
        use crate::geom::Cylinder;
        // The cylinder of radius 1 about z touches the plane x = 1 along
        // the line u = 0; the face of it over u in [-1, 1] lies on the
        // plane's negative side, and so does the whole cylinder.
        let cyl = Surface::Cylinder(Cylinder::new(Frame::WORLD, 1.0).unwrap());
        let plane = Surface::Plane(Plane::new(
            Frame::new(
                Point3::new(1.0, 0.0, 0.0),
                Vector3::new(1.0, 0.0, 0.0),
                Vector3::new(0.0, 1.0, 0.0),
            )
            .unwrap(),
        ));
        let rail =
            Curve2::Line2(Line2::new(Point2::new(0.0, 0.0), Vector2::new(0.0, 1.0)).unwrap());
        let rails = [(&rail, [0.0, 2.0])];
        for cell in [[[-1.0, 1.0], [0.0, 2.0]], [[-PI, PI], [0.0, 2.0]]] {
            assert_eq!(
                beside(&cyl, &plane, cell, &rails, 1e-4),
                Ok(Some(Sign::Negative)),
                "{cell:?}"
            );
        }
    }
}
