//! Flat pieces split off (design 551, plan 18.B, #585).
//!
//! A NURBS face may hold a Bézier span, between knots of full multiplicity,
//! whose control points all lie within half the resolution of one plane
//! ([`coincide::planar_rows`]). Where that plane is coincident with a
//! partner's surface (a face the check lists with it in a self-union, or a
//! face of another operand of a boolean), the face is split along the
//! knot's iso-line, which at a knot of full multiplicity is exactly a row
//! of control points: SEMV where the line crosses the face's loop, MEF
//! along the row ([`split_off`]). The flat piece is given the plane and the
//! curved piece keeps its spline, trimmed by the new edge. The flat piece
//! is then wholly coincident with its partner, and `FaceMeet`, the table,
//! split and classify are unchanged.
//!
//! Knots below full multiplicity aren't split (their iso-curve isn't a
//! control row), nor periodic directions, nor a face with more than one
//! loop, nor one the iso-line crosses anywhere but at two places on two
//! edges. Where anything withholds the split the face is left as it was,
//! so bodies without the case keep their bits.

use crate::euler::{Corner, mef, semv, set_pcurve, set_surface, set_vertex_tolerance};
use crate::geom::intersect2::{Meet2, crossings};
use crate::geom::{
    Box3, Curve, Curve2, Direction, Line2, NurbsCurve, NurbsSurface, Point2, Surface, Vector2,
};
use crate::intersect::coincide::{coincident, planar_rows};
use crate::intersect::{Place, point_in_face};
use crate::name::{BooleanRole, Journal, Name, OpKey, Role};
use crate::tol::LINEAR_RESOLUTION;
use crate::topo::{Body, EdgeId, FaceId, FinId, Pcurve, VertexId};
use std::collections::BTreeSet;
use std::sync::Arc;

/// How many flat pieces one face may give: a face holds at most this many
/// flat spans worth splitting, which bounds the loop over a face's pieces.
const MOST: usize = 8;

/// The least `|n · m|` of the plane's and the spline's unit normals at a
/// point of the flat piece for which the piece's sense is taken from the
/// sign (see [`split_off`]).
const SENSE_MARGIN: f64 = 0.5;

/// A flat Bézier span of a NURBS surface beside a knot of full
/// multiplicity: the direction, the knot, whether the span lies after it
/// (at greater parameters), the span's far knot, its plane and the box of
/// its control points.
#[derive(Clone, Debug)]
pub(crate) struct FlatSpan {
    pub(crate) dir: Direction,
    pub(crate) knot: f64,
    pub(crate) after: bool,
    pub(crate) far: f64,
    pub(crate) plane: Surface,
    pub(crate) bounds: Box3,
}

/// The flat spans of `n` beside a knot of full multiplicity strictly
/// inside `pb`, the face's parameter box: a span flat by
/// [`planar_rows`], its neighbour across the knot not. In `u` then `v`,
/// each by increasing knot.
pub(crate) fn flat_spans_of(n: &NurbsSurface, pb: [[f64; 2]; 2]) -> Vec<FlatSpan> {
    let mut out = Vec::new();
    for dir in [Direction::U, Direction::V] {
        let d = dir as usize;
        let kv = &n.knots()[d];
        let (k, m, p) = (kv.knots(), kv.mults(), kv.degree());
        let r = k.len() - 1;
        // Bodies without an interior knot of full multiplicity pay only this.
        if kv.is_periodic() || !(1..r).any(|i| m[i] == p) {
            continue;
        }
        // Each span's rows and plane.
        let spans: Vec<(usize, Option<Surface>)> = (0..r)
            .map(|i| {
                let s = kv.span(0.5 * (k[i] + k[i + 1]));
                (s, planar_rows(n, dir, s - p..=s))
            })
            .collect();
        for i in 1..r {
            if m[i] != p || !(pb[d][0] < k[i] && k[i] < pb[d][1]) {
                continue;
            }
            let (span, after, far) = match (&spans[i - 1].1, &spans[i].1) {
                (None, Some(_)) => (i, true, k[i + 1]),
                (Some(_), None) => (i - 1, false, k[i - 1]),
                _ => continue,
            };
            let (s, plane) = (spans[span].0, spans[span].1.clone().expect("flat"));
            let [nu, nv] = n.counts();
            let pts = (s - p..=s).flat_map(|row| {
                (0..match dir {
                    Direction::U => nv,
                    Direction::V => nu,
                })
                    .map(move |o| match dir {
                        Direction::U => n.control_point(row, o),
                        Direction::V => n.control_point(o, row),
                    })
            });
            let Some(bounds) = Box3::from_points(pts) else {
                continue;
            };
            out.push(FlatSpan {
                dir,
                knot: k[i],
                after,
                far,
                plane,
                bounds: bounds.grow(LINEAR_RESOLUTION),
            });
        }
    }
    out
}

/// The flat spans of face `f` of `body` ([`flat_spans_of`] over the face's
/// parameter box); none for a face that isn't a spline's.
pub(crate) fn flat_spans(body: &Body, f: FaceId) -> Vec<FlatSpan> {
    let Surface::Nurbs(n) = &body.surfaces[body.faces[f].surface] else {
        return Vec::new();
    };
    // Cheap first: no interior knot of full multiplicity in either way.
    let interior = |kv: &crate::geom::KnotVector| {
        let m = kv.mults();
        !kv.is_periodic() && m.len() > 2 && m[1..m.len() - 1].iter().any(|&x| x == kv.degree())
    };
    if !n.knots().iter().any(interior) {
        return Vec::new();
    }
    flat_spans_of(n, crate::intersect::parameter_box(body, f))
}

/// The key a split of face `face` of operand `op` is made under: its
/// pieces are MEF's `(sub, Part(0|1), [face])`, its split edges SEMV's.
pub(crate) fn flat_key(key: OpKey, face: Name, op: usize) -> OpKey {
    OpKey(
        Name::derive(
            key,
            Role::Boolean(BooleanRole::FlatPiece),
            &[face, Name::from_u128(op as u128)],
        )
        .to_u128(),
    )
}

/// Face `f` of `body` split along `span`'s knot, its flat side on
/// `span.plane`: a copy of the body, the flat piece, and the journal
/// (SEMV's, MEF's, the pcurves' and the surface's events). `None`, the
/// body untouched, where the face has other loops or a fin without a
/// pcurve, where the iso-line doesn't cross its loop at exactly two places
/// on two edges (or their ends) by certified 2D crossings, where the
/// control row misses the vertices by more than their edges' tolerances,
/// or where the result isn't sound topology.
pub(crate) fn split_off(
    body: &Body,
    key: OpKey,
    f: FaceId,
    span: &FlatSpan,
) -> Option<(Body, FaceId, Journal)> {
    let Surface::Nurbs(n) = body.surfaces[body.faces[f].surface].clone() else {
        return None;
    };
    let mut b = body.clone();
    let mut journal = Journal::new();
    let loops: Vec<_> = b.face_loops(f).collect();
    let [l] = loops[..] else {
        return None;
    };
    if b.loops[l].vertex.is_some() {
        return None;
    }
    let (d, o) = (span.dir as usize, 1 - span.dir as usize);
    let pb = crate::intersect::parameter_box(&b, f);
    let reach = 0.01 * (pb[o][1] - pb[o][0]);
    let range = [pb[o][0] - reach, pb[o][1] + reach];
    // The iso-line in the parameter plane, its parameter the other
    // direction's: the same parameter as the control row's curve.
    let (origin, along) = match span.dir {
        Direction::U => (Point2::new(span.knot, 0.0), Vector2::new(0.0, 1.0)),
        Direction::V => (Point2::new(0.0, span.knot), Vector2::new(1.0, 0.0)),
    };
    let iso = Curve2::Line2(Line2::new(origin, along).ok()?);
    // Within a part in 10⁹ of the face's parameters.
    let tol = 1e-9 * (pb[0][1] - pb[0][0]).max(pb[1][1] - pb[1][0]);
    // Every place the iso-line crosses the loop: the edge, the edge's
    // parameter and the iso-line's.
    let mut hits: Vec<(EdgeId, f64, f64)> = Vec::new();
    for fin in b.loop_fins(l).collect::<Vec<_>>() {
        let pc = b.fins[fin].pcurve?;
        let e = b.fins[fin].edge;
        let [t0, t1] = b.edges[e].interval;
        let c2 = b.curves2[pc.curve].clone();
        for m in crossings((&iso, range), (&c2, [pc.param(t0), pc.param(t1)]), tol)? {
            match m {
                Meet2::Point { ta, tb, .. } => {
                    hits.push((e, (tb - pc.map[0]) / pc.map[1], ta));
                }
                Meet2::Overlap { .. } => return None,
            }
        }
    }
    // Each place a vertex: an edge's end it reaches, or a new one.
    let mut ends: Vec<(VertexId, f64)> = Vec::new();
    let mut inner: Vec<(EdgeId, f64, f64)> = Vec::new();
    for (e, t, s) in hits {
        let edge = &b.edges[e];
        let p = b.curves[edge.curve].point(t);
        let at_end = [edge.start, edge.end]
            .into_iter()
            .find(|&v| p.distance(b.vertices[v].point) <= b.vertices[v].tolerance.get());
        match at_end {
            Some(v) => {
                if !ends.iter().any(|&(w, _)| w == v) {
                    ends.push((v, s));
                }
            }
            None => inner.push((e, t, s)),
        }
    }
    if ends.len() + inner.len() != 2 {
        return None;
    }
    if inner.len() == 2 && inner[0].0 == inner[1].0 {
        return None;
    }
    for (e, t, s) in inner {
        let p = b.curves[b.edges[e].curve].point(t);
        let tol = b.edges[e].tolerance;
        let (made, j) = semv(&mut b, key, e, t, p).ok()?;
        journal.append(j);
        if tol.get() > LINEAR_RESOLUTION {
            journal.append(set_vertex_tolerance(&mut b, made.vertex, tol).ok()?);
        }
        ends.push((made.vertex, s));
    }
    ends.sort_by(|x, y| x.1.total_cmp(&y.1));
    let [(va, sa), (vb, sb)] = [ends[0], ends[1]];
    if sa.partial_cmp(&sb) != Some(core::cmp::Ordering::Less) {
        return None;
    }
    // The knot's control row, exactly: at a knot of full multiplicity the
    // iso-curve passes through that row, which is its control polygon.
    let kv = &n.knots()[d];
    let row = kv.span(span.knot) - kv.degree();
    let [nu, nv] = n.counts();
    let w = n.weights();
    let (pts, ws): (Vec<_>, Vec<_>) = match span.dir {
        Direction::U => (0..nv)
            .map(|j| (n.control_point(row, j), w[row * nv + j]))
            .unzip(),
        Direction::V => (0..nu)
            .map(|i| (n.control_point(i, row), w[i * nv + row]))
            .unzip(),
    };
    let row_curve = NurbsCurve::new(n.knots()[o].clone(), pts, Some(ws)).ok()?;
    for (v, s) in [(va, sa), (vb, sb)] {
        let gap = row_curve.point(s).distance(b.vertices[v].point);
        if gap > b.vertices[v].tolerance.get().max(LINEAR_RESOLUTION) {
            return None;
        }
    }
    let at = |b: &Body, v: VertexId| b.loop_fins(l).find(|&fin| b.start_of(fin) == v);
    let (fa, fb) = (at(&b, va)?, at(&b, vb)?);
    let cid = b.add_curve(Curve::Nurbs(Arc::new(row_curve)));
    let (made, j) = mef(
        &mut b,
        key,
        Corner::Before(fa),
        Corner::Before(fb),
        cid,
        [sa, sb],
    )
    .ok()?;
    journal.append(j);
    // Both new fins run along the iso-line in the spline's parameters.
    for fin in made.fins {
        let c2 = b.add_curve2(iso.clone());
        journal.append(
            set_pcurve(
                &mut b,
                fin,
                Some(Pcurve {
                    curve: c2,
                    map: [0.0, 1.0],
                }),
            )
            .ok()?,
        );
    }
    // The flat piece: past the knot the face is all one piece, the loop
    // crossing the iso-line only at the split's two ends, so the first
    // point there certified inside a piece names it. The span's middle
    // alone won't do: a wedge-shaped piece's slanted edge can run through
    // it (Voron's molex face, #585).
    let inside_far = if span.after {
        span.far.min(pb[d][1])
    } else {
        span.far.max(pb[d][0])
    };
    let (lo_o, hi_o) = (pb[o][0], pb[o][1]);
    let mut flat = None;
    'search: for f in [0.5, 0.25, 0.75, 0.1, 0.9, 0.05] {
        let along = span.knot + f * (inside_far - span.knot);
        for g in [0.5, 0.25, 0.75, 0.1, 0.9, 0.05, 0.95] {
            let across = sa + g * (sb - sa);
            for across in [across, lo_o + g * (hi_o - lo_o)] {
                let uv = match span.dir {
                    Direction::U => Point2::new(along, across),
                    Direction::V => Point2::new(across, along),
                };
                let inside: Vec<FaceId> = made
                    .faces
                    .into_iter()
                    .filter(|&g| matches!(point_in_face(&b, g, uv), Ok(Place::Inside)))
                    .collect();
                if let [one] = inside[..] {
                    flat = Some((one, uv));
                    break 'search;
                }
            }
        }
    }
    let (flat, uv) = flat?;
    let curved = if made.faces[0] == flat {
        made.faces[1]
    } else {
        made.faces[0]
    };
    // The plane facing the way the spline faced there. `uv` lies inside
    // the flat span, whose control points are all within half the
    // resolution of the plane, so the spline's normal there is the plane's
    // or its reverse: `|dot|` is 1 less the tilt the net's flatness allows
    // over the span (a few thousandths for the molex span, 1.8 µm long),
    // nowhere near 0. The sign of a dot product past `SENSE_MARGIN` is
    // certain under any rounding of two unit normals; anything nearer 0
    // means the normal there isn't the span's (a degenerate point), and the
    // split is withheld rather than the sense guessed.
    let pn = span.plane.frame()?.z().get();
    let sn = n.normal(uv.x, uv.y)?.get();
    let dot = pn.dot(sn);
    if dot.abs() < SENSE_MARGIN {
        return None;
    }
    let sense = if dot > 0.0 {
        b.faces[flat].sense
    } else {
        b.faces[flat].sense.flipped()
    };
    let sid = b.add_surface(span.plane.clone());
    journal.append(set_surface(&mut b, flat, sid, sense).ok()?);
    // The curved piece on its own half of the spline, cut at the knot (at
    // a knot of full multiplicity, its control rows as they were): the
    // whole spline would still hold the flat span past its trim, on the
    // partner's plane, where no trace of the two surfaces settles.
    let (lo, hi) = n.split(span.dir, span.knot).ok()?;
    let half = if span.after { lo } else { hi };
    let cid = b.add_surface(Surface::Nurbs(Arc::new(half)));
    let cs = b.faces[curved].sense;
    // Its pcurves stay exact: the half has the spline's own parameters,
    // and an edge the spline met another surface in (a cut's) may have
    // no exact pcurve `fill` could make again.
    let kept: Vec<(FinId, Option<Pcurve>)> = b
        .face_loops(curved)
        .flat_map(|l| b.loop_fins(l))
        .map(|fin| (fin, b.fins[fin].pcurve))
        .collect();
    journal.append(set_surface(&mut b, curved, cid, cs).ok()?);
    for (fin, pc) in kept {
        journal.append(set_pcurve(&mut b, fin, pc).ok()?);
    }
    crate::pcurve::fill(&mut b).ok()?;
    if !crate::check::check_topology(&b).is_empty() {
        return None;
    }
    Some((b, flat, journal))
}

/// [`split_off`] on every face of `body` listed in `pairs` whose flat span
/// lies on the surface of a face listed with it: the split body and
/// journal, or `None` where nothing was split. Each face's curved piece
/// keeps its id and is looked at again, up to [`MOST`] times, for a flat
/// span on its other side.
pub(crate) fn split_listed(
    body: &Body,
    key: OpKey,
    pairs: &BTreeSet<(FaceId, FaceId)>,
) -> Option<(Body, Journal)> {
    let mut faces: Vec<FaceId> = pairs.iter().flat_map(|&(a, b)| [a, b]).collect();
    faces.sort_unstable();
    faces.dedup();
    let partners = |f: FaceId| -> Vec<FaceId> {
        pairs
            .iter()
            .filter_map(|&(a, b)| (a == f).then_some(b).or((b == f).then_some(a)))
            .collect()
    };
    let mut out: Option<(Body, Journal)> = None;
    for f in faces {
        for _ in 0..MOST {
            let b = out.as_ref().map_or(body, |(b, _)| b);
            let name = b.faces[f].name;
            // On a listed partner's surface, or on that of any other face
            // whose box meets the span's: the check lists a face against
            // what it crosses, which needn't be the face its flat span
            // lies on (Voron's molex face, #585).
            let listed = partners(f);
            let found = flat_spans(b, f).into_iter().find(|s| {
                b.faces().any(|g| {
                    g != f
                        && (listed.contains(&g)
                            || crate::intersect::face_box(b, g).intersects(&s.bounds))
                        && coincident(&s.plane, &b.surfaces[b.faces[g].surface]).is_some()
                })
            });
            let Some(span) = found else {
                break;
            };
            let Some((split, _, j)) = split_off(b, flat_key(key, name, 0), f, &span) else {
                break;
            };
            match &mut out {
                Some((ob, oj)) => {
                    *ob = split;
                    oj.append(j);
                }
                None => out = Some((split, j)),
            }
        }
    }
    out
}

/// [`split_off`] on every operand's spline faces whose flat span lies on
/// the surface of another operand's face that its box meets: the operands
/// with those faces split, and the journal of the splits, empty where
/// none was. Each split is keyed by the boolean's key, the face's name and
/// its operand's index, so copies of one body placed as two tools split
/// apart.
pub(crate) fn split_operands(mut ops: Vec<Body>, key: OpKey) -> (Vec<Body>, Journal) {
    let mut journal = Journal::new();
    for op in 0..ops.len() {
        let faces: Vec<FaceId> = ops[op].faces().collect();
        for f in faces {
            for _ in 0..MOST {
                let found = flat_spans(&ops[op], f).into_iter().find(|s| {
                    ops.iter().enumerate().any(|(other, b)| {
                        other != op
                            && b.faces().any(|g| {
                                crate::intersect::face_box(b, g).intersects(&s.bounds)
                                    && coincident(&s.plane, &b.surfaces[b.faces[g].surface])
                                        .is_some()
                            })
                    })
                });
                let Some(span) = found else {
                    break;
                };
                let name = ops[op].faces[f].name;
                let Some((split, _, j)) = split_off(&ops[op], flat_key(key, name, op), f, &span)
                else {
                    break;
                };
                ops[op] = split;
                journal.append(j);
            }
        }
    }
    (ops, journal)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{KnotVector, NurbsSurface, Point3};
    use crate::tol::LINEAR_RESOLUTION;
    use crate::topo::Body;

    const MM: f64 = 1e-3;

    /// A rational bicubic-by-linear patch 1 mm deep in y over a profile in
    /// xz, as Voron's molex face #1789906 is: `knots`/`mults` along u, the
    /// profile's points `(x, z)` in mm, weights `w`.
    fn patch(knots: &[f64], mults: &[usize], xz: &[(f64, f64)], w: &[f64]) -> NurbsSurface {
        let ku = KnotVector::clamped(3, knots, mults).unwrap();
        let kv = KnotVector::clamped(1, &[0.0, 1.0], &[2, 2]).unwrap();
        let mut pts = Vec::new();
        let mut ws = Vec::new();
        for (i, &(x, z)) in xz.iter().enumerate() {
            for y in [0.0, MM] {
                pts.push(Point3::new(x * MM, y, z * MM));
                ws.push(w[i]);
            }
        }
        NurbsSurface::new([ku, kv], pts, Some(ws)).unwrap()
    }

    /// The molex shape: a curved span leaving z = 0 at an angle at u = 1,
    /// then a flat span on z = 0 to u = 1.01.
    const END: [(f64, f64); 7] = [
        (0.0, 1.0),
        (0.0, 0.5),
        (0.6, 0.3),
        (1.0, 0.0),
        (1.1, 0.0),
        (1.2, 0.0),
        (1.3, 0.0),
    ];
    const W: [f64; 7] = [1.0, 0.9, 0.9, 1.0, 1.0, 1.0, 1.0];
    const ALL: [[f64; 2]; 2] = [[0.0, 1.01], [0.0, 1.0]];

    #[test]
    fn a_flat_end_span_is_found_at_its_knot() {
        let n = patch(&[0.0, 1.0, 1.01], &[4, 3, 4], &END, &W);
        let spans = flat_spans_of(&n, ALL);
        assert_eq!(spans.len(), 1, "{spans:?}");
        let s = &spans[0];
        assert_eq!(
            (s.dir, s.knot, s.after, s.far),
            (Direction::U, 1.0, true, 1.01)
        );
        let f = s.plane.frame().unwrap();
        assert!(f.z().get().z.abs() > 1.0 - 1e-12, "{f:?}");
        assert!(f.origin().z.abs() <= LINEAR_RESOLUTION);
    }

    #[test]
    fn a_flat_middle_span_is_found_at_both_its_knots() {
        let mut xz = END.to_vec();
        xz.extend([(1.4, 0.3), (1.6, 0.6), (1.6, 1.0)]);
        let n = patch(&[0.0, 1.0, 1.01, 2.0], &[4, 3, 3, 4], &xz, &[1.0; 10]);
        let spans = flat_spans_of(&n, [[0.0, 2.0], [0.0, 1.0]]);
        let got: Vec<(f64, bool)> = spans.iter().map(|s| (s.knot, s.after)).collect();
        assert_eq!(got, [(1.0, true), (1.01, false)]);
    }

    #[test]
    fn a_span_flat_only_to_twice_the_resolution_is_not_split() {
        let mut xz = END;
        xz[5].1 = 2.0 * LINEAR_RESOLUTION / MM;
        let n = patch(&[0.0, 1.0, 1.01], &[4, 3, 4], &xz, &W);
        assert!(flat_spans_of(&n, ALL).is_empty());
    }

    #[test]
    fn a_flat_span_at_a_knot_below_full_multiplicity_is_not_split() {
        // Knot 1 of multiplicity 2: its iso-curve isn't a control row.
        let xz = [
            (0.0, 1.0),
            (0.0, 0.5),
            (1.0, 0.0),
            (1.1, 0.0),
            (1.2, 0.0),
            (1.3, 0.0),
        ];
        let n = patch(&[0.0, 1.0, 1.01], &[4, 2, 4], &xz, &[1.0; 6]);
        assert!(flat_spans_of(&n, ALL).is_empty());
    }

    #[test]
    fn a_knot_outside_the_face_is_not_split() {
        let n = patch(&[0.0, 1.0, 1.01], &[4, 3, 4], &END, &W);
        assert!(flat_spans_of(&n, [[0.0, 0.9], [0.0, 1.0]]).is_empty());
        assert!(flat_spans_of(&n, [[1.0, 1.01], [0.0, 1.0]]).is_empty());
    }

    #[test]
    fn planar_is_unchanged_by_its_split_into_rows() {
        // `planar` over the whole net and `planar_rows` over every row agree.
        let n = patch(&[0.0, 1.0, 1.01], &[4, 3, 4], &END, &W);
        assert_eq!(
            crate::intersect::coincide::planar(&n),
            crate::intersect::coincide::planar_rows(&n, Direction::U, 0..=6)
        );
        assert!(crate::intersect::coincide::planar_rows(&n, Direction::U, 3..=6).is_some());
        assert!(crate::intersect::coincide::planar_rows(&n, Direction::U, 0..=3).is_none());
    }

    /// Voron's molex face's flat span as its file gives it (#585): 1.8 µm
    /// long and 0.43 mm wide, every point on z = -2.55 mm. The cross
    /// product of its diagonals is about 2e-9 m², under the resolution
    /// `normalize` takes as zero, so the plane was never formed.
    #[test]
    fn a_thin_flat_span_is_flat() {
        let pts = [
            (2.991446443e-3, 7.249998331e-4),
            (3.296916573e-3, 4.195297036e-4),
            (2.992056344e-3, 7.256097338e-4),
            (3.297526473e-3, 4.201396043e-4),
            (2.992666244e-3, 7.262196344e-4),
            (3.298136374e-3, 4.207495050e-4),
            (2.993276145e-3, 7.268295351e-4),
            (3.298746275e-3, 4.213594056e-4),
        ]
        .map(|(x, y)| Point3::new(x, y, -2.55e-3));
        let n = NurbsSurface::new(
            [
                KnotVector::clamped(3, &[1.0, 1.01], &[4, 4]).unwrap(),
                KnotVector::clamped(1, &[0.0, 0.0432], &[2, 2]).unwrap(),
            ],
            pts.to_vec(),
            None,
        )
        .unwrap();
        let plane = crate::intersect::coincide::planar_rows(&n, Direction::U, 0..=3)
            .expect("the span is flat");
        let f = plane.frame().unwrap();
        assert!(f.z().get().z.abs() > 1.0 - 1e-12, "{f:?}");
        assert!((f.origin().z + 2.55e-3).abs() <= LINEAR_RESOLUTION);
        // `planar` keeps its own rule, bit for bit (no golden moves).
        assert_eq!(crate::intersect::coincide::planar(&n), None);
    }

    /// The twin's prism (`ryukei_testkit::nurbs::flat_span_twin`), built
    /// here from its extrusion and its underside set to `patch`'s form.
    fn prism() -> Body {
        use crate::geom::{Curve2, Frame, Line2, NurbsCurve2, Point2, Vector3};
        use crate::sweep::{Loop, LoopKey, Profile, Segment, SegmentKey, ValidProfile, extrude};
        let frame = Frame::new(
            Point3::ORIGIN,
            Vector3::new(0.0, -1.0, 0.0),
            Vector3::new(1.0, 0.0, 0.0),
        )
        .unwrap();
        let kv = KnotVector::clamped(3, &[0.0, 1.0, 1.01], &[4, 3, 4]).unwrap();
        let spline = NurbsCurve2::new(
            kv,
            END.iter()
                .map(|&(x, z)| Point2::new(x * MM, z * MM))
                .collect(),
            Some(W.to_vec()),
        )
        .unwrap();
        let p = |x: f64, z: f64| Point2::new(x * MM, z * MM);
        let line = |k: u128, a: Point2, b: Point2| Segment {
            key: SegmentKey(k),
            curve: Curve2::Line2(Line2::through(a, b).unwrap()),
            t0: 0.0,
            t1: a.distance(b),
        };
        let profile = ValidProfile::new(Profile {
            frame,
            loops: vec![Loop {
                key: LoopKey(3),
                segments: vec![
                    Segment {
                        key: SegmentKey(30),
                        curve: Curve2::Nurbs2(Arc::new(spline)),
                        t0: 0.0,
                        t1: 1.01,
                    },
                    line(31, p(1.3, 0.0), p(1.3, 1.0)),
                    line(32, p(1.3, 1.0), p(0.0, 1.0)),
                ],
            }],
        })
        .unwrap();
        let (mut b, _) = extrude(OpKey(0x585), &profile, frame.z().get(), 0.0, MM).unwrap();
        // The underside on its NURBS form: the patch over y in [-1, 0] mm.
        let f = b
            .faces()
            .find(|&f| !matches!(b.surfaces[b.faces[f].surface], Surface::Plane(_)))
            .unwrap();
        let mut pts = Vec::new();
        let mut ws = Vec::new();
        for (i, &(x, z)) in END.iter().enumerate() {
            for y in [0.0, -MM] {
                pts.push(Point3::new(x * MM, y, z * MM));
                ws.push(W[i]);
            }
        }
        let n = NurbsSurface::new(
            [
                KnotVector::clamped(3, &[0.0, 1.0, 1.01], &[4, 3, 4]).unwrap(),
                KnotVector::clamped(1, &[0.0, MM], &[2, 2]).unwrap(),
            ],
            pts,
            Some(ws),
        )
        .unwrap();
        let sense = b.faces[f].sense;
        let sid = b.add_surface(Surface::Nurbs(Arc::new(n)));
        crate::euler::set_surface(&mut b, f, sid, sense).unwrap();
        crate::pcurve::fill(&mut b).unwrap();
        assert_eq!(crate::check::check(&b), []);
        b
    }

    /// The prism cut by a block turned about z, so its flat piece is the
    /// triangle (1, 0), (1.3, 0), (1, -0.75) mm, as Voron's molex face's is
    /// a wedge (#585): the slanted edge runs through the middle of the
    /// span's box in the parameters, where no piece holds the point. The
    /// flat piece is still found, by a point certified inside it past the
    /// knot.
    #[test]
    fn a_wedge_shaped_flat_piece_splits_off() {
        use crate::boolean::{BooleanOp, Tool, boolean};
        use crate::geom::{Rigid, UnitVector3, Vector3};
        use crate::name::ToolKey;
        let b = prism();
        // The cut's line from (1.3, 0) mm towards (1, -0.75) mm; the block
        // lies on its left, the side holding (1.3, -1), its local y
        // turning with it to the line's left.
        let (dx, dy) = (-0.3 * MM, -0.75 * MM);
        let len = crate::math::sqrt(dx * dx + dy * dy);
        let u = (dx / len, dy / len);
        let theta = crate::math::atan2(u.1, u.0);
        let (w, l) = (3.0 * MM, 4.0 * MM);
        let origin = Vector3::new(1.3 * MM - 2.0 * MM * u.0, -2.0 * MM * u.1, -MM);
        let place = Rigid::translation(origin).then_after(&Rigid::rotation(UnitVector3::Z, theta));
        let cutter = crate::primitive::block(OpKey(0x5c), l, w, 3.0 * MM, &place)
            .unwrap()
            .0;
        let (cut, _) = boolean(
            OpKey(0x5d),
            BooleanOp::Subtract,
            &b,
            &[Tool {
                key: ToolKey(1),
                body: &cutter,
                place: Rigid::IDENTITY,
            }],
        )
        .unwrap();
        assert_eq!(crate::check::check(&cut), []);
        let f = cut
            .faces()
            .find(|&f| matches!(cut.surfaces[cut.faces[f].surface], Surface::Nurbs(_)))
            .unwrap();
        let spans = flat_spans(&cut, f);
        assert_eq!(spans.len(), 1, "{spans:?}");
        let (s, flat, _) = split_off(&cut, OpKey(78), f, &spans[0]).expect("split");
        assert_eq!(crate::check::check(&s), []);
        assert!(matches!(
            s.surfaces[s.faces[flat].surface],
            Surface::Plane(_)
        ));
        // The triangle: three edges.
        let edges: usize = s.face_loops(flat).map(|l| s.loop_fins(l).count()).sum();
        assert_eq!(edges, 3);
    }

    #[test]
    fn a_flat_end_span_splits_off_exactly() {
        let b = prism();
        let f = b
            .faces()
            .find(|&f| matches!(b.surfaces[b.faces[f].surface], Surface::Nurbs(_)))
            .unwrap();
        let spans = flat_spans(&b, f);
        assert_eq!(spans.len(), 1, "{spans:?}");
        let (s, flat, journal) = split_off(&b, OpKey(77), f, &spans[0]).expect("split");
        assert_eq!(crate::check::check(&s), []);
        assert_eq!(s.faces().count(), b.faces().count() + 1);
        assert!(matches!(
            s.surfaces[s.faces[flat].surface],
            Surface::Plane(_)
        ));
        // The curved piece keeps its id, on the spline's half before the
        // knot: its control rows bit for bit.
        let Surface::Nurbs(n) = &b.surfaces[b.faces[f].surface] else {
            unreachable!("the spline face")
        };
        let (lo, _) = n.split(Direction::U, 1.0).unwrap();
        assert_eq!(s.surfaces[s.faces[f].surface], Surface::Nurbs(Arc::new(lo)));
        for (i, j) in (0..4).flat_map(|i| (0..2).map(move |j| (i, j))) {
            let Surface::Nurbs(m) = &s.surfaces[s.faces[f].surface] else {
                unreachable!()
            };
            assert_eq!(m.control_point(i, j), n.control_point(i, j));
        }
        // Two edges split (SEMV) and one made (MEF); two vertices made.
        assert_eq!(s.edges().count(), b.edges().count() + 3);
        assert_eq!(s.vertices().count(), b.vertices().count() + 2);
        assert!(!journal.is_empty());
    }
}
