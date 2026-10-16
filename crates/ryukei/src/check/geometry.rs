use super::Fault;
use crate::geom::{Curve, Point3, Surface};
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
        if !in_size_box(b.surfaces[face.surface].frame().origin()) {
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

fn edge(b: &Body, e: EdgeId, out: &mut Vec<Fault>) {
    let edge = &b.edges[e];
    let curve = &b.curves[edge.curve];
    let [t0, t1] = edge.interval;
    let closed = edge.start == edge.end;
    let bad_interval = match (closed, curve.period()) {
        (true, Some(p)) => (t1 - t0 - p).abs() > ANGULAR_RESOLUTION,
        (true, None) => true,
        (false, Some(p)) => t1 - t0 >= p - ANGULAR_RESOLUTION,
        (false, None) => t0 >= t1,
    };
    if bad_interval {
        out.push(Fault::BadInterval { edge: e });
    }
    let origin = match curve {
        Curve::Line(l) => l.origin(),
        Curve::Circle(c) => c.frame().origin(),
    };
    let samples: Vec<Point3> = (0..SAMPLES)
        .map(|i| curve.point(t0 + (t1 - t0) * i as f64 / (SAMPLES - 1) as f64))
        .collect();
    if !in_size_box(origin) || !samples.iter().all(|&p| in_size_box(p)) {
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

/// Faces wind the way their normals say, and solids enclose positive
/// volume. Decided exactly; only for planar faces with straight edges.
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
        let mut triangles = Vec::new();
        let all_planar = b.region_shells(r).all(|s| {
            b.shell_faces(s).all(|f| match &polygons[f.index()] {
                Some(t) => {
                    triangles.extend_from_slice(t);
                    true
                }
                None => false,
            })
        });
        if all_planar && volume_sign(&triangles) != Sign::Positive {
            out.push(Fault::NotOutward { region: r });
        }
    }
}
