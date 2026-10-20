//! Bodies: B-rep topology in flat arrays referenced by typed IDs.
//!
//! A [`Body`] owns its regions, shells, faces, loops, fins, edges and
//! vertices, the curves and surfaces they lie on, and the fins' pcurves. It is plain data:
//! cheap to clone and safe to share between threads. Edits go through
//! [`crate::euler`]; everything here only reads.
//!
//! The read API assumes a valid body (see [`crate::check`]). Walking a
//! corrupted ring could loop, so the checker never uses it.

mod compact;
mod dump;
mod id;
mod store;
mod transform;

#[cfg(test)]
pub(crate) mod fixture;

pub use compact::Remap;
pub(crate) use id::Id;
pub use id::{
    Curve2Id, CurveId, EdgeId, EntityId, FaceId, FinId, LoopId, RegionId, ShellId, SurfaceId,
    VertexId,
};
pub(crate) use store::Arena;
pub use store::{Edge, Face, Fin, Loop, Pcurve, Region, RegionKind, Sense, Shell, Vertex};

use crate::geom::{Curve, Curve2, Point2, Surface};
use crate::name::Name;
use std::collections::BTreeMap;

/// A solid body (or, during construction, part of one).
#[derive(Clone, Debug)]
pub struct Body {
    pub(crate) regions: Arena<RegionId, Region>,
    pub(crate) shells: Arena<ShellId, Shell>,
    pub(crate) faces: Arena<FaceId, Face>,
    pub(crate) loops: Arena<LoopId, Loop>,
    pub(crate) fins: Arena<FinId, Fin>,
    pub(crate) edges: Arena<EdgeId, Edge>,
    pub(crate) vertices: Arena<VertexId, Vertex>,
    pub(crate) curves: Arena<CurveId, Curve>,
    pub(crate) surfaces: Arena<SurfaceId, Surface>,
    pub(crate) curves2: Arena<Curve2Id, Curve2>,
    /// Every live face, edge and vertex by name.
    pub(crate) names: BTreeMap<Name, EntityId>,
    /// Readable recipes of live names, when enabled.
    pub(crate) recipes: Option<BTreeMap<Name, String>>,
}

/// Live entity counts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub regions: usize,
    pub shells: usize,
    pub faces: usize,
    pub loops: usize,
    pub fins: usize,
    pub edges: usize,
    pub vertices: usize,
}

impl Default for Body {
    fn default() -> Self {
        Self::new()
    }
}

/// The members of a circular list, starting at `first`.
fn ring<I: Copy + Eq>(first: Option<I>, next: impl Fn(I) -> I) -> impl Iterator<Item = I> {
    let mut cur = first;
    core::iter::from_fn(move || {
        let c = cur?;
        let n = next(c);
        cur = (Some(n) != first).then_some(n);
        Some(c)
    })
}

impl Body {
    /// An empty body: only the void region, with recipes enabled.
    pub fn new() -> Self {
        let mut regions = Arena::default();
        regions.push(Region {
            kind: RegionKind::Void,
            first_shell: None,
        });
        Self {
            regions,
            shells: Arena::default(),
            faces: Arena::default(),
            loops: Arena::default(),
            fins: Arena::default(),
            edges: Arena::default(),
            vertices: Arena::default(),
            curves: Arena::default(),
            surfaces: Arena::default(),
            curves2: Arena::default(),
            names: BTreeMap::new(),
            recipes: Some(BTreeMap::new()),
        }
    }

    /// The void region around everything.
    pub fn outer_region(&self) -> RegionId {
        RegionId::from_index(0)
    }

    pub fn add_curve(&mut self, curve: Curve) -> CurveId {
        self.curves.push(curve)
    }

    pub fn add_surface(&mut self, surface: Surface) -> SurfaceId {
        self.surfaces.push(surface)
    }

    pub fn add_curve2(&mut self, curve: Curve2) -> Curve2Id {
        self.curves2.push(curve)
    }

    pub fn curve(&self, id: CurveId) -> Option<&Curve> {
        self.curves.get(id)
    }

    pub fn surface(&self, id: SurfaceId) -> Option<&Surface> {
        self.surfaces.get(id)
    }

    pub fn curve2(&self, id: Curve2Id) -> Option<&Curve2> {
        self.curves2.get(id)
    }

    /// Where a fin's pcurve puts the edge's parameter `t` in its face's
    /// `(u, v)` plane, or `None` for a dead fin or one with no pcurve.
    pub fn fin_uv(&self, f: FinId, t: f64) -> Option<Point2> {
        let p = self.fins.get(f)?.pcurve?;
        Some(self.curves2[p.curve].point(p.param(t)))
    }

    pub fn region(&self, id: RegionId) -> Option<&Region> {
        self.regions.get(id)
    }

    pub fn shell(&self, id: ShellId) -> Option<&Shell> {
        self.shells.get(id)
    }

    pub fn face(&self, id: FaceId) -> Option<&Face> {
        self.faces.get(id)
    }

    pub fn loop_(&self, id: LoopId) -> Option<&Loop> {
        self.loops.get(id)
    }

    pub fn fin(&self, id: FinId) -> Option<&Fin> {
        self.fins.get(id)
    }

    pub fn edge(&self, id: EdgeId) -> Option<&Edge> {
        self.edges.get(id)
    }

    pub fn vertex(&self, id: VertexId) -> Option<&Vertex> {
        self.vertices.get(id)
    }

    pub fn regions(&self) -> impl Iterator<Item = RegionId> + '_ {
        self.regions.live().map(|(id, _)| id)
    }

    pub fn shells(&self) -> impl Iterator<Item = ShellId> + '_ {
        self.shells.live().map(|(id, _)| id)
    }

    pub fn faces(&self) -> impl Iterator<Item = FaceId> + '_ {
        self.faces.live().map(|(id, _)| id)
    }

    pub fn loops(&self) -> impl Iterator<Item = LoopId> + '_ {
        self.loops.live().map(|(id, _)| id)
    }

    pub fn fins(&self) -> impl Iterator<Item = FinId> + '_ {
        self.fins.live().map(|(id, _)| id)
    }

    pub fn edges(&self) -> impl Iterator<Item = EdgeId> + '_ {
        self.edges.live().map(|(id, _)| id)
    }

    pub fn vertices(&self) -> impl Iterator<Item = VertexId> + '_ {
        self.vertices.live().map(|(id, _)| id)
    }

    pub fn counts(&self) -> Counts {
        Counts {
            regions: self.regions().count(),
            shells: self.shells().count(),
            faces: self.faces().count(),
            loops: self.loops().count(),
            fins: self.fins().count(),
            edges: self.edges().count(),
            vertices: self.vertices().count(),
        }
    }

    /// The shells of a region, in ring order. Empty for a dead ID.
    pub fn region_shells(&self, r: RegionId) -> impl Iterator<Item = ShellId> + '_ {
        let first = self.regions.get(r).and_then(|r| r.first_shell);
        ring(first, |s| self.shells[s].next)
    }

    pub fn shell_faces(&self, s: ShellId) -> impl Iterator<Item = FaceId> + '_ {
        let first = self.shells.get(s).and_then(|s| s.first_face);
        ring(first, |f| self.faces[f].next)
    }

    pub fn face_loops(&self, f: FaceId) -> impl Iterator<Item = LoopId> + '_ {
        let first = self.faces.get(f).and_then(|f| f.first_loop);
        ring(first, |l| self.loops[l].next)
    }

    /// The fins of a loop in order; empty for a lone-vertex loop.
    pub fn loop_fins(&self, l: LoopId) -> impl Iterator<Item = FinId> + '_ {
        let first = self.loops.get(l).and_then(|l| l.first_fin);
        ring(first, |f| self.fins[f].next)
    }

    /// The ring of fins on an edge.
    pub fn edge_fins(&self, e: EdgeId) -> impl Iterator<Item = FinId> + '_ {
        let first = self.edges.get(e).map(|e| e.fin);
        ring(first, |f| self.fins[f].around)
    }

    /// The vertex a fin starts from, following the fin's sense.
    pub fn fin_start(&self, f: FinId) -> Option<VertexId> {
        self.fins.get(f).map(|_| self.start_of(f))
    }

    pub fn fin_end(&self, f: FinId) -> Option<VertexId> {
        self.fins.get(f).map(|_| self.end_of(f))
    }

    pub(crate) fn start_of(&self, f: FinId) -> VertexId {
        let fin = &self.fins[f];
        let e = &self.edges[fin.edge];
        match fin.sense {
            Sense::Forward => e.start,
            Sense::Reversed => e.end,
        }
    }

    pub(crate) fn end_of(&self, f: FinId) -> VertexId {
        let fin = &self.fins[f];
        let e = &self.edges[fin.edge];
        match fin.sense {
            Sense::Forward => e.end,
            Sense::Reversed => e.start,
        }
    }

    /// The edges at a vertex, each once, found by walking the fins around
    /// it. Assumes every edge at the vertex has two fins (a manifold
    /// vertex); empty for a lone vertex or a dead ID.
    pub fn vertex_edges(&self, v: VertexId) -> Vec<EdgeId> {
        let Some(e0) = self.vertices.get(v).and_then(|v| v.edge) else {
            return Vec::new();
        };
        let f0 = self.edges[e0].fin;
        // Start from a fin that ends at v.
        let start = if self.end_of(f0) == v {
            f0
        } else {
            self.fins[f0].prev
        };
        let mut out = Vec::new();
        let mut f = start;
        for _ in 0..=self.fins.slot_count() {
            // f ends at v, so the next fin in its loop starts there.
            let g = self.fins[f].next;
            let e = self.fins[g].edge;
            if !out.contains(&e) {
                out.push(e);
            }
            // The other use of g's edge ends at v.
            f = self.fins[g].around;
            if f == start {
                break;
            }
        }
        out
    }

    /// The live face, edge or vertex with this name.
    pub fn by_name(&self, name: Name) -> Option<EntityId> {
        self.names.get(&name).copied()
    }

    /// The name of a live face, edge or vertex.
    pub fn name_of(&self, entity: EntityId) -> Option<Name> {
        match entity {
            EntityId::Face(f) => self.faces.get(f).map(|f| f.name),
            EntityId::Edge(e) => self.edges.get(e).map(|e| e.name),
            EntityId::Vertex(v) => self.vertices.get(v).map(|v| v.name),
            _ => None,
        }
    }

    /// The readable recipe of a live name, when recipes are enabled.
    pub fn recipe(&self, name: Name) -> Option<&str> {
        self.recipes.as_ref()?.get(&name).map(String::as_str)
    }

    /// Turns recipe recording on or off. Turning it off frees the table;
    /// names made while it is off never get a recipe.
    pub fn set_recipes(&mut self, on: bool) {
        match (on, self.recipes.is_some()) {
            (true, false) => self.recipes = Some(BTreeMap::new()),
            (false, true) => self.recipes = None,
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fixture::tetrahedron;
    use super::*;
    use crate::geom::{Frame, Plane};

    #[test]
    fn a_new_body_is_just_the_void() {
        let b = Body::new();
        let r = b.outer_region();
        assert_eq!(b.region(r).unwrap().kind, RegionKind::Void);
        assert_eq!(b.regions().collect::<Vec<_>>(), [r]);
        assert_eq!(b.region_shells(r).count(), 0);
        assert_eq!(
            b.counts(),
            Counts {
                regions: 1,
                ..Counts::default()
            }
        );
    }

    #[test]
    fn geometry_is_stored_by_id() {
        let mut b = Body::new();
        let s = b.add_surface(Surface::Plane(Plane::new(Frame::WORLD)));
        assert_eq!(
            b.surface(s),
            Some(&Surface::Plane(Plane::new(Frame::WORLD)))
        );
        assert_eq!(b.curve(CurveId::from_index(0)), None);
    }

    #[test]
    fn tetrahedron_counts_and_rings() {
        let b = tetrahedron();
        assert_eq!(
            b.counts(),
            Counts {
                regions: 2,
                shells: 1,
                faces: 4,
                loops: 4,
                fins: 12,
                edges: 6,
                vertices: 4,
            }
        );
        let solid = b.regions().nth(1).unwrap();
        assert_eq!(b.region(solid).unwrap().kind, RegionKind::Solid);
        let shell = b.region_shells(solid).next().unwrap();
        assert_eq!(b.shell_faces(shell).count(), 4);
        for f in b.faces() {
            let loops: Vec<_> = b.face_loops(f).collect();
            assert_eq!(loops.len(), 1);
            let fins: Vec<_> = b.loop_fins(loops[0]).collect();
            assert_eq!(fins.len(), 3);
            for (i, &fin) in fins.iter().enumerate() {
                let next = fins[(i + 1) % 3];
                assert_eq!(b.fin_end(fin), b.fin_start(next));
            }
        }
        for e in b.edges() {
            let fins: Vec<_> = b.edge_fins(e).collect();
            assert_eq!(fins.len(), 2);
            assert_ne!(b.fin(fins[0]).unwrap().sense, b.fin(fins[1]).unwrap().sense);
        }
    }

    #[test]
    fn vertex_edges_walks_the_fan() {
        let b = tetrahedron();
        for v in b.vertices() {
            let mut edges = b.vertex_edges(v);
            assert_eq!(edges.len(), 3, "{v:?}");
            edges.sort();
            edges.dedup();
            assert_eq!(edges.len(), 3);
            for e in edges {
                let e = b.edge(e).unwrap();
                assert!(e.start == v || e.end == v);
            }
        }
        assert!(b.vertex_edges(VertexId::from_index(99)).is_empty());
    }

    #[test]
    fn names_and_recipes() {
        let mut b = tetrahedron();
        for f in b.faces().collect::<Vec<_>>() {
            let n = b.name_of(EntityId::Face(f)).unwrap();
            assert_eq!(b.by_name(n), Some(EntityId::Face(f)));
            assert_eq!(b.recipe(n), Some("fixture#0/new-face"));
        }
        assert_eq!(b.name_of(EntityId::Region(b.outer_region())), None);
        let n = b
            .name_of(EntityId::Vertex(VertexId::from_index(0)))
            .unwrap();
        b.set_recipes(false);
        assert_eq!(b.recipe(n), None);
        b.set_recipes(true);
        assert_eq!(b.recipe(n), None);
    }

    #[test]
    fn dead_ids_read_as_nothing() {
        let b = tetrahedron();
        let dead = FaceId::from_index(40);
        assert_eq!(b.face(dead), None);
        assert_eq!(b.face_loops(dead).count(), 0);
        assert_eq!(b.loop_fins(LoopId::from_index(40)).count(), 0);
        assert_eq!(b.edge_fins(EdgeId::from_index(40)).count(), 0);
        assert_eq!(b.fin_start(FinId::from_index(40)), None);
        assert_eq!(b.fin_end(FinId::from_index(40)), None);
    }
}
