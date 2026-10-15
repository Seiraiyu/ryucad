use super::{
    Body, CurveId, EdgeId, EntityId, FaceId, FinId, Id, LoopId, RegionId, ShellId, SurfaceId,
    VertexId,
};

/// Where each entity went in [`Body::compact`]: the new ID of every old
/// one, or `None` for an entity that was dead (or geometry nothing used).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Remap {
    regions: Vec<Option<RegionId>>,
    shells: Vec<Option<ShellId>>,
    faces: Vec<Option<FaceId>>,
    loops: Vec<Option<LoopId>>,
    fins: Vec<Option<FinId>>,
    edges: Vec<Option<EdgeId>>,
    vertices: Vec<Option<VertexId>>,
    curves: Vec<Option<CurveId>>,
    surfaces: Vec<Option<SurfaceId>>,
}

fn lookup<I: Id>(map: &[Option<I>], old: I) -> Option<I> {
    map.get(Id::index(old)).copied().flatten()
}

/// A reference held by a live record is live; anything else is a kernel
/// bug.
fn to<I: Id>(map: &[Option<I>], old: I) -> I {
    lookup(map, old).expect("live records only reference live entities")
}

impl Remap {
    pub fn region(&self, old: RegionId) -> Option<RegionId> {
        lookup(&self.regions, old)
    }

    pub fn shell(&self, old: ShellId) -> Option<ShellId> {
        lookup(&self.shells, old)
    }

    pub fn face(&self, old: FaceId) -> Option<FaceId> {
        lookup(&self.faces, old)
    }

    pub fn loop_(&self, old: LoopId) -> Option<LoopId> {
        lookup(&self.loops, old)
    }

    pub fn fin(&self, old: FinId) -> Option<FinId> {
        lookup(&self.fins, old)
    }

    pub fn edge(&self, old: EdgeId) -> Option<EdgeId> {
        lookup(&self.edges, old)
    }

    pub fn vertex(&self, old: VertexId) -> Option<VertexId> {
        lookup(&self.vertices, old)
    }

    pub fn curve(&self, old: CurveId) -> Option<CurveId> {
        lookup(&self.curves, old)
    }

    pub fn surface(&self, old: SurfaceId) -> Option<SurfaceId> {
        lookup(&self.surfaces, old)
    }
}

impl Body {
    /// Drops dead slots and geometry that no live edge or face uses, and
    /// renumbers everything in its existing order. Names are unchanged.
    /// Every ID held from before is invalid afterwards; translate it
    /// through the returned [`Remap`].
    pub fn compact(&mut self) -> Remap {
        let mut used_curves = vec![false; self.curves.slot_count()];
        for (_, e) in self.edges.live() {
            used_curves[e.curve.index()] = true;
        }
        let mut used_surfaces = vec![false; self.surfaces.slot_count()];
        for (_, f) in self.faces.live() {
            used_surfaces[f.surface.index()] = true;
        }
        for (i, used) in used_curves.into_iter().enumerate() {
            if !used {
                self.curves.kill(CurveId::from_index(i));
            }
        }
        for (i, used) in used_surfaces.into_iter().enumerate() {
            if !used {
                self.surfaces.kill(SurfaceId::from_index(i));
            }
        }
        let m = Remap {
            regions: self.regions.compact(),
            shells: self.shells.compact(),
            faces: self.faces.compact(),
            loops: self.loops.compact(),
            fins: self.fins.compact(),
            edges: self.edges.compact(),
            vertices: self.vertices.compact(),
            curves: self.curves.compact(),
            surfaces: self.surfaces.compact(),
        };
        for r in self.regions.values_mut() {
            r.first_shell = r.first_shell.map(|s| to(&m.shells, s));
        }
        for s in self.shells.values_mut() {
            s.region = to(&m.regions, s.region);
            s.next = to(&m.shells, s.next);
            s.prev = to(&m.shells, s.prev);
            s.first_face = s.first_face.map(|f| to(&m.faces, f));
        }
        for f in self.faces.values_mut() {
            f.shell = to(&m.shells, f.shell);
            f.next = to(&m.faces, f.next);
            f.prev = to(&m.faces, f.prev);
            f.first_loop = f.first_loop.map(|l| to(&m.loops, l));
            f.surface = to(&m.surfaces, f.surface);
        }
        for l in self.loops.values_mut() {
            l.face = to(&m.faces, l.face);
            l.next = to(&m.loops, l.next);
            l.prev = to(&m.loops, l.prev);
            l.first_fin = l.first_fin.map(|f| to(&m.fins, f));
            l.vertex = l.vertex.map(|v| to(&m.vertices, v));
        }
        for f in self.fins.values_mut() {
            f.loop_ = to(&m.loops, f.loop_);
            f.edge = to(&m.edges, f.edge);
            f.next = to(&m.fins, f.next);
            f.prev = to(&m.fins, f.prev);
            f.around = to(&m.fins, f.around);
        }
        for e in self.edges.values_mut() {
            e.fin = to(&m.fins, e.fin);
            e.start = to(&m.vertices, e.start);
            e.end = to(&m.vertices, e.end);
            e.curve = to(&m.curves, e.curve);
        }
        for v in self.vertices.values_mut() {
            v.edge = v.edge.map(|e| to(&m.edges, e));
            v.lone = v.lone.map(|l| to(&m.loops, l));
        }
        for entity in self.names.values_mut() {
            *entity = match *entity {
                EntityId::Face(f) => EntityId::Face(to(&m.faces, f)),
                EntityId::Edge(e) => EntityId::Edge(to(&m.edges, e)),
                EntityId::Vertex(v) => EntityId::Vertex(to(&m.vertices, v)),
                other => other,
            };
        }
        m
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{Curve, Line, Point3, Vector3};
    use crate::topo::fixture::tetrahedron;

    /// Moves vertex 0 to a new slot at the end, leaving a tombstone, and
    /// adds an unused curve.
    fn shuffled() -> (Body, VertexId) {
        let mut b = tetrahedron();
        let old = VertexId::from_index(0);
        let record = b.vertices[old].clone();
        let moved = b.vertices.push(record);
        b.vertices.kill(old);
        for e in b.edges.values_mut() {
            if e.start == old {
                e.start = moved;
            }
            if e.end == old {
                e.end = moved;
            }
        }
        let n = b.vertices[moved].name;
        b.names.insert(n, EntityId::Vertex(moved));
        b.add_curve(Curve::Line(
            Line::new(Point3::ORIGIN, Vector3::new(1.0, 0.0, 0.0)).unwrap(),
        ));
        (b, moved)
    }

    #[test]
    fn compact_renumbers_in_order_and_drops_unused_geometry() {
        let (mut b, moved) = shuffled();
        let name = b.vertices[moved].name;
        let before = b.counts();
        assert_eq!(b.curves.slot_count(), 7);
        let m = b.compact();
        assert_eq!(b.counts(), before);
        assert_eq!(m.vertex(VertexId::from_index(0)), None);
        assert_eq!(m.vertex(moved), Some(VertexId::from_index(3)));
        assert_eq!(
            m.vertex(VertexId::from_index(1)),
            Some(VertexId::from_index(0))
        );
        assert_eq!(m.curve(CurveId::from_index(6)), None);
        assert_eq!(m.face(FaceId::from_index(2)), Some(FaceId::from_index(2)));
        assert_eq!(b.curves.slot_count(), 6);
        assert_eq!(b.vertices.slot_count(), 4);
        assert_eq!(
            b.by_name(name),
            Some(EntityId::Vertex(VertexId::from_index(3)))
        );
        for v in b.vertices() {
            assert_eq!(b.vertex_edges(v).len(), 3);
        }
        for l in b.loops() {
            let fins: Vec<_> = b.loop_fins(l).collect();
            for (i, &f) in fins.iter().enumerate() {
                assert_eq!(b.fin_end(f), b.fin_start(fins[(i + 1) % fins.len()]));
            }
        }
    }

    #[test]
    fn compacting_a_compact_body_changes_nothing() {
        let mut b = tetrahedron();
        let m = b.compact();
        assert_eq!(m.edge(EdgeId::from_index(5)), Some(EdgeId::from_index(5)));
        assert_eq!(
            m.region(RegionId::from_index(1)),
            Some(RegionId::from_index(1))
        );
        assert_eq!(
            m.shell(ShellId::from_index(0)),
            Some(ShellId::from_index(0))
        );
        assert_eq!(m.loop_(LoopId::from_index(3)), Some(LoopId::from_index(3)));
        assert_eq!(m.fin(FinId::from_index(11)), Some(FinId::from_index(11)));
        assert_eq!(
            m.surface(SurfaceId::from_index(3)),
            Some(SurfaceId::from_index(3))
        );
        assert_eq!(m.fin(FinId::from_index(12)), None);
    }
}
