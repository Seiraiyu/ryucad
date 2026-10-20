use super::Fault;
use crate::geom::{Box3, Curve, Point3, Surface};
use crate::predicates::{Sign, volume_sign, winding_sign};
use crate::tol::{ANGULAR_RESOLUTION, in_size_box};
use crate::topo::{Body, EdgeId, EntityId, FaceId, RegionKind, Sense, VertexId};
use std::collections::BTreeSet;

/// Samples per edge for the edge-on-surface check, including both ends.
const SAMPLES: usize = 9;

/// Points within tolerance of the curves and surfaces they lie on, and
/// everything inside the size box. For a structurally sound body.
pub(super) fn geometry(b: &Body, out: &mut Vec<Fault>) {
    for (v, vx) in b.vertices.live() {
        if !in_size_box(vx.point) {
            out.push(Fault::OutsideSizeBox {
                entity: EntityId::Vertex(v),
            });
        }
    }
    for (f, face) in b.faces.live() {
        if !surface_inside(&b.surfaces[face.surface]) {
            out.push(Fault::OutsideSizeBox {
                entity: EntityId::Face(f),
            });
        }
    }
    for e in b.edges() {
        edge(b, e, out);
    }
    // Each vertex on the surface of every face it bounds, once per face.
    let mut seen: BTreeSet<(VertexId, FaceId)> = BTreeSet::new();
    for (l, lp) in b.loops.live() {
        let surface = &b.surfaces[b.faces[lp.face].surface];
        let corners: Vec<VertexId> = match lp.vertex {
            Some(v) => vec![v],
            None => b.loop_fins(l).map(|f| b.start_of(f)).collect(),
        };
        for v in corners {
            let vx = &b.vertices[v];
            let distance = surface.distance(vx.point);
            if distance > vx.tolerance.get() && seen.insert((v, lp.face)) {
                out.push(Fault::VertexOffSurface {
                    vertex: v,
                    face: lp.face,
                    distance,
                });
            }
        }
    }
}

/// True when a box lies inside the size box.
fn box_inside(b: &Box3) -> bool {
    in_size_box(b.min()) && in_size_box(b.max())
}

/// A face's surface in the size box: its frame's origin, or its bounds.
fn surface_inside(surface: &crate::geom::Surface) -> bool {
    match surface.frame() {
        Some(frame) => in_size_box(frame.origin()),
        None => box_inside(&surface.bounds()),
    }
}

/// An edge's curve in the size box: a line's, circle's or ellipse's frame
/// origin, otherwise the hull of the pieces the edge uses, and samples
/// along the edge.
fn edge_inside(b: &Body, e: EdgeId) -> bool {
    let edge = &b.edges[e];
    let curve = &b.curves[edge.curve];
    let [t0, t1] = edge.interval;
    let inside = match curve {
        Curve::Line(l) => in_size_box(l.origin()),
        Curve::Circle(c) => in_size_box(c.frame().origin()),
        Curve::Ellipse(e) => in_size_box(e.frame().origin()),
        _ => box_inside(&curve.bounds(edge.interval)),
    };
    inside
        && (0..SAMPLES)
            .map(|i| curve.point(t0 + (t1 - t0) * i as f64 / (SAMPLES - 1) as f64))
            .all(in_size_box)
}

/// Whether any vertex, face surface or edge curve leaves the size box, by
/// the same rules as [`geometry`].
pub(crate) fn outside_size_box(b: &Body) -> bool {
    b.vertices.live().any(|(_, v)| !in_size_box(v.point))
        || b.faces
            .live()
            .any(|(_, f)| !surface_inside(&b.surfaces[f.surface]))
        || b.edges().any(|e| !edge_inside(b, e))
}

fn edge(b: &Body, e: EdgeId, out: &mut Vec<Fault>) {
    let edge = &b.edges[e];
    let curve = &b.curves[edge.curve];
    let [t0, t1] = edge.interval;
    let closed = edge.start == edge.end;
    let [d0, d1] = curve.domain();
    let bad_interval = match (closed, curve.period()) {
        (true, Some(p)) => (t1 - t0 - p).abs() > ANGULAR_RESOLUTION,
        // A clamped curve whose ends meet closes over its whole domain.
        (true, None) => {
            !curve.is_closed()
                || (t0 - d0).abs() > ANGULAR_RESOLUTION
                || (t1 - d1).abs() > ANGULAR_RESOLUTION
        }
        (false, Some(p)) => t1 - t0 >= p - ANGULAR_RESOLUTION,
        (false, None) => t0 >= t1 || t0 < d0 || t1 > d1,
    };
    if bad_interval {
        out.push(Fault::BadInterval { edge: e });
    }
    let samples: Vec<Point3> = (0..SAMPLES)
        .map(|i| curve.point(t0 + (t1 - t0) * i as f64 / (SAMPLES - 1) as f64))
        .collect();
    if !edge_inside(b, e) {
        out.push(Fault::OutsideSizeBox {
            entity: EntityId::Edge(e),
        });
    }
    for (t, v) in [(t0, edge.start), (t1, edge.end)] {
        let vx = &b.vertices[v];
        let distance = curve.point(t).distance(vx.point);
        if distance > edge.tolerance.get().max(vx.tolerance.get()) {
            out.push(Fault::VertexOffCurve {
                vertex: v,
                edge: e,
                distance,
            });
        }
    }
    let mut faces: Vec<FaceId> = b
        .edge_fins(e)
        .map(|f| b.loops[b.fins[f].loop_].face)
        .collect();
    faces.dedup();
    for face in faces {
        let surface = &b.surfaces[b.faces[face].surface];
        let worst = samples
            .iter()
            .map(|&p| surface.distance(p))
            .fold(0.0, f64::max);
        if worst > edge.tolerance.get() {
            out.push(Fault::EdgeOffSurface {
                edge: e,
                face,
                distance: worst,
            });
        }
    }
    for fin in b.edge_fins(e) {
        let Some(p) = b.fins[fin].pcurve else {
            out.push(Fault::MissingPcurve { fin });
            continue;
        };
        let surface = &b.surfaces[b.faces[b.loops[b.fins[fin].loop_].face].surface];
        let c2 = &b.curves2[p.curve];
        let worst = samples
            .iter()
            .enumerate()
            .map(|(i, &q)| {
                let t = t0 + (t1 - t0) * i as f64 / (SAMPLES - 1) as f64;
                let uv = c2.point(p.param(t));
                surface.point(uv.x, uv.y).distance(q)
            })
            .fold(0.0, f64::max);
        if worst > edge.tolerance.get() {
            out.push(Fault::PcurveOffEdge {
                fin,
                distance: worst,
            });
        }
    }
}

/// A face's loops as fan triangles, when the face is planar and bounded
/// only by straight edges: the shapes whose area and volume can be
/// summed exactly.
fn polygon(b: &Body, f: FaceId) -> Option<Vec<[Point3; 3]>> {
    if !matches!(b.surfaces[b.faces[f].surface], Surface::Plane(_)) {
        return None;
    }
    let mut triangles = Vec::new();
    for l in b.face_loops(f) {
        let mut corners = Vec::new();
        for fin in b.loop_fins(l) {
            if !matches!(b.curves[b.edges[b.fins[fin].edge].curve], Curve::Line(_)) {
                return None;
            }
            corners.push(b.vertices[b.start_of(fin)].point);
        }
        for i in 1..corners.len().saturating_sub(1) {
            triangles.push([corners[0], corners[i], corners[i + 1]]);
        }
    }
    Some(triangles)
}

/// Faces wind the way their normals say, and each solid has one shell
/// enclosing positive volume (its outside) and any others negative
/// volume (its cavities). Decided exactly; only for planar faces with
/// straight edges.
pub(super) fn orientation(b: &Body, out: &mut Vec<Fault>) {
    let mut polygons = vec![None; b.faces.slot_count()];
    for f in b.faces() {
        let Some(triangles) = polygon(b, f) else {
            continue;
        };
        let face = &b.faces[f];
        let Some(n) = b.surfaces[face.surface].normal(0.0, 0.0) else {
            continue;
        };
        let n = if face.sense == Sense::Forward {
            n.get()
        } else {
            -n.get()
        };
        if winding_sign(&triangles, n) == Sign::Negative {
            out.push(Fault::FaceInsideOut { face: f });
        }
        polygons[f.index()] = Some(triangles);
    }
    for (r, region) in b.regions.live() {
        if region.kind != RegionKind::Solid {
            continue;
        }
        // Each shell's volume sign, when all its faces are polygons.
        let signs: Option<Vec<Sign>> = b
            .region_shells(r)
            .map(|s| {
                let mut triangles = Vec::new();
                for f in b.shell_faces(s) {
                    triangles.extend_from_slice(polygons[f.index()].as_ref()?);
                }
                Some(volume_sign(&triangles))
            })
            .collect();
        let Some(signs) = signs else {
            continue;
        };
        let outer = signs.iter().filter(|&&s| s == Sign::Positive).count();
        let voids = signs.iter().filter(|&&s| s == Sign::Negative).count();
        if outer != 1 || outer + voids != signs.len() {
            out.push(Fault::NotOutward { region: r });
        }
    }
}
