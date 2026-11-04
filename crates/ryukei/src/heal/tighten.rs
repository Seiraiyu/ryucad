//! Tightening: an edge between two faces that meet in a known curve put
//! on that curve, so its tolerance need cover only how far its vertices
//! lie off it.
//!
//! An edge whose curve strays from its faces by more than resolution, on
//! whatever surfaces, is met with the surfaces' intersection
//! ([`crate::intersect::surface_surface`]) inside its box, grown by its
//! reach. Its reach is twice its measured gap (at
//! least ten resolutions, at most the cap): a branch that crosses
//! nowhere near tangentially, every one of 17 samples along the edge, and
//! both its vertices within the reach of it, closer to both surfaces than
//! the edge was, replaces the edge's curve. The edge runs from its start
//! vertex's foot on the branch to its end vertex's, the way the edge's own
//! curve runs at its middle (reversed, uses and all, where the branch runs
//! the other way); on a closed branch the stretch must hold the middle's
//! foot, and a closed edge takes a whole period, or a closed clamped
//! branch (a spline's) started at its vertex's foot
//! ([`crate::intersect::reseamed`]). Vertices stay where they are: what
//! the new curve's ends leave the tolerances cover (see `gaps`). The
//! file's pcurves for a tightened edge give way to the branch's, where
//! their maps are the identity (the general intersector's, certified with
//! the branch), so the pcurves step takes them as the file's; else it
//! finds exact ones or fits them. An edge with no such branch keeps
//! its curve and is tolerated instead, and so is an edge whose faces are
//! tangent or nearly so at its middle (their normals within a tenth of a
//! radian): there the branch lies far further from the edge than its
//! gap, or runs tangentially, and tracing it only costs time. So is an
//! edge whose middle lies within its reach of a bounded domain edge of a
//! face's surface that has no implicit form (a spline's, a swept
//! surface's): the faces meet along that domain edge, where no trace can
//! certify a crossing, so the intersector traces their whole box and
//! then again on the surfaces carried past their edges, seconds to
//! minutes an edge, to gain at most the edge's micron gap (#394, #239).
//! And so is an edge between two faces with no implicit form whose gap
//! is within a fitted curve's tolerance (`TOUCH_TOLERANCE`): their
//! branch would be a fit no closer than that, and tracing it took tens
//! of seconds (#305).

use super::Loop;
use super::Shape;
use super::gaps::edge_gap;
use super::names::Names;
use crate::geom::{Curve, Curve2, Point3, Surface};
use crate::intersect::implicit::Implicit;
use crate::intersect::{SurfaceHit, surface_surface_in};
use crate::name::Name;
use crate::tol::{LINEAR_RESOLUTION, TOUCH_TOLERANCE};
use crate::work::Meter;

/// Samples along an edge, ends included.
const SAMPLES: usize = 16;

/// The least cosine between the edge's direction and the branch's at the
/// edge's middle.
const ALIGNED: f64 = 0.5;

/// The least sine of the angle between the faces' normals at the edge's
/// middle for the edge to be tightened. It was a hundredth until v0.8.2:
/// spline faces of Altium's and ST's board exports meeting at 1° to 4°
/// along an edge (an LED's lens, a part's rounded rim) spent 20 to 470 s
/// each tracing (on a Xeon E5-2650) and none was tightened, while every
/// edge tightened across the committed and owner's corpora and
/// `heal_props` met at a sine of 0.15 or more (issues #279, #434).
const CREASE: f64 = 0.1;

/// A tightened edge: its new curve, interval, whether it now runs the
/// other way, and how far the branch may be from its surfaces.
struct Tight {
    curve: Curve,
    interval: [f64; 2],
    flip: bool,
    tolerance: f64,
    /// The branch on each of the edge's two faces' surfaces, in order,
    /// where its map is the identity.
    pcurves: [Option<Curve2>; 2],
}

/// Tightens every edge of `s` that can be, returning their names, sorted,
/// and the gaps it measured: for each edge it judged and left as it was,
/// its two faces and how far its curve strays from each one's surface
/// ([`edge_gap`]), which `gaps::tolerate` reads rather than measure them
/// again (on a file of rational splines, linkrods', the projections cost
/// a fifth of its heal each time).
pub(super) fn tighten(
    s: &mut Shape,
    names: &Names,
    cap: f64,
    m: &Meter,
) -> (Vec<Name>, Vec<Option<Measured>>) {
    let mut faces: Vec<Vec<usize>> = vec![Vec::new(); s.edges.len()];
    for (k, f) in s.faces.iter().enumerate() {
        for l in &f.loops {
            if let Loop::Edges(uses) = l {
                for u in uses {
                    faces[u.edge].push(k);
                }
            }
        }
    }
    let edges: Vec<usize> = (0..s.edges.len()).collect();
    let found: Vec<(Option<Tightened>, Option<Measured>)> =
        m.par_map(&edges, |m, &g| match faces[g][..] {
            [a, b] if a != b => {
                let (t, gaps) = tight(s, g, [a, b], cap, m);
                let measured = gaps.map(|[ga, gb]| [(a, ga), (b, gb)]);
                (t.map(|t| (t, [a, b])), measured)
            }
            _ => (None, None),
        });
    let mut out = Vec::new();
    let mut known = Vec::with_capacity(found.len());
    for (g, (t, measured)) in found.into_iter().enumerate() {
        let Some((t, pair)) = t else {
            known.push(measured);
            continue;
        };
        // Its curve changes: its gaps are measured afresh.
        known.push(None);
        s.curves.push(t.curve);
        let e = &mut s.edges[g];
        e.curve = s.curves.len() - 1;
        e.interval = t.interval;
        e.tolerance = e.tolerance.max(t.tolerance);
        if t.flip {
            std::mem::swap(&mut e.start, &mut e.end);
        }
        let mut given = [None; 2];
        for (i, c2) in t.pcurves.into_iter().enumerate() {
            if let Some(c2) = c2 {
                s.curves2.push(c2);
                given[i] = Some(s.curves2.len() - 1);
            }
        }
        for (k, f) in s.faces.iter_mut().enumerate() {
            let side = pair.iter().position(|&x| x == k);
            for l in &mut f.loops {
                if let Loop::Edges(uses) = l {
                    for u in uses.iter_mut().filter(|u| u.edge == g) {
                        u.pcurve = side.and_then(|i| given[i]);
                        if t.flip {
                            u.sense = u.sense.flipped();
                        }
                    }
                }
            }
        }
        out.push(names.edges[g]);
    }
    out.sort();
    (out, known)
}

/// A tightened edge and its two faces.
type Tightened = (Tight, [usize; 2]);

/// An edge's two faces and its gap from each one's surface.
pub(super) type Measured = [(usize, f64); 2];

/// A branch's pcurves whose maps are the identity.
fn identity(pcurves: &[(Curve2, [f64; 2]); 2]) -> [Option<Curve2>; 2] {
    pcurves.clone().map(|(c, m)| (m == [0.0, 1.0]).then_some(c))
}

/// Edge `g`'s curve on the intersection of `faces`' surfaces, if one runs
/// along it.
fn tight(
    s: &Shape,
    g: usize,
    faces: [usize; 2],
    cap: f64,
    m: &Meter,
) -> (Option<Tight>, Option<[f64; 2]>) {
    // An edge judged.
    if m.charge(1).is_err() {
        return (None, None);
    }
    let e = &s.edges[g];
    let [sa, sb] = faces.map(|k| &s.surfaces[s.faces[k].surface]);
    let curve = &s.curves[e.curve];
    let gaps = [
        edge_gap(curve, e.interval, sa),
        edge_gap(curve, e.interval, sb),
    ];
    let t = tight_from(s, e, [sa, sb], gaps[0].max(gaps[1]), cap, m);
    (t, Some(gaps))
}

/// [`tight`] once the edge's gap from its faces is measured.
fn tight_from(
    s: &Shape,
    e: &super::Edge,
    [sa, sb]: [&Surface; 2],
    gap: f64,
    cap: f64,
    m: &Meter,
) -> Option<Tight> {
    let curve = &s.curves[e.curve];
    if gap <= LINEAR_RESOLUTION {
        return None;
    }
    // Two surfaces with no implicit form meet in a fitted branch, whose
    // tolerance is a fit's certified bound: an edge already within a
    // fitted curve's tolerance of both gains nothing a trace could give,
    // and their four-parameter trace costs tens of seconds an edge (a
    // KiCad MSOP-8's four 14 nm spline edges spent 30 to 58 s each and
    // none was tightened, on a Xeon E5-2650). Across the committed and
    // pinned corpora and `heal_props`, no edge between two such faces
    // within 2 µm was ever tightened.
    if gap <= TOUCH_TOLERANCE && Implicit::of(sa).is_none() && Implicit::of(sb).is_none() {
        return None;
    }
    let reach = (2.0 * gap).max(10.0 * LINEAR_RESOLUTION).min(cap);
    let [t0, t1] = e.interval;
    let at = |i: usize| t0 + (t1 - t0) * i as f64 / SAMPLES as f64;
    let normal = |x: &Surface| {
        let (u, v) = x.project(curve.point(at(SAMPLES / 2))).param;
        x.normal(u, v)
    };
    if normal(sa)?.get().cross(normal(sb)?.get()).length() < CREASE {
        return None;
    }
    let samples: Vec<_> = (0..=SAMPLES).map(|i| curve.point(at(i))).collect();
    let middle = samples[SAMPLES / 2];
    if [sa, sb].iter().any(|x| along_domain_edge(x, middle, reach)) {
        return None;
    }
    let start = s.vertices[e.start].point;
    let end = s.vertices[e.end].point;
    let region = crate::geom::Box3::from_points(samples.iter().copied().chain([start, end]))?
        .grow(reach + gap);
    let SurfaceHit::Meet { branches, .. } = surface_surface_in(sa, sb, &region, m).ok()? else {
        return None;
    };
    let mid = at(SAMPLES / 2);
    let along = curve.tangent(mid)?;
    branches
        .iter()
        .filter(|b| !b.tangent && b.tolerance < gap)
        .filter(|b| {
            samples
                .iter()
                .chain([&start, &end])
                .all(|&p| b.curve.distance(p) <= reach)
        })
        .find_map(|b| {
            let foot = |p| b.curve.project(p).param;
            let (p0, p1, pm) = (foot(start), foot(end), foot(curve.point(mid)));
            let cos = along.get().dot(b.curve.tangent(pm)?.get());
            if cos.abs() < ALIGNED {
                return None;
            }
            let forward = cos > 0.0;
            // From the foot the edge starts at to the one it ends at,
            // along the branch.
            let (from, to) = if forward { (p0, p1) } else { (p1, p0) };
            let interval = match b.curve.period() {
                // A closed edge whose vertex lies within reach of where
                // the branch starts takes the branch's own range: its
                // pcurves are clamped to it, and an interval starting a
                // hair before the period's end (the vertex's foot rounded
                // there) leaves no map onto them, so a pcurve must be
                // fitted afresh, which a circle passing by a sphere's
                // poles can defeat (#373). The vertex stays, its
                // tolerance covering the gap.
                Some(_)
                    if e.start == e.end && b.curve.point(b.range[0]).distance(start) <= reach =>
                {
                    b.range
                }
                Some(per) => {
                    let mut hi = from + (to - from).rem_euclid(per);
                    if e.start == e.end || hi - from <= LINEAR_RESOLUTION {
                        hi = from + per;
                    }
                    let m = from + (pm - from).rem_euclid(per);
                    if !(from < m && m < hi) {
                        return None;
                    }
                    [from, hi]
                }
                None if e.start == e.end => {
                    // A closed edge on a closed clamped branch (a spline's):
                    // the branch started at the foot of the edge's vertex.
                    let [lo, hi] = b.range;
                    let tol = b.tolerance.max(LINEAR_RESOLUTION);
                    if b.curve.point(lo).distance(b.curve.point(hi)) > tol {
                        return None;
                    }
                    let edge = (hi - lo) * 1e-6;
                    let b = if p0 > lo + edge && p0 < hi - edge {
                        let moved = crate::intersect::reseamed(b.clone(), p0);
                        if moved.range == b.range {
                            return None;
                        }
                        moved
                    } else {
                        b.clone()
                    };
                    return Some(Tight {
                        pcurves: identity(&b.pcurves),
                        curve: b.curve,
                        interval: b.range,
                        flip: !forward,
                        tolerance: b.tolerance,
                    });
                }
                None => {
                    if e.start == e.end || !(from < pm && pm < to) {
                        return None;
                    }
                    let [lo, hi] = b.range;
                    if from < lo || to > hi {
                        return None;
                    }
                    [from, to]
                }
            };
            Some(Tight {
                pcurves: identity(&b.pcurves),
                curve: b.curve.clone(),
                interval,
                flip: !forward,
                tolerance: b.tolerance,
            })
        })
}

/// Whether the foot of `p` on `s`, a surface with no implicit form, lies
/// within `reach` of an end of a direction its domain bounds: one neither
/// periodic nor open (a spline's, a swept surface's curve's).
///
/// The race's sewn bodies and the Switchwire's threaded inserts spent
/// 85 to 99% of their healing tracing edges like these, and none was
/// tightened: two spline patches abutting along their domain edges, or a
/// lofted side standing on a plane at its end row.
fn along_domain_edge(s: &Surface, p: Point3, reach: f64) -> bool {
    if Implicit::of(s).is_some() {
        return false;
    }
    let foot = s.project(p);
    let (u, v) = foot.param;
    let (pu, pv) = s.periods();
    let [du, dv] = s.domain();
    let ends = |[a, b]: [f64; 2], periodic: bool| {
        if periodic || !a.is_finite() || !b.is_finite() {
            Vec::new()
        } else {
            vec![a, b]
        }
    };
    ends(du, pu.is_some())
        .into_iter()
        .any(|e| s.point(e, v).distance(foot.point) <= reach)
        || ends(dv, pv.is_some())
            .into_iter()
            .any(|e| s.point(u, e).distance(foot.point) <= reach)
}
