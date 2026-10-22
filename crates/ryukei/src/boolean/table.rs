//! The intersection table: every vertex and edge piece where a boolean's
//! operands meet, computed once and shared by every face that uses it.
//!
//! Faces of different operands whose boxes overlap are intersected
//! ([`face_face`]), every edge is cut with every other operand's face it
//! may meet ([`curve_surface`], placed by [`point_in_face`]), and every
//! vertex is placed on the faces it may touch. The points these give
//! (edge hits, the ends of the stretches where faces meet, the starts of
//! closed stretches, the rims of coincident faces, touching points, the
//! operands' own vertices, and with three or more operands the points
//! where a stretch crosses a third operand's face) are clustered into
//! global vertices by an explicit rule: two points are one vertex when
//! they are closer than the sum of their tolerances (never two vertices
//! of one operand).
//!
//! Each edge and each stretch is a carrier: it is cut at the global
//! vertices on it into pieces. Carriers known to run together (a stretch
//! along an edge, an edge lying along another operand's edge, two
//! stretches on one curve) share their vertices, and their pieces between
//! the same two vertices are one global edge, which takes the curve of
//! its first member (an edge before a stretch, the target before a tool).
//!
//! Each face then has its boundary as global edges in loop order, and the
//! global edges that lie inside it: the stretches inside it, and other
//! operands' edges lying in it. Every one of them has a pcurve on it.

use super::{EdgeRef, FaceRef, Fail, VertexRef, face_tolerance};
use crate::geom::{Box3, Curve, Curve2, Point2, Point3};
use crate::intersect::broad::BoxTree;
use crate::intersect::{
    CurveHit, FaceMeet, Failure, Place, curve_surface, face_box, face_face, point_in_face,
};
use crate::tol::LINEAR_RESOLUTION;
use crate::topo::{Body, Sense};
use rayon::prelude::*;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// Rounding slack on the clustering rule, as in [`crate::intersect`].
const SLACK: f64 = 1e-10;

/// A touch this many tolerances from an edge's end is at the end: the
/// touch rule of [`crate::intersect::curve_surface`].
const TOUCH: f64 = 100.0;

/// A global vertex.
pub(crate) type GvId = usize;
/// A global edge.
pub(crate) type GeId = usize;

/// A vertex where operands meet, or an operand's own.
#[derive(Clone, Debug)]
pub(crate) struct Gv {
    pub(crate) point: Point3,
    /// Covers every point clustered into it.
    pub(crate) tolerance: f64,
    /// The operands' vertices it is, in order.
    pub(crate) originals: Vec<VertexRef>,
    /// The faces it was found on (by an edge hit, a stretch end, a touch).
    pub(crate) faces: BTreeSet<FaceRef>,
}

/// Where a global edge comes from: a piece of an operand's edge, or of a
/// stretch where two faces meet.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Member {
    Edge(EdgeRef),
    Stretch { pair: usize, index: usize },
}

/// An edge where operands meet, or a piece of an operand's edge.
#[derive(Clone, Debug)]
pub(crate) struct Ge {
    pub(crate) start: GvId,
    pub(crate) end: GvId,
    pub(crate) curve: Curve,
    /// Increasing, from `start` to `end`.
    pub(crate) interval: [f64; 2],
    pub(crate) tolerance: f64,
    /// Every carrier piece it stands for, its own first.
    pub(crate) members: Vec<Member>,
    /// Its pcurve on every face it bounds or lies in: a curve and the
    /// affine map from `curve`'s parameter to its own.
    pub(crate) pcurves: BTreeMap<FaceRef, (Curve2, [f64; 2])>,
}

/// One loop of a face's boundary.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Boundary {
    /// Global edges in loop order, each with the sense the loop runs it.
    Arcs(Vec<(GeId, Sense)>),
    /// A lone vertex.
    Lone(GvId),
}

/// What a face holds once everything is cut.
#[derive(Clone, Debug, Default)]
pub(crate) struct FaceWork {
    pub(crate) loops: Vec<Boundary>,
    /// Global edges inside the face, in order.
    pub(crate) interior: Vec<GeId>,
    /// Isolated vertices inside the face: points where another operand
    /// touches it.
    pub(crate) nodes: Vec<GvId>,
}

/// Faces of two operands on one surface.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Coincidence {
    pub(crate) a: FaceRef,
    pub(crate) b: FaceRef,
    /// The surfaces' natural normals agree.
    pub(crate) same: bool,
}

/// Everything where the operands meet.
#[derive(Clone, Debug, Default)]
pub(crate) struct Table {
    pub(crate) vertices: Vec<Gv>,
    pub(crate) edges: Vec<Ge>,
    /// Every face of every operand.
    pub(crate) faces: BTreeMap<FaceRef, FaceWork>,
    pub(crate) coincident: Vec<Coincidence>,
    /// The faces of each pair intersected, in pair order.
    pub(crate) pairs: Vec<(FaceRef, FaceRef)>,
}

/// Two faces' meeting, as [`face_face`] gave it.
struct Pair {
    a: FaceRef,
    b: FaceRef,
    meet: FaceMeet,
}

/// Where an edge meets another operand's face.
#[derive(Clone, Copy, Debug)]
struct Hit {
    edge: EdgeRef,
    t: f64,
    point: Point3,
    face: FaceRef,
    place: Place,
    radius: f64,
}

/// An edge lying on another operand's face's surface over `range`, its
/// middle placed on the face.
#[derive(Clone, Copy, Debug)]
struct OnHit {
    edge: EdgeRef,
    range: [f64; 2],
    face: FaceRef,
    place: Place,
}

/// An operand's vertex on another operand's face.
#[derive(Clone, Copy, Debug)]
struct Touch {
    vertex: VertexRef,
    face: FaceRef,
    place: Place,
}

/// Two stretches of one pair crossing: where the faces' surfaces touch
/// and the curves they meet in cross.
#[derive(Clone, Copy, Debug)]
struct Cross {
    pair: usize,
    stretches: [usize; 2],
    point: Point3,
    tolerance: f64,
}

/// A stretch crossing a third operand's face.
#[derive(Clone, Copy, Debug)]
struct Triple {
    pair: usize,
    index: usize,
    point: Point3,
    face: FaceRef,
}

/// A point to cluster into a global vertex, by priority: an operand's
/// vertex, an edge hit, then the rest.
#[derive(Clone, Copy, Debug)]
enum Cand {
    Vertex(VertexRef),
    Hit(usize),
    End {
        pair: usize,
        index: usize,
        end: usize,
    },
    Closed {
        pair: usize,
        index: usize,
    },
    Rim {
        pair: usize,
        rim: usize,
        end: usize,
    },
    Contact {
        pair: usize,
        index: usize,
    },
    Triple(usize),
    Cross(usize),
}

/// A curve the table cuts: an operand's edge or a stretch.
#[derive(Clone, Debug)]
struct Carrier {
    member: Member,
    curve: Curve,
    range: [f64; 2],
    closed: bool,
    tolerance: f64,
    /// Global vertices on it, by parameter.
    points: Vec<(f64, GvId)>,
}

/// A piece of a carrier between two of its vertices.
#[derive(Clone, Copy, Debug)]
struct Piece {
    carrier: usize,
    range: [f64; 2],
    start: GvId,
    end: GvId,
}

/// Builds the table for `ops`, the target first.
pub(crate) fn build(ops: &[Body]) -> Result<Table, Fail> {
    let faces: Vec<FaceRef> = ops
        .iter()
        .enumerate()
        .flat_map(|(op, b)| b.faces().map(move |face| FaceRef { op, face }))
        .collect();
    let face_boxes: Vec<(Box3, usize)> = faces
        .iter()
        .enumerate()
        .map(|(i, f)| {
            let b = &ops[f.op];
            (face_box(b, f.face).grow(face_tolerance(b, f.face)), i)
        })
        .collect();
    let tree = BoxTree::new(face_boxes.clone());
    let face_pairs: Vec<(usize, usize)> = tree
        .self_pairs()
        .into_iter()
        .filter(|&(i, j)| faces[i].op != faces[j].op)
        .collect();
    // Every pair's meeting, in pair order.
    let pairs: Vec<Pair> = face_pairs
        .par_iter()
        .map(|&(i, j)| {
            let (a, b) = (faces[i], faces[j]);
            face_face((&ops[a.op], a.face), (&ops[b.op], b.face))
                .map(|meet| Pair { a, b, meet })
                .map_err(|e| fail(e, a, b))
        })
        .collect::<Result<_, _>>()?;
    // Edges against other operands' faces.
    let edges: Vec<EdgeRef> = ops
        .iter()
        .enumerate()
        .flat_map(|(op, b)| b.edges().map(move |edge| EdgeRef { op, edge }))
        .collect();
    let edge_boxes: Vec<(Box3, usize)> = edges
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let b = &ops[e.op];
            let x = &b.edges[e.edge];
            (
                b.curves[x.curve]
                    .bounds(x.interval)
                    .grow(x.tolerance.get().max(LINEAR_RESOLUTION)),
                i,
            )
        })
        .collect();
    let edge_faces: Vec<(usize, usize)> = BoxTree::new(edge_boxes)
        .pairs_with(&tree)
        .into_iter()
        .filter(|&(i, j)| edges[i].op != faces[j].op && analytic(ops, faces[j]))
        .collect();
    let found: Vec<(Vec<Hit>, Vec<OnHit>)> = edge_faces
        .par_iter()
        .map(|&(i, j)| edge_hits(ops, edges[i], faces[j]))
        .collect::<Result<_, _>>()?;
    let mut hits = Vec::new();
    let mut ons = Vec::new();
    for (h, o) in found {
        hits.extend(h);
        ons.extend(o);
    }
    // Vertices on other operands' faces.
    let verts: Vec<VertexRef> = ops
        .iter()
        .enumerate()
        .flat_map(|(op, b)| b.vertices().map(move |vertex| VertexRef { op, vertex }))
        .collect();
    let vert_boxes: Vec<(Box3, usize)> = verts
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let x = &ops[v.op].vertices[v.vertex];
            (Box3::from_point(x.point).grow(x.tolerance.get()), i)
        })
        .collect();
    let vert_faces: Vec<(usize, usize)> = BoxTree::new(vert_boxes)
        .pairs_with(&tree)
        .into_iter()
        .filter(|&(i, j)| verts[i].op != faces[j].op && analytic(ops, faces[j]))
        .collect();
    let touches: Vec<Touch> = vert_faces
        .par_iter()
        .map(|&(i, j)| touch(ops, verts[i], faces[j]))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .flatten()
        .collect();
    // Stretches crossing a third operand's faces.
    let triples = if ops.len() >= 3 {
        triple_points(ops, &pairs, &faces, &tree)?
    } else {
        Vec::new()
    };
    let crosses = crossings_of(ops, &pairs)?;
    let mut table = Table {
        pairs: pairs.iter().map(|p| (p.a, p.b)).collect(),
        ..Table::default()
    };
    for p in &pairs {
        if let FaceMeet::Coincident { same, .. } = p.meet {
            table.coincident.push(Coincidence {
                a: p.a,
                b: p.b,
                same,
            });
        }
    }
    // Cluster every point into global vertices.
    let (cands, points) = candidates(ops, &pairs, &hits, &triples, &crosses);
    let cluster = clusters(&cands, &points);
    let mut gv_of = vec![0; cands.len()];
    let mut roots: BTreeMap<usize, GvId> = BTreeMap::new();
    for (i, &r) in cluster.iter().enumerate() {
        let next = roots.len();
        let g = *roots.entry(r).or_insert(next);
        gv_of[i] = g;
    }
    let mut gvs: Vec<Option<Gv>> = vec![None; roots.len()];
    for (i, c) in cands.iter().enumerate() {
        let (p, radius) = points[i];
        let g = gvs[gv_of[i]].get_or_insert_with(|| Gv {
            point: p,
            tolerance: radius,
            originals: Vec::new(),
            faces: BTreeSet::new(),
        });
        g.tolerance = g.tolerance.max(radius + g.point.distance(p));
        match *c {
            Cand::Vertex(v) => g.originals.push(v),
            Cand::Hit(h) => {
                g.faces.insert(hits[h].face);
            }
            Cand::End { pair, .. } | Cand::Closed { pair, .. } | Cand::Contact { pair, .. } => {
                g.faces.insert(pairs[pair].a);
                g.faces.insert(pairs[pair].b);
            }
            Cand::Rim { pair, .. } => {
                g.faces.insert(pairs[pair].a);
                g.faces.insert(pairs[pair].b);
            }
            Cand::Triple(t) => {
                let tr = &triples[t];
                g.faces.insert(pairs[tr.pair].a);
                g.faces.insert(pairs[tr.pair].b);
                g.faces.insert(tr.face);
            }
            Cand::Cross(x) => {
                g.faces.insert(pairs[crosses[x].pair].a);
                g.faces.insert(pairs[crosses[x].pair].b);
            }
        }
    }
    table.vertices = gvs.into_iter().map(|g| g.expect("a cluster")).collect();
    let vertex_gv: BTreeMap<VertexRef, GvId> = cands
        .iter()
        .enumerate()
        .filter_map(|(i, c)| match c {
            Cand::Vertex(v) => Some((*v, gv_of[i])),
            _ => None,
        })
        .collect();
    // Carriers and the vertices on them.
    let mut carriers: Vec<Carrier> = Vec::new();
    let mut edge_carrier: BTreeMap<EdgeRef, usize> = BTreeMap::new();
    for &e in &edges {
        let b = &ops[e.op];
        let x = &b.edges[e.edge];
        edge_carrier.insert(e, carriers.len());
        let mut points = vec![(
            x.interval[0],
            vertex_gv[&VertexRef {
                op: e.op,
                vertex: x.start,
            }],
        )];
        // Only a whole period wraps round; a closed edge on a clamped
        // curve whose ends meet has its vertex at both ends.
        let curve = &b.curves[x.curve];
        let closed = x.start == x.end && whole_period(curve, x.interval);
        if !closed {
            points.push((
                x.interval[1],
                vertex_gv[&VertexRef {
                    op: e.op,
                    vertex: x.end,
                }],
            ));
        }
        carriers.push(Carrier {
            member: Member::Edge(e),
            curve: curve.clone(),
            range: x.interval,
            closed,
            tolerance: x.tolerance.get(),
            points,
        });
    }
    let mut stretch_carrier: BTreeMap<(usize, usize), usize> = BTreeMap::new();
    for (pi, p) in pairs.iter().enumerate() {
        if let FaceMeet::Meet { stretches, .. } = &p.meet {
            for (k, s) in stretches.iter().enumerate() {
                stretch_carrier.insert((pi, k), carriers.len());
                carriers.push(Carrier {
                    member: Member::Stretch { pair: pi, index: k },
                    curve: s.curve.clone(),
                    range: s.range,
                    closed: whole_period(&s.curve, s.range),
                    tolerance: s.tolerance,
                    points: Vec::new(),
                });
            }
        }
    }
    for (i, c) in cands.iter().enumerate() {
        let g = gv_of[i];
        match *c {
            Cand::Vertex(_) => {}
            Cand::Hit(h) => {
                let hit = &hits[h];
                carriers[edge_carrier[&hit.edge]].points.push((hit.t, g));
                if let Place::OnEdge(e) = hit.place {
                    let other = edge_carrier[&EdgeRef {
                        op: hit.face.op,
                        edge: e,
                    }];
                    add_point(&mut carriers[other], hit.point, g);
                }
            }
            Cand::End { pair, index, end } => {
                let c = stretch_carrier[&(pair, index)];
                let t = carriers[c].range[end];
                carriers[c].points.push((t, g));
            }
            // A closed stretch's start is a vertex only if nothing else
            // is on it: see below.
            Cand::Closed { .. } => {}
            Cand::Rim { pair, rim, end } => {
                let p = &pairs[pair];
                let FaceMeet::Coincident { rims, .. } = &p.meet else {
                    unreachable!("rims only of coincident faces")
                };
                let r = &rims[rim];
                let of = if r.of == 0 { p.a } else { p.b };
                let c = edge_carrier[&EdgeRef {
                    op: of.op,
                    edge: r.edge,
                }];
                carriers[c].points.push((r.range[end], g));
                let other = if r.of == 0 { p.b } else { p.a };
                if let Place::OnEdge(e) = r.place {
                    let oc = edge_carrier[&EdgeRef {
                        op: other.op,
                        edge: e,
                    }];
                    add_point(&mut carriers[oc], points[i].0, g);
                }
            }
            Cand::Contact { pair, index } => {
                let p = &pairs[pair];
                let FaceMeet::Meet { contacts, .. } = &p.meet else {
                    unreachable!("contacts only of meeting faces")
                };
                for (k, f) in [p.a, p.b].into_iter().enumerate() {
                    if let Place::OnEdge(e) = contacts[index].places[k] {
                        let c = edge_carrier[&EdgeRef { op: f.op, edge: e }];
                        add_point(&mut carriers[c], points[i].0, g);
                    }
                }
            }
            Cand::Triple(t) => {
                let tr = &triples[t];
                let c = stretch_carrier[&(tr.pair, tr.index)];
                add_point(&mut carriers[c], tr.point, g);
            }
            Cand::Cross(x) => {
                let cr = &crosses[x];
                for k in cr.stretches {
                    let c = stretch_carrier[&(cr.pair, k)];
                    add_point(&mut carriers[c], cr.point, g);
                }
            }
        }
    }
    for t in &touches {
        if let Place::OnEdge(e) = t.place {
            let c = edge_carrier[&EdgeRef {
                op: t.face.op,
                edge: e,
            }];
            let g = vertex_gv[&t.vertex];
            add_point(&mut carriers[c], table.vertices[g].point, g);
        }
    }
    // Carriers that run together share their vertices.
    let mut related: Vec<(usize, usize)> = Vec::new();
    for (pi, p) in pairs.iter().enumerate() {
        match &p.meet {
            FaceMeet::Meet { stretches, .. } => {
                for (k, s) in stretches.iter().enumerate() {
                    for (side, f) in [p.a, p.b].into_iter().enumerate() {
                        if let Place::OnEdge(e) = s.places[side] {
                            related.push((
                                stretch_carrier[&(pi, k)],
                                edge_carrier[&EdgeRef { op: f.op, edge: e }],
                            ));
                        }
                    }
                }
            }
            FaceMeet::Coincident { rims, .. } => {
                for r in rims {
                    let (of, other) = if r.of == 0 { (p.a, p.b) } else { (p.b, p.a) };
                    if let Place::OnEdge(e) = r.place {
                        related.push((
                            edge_carrier[&EdgeRef {
                                op: of.op,
                                edge: r.edge,
                            }],
                            edge_carrier[&EdgeRef {
                                op: other.op,
                                edge: e,
                            }],
                        ));
                    }
                }
            }
        }
    }
    for o in &ons {
        if let Place::OnEdge(e) = o.place {
            related.push((
                edge_carrier[&o.edge],
                edge_carrier[&EdgeRef {
                    op: o.face.op,
                    edge: e,
                }],
            ));
        }
    }
    related.extend(stretches_together(&pairs, &stretch_carrier, &carriers));
    related.sort_unstable();
    related.dedup();
    share_points(&mut carriers, &related, &table.vertices);
    // A closed stretch with no vertex on it gets one at its start (and so
    // does every carrier running along it).
    for (i, c) in cands.iter().enumerate() {
        if let Cand::Closed { pair, index } = *c {
            let k = stretch_carrier[&(pair, index)];
            if carriers[k].points.is_empty() {
                let t = carriers[k].range[0];
                carriers[k].points.push((t, gv_of[i]));
            }
        }
    }
    share_points(&mut carriers, &related, &table.vertices);
    // Pieces, and the global edges they make.
    let mut pieces: Vec<Piece> = Vec::new();
    let mut carrier_pieces: Vec<Vec<usize>> = vec![Vec::new(); carriers.len()];
    for (ci, c) in carriers.iter_mut().enumerate() {
        c.points
            .sort_by(|x, y| x.0.total_cmp(&y.0).then(x.1.cmp(&y.1)));
        // A vertex repeated at one point of the carrier is one; the same
        // vertex again further along (a loop's ends meeting) is kept, but
        // a periodic carrier's own vertex coming back at the period's end
        // is its start again.
        let reach = c.tolerance.max(LINEAR_RESOLUTION) * 4.0 + SLACK;
        let curve = c.curve.clone();
        c.points.dedup_by(|x, y| {
            x.1 == y.1 && curve.point(0.5 * (x.0 + y.0)).distance(curve.point(y.0)) <= reach
        });
        if c.closed && c.points.len() > 1 && c.points[c.points.len() - 1].1 == c.points[0].1 {
            c.points.pop();
        }
        let n = c.points.len();
        if n == 0 {
            continue;
        }
        let period = c.curve.period();
        let mut add = |range: [f64; 2], start: GvId, end: GvId| {
            carrier_pieces[ci].push(pieces.len());
            pieces.push(Piece {
                carrier: ci,
                range,
                start,
                end,
            });
        };
        if c.closed {
            let p = period.unwrap_or(c.range[1] - c.range[0]);
            for k in 0..n {
                let (t0, g0) = c.points[k];
                let (t1, g1) = if k + 1 < n {
                    c.points[k + 1]
                } else {
                    (c.points[0].0 + p, c.points[0].1)
                };
                add([t0, t1], g0, g1);
            }
        } else {
            // A piece from a vertex back to itself is a loop (a closed
            // curve's clamped ends) unless it is no length at all.
            let curve = c.curve.clone();
            for w in c.points.windows(2) {
                let mid = curve.point(0.5 * (w[0].0 + w[1].0));
                if w[0].1 != w[1].1 || mid.distance(curve.point(w[0].0)) > reach {
                    add([w[0].0, w[1].0], w[0].1, w[1].1);
                }
            }
        }
    }
    let piece_ge = identify(&pieces, &carriers, &related);
    let ge_count = piece_ge.iter().copied().max().map_or(0, |m| m + 1);
    let mut ges: Vec<Option<Ge>> = vec![None; ge_count];
    // Members in carrier order, so each edge's first member is the
    // lowest: carriers are made edges first, operand by operand.
    for (pi, piece) in pieces.iter().enumerate() {
        let c = &carriers[piece.carrier];
        let ge = ges[piece_ge[pi]].get_or_insert_with(|| Ge {
            start: piece.start,
            end: piece.end,
            curve: c.curve.clone(),
            interval: piece.range,
            tolerance: c.tolerance.max(LINEAR_RESOLUTION),
            members: Vec::new(),
            pcurves: BTreeMap::new(),
        });
        ge.tolerance = ge.tolerance.max(c.tolerance);
        ge.members.push(c.member);
    }
    table.edges = ges.into_iter().map(|g| g.expect("a piece")).collect();
    // Each face's boundary and what lies inside it.
    for &f in &faces {
        table.faces.insert(
            f,
            boundary(
                ops,
                f,
                &carriers,
                &carrier_pieces,
                &edge_carrier,
                &pieces,
                &piece_ge,
                &table.edges,
                &vertex_gv,
            ),
        );
    }
    let add_interior = |table: &mut Table, f: FaceRef, ge: GeId| {
        let work = table.faces.get_mut(&f).expect("every face has work");
        if !work.interior.contains(&ge) {
            work.interior.push(ge);
        }
    };
    for (pi, p) in pairs.iter().enumerate() {
        match &p.meet {
            FaceMeet::Meet {
                stretches,
                contacts,
            } => {
                for (k, s) in stretches.iter().enumerate() {
                    let c = stretch_carrier[&(pi, k)];
                    for (side, f) in [p.a, p.b].into_iter().enumerate() {
                        if s.places[side] == Place::Inside {
                            for &q in &carrier_pieces[c] {
                                add_interior(&mut table, f, piece_ge[q]);
                            }
                        }
                    }
                }
                for (k, ct) in contacts.iter().enumerate() {
                    if ct.places == [Place::Inside; 2] {
                        let g = cands
                            .iter()
                            .position(|c| matches!(c, Cand::Contact { pair, index } if *pair == pi && *index == k))
                            .map(|i| gv_of[i])
                            .expect("a contact candidate");
                        for f in [p.a, p.b] {
                            let w = table.faces.get_mut(&f).expect("work");
                            if !w.nodes.contains(&g) {
                                w.nodes.push(g);
                            }
                        }
                    }
                }
            }
            FaceMeet::Coincident { rims, .. } => {
                for r in rims {
                    let (of, other) = if r.of == 0 { (p.a, p.b) } else { (p.b, p.a) };
                    if r.place != Place::Inside {
                        continue;
                    }
                    let c = edge_carrier[&EdgeRef {
                        op: of.op,
                        edge: r.edge,
                    }];
                    for &q in &carrier_pieces[c] {
                        if inside_range(&pieces[q], r.range, &carriers[c]) {
                            add_interior(&mut table, other, piece_ge[q]);
                        }
                    }
                }
            }
        }
    }
    // An edge lying on another operand's face's surface: each of its
    // pieces in the stretch, by its own middle (the stretch may run out of
    // the face and back).
    let mut jobs: Vec<(usize, FaceRef)> = Vec::new();
    for o in &ons {
        let c = edge_carrier[&o.edge];
        for &q in &carrier_pieces[c] {
            if inside_range(&pieces[q], o.range, &carriers[c]) && !jobs.contains(&(q, o.face)) {
                jobs.push((q, o.face));
            }
        }
    }
    let placed: Vec<Place> = jobs
        .par_iter()
        .map(|&(q, f)| {
            let c = &carriers[pieces[q].carrier];
            let mid = c
                .curve
                .point((pieces[q].range[0] + pieces[q].range[1]) / 2.0);
            let fb = &ops[f.op];
            let surface = &fb.surfaces[fb.faces[f.face].surface];
            let (u, v) = surface.project(mid).param;
            let own = match c.member {
                Member::Edge(e) => face_of_edge(ops, e),
                Member::Stretch { pair, .. } => pairs[pair].a,
            };
            point_in_face(fb, f.face, Point2::new(u, v)).map_err(|e| fail(e, own, f))
        })
        .collect::<Result<_, _>>()?;
    for ((q, f), place) in jobs.into_iter().zip(placed) {
        if place == Place::Inside {
            add_interior(&mut table, f, piece_ge[q]);
        }
    }
    for t in &touches {
        if t.place == Place::Inside {
            let g = vertex_gv[&t.vertex];
            let w = table.faces.get_mut(&t.face).expect("work");
            if !w.nodes.contains(&g) {
                w.nodes.push(g);
            }
        }
    }
    // An edge on a face's boundary is not inside it too.
    for w in table.faces.values_mut() {
        let on: BTreeSet<GeId> = w
            .loops
            .iter()
            .flat_map(|l| match l {
                Boundary::Arcs(a) => a.iter().map(|x| x.0).collect(),
                Boundary::Lone(_) => Vec::new(),
            })
            .collect();
        w.interior.retain(|g| !on.contains(g));
    }
    // A touching vertex at the end of an arc of the face is not on its own.
    let edges = &table.edges;
    for w in table.faces.values_mut() {
        let ends: BTreeSet<GvId> = w
            .loops
            .iter()
            .flat_map(|l| match l {
                Boundary::Arcs(a) => a.iter().map(|x| x.0).collect(),
                Boundary::Lone(_) => Vec::new(),
            })
            .chain(w.interior.iter().copied())
            .flat_map(|g| [edges[g].start, edges[g].end])
            .collect();
        w.nodes.retain(|g| !ends.contains(g));
    }
    pcurves(ops, &pairs, &mut table)?;
    Ok(table)
}

/// The boolean's failure for an intersection failure between two faces.
fn fail(e: Failure, a: FaceRef, b: FaceRef) -> Fail {
    match e {
        Failure::Unsupported => Fail::Unsupported(a, b),
        Failure::Uncertain => Fail::Uncertain(a, b),
    }
}

/// Whether a face's surface has an implicit form, or is a pipe: only
/// those take part in edge hits and touches. Faces on other surfaces must
/// be certified apart by [`face_face`], so nothing can hit them.
fn analytic(ops: &[Body], f: FaceRef) -> bool {
    let b = &ops[f.op];
    matches!(
        b.surfaces[b.faces[f.face].surface],
        crate::geom::Surface::Plane(_)
            | crate::geom::Surface::Cylinder(_)
            | crate::geom::Surface::Cone(_)
            | crate::geom::Surface::Sphere(_)
            | crate::geom::Surface::Torus(_)
            | crate::geom::Surface::Pipe(_)
    )
}

/// A face of `f`'s operand that an edge bounds, for naming a failure.
fn face_of_edge(ops: &[Body], e: EdgeRef) -> FaceRef {
    let b = &ops[e.op];
    let fin = b.edges[e.edge].fin;
    FaceRef {
        op: e.op,
        face: b.loops[b.fins[fin].loop_].face,
    }
}

/// Where edge `e` meets face `f` of another operand: its crossings and
/// touches on or in the face, and the stretches it lies on the surface.
fn edge_hits(ops: &[Body], e: EdgeRef, f: FaceRef) -> Result<(Vec<Hit>, Vec<OnHit>), Fail> {
    let (eb, fb) = (&ops[e.op], &ops[f.op]);
    let x = &eb.edges[e.edge];
    let curve = &eb.curves[x.curve];
    let surface = &fb.surfaces[fb.faces[f.face].surface];
    let tol = x.tolerance.get().max(face_tolerance(fb, f.face));
    let fail_here = |err: Failure| fail(err, face_of_edge(ops, e), f);
    let found = curve_surface(curve, x.interval, surface, tol).map_err(fail_here)?;
    let place_of = |p: Point3| -> Result<Place, Fail> {
        let (u, v) = surface.project(p).param;
        point_in_face(fb, f.face, Point2::new(u, v)).map_err(fail_here)
    };
    let mut hits = Vec::new();
    let mut ons = Vec::new();
    // A touch is placed only to within rounding's reach of a double root,
    // more than the tolerance: one within the touch rule's length of the
    // edge's end is at that end.
    let snap = |t: f64, point: Point3, touch: bool| -> (f64, Point3) {
        if touch {
            for (end, v) in [(x.interval[0], x.start), (x.interval[1], x.end)] {
                let p = eb.vertices[v].point;
                if p.distance(point) <= TOUCH * tol {
                    return (end, p);
                }
            }
        }
        (t, point)
    };
    for h in found {
        match h {
            CurveHit::Point {
                t,
                point,
                before,
                after,
            } => {
                let touch = before == after;
                let (t, point) = snap(t, point, touch);
                let place = place_of(point)?;
                if place != Place::Outside {
                    hits.push(Hit {
                        edge: e,
                        t,
                        point,
                        face: f,
                        place,
                        // A touch is known only to within the rule's length.
                        radius: if touch { TOUCH * tol } else { tol },
                    });
                }
            }
            CurveHit::On { t: [t0, t1] } => {
                let mid = curve.point((t0 + t1) / 2.0);
                let place = place_of(mid)?;
                for t in [t0, t1] {
                    let point = curve.point(t);
                    let at = place_of(point)?;
                    if at != Place::Outside {
                        hits.push(Hit {
                            edge: e,
                            t,
                            point,
                            face: f,
                            place: at,
                            radius: tol,
                        });
                    }
                }
                if place != Place::Outside {
                    ons.push(OnHit {
                        edge: e,
                        range: [t0, t1],
                        face: f,
                        place,
                    });
                }
            }
        }
    }
    Ok((hits, ons))
}

/// Vertex `v` on face `f` of another operand, if it lies on the face.
fn touch(ops: &[Body], v: VertexRef, f: FaceRef) -> Result<Option<Touch>, Fail> {
    let (vb, fb) = (&ops[v.op], &ops[f.op]);
    let x = &vb.vertices[v.vertex];
    let surface = &fb.surfaces[fb.faces[f.face].surface];
    if surface.distance(x.point) > x.tolerance.get() + face_tolerance(fb, f.face) {
        return Ok(None);
    }
    let (u, w) = surface.project(x.point).param;
    let mine = vb
        .vertex(v.vertex)
        .and_then(|vx| vx.edge)
        .map(|e| face_of_edge(ops, EdgeRef { op: v.op, edge: e }))
        .unwrap_or(f);
    let place = point_in_face(fb, f.face, Point2::new(u, w)).map_err(|e| fail(e, mine, f))?;
    Ok((place != Place::Outside).then_some(Touch {
        vertex: v,
        face: f,
        place,
    }))
}

/// Where stretches cross faces of a third operand, inside them.
fn triple_points(
    ops: &[Body],
    pairs: &[Pair],
    faces: &[FaceRef],
    tree: &BoxTree,
) -> Result<Vec<Triple>, Fail> {
    let mut jobs: Vec<(usize, usize, FaceRef)> = Vec::new();
    for (pi, p) in pairs.iter().enumerate() {
        let FaceMeet::Meet { stretches, .. } = &p.meet else {
            continue;
        };
        for (k, s) in stretches.iter().enumerate() {
            let b = s.curve.bounds(s.range).grow(s.tolerance + SLACK);
            let probe = BoxTree::new(vec![(b, 0)]);
            for (_, j) in probe.pairs_with(tree) {
                let f = faces[j];
                if f.op != p.a.op && f.op != p.b.op && analytic(ops, f) {
                    jobs.push((pi, k, f));
                }
            }
        }
    }
    let found: Vec<Vec<Triple>> = jobs
        .par_iter()
        .map(|&(pi, k, f)| {
            let p = &pairs[pi];
            let FaceMeet::Meet { stretches, .. } = &p.meet else {
                unreachable!()
            };
            let s = &stretches[k];
            let fb = &ops[f.op];
            let surface = &fb.surfaces[fb.faces[f.face].surface];
            let tol = s.tolerance.max(face_tolerance(fb, f.face));
            let hits =
                curve_surface(&s.curve, s.range, surface, tol).map_err(|e| fail(e, p.a, f))?;
            let mut out = Vec::new();
            for h in hits {
                let points = match h {
                    CurveHit::Point { point, .. } => vec![point],
                    CurveHit::On { t } => t.iter().map(|&t| s.curve.point(t)).collect(),
                };
                for point in points {
                    let (u, v) = surface.project(point).param;
                    let place = point_in_face(fb, f.face, Point2::new(u, v))
                        .map_err(|e| fail(e, p.a, f))?;
                    if place != Place::Outside {
                        out.push(Triple {
                            pair: pi,
                            index: k,
                            point,
                            face: f,
                        });
                    }
                }
            }
            Ok(out)
        })
        .collect::<Result<_, _>>()?;
    Ok(found.into_iter().flatten().collect())
}

/// Where stretches of one pair cross each other, found in the first
/// face's parameter plane by certified plane-curve crossings at every
/// shift by its surface's periods.
fn crossings_of(ops: &[Body], pairs: &[Pair]) -> Result<Vec<Cross>, Fail> {
    use crate::geom::intersect2::{Meet2, crossings};
    use crate::intersect::{shifted, shifts};
    let mut out = Vec::new();
    for (pi, p) in pairs.iter().enumerate() {
        let FaceMeet::Meet { stretches, .. } = &p.meet else {
            continue;
        };
        if stretches.len() < 2 {
            continue;
        }
        let b = &ops[p.a.op];
        let surface = &b.surfaces[b.faces[p.a.face].surface];
        let (pu, pv) = surface.periods();
        for i in 0..stretches.len() {
            for j in i + 1..stretches.len() {
                let (x, y) = (&stretches[i], &stretches[j]);
                let tol = x.tolerance.max(y.tolerance);
                let reach = x.curve.bounds(x.range).grow(tol);
                if !reach.intersects(&y.curve.bounds(y.range).grow(tol)) {
                    continue;
                }
                let (cx, mx) = &x.pcurves[0];
                let (cy, my) = &y.pcurves[0];
                let rx = x.range.map(|t| mx[0] + mx[1] * t);
                let ry = y.range.map(|t| my[0] + my[1] * t);
                for shift in shifts(cx, rx, cy, ry, [pu, pv]) {
                    let moved = shifted(cy, shift).ok_or(Fail::Uncertain(p.a, p.b))?;
                    let found =
                        crossings((cx, rx), (&moved, ry), tol).ok_or(Fail::Uncertain(p.a, p.b))?;
                    for m in found {
                        let Meet2::Point { ta, .. } = m else {
                            continue;
                        };
                        let t = (ta - mx[0]) / mx[1];
                        out.push(Cross {
                            pair: pi,
                            stretches: [i, j],
                            point: x.curve.point(t),
                            tolerance: tol,
                        });
                    }
                }
            }
        }
    }
    Ok(out)
}

/// Every point to cluster, with its position and radius.
fn candidates(
    ops: &[Body],
    pairs: &[Pair],
    hits: &[Hit],
    triples: &[Triple],
    crosses: &[Cross],
) -> (Vec<Cand>, Vec<(Point3, f64)>) {
    let mut cands = Vec::new();
    let mut points = Vec::new();
    for (op, b) in ops.iter().enumerate() {
        for v in b.vertices() {
            let x = &b.vertices[v];
            cands.push(Cand::Vertex(VertexRef { op, vertex: v }));
            points.push((x.point, x.tolerance.get()));
        }
    }
    for (i, h) in hits.iter().enumerate() {
        cands.push(Cand::Hit(i));
        points.push((h.point, h.radius));
    }
    for (pi, p) in pairs.iter().enumerate() {
        let tol_a = face_tolerance(&ops[p.a.op], p.a.face);
        let tol_b = face_tolerance(&ops[p.b.op], p.b.face);
        match &p.meet {
            FaceMeet::Meet {
                stretches,
                contacts,
            } => {
                for (k, s) in stretches.iter().enumerate() {
                    let r = s.tolerance + SLACK;
                    if whole_period(&s.curve, s.range) {
                        cands.push(Cand::Closed { pair: pi, index: k });
                        points.push((s.curve.point(s.range[0]), r));
                    } else {
                        for end in 0..2 {
                            cands.push(Cand::End {
                                pair: pi,
                                index: k,
                                end,
                            });
                            points.push((s.curve.point(s.range[end]), r));
                        }
                    }
                }
                for (k, c) in contacts.iter().enumerate() {
                    cands.push(Cand::Contact { pair: pi, index: k });
                    points.push((c.point, tol_a.max(tol_b)));
                }
            }
            FaceMeet::Coincident { rims, .. } => {
                for (k, r) in rims.iter().enumerate() {
                    let of = if r.of == 0 { p.a } else { p.b };
                    let b = &ops[of.op];
                    let x = &b.edges[r.edge];
                    let curve = &b.curves[x.curve];
                    for end in 0..2 {
                        cands.push(Cand::Rim {
                            pair: pi,
                            rim: k,
                            end,
                        });
                        points.push((
                            curve.point(r.range[end]),
                            x.tolerance.get().max(tol_a.max(tol_b)),
                        ));
                    }
                }
            }
        }
    }
    for (i, t) in triples.iter().enumerate() {
        cands.push(Cand::Triple(i));
        let p = &pairs[t.pair];
        let FaceMeet::Meet { stretches, .. } = &p.meet else {
            unreachable!()
        };
        points.push((t.point, stretches[t.index].tolerance + SLACK));
    }
    for (i, c) in crosses.iter().enumerate() {
        cands.push(Cand::Cross(i));
        points.push((c.point, c.tolerance + SLACK));
    }
    (cands, points)
}

/// Each candidate's cluster, as the index of its cluster's first member.
/// Two candidates are one when closer than the sum of their radii; two
/// vertices of one operand never are.
fn clusters(cands: &[Cand], points: &[(Point3, f64)]) -> Vec<usize> {
    let n = cands.len();
    let mut parent: Vec<usize> = (0..n).collect();
    let mut ops: Vec<Vec<usize>> = cands
        .iter()
        .map(|c| match c {
            Cand::Vertex(v) => vec![v.op],
            _ => Vec::new(),
        })
        .collect();
    fn find(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
    let boxes: Vec<(Box3, usize)> = points
        .iter()
        .enumerate()
        .map(|(i, &(p, r))| (Box3::from_point(p).grow(r + SLACK), i))
        .collect();
    for (i, j) in BoxTree::new(boxes).self_pairs() {
        let (pi, ri) = points[i];
        let (pj, rj) = points[j];
        if pi.distance(pj) > ri + rj + SLACK {
            continue;
        }
        let (a, b) = (find(&mut parent, i), find(&mut parent, j));
        if a == b || ops[a].iter().any(|o| ops[b].contains(o)) {
            continue;
        }
        let (lo, hi) = (a.min(b), a.max(b));
        parent[hi] = lo;
        let moved = std::mem::take(&mut ops[hi]);
        ops[lo].extend(moved);
    }
    (0..n).map(|i| find(&mut parent, i)).collect()
}

/// Whether `range` is a whole period of a periodic curve.
fn whole_period(c: &Curve, [r0, r1]: [f64; 2]) -> bool {
    c.period().is_some_and(|p| (r1 - r0 - p).abs() <= 1e-12 * p)
}

/// The parameter of `p` on a carrier, within its range, if it lies on it.
fn param_on(c: &Carrier, p: Point3, reach: f64) -> Option<f64> {
    let [r0, r1] = c.range;
    let foot = c.curve.project(p);
    let mut t = foot.param;
    if let Some(per) = c.curve.period() {
        t = r0 + (t - r0).rem_euclid(per);
    }
    let slack = 1e-12 * (r1 - r0).abs().max(1.0);
    if foot.distance <= reach && t >= r0 - slack && t <= r1 + slack {
        return Some(t.clamp(r0, r1));
    }
    // Near an end, the projection may land just past it.
    [r0, r1]
        .into_iter()
        .find(|&end| c.curve.point(end).distance(p) <= reach)
}

/// Adds global vertex `g` at `p` to a carrier, at its projection.
fn add_point(c: &mut Carrier, p: Point3, g: GvId) {
    if c.points.iter().any(|x| x.1 == g) {
        return;
    }
    let reach = c.tolerance.max(LINEAR_RESOLUTION) * 4.0 + SLACK;
    if let Some(t) = param_on(c, p, reach) {
        c.points.push((t, g));
    }
}

/// Stretches on one face that run along one curve: two coincident faces
/// of other operands both crossing it.
fn stretches_together(
    pairs: &[Pair],
    stretch_carrier: &BTreeMap<(usize, usize), usize>,
    carriers: &[Carrier],
) -> Vec<(usize, usize)> {
    let mut by_face: BTreeMap<FaceRef, Vec<usize>> = BTreeMap::new();
    for (pi, p) in pairs.iter().enumerate() {
        if let FaceMeet::Meet { stretches, .. } = &p.meet {
            for k in 0..stretches.len() {
                let c = stretch_carrier[&(pi, k)];
                by_face.entry(p.a).or_default().push(c);
                by_face.entry(p.b).or_default().push(c);
            }
        }
    }
    let mut out = Vec::new();
    for list in by_face.values() {
        for (i, &x) in list.iter().enumerate() {
            for &y in &list[i + 1..] {
                if together(&carriers[x], &carriers[y]) {
                    out.push((x.min(y), x.max(y)));
                }
            }
        }
    }
    out
}

/// Whether two carriers run along each other: three points of the
/// shorter within both tolerances of the other.
fn together(a: &Carrier, b: &Carrier) -> bool {
    let reach = a.tolerance + b.tolerance + SLACK;
    let ba = a.curve.bounds(a.range).grow(reach);
    let bb = b.curve.bounds(b.range).grow(reach);
    if !ba.intersects(&bb) {
        return false;
    }
    [0.25, 0.5, 0.75].iter().all(|&f| {
        let p = a.curve.point(a.range[0] + (a.range[1] - a.range[0]) * f);
        param_on(b, p, reach).is_some()
    }) || [0.25, 0.5, 0.75].iter().all(|&f| {
        let p = b.curve.point(b.range[0] + (b.range[1] - b.range[0]) * f);
        param_on(a, p, reach).is_some()
    })
}

/// Shares vertices between related carriers until nothing changes.
fn share_points(carriers: &mut [Carrier], related: &[(usize, usize)], gvs: &[Gv]) {
    for _ in 0..8 {
        let mut changed = false;
        for &(x, y) in related {
            for (from, to) in [(x, y), (y, x)] {
                let pts: Vec<GvId> = carriers[from].points.iter().map(|p| p.1).collect();
                for g in pts {
                    if carriers[to].points.iter().any(|p| p.1 == g) {
                        continue;
                    }
                    let before = carriers[to].points.len();
                    add_point(&mut carriers[to], gvs[g].point, g);
                    changed |= carriers[to].points.len() > before;
                }
            }
        }
        if !changed {
            break;
        }
    }
}

/// Each piece's global edge: pieces of related carriers between the same
/// two vertices whose middles lie on each other are one.
fn identify(pieces: &[Piece], carriers: &[Carrier], related: &[(usize, usize)]) -> Vec<GeId> {
    let mut parent: Vec<usize> = (0..carriers.len()).collect();
    fn find(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
    for &(x, y) in related {
        let (a, b) = (find(&mut parent, x), find(&mut parent, y));
        if a != b {
            parent[a.max(b)] = a.min(b);
        }
    }
    let mut groups: BTreeMap<(usize, GvId, GvId), Vec<usize>> = BTreeMap::new();
    for (i, p) in pieces.iter().enumerate() {
        let class = find(&mut parent, p.carrier);
        let key = (class, p.start.min(p.end), p.start.max(p.end));
        groups.entry(key).or_default().push(i);
    }
    let mut out = vec![usize::MAX; pieces.len()];
    let mut next = 0;
    // Number global edges in piece order, so they follow the carriers.
    let mut order: Vec<usize> = (0..pieces.len()).collect();
    order.sort_unstable();
    let mut key_of = vec![(0, 0, 0); pieces.len()];
    for (k, list) in &groups {
        for &i in list {
            key_of[i] = *k;
        }
    }
    for i in order {
        if out[i] != usize::MAX {
            continue;
        }
        out[i] = next;
        let mid = middle(&pieces[i], carriers);
        for &j in &groups[&key_of[i]] {
            if out[j] != usize::MAX {
                continue;
            }
            let c = &carriers[pieces[j].carrier];
            let reach = c.tolerance + carriers[pieces[i].carrier].tolerance + SLACK;
            if c.curve.distance(mid) <= reach * 4.0
                && middle(&pieces[j], carriers).distance(mid)
                    <= reach * 4.0 + 1e-6 * piece_length(&pieces[i], carriers)
            {
                out[j] = next;
            }
        }
        next += 1;
    }
    out
}

fn middle(p: &Piece, carriers: &[Carrier]) -> Point3 {
    carriers[p.carrier]
        .curve
        .point((p.range[0] + p.range[1]) / 2.0)
}

fn piece_length(p: &Piece, carriers: &[Carrier]) -> f64 {
    let c = &carriers[p.carrier].curve;
    let n = 8;
    (0..n)
        .map(|k| {
            let at =
                |k: usize| c.point(p.range[0] + (p.range[1] - p.range[0]) * k as f64 / n as f64);
            at(k).distance(at(k + 1))
        })
        .sum()
}

/// Whether a piece lies inside `range` of its carrier (by its middle).
fn inside_range(p: &Piece, [r0, r1]: [f64; 2], c: &Carrier) -> bool {
    let mid = (p.range[0] + p.range[1]) / 2.0;
    let mid = match (c.curve.period(), c.closed) {
        (Some(per), _) => r0 + (mid - r0).rem_euclid(per),
        _ => mid,
    };
    r0 <= mid && mid <= r1
}

/// A face's loops as global edges.
#[allow(clippy::too_many_arguments)]
fn boundary(
    ops: &[Body],
    f: FaceRef,
    carriers: &[Carrier],
    carrier_pieces: &[Vec<usize>],
    edge_carrier: &BTreeMap<EdgeRef, usize>,
    pieces: &[Piece],
    piece_ge: &[GeId],
    ges: &[Ge],
    vertex_gv: &BTreeMap<VertexRef, GvId>,
) -> FaceWork {
    let b = &ops[f.op];
    let mut work = FaceWork::default();
    for l in b.face_loops(f.face) {
        if let Some(v) = b.loops[l].vertex {
            work.loops.push(Boundary::Lone(
                vertex_gv[&VertexRef {
                    op: f.op,
                    vertex: v,
                }],
            ));
            continue;
        }
        let mut arcs = Vec::new();
        for fin in b.loop_fins(l) {
            let fi = &b.fins[fin];
            let c = edge_carrier[&EdgeRef {
                op: f.op,
                edge: fi.edge,
            }];
            let mut run: Vec<(GeId, Sense)> = carrier_pieces[c]
                .iter()
                .map(|&q| {
                    let ge = piece_ge[q];
                    (ge, along(&ges[ge], &pieces[q], &carriers[c]))
                })
                .collect();
            if fi.sense == Sense::Reversed {
                run.reverse();
                for x in &mut run {
                    x.1 = x.1.flipped();
                }
            }
            arcs.extend(run);
        }
        work.loops.push(Boundary::Arcs(arcs));
    }
    work
}

/// Whether a carrier's piece runs the way of its global edge.
fn along(ge: &Ge, piece: &Piece, c: &Carrier) -> Sense {
    if ge.start != ge.end && piece.start != piece.end {
        return if ge.start == piece.start {
            Sense::Forward
        } else {
            Sense::Reversed
        };
    }
    // A closed piece: compare the curves' directions in the middle.
    let t = (piece.range[0] + piece.range[1]) / 2.0;
    let p = c.curve.point(t);
    let d = c.curve.eval(t).d1;
    let s = ge.curve.project(p).param;
    let e = ge.curve.eval(s).d1;
    if d.dot(e) >= 0.0 {
        Sense::Forward
    } else {
        Sense::Reversed
    }
}

/// Gives every global edge its pcurve on each face that uses it.
fn pcurves(ops: &[Body], pairs: &[Pair], table: &mut Table) -> Result<(), Fail> {
    let mut uses: BTreeSet<(GeId, FaceRef)> = BTreeSet::new();
    for (&f, w) in &table.faces {
        for l in &w.loops {
            if let Boundary::Arcs(a) = l {
                for &(g, _) in a {
                    uses.insert((g, f));
                }
            }
        }
        for &g in &w.interior {
            uses.insert((g, f));
        }
    }
    let uses: Vec<(GeId, FaceRef)> = uses.into_iter().collect();
    let found: Vec<((Curve2, [f64; 2]), f64)> = uses
        .par_iter()
        .map(|&(g, f)| pcurve_of(ops, pairs, &table.edges[g], f))
        .collect::<Result<_, _>>()?;
    for ((g, f), (pc, tol)) in uses.into_iter().zip(found) {
        let ge = &mut table.edges[g];
        ge.tolerance = ge.tolerance.max(tol);
        ge.pcurves.insert(f, pc);
    }
    Ok(())
}

/// A global edge's pcurve on face `f`, and the tolerance it needs: its
/// own fin's where its curve is the face's own edge's, its stretch's where
/// it is a stretch of the face, otherwise exact or fitted.
fn pcurve_of(
    ops: &[Body],
    pairs: &[Pair],
    ge: &Ge,
    f: FaceRef,
) -> Result<((Curve2, [f64; 2]), f64), Fail> {
    let b = &ops[f.op];
    match ge.members[0] {
        Member::Edge(e) if e.op == f.op => {
            let fin = b
                .edge_fins(e.edge)
                .find(|&x| b.loops[b.fins[x].loop_].face == f.face);
            if let Some(pc) = fin.and_then(|x| b.fins[x].pcurve) {
                return Ok(((b.curves2[pc.curve].clone(), pc.map), ge.tolerance));
            }
        }
        Member::Stretch { pair, index } => {
            let p = &pairs[pair];
            if let FaceMeet::Meet { stretches, .. } = &p.meet {
                let s = &stretches[index];
                let own = if p.a == f {
                    Some(&s.pcurves[0])
                } else if p.b == f {
                    Some(&s.pcurves[1])
                } else {
                    None
                };
                // A piece of a closed stretch past the end of its period
                // leaves a fitted pcurve's domain: fitted again below.
                if let Some((c2, map)) = own {
                    let [d0, d1] = c2.domain();
                    let [p0, p1] = ge.interval.map(|t| map[0] + map[1] * t);
                    let slack = 1e-12 * (d1 - d0).abs().max(1.0);
                    if p0.min(p1) >= d0 - slack && p0.max(p1) <= d1 + slack {
                        return Ok(((c2.clone(), *map), ge.tolerance));
                    }
                }
            }
        }
        Member::Edge(_) => {}
    }
    let surface = &b.surfaces[b.faces[f.face].surface];
    if let Some(found) = crate::pcurve::exact(surface, &ge.curve, ge.interval, ge.tolerance) {
        return Ok((found, ge.tolerance));
    }
    let other = match ge.members[0] {
        Member::Edge(e) => face_of_edge(ops, e),
        Member::Stretch { pair, .. } => pairs[pair].a,
    };
    let (n, bound) = crate::intersect::fit_pcurve(surface, &ge.curve, ge.interval)
        .map_err(|e| fail(e, other, f))?;
    Ok((
        (Curve2::Nurbs2(Arc::new(n)), [0.0, 1.0]),
        ge.tolerance.max(bound),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{Rigid, Surface, Vector3};
    use crate::name::OpKey;
    use crate::primitive;

    fn at(x: f64, y: f64, z: f64) -> Rigid {
        Rigid::translation(Vector3::new(x, y, z))
    }

    /// Checks that every loop of every face closes through its global
    /// edges, and that every pcurve fits its edge on its face.
    fn sound(ops: &[Body], t: &Table) {
        let ends = |g: GeId, s: Sense| {
            let e = &t.edges[g];
            match s {
                Sense::Forward => (e.start, e.end),
                Sense::Reversed => (e.end, e.start),
            }
        };
        for (f, w) in &t.faces {
            for l in &w.loops {
                if let Boundary::Arcs(a) = l {
                    for (k, &(g, s)) in a.iter().enumerate() {
                        let (g2, s2) = a[(k + 1) % a.len()];
                        assert_eq!(ends(g, s).1, ends(g2, s2).0, "{f:?} loop breaks");
                    }
                }
            }
            let b = &ops[f.op];
            let surface = &b.surfaces[b.faces[f.face].surface];
            let used = w
                .loops
                .iter()
                .flat_map(|l| match l {
                    Boundary::Arcs(a) => a.iter().map(|x| x.0).collect(),
                    Boundary::Lone(_) => Vec::new(),
                })
                .chain(w.interior.iter().copied());
            for g in used {
                let e = &t.edges[g];
                let (c2, map) = &e.pcurves[f];
                for k in 0..=8 {
                    let s = e.interval[0] + (e.interval[1] - e.interval[0]) * f64::from(k) / 8.0;
                    let q = c2.point(map[0] + map[1] * s);
                    let d = surface.point(q.x, q.y).distance(e.curve.point(s));
                    assert!(d <= e.tolerance * 1.01 + 1e-12, "{f:?} edge {g}: {d}");
                }
                for (end, gv) in [(e.interval[0], e.start), (e.interval[1], e.end)] {
                    let v = &t.vertices[gv];
                    let d = e.curve.point(end).distance(v.point);
                    assert!(
                        d <= v.tolerance.max(e.tolerance) * 4.0 + 1e-12,
                        "vertex {gv}: {d}"
                    );
                }
            }
        }
    }

    /// The face of `ops[op]` on a plane through `p` with normal along `n`,
    /// or the first face on a surface of that kind.
    fn plane_face(ops: &[Body], op: usize, p: Point3, n: Vector3) -> FaceRef {
        let b = &ops[op];
        let face = b
            .faces()
            .find(|&f| {
                let s = &b.surfaces[b.faces[f].surface];
                matches!(s, Surface::Plane(_))
                    && s.distance(p) < 1e-12
                    && s.frame().unwrap().z().get().cross(n).length() < 1e-12
            })
            .expect("a face on the plane");
        FaceRef { op, face }
    }

    fn cylinder_face(ops: &[Body], op: usize) -> FaceRef {
        let b = &ops[op];
        let face = b
            .faces()
            .find(|&f| matches!(b.surfaces[b.faces[f].surface], Surface::Cylinder(_)))
            .unwrap();
        FaceRef { op, face }
    }

    #[test]
    fn a_rod_through_a_plate() {
        let plate = primitive::block(OpKey(1), 4.0, 4.0, 1.0, &Rigid::IDENTITY)
            .unwrap()
            .0;
        let rod = primitive::cylinder(OpKey(2), 0.5, 3.0, &at(2.0, 2.0, -1.0))
            .unwrap()
            .0;
        let ops = [plate, rod];
        let t = build(&ops).unwrap();
        sound(&ops, &t);
        // The plate's 8 corners, the rod's 2 circle vertices, and one
        // vertex on each circle where the rod passes the plate's faces.
        assert_eq!(t.vertices.len(), 12);
        assert_eq!(t.edges.len(), 12 + 2 + 2);
        let top = plane_face(
            &ops,
            0,
            Point3::new(0.0, 0.0, 1.0),
            Vector3::new(0.0, 0.0, 1.0),
        );
        let bottom = plane_face(&ops, 0, Point3::ORIGIN, Vector3::new(0.0, 0.0, 1.0));
        let side = cylinder_face(&ops, 1);
        assert_eq!(t.faces[&top].interior.len(), 1);
        assert_eq!(t.faces[&bottom].interior.len(), 1);
        assert_eq!(t.faces[&side].interior.len(), 2);
        let circle = &t.edges[t.faces[&top].interior[0]];
        assert_eq!(circle.start, circle.end);
        assert!(matches!(circle.curve, Curve::Circle(_)));
        assert!(t.coincident.is_empty());
    }

    #[test]
    fn crossing_ellipses_are_cut_where_they_cross() {
        // Equal cylinders crossing at right angles meet in two ellipses
        // that cross where the cylinders touch: each ellipse is cut there,
        // so each side face holds four arcs between two new vertices.
        let a = primitive::cylinder(OpKey(1), 1.0, 4.0, &at(0.0, 0.0, -2.0))
            .unwrap()
            .0;
        let across = Rigid::rotation(
            Vector3::new(1.0, 0.0, 0.0).normalize().unwrap(),
            core::f64::consts::FRAC_PI_2,
        );
        let b = a.transformed(&across).unwrap();
        let ops = [a, b];
        let t = build(&ops).unwrap();
        sound(&ops, &t);
        for op in 0..2 {
            let side = cylinder_face(&ops, op);
            let w = &t.faces[&side];
            assert_eq!(w.interior.len(), 4);
            let ends: BTreeSet<GvId> = w
                .interior
                .iter()
                .flat_map(|&g| [t.edges[g].start, t.edges[g].end])
                .collect();
            assert_eq!(ends.len(), 2);
        }
    }

    #[test]
    fn crossing_blocks() {
        let a = primitive::block(OpKey(1), 2.0, 2.0, 2.0, &Rigid::IDENTITY)
            .unwrap()
            .0;
        let b = primitive::block(OpKey(2), 2.0, 1.0, 1.0, &at(1.0, 0.5, 0.5))
            .unwrap()
            .0;
        let ops = [a, b];
        let t = build(&ops).unwrap();
        sound(&ops, &t);
        // B's four long edges cross A's +x face.
        assert_eq!(t.vertices.len(), 8 + 8 + 4);
        assert_eq!(t.edges.len(), 12 + 16 + 4);
        let px = plane_face(
            &ops,
            0,
            Point3::new(2.0, 0.0, 0.0),
            Vector3::new(1.0, 0.0, 0.0),
        );
        assert_eq!(t.faces[&px].interior.len(), 4);
        for (n, p) in [
            (Vector3::new(0.0, 1.0, 0.0), Point3::new(0.0, 0.5, 0.0)),
            (Vector3::new(0.0, 0.0, 1.0), Point3::new(0.0, 0.0, 1.5)),
        ] {
            let f = plane_face(&ops, 1, p, n);
            assert_eq!(t.faces[&f].interior.len(), 1);
            // Its long edges are cut where they leave A.
            let Boundary::Arcs(arcs) = &t.faces[&f].loops[0] else {
                panic!()
            };
            assert_eq!(arcs.len(), 6);
        }
    }

    #[test]
    fn flush_blocks_share_edges_and_rims() {
        // B sits on A's top, shifted half over its edge, flush with A's
        // front face.
        let a = primitive::block(OpKey(1), 2.0, 2.0, 1.0, &Rigid::IDENTITY)
            .unwrap()
            .0;
        let b = primitive::block(OpKey(2), 2.0, 1.0, 1.0, &at(1.0, 0.0, 0.0))
            .unwrap()
            .0;
        let ops = [a, b];
        let t = build(&ops).unwrap();
        sound(&ops, &t);
        // A's top and B's top, A's bottom and B's bottom, A's front and
        // B's front, lie on one plane each.
        assert_eq!(t.coincident.len(), 3);
        // B's corners on A's front edges are A's edge points: the pieces
        // of A's and B's edges along the front are shared.
        let front_a = plane_face(&ops, 0, Point3::ORIGIN, Vector3::new(0.0, 1.0, 0.0));
        let front_b = plane_face(&ops, 1, Point3::ORIGIN, Vector3::new(0.0, 1.0, 0.0));
        let ges = |f: FaceRef| -> BTreeSet<GeId> {
            t.faces[&f]
                .loops
                .iter()
                .flat_map(|l| match l {
                    Boundary::Arcs(a) => a.iter().map(|x| x.0).collect(),
                    Boundary::Lone(_) => Vec::new(),
                })
                .collect()
        };
        let shared: Vec<GeId> = ges(front_a).intersection(&ges(front_b)).copied().collect();
        // Along the bottom (x from 1 to 2) and the top (x from 1 to 2).
        assert_eq!(shared.len(), 2, "{shared:?}");
        // A's +x side face lies inside B's region: B's front face gets
        // the piece of A's vertical edge at x = 2 inside it.
        assert_eq!(t.faces[&front_b].interior.len(), 1);
        assert_eq!(t.faces[&front_a].interior.len(), 1);
    }

    #[test]
    fn a_vertex_on_a_face_and_an_edge_on_an_edge() {
        // A small block standing on A's top with one corner at A's top
        // centre: its bottom face lies on A's top.
        let a = primitive::block(OpKey(1), 2.0, 2.0, 1.0, &Rigid::IDENTITY)
            .unwrap()
            .0;
        let b = primitive::block(OpKey(2), 0.5, 0.5, 0.5, &at(1.0, 1.0, 1.0))
            .unwrap()
            .0;
        let ops = [a, b];
        let t = build(&ops).unwrap();
        sound(&ops, &t);
        assert_eq!(t.coincident.len(), 1);
        let top = plane_face(
            &ops,
            0,
            Point3::new(0.0, 0.0, 1.0),
            Vector3::new(0.0, 0.0, 1.0),
        );
        // B's bottom square is imprinted inside A's top.
        assert_eq!(t.faces[&top].interior.len(), 4);
        // No new vertices: B's corners are its own.
        assert_eq!(t.vertices.len(), 16);
        // A block touching A's top edge along its length from outside.
        let c = primitive::block(OpKey(3), 4.0, 1.0, 1.0, &at(-1.0, -1.0, 1.0))
            .unwrap()
            .0;
        let ops = [ops[0].clone(), c];
        let t = build(&ops).unwrap();
        sound(&ops, &t);
        // C's bottom-back edge runs along A's top-front edge: both are cut
        // at A's corners and share the piece between them.
        let shared = t.edges.iter().filter(|e| e.members.len() > 1).count();
        assert!(shared >= 1, "{shared}");
    }
}
