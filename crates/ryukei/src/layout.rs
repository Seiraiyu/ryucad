//! Bodies from a layout: faces bounded by loops of edges, as plain data,
//! built through Euler operators.
//!
//! A [`Layout`] holds geometry in arrays and faces whose loops are runs of
//! edges or lone vertices. Every edge must have exactly two uses of
//! opposite sense, and the faces around every vertex must form one fan;
//! otherwise the result would be non-manifold and is refused. The fans
//! fix each vertex's rotation: the cyclic order of the edge ends at it.
//!
//! Each shell (faces joined by edges) is grown one connected component
//! of its edge graph at a time. The first starts from a vertex made by
//! MVFS (or MVS in an outer shell's region); each later one from a vertex
//! made by MVR in the face that holds another loop of a face it shares.
//! A spanning tree of the component is grown by MEV (or [`mev_reversed`]
//! where the edge's curve runs into the tree), each edge inserted at the
//! corner its rotation calls for, the first along the loop the component
//! starts from, so every later MEF keeps the older loops on that loop's
//! side. The other edges close faces by MEF, join loops of one face by
//! MEKR, or (two faces of a handle) KFMRH then MEKR. Loops of one face
//! left on separate faces are then merged into it by KFMRH, and faces get
//! their surfaces, fins their pcurves, and edges and vertices their
//! tolerances, grown to cover the gap between a curve's end and its
//! vertex.
//!
//! Which shells are voids, and in which region, is the caller's to say:
//! booleans find out by building every shell as a solid of its own first
//! (see [`crate::boolean`]); an import is told by its file.
//!
//! [`mev_reversed`]: crate::euler::mev_reversed

use crate::euler::{self, Corner};
use crate::geom::{Curve, Curve2, Point3, Surface};
use crate::name::{Name, OpKey, Role};
use crate::tol::{LINEAR_RESOLUTION, Tolerance};
use crate::topo::{
    Body, Curve2Id, CurveId, EdgeId, FaceId, FinId, LoopId, Pcurve, RegionId, Sense, SurfaceId,
    VertexId,
};
use std::collections::{BTreeMap, BTreeSet};

/// A body to build. Geometry is shared by index: faces on one surface,
/// edges on one curve and fins on one pcurve name the same entry, and the
/// body gets one copy of it.
#[derive(Clone, Debug, Default)]
pub(crate) struct Layout {
    pub(crate) surfaces: Vec<Surface>,
    pub(crate) curves: Vec<Curve>,
    pub(crate) curves2: Vec<Curve2>,
    pub(crate) vertices: Vec<Vertex>,
    pub(crate) edges: Vec<Edge>,
    pub(crate) faces: Vec<Face>,
}

#[derive(Clone, Debug)]
pub(crate) struct Vertex {
    pub(crate) point: Point3,
    pub(crate) tolerance: f64,
}

/// An edge from `start` to `end` along `curve` over `interval`
/// (increasing).
#[derive(Clone, Debug)]
pub(crate) struct Edge {
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) curve: usize,
    pub(crate) interval: [f64; 2],
    pub(crate) tolerance: f64,
}

/// A face on `surface`, its normal the surface's natural one when `sense`
/// is `Forward`; its loops run counterclockwise about that normal.
#[derive(Clone, Debug)]
pub(crate) struct Face {
    pub(crate) surface: usize,
    pub(crate) sense: Sense,
    pub(crate) loops: Vec<Boundary>,
}

/// One loop of a face.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Boundary {
    /// Edges in loop order.
    Arcs(Vec<Use>),
    /// A lone vertex.
    Lone(usize),
}

/// One use of an edge by a loop: the sense the loop runs it, and its
/// pcurve on the face (a `curves2` entry and the affine map from the
/// edge's parameter to the pcurve's), if it has one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Use {
    pub(crate) edge: usize,
    pub(crate) sense: Sense,
    pub(crate) pcurve: Option<(usize, [f64; 2])>,
}

/// Why a layout couldn't be built.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum BuildError {
    /// An edge without exactly two uses of opposite sense, a vertex where
    /// faces meet in more than one fan, or a lone vertex that also ends
    /// edges: `faces` are those meeting there.
    NonManifold { at: Point3, faces: Vec<usize> },
    /// An Euler operator refused, or the faces didn't come out as the
    /// layout says: a layout whose loops don't bound its faces.
    Inconsistent,
}

/// The built body, and where each face, edge and vertex of the layout
/// went.
#[derive(Clone, Debug)]
pub(crate) struct Built {
    pub(crate) body: Body,
    pub(crate) faces: Vec<FaceId>,
    pub(crate) edges: BTreeMap<usize, EdgeId>,
    pub(crate) vertices: BTreeMap<usize, VertexId>,
}

/// An end of an edge at a vertex: its start or its end.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct End {
    ge: usize,
    start: bool,
}

/// What the builder knows before it starts: the layout checked for
/// manifoldness, with each vertex's rotation.
pub(crate) struct Plan<'a> {
    layout: &'a Layout,
    /// Each edge's two uses: face, loop, position.
    uses: BTreeMap<usize, Vec<(usize, usize, usize)>>,
    /// Each vertex's edge ends in rotation order.
    rotation: BTreeMap<usize, Vec<End>>,
    /// Lone vertices: their face.
    lone: BTreeMap<usize, usize>,
}

impl<'a> Plan<'a> {
    pub(crate) fn new(layout: &'a Layout) -> Result<Self, BuildError> {
        let mut uses: BTreeMap<usize, Vec<(usize, usize, usize)>> = BTreeMap::new();
        let mut lone: BTreeMap<usize, usize> = BTreeMap::new();
        for (k, face) in layout.faces.iter().enumerate() {
            for (l, lp) in face.loops.iter().enumerate() {
                match lp {
                    Boundary::Arcs(a) => {
                        for (i, u) in a.iter().enumerate() {
                            uses.entry(u.edge).or_default().push((k, l, i));
                        }
                    }
                    Boundary::Lone(g) => {
                        if lone.insert(*g, k).is_some() {
                            return Err(BuildError::NonManifold {
                                at: layout.vertices[*g].point,
                                faces: vec![k],
                            });
                        }
                    }
                }
            }
        }
        let arc = |k: usize, l: usize, i: usize| -> (usize, Sense) {
            match &layout.faces[k].loops[l] {
                Boundary::Arcs(a) => (a[i].edge, a[i].sense),
                Boundary::Lone(_) => unreachable!("arcs only"),
            }
        };
        for (&g, list) in &uses {
            let ok = list.len() == 2
                && arc(list[0].0, list[0].1, list[0].2).1 != arc(list[1].0, list[1].1, list[1].2).1;
            if !ok {
                let e = &layout.edges[g];
                let at = layout.curves[e.curve].point((e.interval[0] + e.interval[1]) / 2.0);
                return Err(BuildError::NonManifold {
                    at,
                    faces: list.iter().map(|u| u.0).collect(),
                });
            }
        }
        // The fans: corners at each vertex, walked in rotation order.
        let ends_of = |g: usize, s: Sense| -> (usize, End, usize, End) {
            let e = &layout.edges[g];
            match s {
                Sense::Forward => (
                    e.start,
                    End { ge: g, start: true },
                    e.end,
                    End {
                        ge: g,
                        start: false,
                    },
                ),
                Sense::Reversed => (
                    e.end,
                    End {
                        ge: g,
                        start: false,
                    },
                    e.start,
                    End { ge: g, start: true },
                ),
            }
        };
        // A corner (face, loop, i) is at the end of arc i, before arc
        // i + 1; its outgoing edge end, and the corner its twin starts.
        let mut corners: BTreeMap<usize, Vec<(usize, usize, usize)>> = BTreeMap::new();
        for (k, face) in layout.faces.iter().enumerate() {
            for (l, lp) in face.loops.iter().enumerate() {
                if let Boundary::Arcs(a) = lp {
                    for (i, u) in a.iter().enumerate() {
                        let (_, _, v, _) = ends_of(u.edge, u.sense);
                        corners.entry(v).or_default().push((k, l, i));
                    }
                }
            }
        }
        let mut rotation: BTreeMap<usize, Vec<End>> = BTreeMap::new();
        for (&v, list) in &corners {
            if lone.contains_key(&v) {
                return Err(BuildError::NonManifold {
                    at: layout.vertices[v].point,
                    faces: list.iter().map(|c| c.0).collect(),
                });
            }
            let mut seen: BTreeSet<(usize, usize, usize)> = BTreeSet::new();
            let mut order = Vec::new();
            let mut c = list[0];
            loop {
                if !seen.insert(c) {
                    break;
                }
                let n = match &layout.faces[c.0].loops[c.1] {
                    Boundary::Arcs(a) => a.len(),
                    Boundary::Lone(_) => unreachable!(),
                };
                let (g, s) = arc(c.0, c.1, (c.2 + 1) % n);
                let (_, out_end, _, _) = ends_of(g, s);
                order.push(out_end);
                // The twin arrives along the same end: the next corner
                // follows it.
                let twin = uses[&g]
                    .iter()
                    .copied()
                    .find(|&u| (u.0, u.1, u.2) != (c.0, c.1, (c.2 + 1) % n))
                    .expect("two uses");
                c = (twin.0, twin.1, twin.2);
            }
            if seen.len() != list.len() {
                return Err(BuildError::NonManifold {
                    at: layout.vertices[v].point,
                    faces: list.iter().map(|c| c.0).collect(),
                });
            }
            rotation.insert(v, order);
        }
        Ok(Self {
            layout,
            uses,
            rotation,
            lone,
        })
    }

    /// The shells: faces joined by edges, each listed in face order.
    pub(crate) fn shells(&self) -> Vec<Vec<usize>> {
        let n = self.layout.faces.len();
        let mut parent: Vec<usize> = (0..n).collect();
        fn find(parent: &mut [usize], mut i: usize) -> usize {
            while parent[i] != i {
                parent[i] = parent[parent[i]];
                i = parent[i];
            }
            i
        }
        for list in self.uses.values() {
            let (a, b) = (find(&mut parent, list[0].0), find(&mut parent, list[1].0));
            if a != b {
                parent[a.max(b)] = a.min(b);
            }
        }
        let mut by_root: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        for k in 0..n {
            let r = find(&mut parent, k);
            by_root.entry(r).or_default().push(k);
        }
        by_root.into_values().collect()
    }

    /// Builds every shell: outer shells (`outer_of` `None`) as regions of
    /// their own first, then voids in their outer shells' regions. The
    /// Euler operators' names are scaffolding derived from `key` and
    /// `role`; the caller names what it built.
    pub(crate) fn build(
        &self,
        key: OpKey,
        role: Role,
        shells: &[Vec<usize>],
        outer_of: &[Option<usize>],
    ) -> Result<Built, BuildError> {
        let mut g = Grower {
            plan: self,
            body: Body::new(),
            keys: Keys {
                base: key,
                role,
                count: 0,
            },
            faces: vec![None; self.layout.faces.len()],
            edges: BTreeMap::new(),
            vertices: BTreeMap::new(),
            edge_of: BTreeMap::new(),
            vertex_of: BTreeMap::new(),
            surfaces: BTreeMap::new(),
            curves: BTreeMap::new(),
            curves2: BTreeMap::new(),
        };
        g.body.set_recipes(false);
        let mut regions: Vec<Option<RegionId>> = vec![None; shells.len()];
        for pass in 0..2 {
            for (s, faces) in shells.iter().enumerate() {
                let void = outer_of[s].is_some();
                if (pass == 0) == void {
                    continue;
                }
                let region = outer_of[s].map(|o| regions[o].expect("outer shells first"));
                regions[s] = Some(g.shell(faces, region)?);
            }
        }
        g.finish()
    }

    fn arc(&self, k: usize, l: usize, i: usize) -> (usize, Sense) {
        match &self.layout.faces[k].loops[l] {
            Boundary::Arcs(a) => (a[i].edge, a[i].sense),
            Boundary::Lone(_) => unreachable!("arcs only"),
        }
    }
}

/// A component to grow: its vertex class, the face and loop it starts
/// from, and the face loop whose face hosts it (none for a shell's
/// first).
type Reach = (usize, usize, usize, Option<(usize, usize)>);

/// Fresh keys for the Euler operators: their names are scaffolding, but
/// must stay unique on the way.
struct Keys {
    base: OpKey,
    role: Role,
    count: u128,
}

impl Keys {
    fn next(&mut self) -> OpKey {
        self.count += 1;
        let scaffold = Name::from_u128(self.count);
        OpKey(Name::derive(self.base, self.role, &[scaffold]).to_u128())
    }
}

/// The body being grown and where everything went.
struct Grower<'p, 'a> {
    plan: &'p Plan<'a>,
    body: Body,
    keys: Keys,
    faces: Vec<Option<FaceId>>,
    edges: BTreeMap<usize, EdgeId>,
    vertices: BTreeMap<usize, VertexId>,
    /// The other way: the layout edge and vertex each built one is.
    edge_of: BTreeMap<EdgeId, usize>,
    vertex_of: BTreeMap<VertexId, usize>,
    /// Geometry added so far, by layout index, in the order first used.
    surfaces: BTreeMap<usize, SurfaceId>,
    curves: BTreeMap<usize, CurveId>,
    curves2: BTreeMap<usize, Curve2Id>,
}

impl Grower<'_, '_> {
    fn topo(e: crate::Error) -> BuildError {
        let _ = e;
        BuildError::Inconsistent
    }

    fn layout(&self) -> &Layout {
        self.plan.layout
    }

    fn surface(&mut self, k: usize) -> SurfaceId {
        let s = self.layout().faces[k].surface;
        if let Some(&id) = self.surfaces.get(&s) {
            return id;
        }
        let id = self.body.add_surface(self.layout().surfaces[s].clone());
        self.surfaces.insert(s, id);
        id
    }

    fn curve(&mut self, g: usize) -> CurveId {
        let c = self.layout().edges[g].curve;
        if let Some(&id) = self.curves.get(&c) {
            return id;
        }
        let id = self.body.add_curve(self.layout().curves[c].clone());
        self.curves.insert(c, id);
        id
    }

    fn point(&self, g: usize) -> Point3 {
        self.layout().vertices[g].point
    }

    /// The fin of a built edge running with sense `s`.
    fn fin(&self, g: usize, s: Sense) -> FinId {
        let e = self.edges[&g];
        self.body
            .edge_fins(e)
            .find(|&f| self.body.fin(f).expect("live").sense == s)
            .expect("a fin each way")
    }

    /// The corner at vertex `v` where edge end `x` goes in, from the
    /// rotation and the edge ends already built there.
    fn corner(&self, v: usize, x: End) -> Result<Corner, BuildError> {
        let built = |e: &End| self.edges.contains_key(&e.ge);
        let rot = &self.plan.rotation[&v];
        if !rot.iter().any(built) {
            let vid = self.vertices[&v];
            let l = self
                .body
                .vertex(vid)
                .expect("live")
                .lone
                .ok_or(BuildError::Inconsistent)?;
            return Ok(Corner::Lone(l));
        }
        let n = rot.len();
        let i = rot
            .iter()
            .position(|e| *e == x)
            .ok_or(BuildError::Inconsistent)?;
        let after = (1..n)
            .map(|d| rot[(i + d) % n])
            .find(built)
            .ok_or(BuildError::Inconsistent)?;
        let before = (1..n)
            .map(|d| rot[(i + n - d) % n])
            .find(built)
            .ok_or(BuildError::Inconsistent)?;
        // Leaving along `after`, after arriving along `before`.
        let f = self.fin(
            after.ge,
            if after.start {
                Sense::Forward
            } else {
                Sense::Reversed
            },
        );
        let p = self.body.fin(f).expect("live").prev;
        let want = self.fin(
            before.ge,
            if before.start {
                Sense::Reversed
            } else {
                Sense::Forward
            },
        );
        if p != want {
            return Err(BuildError::Inconsistent);
        }
        Ok(Corner::Before(f))
    }

    /// The built face holding a layout face's loop.
    fn face_of_loop(&self, k: usize, l: usize) -> FaceId {
        let lid = match &self.layout().faces[k].loops[l] {
            Boundary::Arcs(a) => {
                let f = self.fin(a[0].edge, a[0].sense);
                self.body.fin(f).expect("live").loop_
            }
            Boundary::Lone(g) => self
                .body
                .vertex(self.vertices[g])
                .expect("live")
                .lone
                .expect("lone"),
        };
        self.body.loop_(lid).expect("live").face
    }

    /// The vertices joined by a shell's edges into components, with lone
    /// vertices on their own: each vertex's component.
    fn components(&self, shell: &[usize]) -> BTreeMap<usize, usize> {
        let mut parent: BTreeMap<usize, usize> = BTreeMap::new();
        fn find(parent: &mut BTreeMap<usize, usize>, i: usize) -> usize {
            let mut r = i;
            while parent[&r] != r {
                r = parent[&r];
            }
            let mut c = i;
            while parent[&c] != r {
                let n = parent[&c];
                parent.insert(c, r);
                c = n;
            }
            r
        }
        for &k in shell {
            for lp in &self.layout().faces[k].loops {
                match lp {
                    Boundary::Lone(g) => {
                        parent.entry(*g).or_insert(*g);
                    }
                    Boundary::Arcs(a) => {
                        for u in a {
                            let e = &self.layout().edges[u.edge];
                            parent.entry(e.start).or_insert(e.start);
                            parent.entry(e.end).or_insert(e.end);
                            let (x, y) = (find(&mut parent, e.start), find(&mut parent, e.end));
                            if x != y {
                                parent.insert(x.max(y), x.min(y));
                            }
                        }
                    }
                }
            }
        }
        let keys: Vec<usize> = parent.keys().copied().collect();
        keys.into_iter()
            .map(|g| (g, find(&mut parent, g)))
            .collect()
    }

    /// Whether a face loop is one closed edge run forwards.
    fn closed_forward(&self, k: usize, l: usize) -> bool {
        match &self.layout().faces[k].loops[l] {
            Boundary::Arcs(a) => {
                let e = &self.layout().edges[a[0].edge];
                a.len() == 1 && e.start == e.end && a[0].sense == Sense::Forward
            }
            Boundary::Lone(_) => false,
        }
    }

    /// The layout face a built loop belongs to.
    fn owner(&self, l: LoopId) -> Option<usize> {
        let lp = self.body.loop_(l)?;
        if let Some(v) = lp.vertex {
            let g = self.vertex_of.get(&v)?;
            return self.plan.lone.get(g).copied();
        }
        let f = lp.first_fin?;
        let fin = self.body.fin(f)?;
        let g = *self.edge_of.get(&fin.edge)?;
        self.plan.uses[&g]
            .iter()
            .find(|u| self.plan.arc(u.0, u.1, u.2).1 == fin.sense)
            .map(|u| u.0)
    }

    /// After a component hosted in the face of face `k`'s loop `a` from
    /// its loop `l`: if MEF left loop `l` on a face of its own (a closed
    /// edge run forwards), that face joins the host by KFMRH, and the loops
    /// of other faces on the host leave it again by MFKRH.
    fn settle(&mut self, k: usize, a: usize, l: usize) -> Result<(), BuildError> {
        let host = self.face_of_loop(k, a);
        let mine = self.face_of_loop(k, l);
        if host == mine {
            return Ok(());
        }
        if self.body.face_loops(mine).count() != 1 {
            return Err(BuildError::Inconsistent);
        }
        euler::kfmrh(&mut self.body, self.keys.next(), host, mine).map_err(Self::topo)?;
        let foreign: Vec<LoopId> = self
            .body
            .face_loops(host)
            .filter(|&x| self.owner(x) != Some(k))
            .collect();
        if foreign.len() > 1 {
            return Err(BuildError::Inconsistent);
        }
        for x in foreign {
            euler::mfkrh(&mut self.body, self.keys.next(), x).map_err(Self::topo)?;
        }
        Ok(())
    }

    /// The loop's starting vertex.
    fn loop_start(&self, k: usize, l: usize) -> usize {
        match &self.layout().faces[k].loops[l] {
            Boundary::Lone(g) => *g,
            Boundary::Arcs(a) => {
                let e = &self.layout().edges[a[0].edge];
                if a[0].sense == Sense::Forward {
                    e.start
                } else {
                    e.end
                }
            }
        }
    }

    /// Grows one shell; returns its region.
    fn shell(&mut self, shell: &[usize], region: Option<RegionId>) -> Result<RegionId, BuildError> {
        let comp = self.components(shell);
        // Loopless faces: a whole sphere or torus.
        if shell.len() == 1 && self.layout().faces[shell[0]].loops.is_empty() {
            let k = shell[0];
            let s = self.surface(k);
            let p = self.layout().surfaces[self.layout().faces[k].surface].point(0.0, 0.0);
            let (r, face, lp) = match region {
                None => {
                    let m = euler::mvfs(&mut self.body, self.keys.next(), p, s)
                        .map_err(Self::topo)?
                        .0;
                    (m.region, m.face, m.loop_)
                }
                Some(r) => {
                    let m = euler::mvs(&mut self.body, self.keys.next(), r, p, s)
                        .map_err(Self::topo)?
                        .0;
                    (r, m.face, m.loop_)
                }
            };
            euler::kvr(&mut self.body, lp).map_err(Self::topo)?;
            self.faces[k] = Some(face);
            return Ok(r);
        }
        // Components in the order faces reach them, each with the face
        // loop it starts from.
        // Each face loop's component, as a dense index.
        let mut index: BTreeMap<usize, usize> = BTreeMap::new();
        let mut roots: Vec<usize> = Vec::new();
        let of: Vec<Vec<usize>> = shell
            .iter()
            .map(|&k| {
                (0..self.layout().faces[k].loops.len())
                    .map(|l| {
                        let c = comp[&self.loop_start(k, l)];
                        *index.entry(c).or_insert_with(|| {
                            roots.push(c);
                            roots.len() - 1
                        })
                    })
                    .collect()
            })
            .collect();
        let mut done = vec![false; roots.len()];
        let mut order: Vec<Reach> = Vec::new();
        let first = (shell[0], 0usize);
        order.push((roots[of[0][0]], first.0, first.1, None));
        done[of[0][0]] = true;
        loop {
            // Every way to reach a new component: a face with a loop in a
            // built component and one in the new one.
            let mut ways: Vec<(usize, usize, usize, usize)> = Vec::new();
            for (i, &k) in shell.iter().enumerate() {
                let Some(a) = of[i].iter().position(|&c| done[c]) else {
                    continue;
                };
                for (l, &c) in of[i].iter().enumerate() {
                    if !done[c] {
                        ways.push((c, k, l, a));
                    }
                }
            }
            if ways.is_empty() {
                break;
            }
            // Prefer a loop that doesn't start on a closed edge it runs
            // forwards: MEF leaves that one off the host face.
            let c = ways[0].0;
            let pick = ways
                .iter()
                .filter(|w| w.0 == c)
                .find(|w| !self.closed_forward(w.1, w.2))
                .or_else(|| ways.iter().find(|w| w.0 == c))
                .copied()
                .expect("a way");
            done[c] = true;
            order.push((roots[c], pick.1, pick.2, Some((pick.1, pick.3))));
        }
        let mut region_out = region;
        for (c, k, l, host) in order {
            let r = self.component(shell, &comp, c, (k, l), host, region_out)?;
            region_out.get_or_insert(r);
            if let Some((hk, ha)) = host {
                self.settle(hk, ha, l)?;
            }
        }
        // Loops of one face left on separate faces join the face holding
        // the most of them (KFMRH takes faces with one loop).
        for &k in shell {
            let n = self.layout().faces[k].loops.len();
            let holders: Vec<FaceId> = (0..n).map(|l| self.face_of_loop(k, l)).collect();
            let count = |f: FaceId| holders.iter().filter(|&&h| h == f).count();
            let keep = holders
                .iter()
                .copied()
                .max_by_key(|&f| (count(f), core::cmp::Reverse(f)))
                .unwrap_or(holders[0]);
            for &f in &holders {
                if f != keep && self.body.face(f).is_some() {
                    euler::kfmrh(&mut self.body, self.keys.next(), keep, f).map_err(Self::topo)?;
                }
            }
            self.faces[k] = Some(keep);
        }
        let mut used: BTreeSet<FaceId> = BTreeSet::new();
        for &k in shell {
            if !used.insert(self.faces[k].expect("built")) {
                return Err(BuildError::Inconsistent);
            }
        }
        region_out.ok_or(BuildError::Inconsistent)
    }

    /// Grows one component of a shell's edge graph, from the start of
    /// face loop `start`, inside the face holding face loop `host` (or as
    /// a new shell). Returns the shell's region.
    fn component(
        &mut self,
        shell: &[usize],
        comp: &BTreeMap<usize, usize>,
        c: usize,
        start: (usize, usize),
        host: Option<(usize, usize)>,
        region: Option<RegionId>,
    ) -> Result<RegionId, BuildError> {
        let root = self.loop_start(start.0, start.1);
        let p = self.point(root);
        let mut out_region = region;
        match host {
            None => {
                let s = self.surface(start.0);
                let (r, v) = match region {
                    None => {
                        let m = euler::mvfs(&mut self.body, self.keys.next(), p, s)
                            .map_err(Self::topo)?
                            .0;
                        (m.region, m.vertex)
                    }
                    Some(r) => {
                        let m = euler::mvs(&mut self.body, self.keys.next(), r, p, s)
                            .map_err(Self::topo)?
                            .0;
                        (r, m.vertex)
                    }
                };
                out_region = Some(r);
                self.vertices.insert(root, v);
                self.vertex_of.insert(v, root);
            }
            Some((k, l)) => {
                let f = self.face_of_loop(k, l);
                let m = euler::mvr(&mut self.body, self.keys.next(), f, p)
                    .map_err(Self::topo)?
                    .0;
                self.vertices.insert(root, m.vertex);
                self.vertex_of.insert(m.vertex, root);
            }
        }
        // The component's edges, the start loop's first arc first.
        let mut ges: Vec<usize> = Vec::new();
        for &k in shell {
            for lp in &self.layout().faces[k].loops {
                if let Boundary::Arcs(a) = lp {
                    for u in a {
                        let g = u.edge;
                        if comp[&self.layout().edges[g].start] == c && !ges.contains(&g) {
                            ges.push(g);
                        }
                    }
                }
            }
        }
        ges.sort_unstable();
        if let Boundary::Arcs(a) = &self.layout().faces[start.0].loops[start.1] {
            let g0 = a[0].edge;
            ges.retain(|&g| g != g0);
            ges.insert(0, g0);
        }
        // A spanning tree, breadth first from the root, each vertex's edges
        // in the order above.
        let mut around: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        for &g in &ges {
            let e = &self.layout().edges[g];
            if e.start != e.end {
                around.entry(e.start).or_default().push(g);
                around.entry(e.end).or_default().push(g);
            }
        }
        let mut tree: BTreeSet<usize> = BTreeSet::new();
        let mut reached: BTreeSet<usize> = BTreeSet::from([root]);
        let mut queue = std::collections::VecDeque::from([root]);
        while let Some(u) = queue.pop_front() {
            for &g in around.get(&u).map_or(&[][..], |v| &v[..]) {
                let (es, ee, interval) = {
                    let e = &self.layout().edges[g];
                    (e.start, e.end, e.interval)
                };
                let w = if es == u { ee } else { es };
                if reached.contains(&w) {
                    continue;
                }
                let has_s = es == u;
                let at = self.corner(
                    u,
                    End {
                        ge: g,
                        start: has_s,
                    },
                )?;
                let curve = self.curve(g);
                let pw = self.point(w);
                let key = self.keys.next();
                let m = if has_s {
                    euler::mev(&mut self.body, key, at, pw, curve, interval)
                } else {
                    euler::mev_reversed(&mut self.body, key, at, pw, curve, interval)
                }
                .map_err(Self::topo)?
                .0;
                self.edges.insert(g, m.edge);
                self.edge_of.insert(m.edge, g);
                self.vertices.insert(w, m.vertex);
                self.vertex_of.insert(m.vertex, w);
                reached.insert(w);
                tree.insert(g);
                queue.push_back(w);
            }
        }
        // Every other edge closes a face, joins two loops, or makes a
        // handle.
        for &g in &ges {
            if tree.contains(&g) {
                continue;
            }
            let (es, ee, interval) = {
                let e = &self.layout().edges[g];
                (e.start, e.end, e.interval)
            };
            let from = self.corner(es, End { ge: g, start: true })?;
            let to = self.corner(
                ee,
                End {
                    ge: g,
                    start: false,
                },
            )?;
            if es == ee && from == to {
                // A closed edge between the same two ends: MEF puts its end
                // before its start in the rotation.
                let rot = &self.plan.rotation[&es];
                let si = rot.iter().position(|x| *x == End { ge: g, start: true });
                let ei = rot.iter().position(|x| {
                    *x == End {
                        ge: g,
                        start: false,
                    }
                });
                let (Some(si), Some(ei)) = (si, ei) else {
                    return Err(BuildError::Inconsistent);
                };
                if (ei + 1) % rot.len() != si && !matches!(from, Corner::Lone(_)) {
                    return Err(BuildError::Inconsistent);
                }
            }
            let curve = self.curve(g);
            let (la, lb) = (self.corner_loop(from), self.corner_loop(to));
            let made = if la == lb {
                euler::mef(&mut self.body, self.keys.next(), from, to, curve, interval)
                    .map_err(Self::topo)?
                    .0
                    .edge
            } else {
                let (fa, fb) = (
                    self.body.loop_(la).expect("live").face,
                    self.body.loop_(lb).expect("live").face,
                );
                if fa != fb {
                    let one = |f: FaceId| self.body.face_loops(f).count() == 1;
                    let (keep, kill) = if one(fb) {
                        (fa, fb)
                    } else if one(fa) {
                        (fb, fa)
                    } else {
                        return Err(BuildError::Inconsistent);
                    };
                    euler::kfmrh(&mut self.body, self.keys.next(), keep, kill)
                        .map_err(Self::topo)?;
                }
                euler::mekr(&mut self.body, self.keys.next(), from, to, curve, interval)
                    .map_err(Self::topo)?
                    .0
                    .edge
            };
            self.edges.insert(g, made);
            self.edge_of.insert(made, g);
        }
        out_region.ok_or(BuildError::Inconsistent)
    }

    fn corner_loop(&self, c: Corner) -> LoopId {
        match c {
            Corner::Before(f) => self.body.fin(f).expect("live").loop_,
            Corner::Lone(l) => l,
        }
    }

    /// Surfaces, pcurves and tolerances, once every shell is grown.
    fn finish(mut self) -> Result<Built, BuildError> {
        let layout = self.plan.layout;
        for k in 0..layout.faces.len() {
            let face = self.faces[k].ok_or(BuildError::Inconsistent)?;
            let s = self.surface(k);
            euler::set_surface(&mut self.body, face, s, layout.faces[k].sense)
                .map_err(Self::topo)?;
            for lp in &layout.faces[k].loops {
                if let Boundary::Arcs(a) = lp {
                    for u in a {
                        let Some((c, map)) = u.pcurve else {
                            continue;
                        };
                        let fin = self.fin(u.edge, u.sense);
                        let id = match self.curves2.get(&c) {
                            Some(&id) => id,
                            None => {
                                let id = self.body.add_curve2(layout.curves2[c].clone());
                                self.curves2.insert(c, id);
                                id
                            }
                        };
                        euler::set_pcurve(&mut self.body, fin, Some(Pcurve { curve: id, map }))
                            .map_err(Self::topo)?;
                    }
                }
            }
        }
        let tol =
            |t: f64| Tolerance::new(t.max(LINEAR_RESOLUTION)).map_err(|_| BuildError::Inconsistent);
        let mut vertex_tol: BTreeMap<usize, f64> = BTreeMap::new();
        for (&g, &e) in &self.edges {
            let le = &layout.edges[g];
            let curve = &layout.curves[le.curve];
            // The edge's curve may end a little way from its vertex: both
            // tolerances cover the gap.
            let mut t_edge = le.tolerance;
            for (t, v) in [(le.interval[0], le.start), (le.interval[1], le.end)] {
                let d = curve.point(t).distance(layout.vertices[v].point) * 1.0001 + 1e-12;
                t_edge = t_edge.max(d);
                let x = vertex_tol.entry(v).or_insert(0.0);
                *x = x.max(d);
            }
            euler::set_edge_tolerance(&mut self.body, e, tol(t_edge)?).map_err(Self::topo)?;
        }
        for (&g, &v) in &self.vertices {
            let t = layout.vertices[g]
                .tolerance
                .max(vertex_tol.get(&g).copied().unwrap_or(0.0));
            euler::set_vertex_tolerance(&mut self.body, v, tol(t)?).map_err(Self::topo)?;
        }
        Ok(Built {
            body: self.body,
            faces: self.faces.into_iter().map(|f| f.expect("built")).collect(),
            edges: self.edges,
            vertices: self.vertices,
        })
    }
}
