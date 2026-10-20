//! The shell builder: kept pieces into a new body, through Euler
//! operators.
//!
//! Every global edge must have exactly two kept uses of opposite sense,
//! and the faces around every vertex must form one fan; otherwise the
//! result would be non-manifold and is refused. The fans fix each
//! vertex's rotation: the cyclic order of the edge ends at it.
//!
//! Each shell (pieces joined by edges) is grown one connected component
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
//! tolerances.
//!
//! Each shell is first built as a solid region of its own. A shell whose
//! outward side is inside it (a point just off one of its faces, along
//! the outward normal, classified against it) is a void: it goes in the
//! region of the smallest outer shell around it, and the body is built
//! again with it made by MVS there.
//!
//! [`mev_reversed`]: crate::euler::mev_reversed

use super::table::{Boundary, GeId, GvId, Member, Table};
use super::{FaceRef, Fail};
use crate::euler::{self, Corner};
use crate::geom::{Box3, Point3};
use crate::intersect::{Inclusion, face_box, point_in_faces};
use crate::name::{BooleanRole, Name, OpKey, Role};
use crate::split::Piece;
use crate::tol::{LINEAR_RESOLUTION, Tolerance};
use crate::topo::{
    Body, Curve2Id, CurveId, EdgeId, FaceId, FinId, Pcurve, RegionId, Sense, SurfaceId, VertexId,
};
use std::collections::{BTreeMap, BTreeSet};

/// A piece kept for the result, run backwards when `reversed`.
#[derive(Clone, Debug)]
pub(crate) struct Kept {
    pub(crate) piece: Piece,
    pub(crate) reversed: bool,
}

impl Kept {
    /// Its loops as the result runs them.
    pub(crate) fn loops(&self) -> Vec<Boundary> {
        if !self.reversed {
            return self.piece.loops.clone();
        }
        self.piece
            .loops
            .iter()
            .map(|l| match l {
                Boundary::Lone(g) => Boundary::Lone(*g),
                Boundary::Arcs(a) => {
                    Boundary::Arcs(a.iter().rev().map(|&(g, s)| (g, s.flipped())).collect())
                }
            })
            .collect()
    }
}

/// The built body, and where each kept piece, global edge and global
/// vertex went.
#[derive(Clone, Debug)]
pub(crate) struct Built {
    pub(crate) body: Body,
    pub(crate) faces: Vec<FaceId>,
    pub(crate) edges: BTreeMap<GeId, EdgeId>,
    pub(crate) vertices: BTreeMap<GvId, VertexId>,
}

/// An end of a global edge at a vertex: its start or its end.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct End {
    ge: GeId,
    start: bool,
}

/// Builds the kept pieces into a body.
pub(crate) fn build(ops: &[Body], table: &Table, kept: &[Kept], key: OpKey) -> Result<Built, Fail> {
    let plan = Plan::new(ops, table, kept)?;
    let shells = plan.shells();
    let first = plan.build_all(key, &shells, &vec![None; shells.len()])?;
    // Which shells are voids, and the outer shell around each.
    let mut outer_of: Vec<Option<usize>> = vec![None; shells.len()];
    let mut voids = Vec::new();
    for (s, faces) in shells.iter().enumerate() {
        if plan.is_void(&first, faces)? {
            voids.push(s);
        }
    }
    if voids.is_empty() {
        return Ok(first);
    }
    let outers: Vec<usize> = (0..shells.len()).filter(|s| !voids.contains(s)).collect();
    for &s in &voids {
        let p = plan.point_on(&first, &shells[s])?;
        let mut best: Option<(f64, usize)> = None;
        for &o in &outers {
            let faces: Vec<FaceId> = shells[o].iter().map(|&k| first.faces[k]).collect();
            match point_in_faces(&first.body, &faces, p) {
                Ok(Inclusion::Inside) => {
                    let b = faces
                        .iter()
                        .map(|&f| face_box(&first.body, f))
                        .reduce(|x, y| x.union(&y))
                        .map_or(0.0, |b| volume(&b));
                    if best.is_none_or(|(v, _)| b < v) {
                        best = Some((b, o));
                    }
                }
                Ok(Inclusion::Outside) => {}
                _ => return Err(plan.uncertain(&shells[s])),
            }
        }
        // A void outside every outer shell can't be in a regularized
        // result.
        outer_of[s] = Some(best.ok_or(Fail::Inconsistent)?.1);
    }
    plan.build_all(key, &shells, &outer_of)
}

fn volume(b: &Box3) -> f64 {
    let d = b.max() - b.min();
    d.x * d.y * d.z
}

/// What the builder knows before it starts.
struct Plan<'a> {
    ops: &'a [Body],
    table: &'a Table,
    kept: &'a [Kept],
    /// Each kept piece's loops as the result runs them.
    loops: Vec<Vec<Boundary>>,
    /// Each global edge's two uses: piece, loop, position.
    uses: BTreeMap<GeId, Vec<(usize, usize, usize)>>,
    /// Each vertex's edge ends in rotation order.
    rotation: BTreeMap<GvId, Vec<End>>,
    /// Lone vertices: their piece.
    lone: BTreeMap<GvId, usize>,
}

impl<'a> Plan<'a> {
    fn new(ops: &'a [Body], table: &'a Table, kept: &'a [Kept]) -> Result<Self, Fail> {
        let loops: Vec<Vec<Boundary>> = kept.iter().map(Kept::loops).collect();
        let mut uses: BTreeMap<GeId, Vec<(usize, usize, usize)>> = BTreeMap::new();
        let mut lone: BTreeMap<GvId, usize> = BTreeMap::new();
        for (k, ls) in loops.iter().enumerate() {
            for (l, lp) in ls.iter().enumerate() {
                match lp {
                    Boundary::Arcs(a) => {
                        for (i, &(g, _)) in a.iter().enumerate() {
                            uses.entry(g).or_default().push((k, l, i));
                        }
                    }
                    Boundary::Lone(g) => {
                        if lone.insert(*g, k).is_some() {
                            let at = table.vertices[*g].point;
                            return Err(Fail::NonManifold {
                                at,
                                faces: vec![kept[k].piece.face],
                            });
                        }
                    }
                }
            }
        }
        let arc = |k: usize, l: usize, i: usize| -> (GeId, Sense) {
            match &loops[k][l] {
                Boundary::Arcs(a) => a[i],
                Boundary::Lone(_) => unreachable!("arcs only"),
            }
        };
        for (&g, list) in &uses {
            let ok = list.len() == 2
                && arc(list[0].0, list[0].1, list[0].2).1 != arc(list[1].0, list[1].1, list[1].2).1;
            if !ok {
                let e = &table.edges[g];
                let at = e.curve.point((e.interval[0] + e.interval[1]) / 2.0);
                return Err(Fail::NonManifold {
                    at,
                    faces: list.iter().map(|u| kept[u.0].piece.face).collect(),
                });
            }
        }
        // The fans: corners at each vertex, walked in rotation order.
        let ends_of = |g: GeId, s: Sense| -> (GvId, End, GvId, End) {
            let e = &table.edges[g];
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
        // A corner (piece, loop, i) is at the end of arc i, before arc
        // i + 1; its outgoing edge end, and the corner its twin starts.
        let mut corners: BTreeMap<GvId, Vec<(usize, usize, usize)>> = BTreeMap::new();
        for (k, ls) in loops.iter().enumerate() {
            for (l, lp) in ls.iter().enumerate() {
                if let Boundary::Arcs(a) = lp {
                    for (i, &(g, sn)) in a.iter().enumerate() {
                        let (_, _, v, _) = ends_of(g, sn);
                        corners.entry(v).or_default().push((k, l, i));
                    }
                }
            }
        }
        let mut rotation: BTreeMap<GvId, Vec<End>> = BTreeMap::new();
        for (&v, list) in &corners {
            if lone.contains_key(&v) {
                return Err(Fail::NonManifold {
                    at: table.vertices[v].point,
                    faces: list.iter().map(|c| kept[c.0].piece.face).collect(),
                });
            }
            let mut seen: BTreeSet<(usize, usize, usize)> = BTreeSet::new();
            let mut order = Vec::new();
            let mut c = list[0];
            loop {
                if !seen.insert(c) {
                    break;
                }
                let n = match &loops[c.0][c.1] {
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
                return Err(Fail::NonManifold {
                    at: table.vertices[v].point,
                    faces: list.iter().map(|c| kept[c.0].piece.face).collect(),
                });
            }
            rotation.insert(v, order);
        }
        Ok(Self {
            ops,
            table,
            kept,
            loops,
            uses,
            rotation,
            lone,
        })
    }

    fn uncertain(&self, shell: &[usize]) -> Fail {
        let f = self.kept[shell[0]].piece.face;
        Fail::Uncertain(f, f)
    }

    /// The shells: pieces joined by edges, each listed in piece order.
    fn shells(&self) -> Vec<Vec<usize>> {
        let n = self.kept.len();
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
    /// their own first, then voids in their outer shells' regions.
    fn build_all(
        &self,
        key: OpKey,
        shells: &[Vec<usize>],
        outer_of: &[Option<usize>],
    ) -> Result<Built, Fail> {
        let mut g = Grower {
            plan: self,
            body: Body::new(),
            keys: Keys {
                base: key,
                count: 0,
            },
            faces: vec![None; self.kept.len()],
            edges: BTreeMap::new(),
            vertices: BTreeMap::new(),
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

    /// Whether a shell's faces point into what it encloses: a point just
    /// off one of them, along its outward normal, is inside the shell.
    fn is_void(&self, built: &Built, shell: &[usize]) -> Result<bool, Fail> {
        let faces: Vec<FaceId> = shell.iter().map(|&k| built.faces[k]).collect();
        let size = faces
            .iter()
            .map(|&f| face_box(&built.body, f))
            .reduce(|x, y| x.union(&y))
            .map_or(1.0, |b| (b.max() - b.min()).length());
        for &k in shell {
            for attempt in 0..4 {
                let Some((q, uv)) = super::classify::inner_point(
                    self.ops,
                    self.table,
                    &self.kept[k].piece,
                    attempt,
                ) else {
                    continue;
                };
                let src = self.kept[k].piece.face;
                let b = &self.ops[src.op];
                let surface = &b.surfaces[b.faces[src.face].surface];
                let Some(n) = surface.normal(uv.x, uv.y) else {
                    continue;
                };
                let mut sign = if b.faces[src.face].sense == Sense::Forward {
                    1.0
                } else {
                    -1.0
                };
                if self.kept[k].reversed {
                    sign = -sign;
                }
                for eps in [1e-4, 1e-5, 1e-6] {
                    let p = q + n.get() * (sign * eps * size);
                    match point_in_faces(&built.body, &faces, p) {
                        Ok(Inclusion::Inside) => return Ok(true),
                        Ok(Inclusion::Outside) => return Ok(false),
                        _ => {}
                    }
                }
            }
        }
        Err(self.uncertain(shell))
    }

    /// A point of a shell: one of its vertices, or a point of its first
    /// face.
    fn point_on(&self, built: &Built, shell: &[usize]) -> Result<Point3, Fail> {
        let f = built.faces[shell[0]];
        let b = &built.body;
        for l in b.face_loops(f) {
            if let Some(v) = b.loop_(l).and_then(|l| l.vertex) {
                return Ok(b.vertex(v).expect("live").point);
            }
            if let Some(fin) = b.loop_fins(l).next() {
                let v = b.fin_start(fin).expect("live");
                return Ok(b.vertex(v).expect("live").point);
            }
        }
        let (q, _) =
            super::classify::inner_point(self.ops, self.table, &self.kept[shell[0]].piece, 0)
                .ok_or_else(|| self.uncertain(shell))?;
        Ok(q)
    }

    fn arc(&self, k: usize, l: usize, i: usize) -> (GeId, Sense) {
        match &self.loops[k][l] {
            Boundary::Arcs(a) => a[i],
            Boundary::Lone(_) => unreachable!("arcs only"),
        }
    }
}

/// A component to grow: its vertex class, the piece and loop it starts
/// from, and the piece loop whose face hosts it (none for a shell's
/// first).
type Reach = (GvId, usize, usize, Option<(usize, usize)>);

/// Fresh keys for the Euler operators: their names are scaffolding, but
/// must stay unique on the way.
struct Keys {
    base: OpKey,
    count: u128,
}

impl Keys {
    fn next(&mut self) -> OpKey {
        self.count += 1;
        let scaffold = Name::from_u128(self.count);
        OpKey(Name::derive(self.base, Role::Boolean(BooleanRole::Merged), &[scaffold]).to_u128())
    }
}

/// The body being grown and where everything went.
struct Grower<'p, 'a> {
    plan: &'p Plan<'a>,
    body: Body,
    keys: Keys,
    faces: Vec<Option<FaceId>>,
    edges: BTreeMap<GeId, EdgeId>,
    vertices: BTreeMap<GvId, VertexId>,
    surfaces: BTreeMap<(usize, SurfaceId), SurfaceId>,
    curves: BTreeMap<Member, CurveId>,
    /// Pcurves added, by face and carrier: pieces of one carrier on one
    /// face share their pcurve, so JEKV can join them again.
    curves2: BTreeMap<(FaceRef, Member), Vec<(crate::geom::Curve2, Curve2Id)>>,
}

impl Grower<'_, '_> {
    fn topo(e: crate::Error) -> Fail {
        let _ = e;
        Fail::Inconsistent
    }

    fn surface(&mut self, k: usize) -> SurfaceId {
        let src = self.plan.kept[k].piece.face;
        let b = &self.plan.ops[src.op];
        let s = b.faces[src.face].surface;
        if let Some(&id) = self.surfaces.get(&(src.op, s)) {
            return id;
        }
        let id = self.body.add_surface(b.surfaces[s].clone());
        self.surfaces.insert((src.op, s), id);
        id
    }

    fn curve(&mut self, g: GeId) -> CurveId {
        let e = &self.plan.table.edges[g];
        let m = e.members[0];
        if let Some(&id) = self.curves.get(&m) {
            return id;
        }
        let id = self.body.add_curve(e.curve.clone());
        self.curves.insert(m, id);
        id
    }

    fn point(&self, g: GvId) -> Point3 {
        self.plan.table.vertices[g].point
    }

    /// The fin of a built edge running with sense `s`.
    fn fin(&self, g: GeId, s: Sense) -> FinId {
        let e = self.edges[&g];
        self.body
            .edge_fins(e)
            .find(|&f| self.body.fin(f).expect("live").sense == s)
            .expect("a fin each way")
    }

    /// The corner at vertex `v` where edge end `x` goes in, from the
    /// rotation and the edge ends already built there.
    fn corner(&self, v: GvId, x: End) -> Result<Corner, Fail> {
        let built = |e: &End| self.edges.contains_key(&e.ge);
        let rot = &self.plan.rotation[&v];
        if !rot.iter().any(built) {
            let vid = self.vertices[&v];
            let l = self
                .body
                .vertex(vid)
                .expect("live")
                .lone
                .ok_or(Fail::Inconsistent)?;
            return Ok(Corner::Lone(l));
        }
        let n = rot.len();
        let i = rot.iter().position(|e| *e == x).ok_or(Fail::Inconsistent)?;
        let after = (1..n)
            .map(|d| rot[(i + d) % n])
            .find(built)
            .ok_or(Fail::Inconsistent)?;
        let before = (1..n)
            .map(|d| rot[(i + n - d) % n])
            .find(built)
            .ok_or(Fail::Inconsistent)?;
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
            return Err(Fail::Inconsistent);
        }
        Ok(Corner::Before(f))
    }

    /// The face holding a piece's loop.
    fn face_of_loop(&self, k: usize, l: usize) -> FaceId {
        let lid = match &self.plan.loops[k][l] {
            Boundary::Arcs(a) => {
                let f = self.fin(a[0].0, a[0].1);
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
    fn components(&self, shell: &[usize]) -> BTreeMap<GvId, GvId> {
        let mut parent: BTreeMap<GvId, GvId> = BTreeMap::new();
        fn find(parent: &mut BTreeMap<GvId, GvId>, i: GvId) -> GvId {
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
            for lp in &self.plan.loops[k] {
                match lp {
                    Boundary::Lone(g) => {
                        parent.entry(*g).or_insert(*g);
                    }
                    Boundary::Arcs(a) => {
                        for &(g, _) in a {
                            let e = &self.plan.table.edges[g];
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
        let keys: Vec<GvId> = parent.keys().copied().collect();
        keys.into_iter()
            .map(|g| (g, find(&mut parent, g)))
            .collect()
    }

    /// Whether a piece loop is one closed edge run forwards.
    fn closed_forward(&self, k: usize, l: usize) -> bool {
        match &self.plan.loops[k][l] {
            Boundary::Arcs(a) => {
                let e = &self.plan.table.edges[a[0].0];
                a.len() == 1 && e.start == e.end && a[0].1 == Sense::Forward
            }
            Boundary::Lone(_) => false,
        }
    }

    /// The piece a built loop belongs to.
    fn owner(&self, l: crate::topo::LoopId) -> Option<usize> {
        let lp = self.body.loop_(l)?;
        if let Some(v) = lp.vertex {
            let g = self.vertices.iter().find(|x| *x.1 == v)?.0;
            return self.plan.lone.get(g).copied();
        }
        let f = lp.first_fin?;
        let fin = self.body.fin(f)?;
        let g = *self.edges.iter().find(|x| *x.1 == fin.edge)?.0;
        self.plan.uses[&g]
            .iter()
            .find(|u| self.plan.arc(u.0, u.1, u.2).1 == fin.sense)
            .map(|u| u.0)
    }

    /// After a component hosted in the face of piece `k`'s loop `a` from
    /// its loop `l`: if MEF left loop `l` on a face of its own (a closed
    /// edge run forwards), that face joins the host by KFMRH, and the loops
    /// of other pieces on the host leave it again by MFKRH.
    fn settle(&mut self, k: usize, a: usize, l: usize) -> Result<(), Fail> {
        let host = self.face_of_loop(k, a);
        let mine = self.face_of_loop(k, l);
        if host == mine {
            return Ok(());
        }
        if self.body.face_loops(mine).count() != 1 {
            return Err(Fail::Inconsistent);
        }
        euler::kfmrh(&mut self.body, self.keys.next(), host, mine).map_err(Self::topo)?;
        let foreign: Vec<crate::topo::LoopId> = self
            .body
            .face_loops(host)
            .filter(|&x| self.owner(x) != Some(k))
            .collect();
        if foreign.len() > 1 {
            return Err(Fail::Inconsistent);
        }
        for x in foreign {
            euler::mfkrh(&mut self.body, self.keys.next(), x).map_err(Self::topo)?;
        }
        Ok(())
    }

    /// The loop's starting vertex.
    fn loop_start(&self, k: usize, l: usize) -> GvId {
        match &self.plan.loops[k][l] {
            Boundary::Lone(g) => *g,
            Boundary::Arcs(a) => {
                let e = &self.plan.table.edges[a[0].0];
                if a[0].1 == Sense::Forward {
                    e.start
                } else {
                    e.end
                }
            }
        }
    }

    /// Grows one shell; returns its region.
    fn shell(&mut self, shell: &[usize], region: Option<RegionId>) -> Result<RegionId, Fail> {
        let comp = self.components(shell);
        // Loopless faces: a whole sphere or torus.
        if shell.len() == 1 && self.plan.loops[shell[0]].is_empty() {
            let k = shell[0];
            let s = self.surface(k);
            let src = self.plan.kept[k].piece.face;
            let b = &self.plan.ops[src.op];
            let p = b.surfaces[b.faces[src.face].surface].point(0.0, 0.0);
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
        // Components in the order faces reach them, each with the piece
        // loop it starts from.
        let mut done: BTreeSet<GvId> = BTreeSet::new();
        let mut order: Vec<Reach> = Vec::new();
        let first = (shell[0], 0usize);
        order.push((
            comp[&self.loop_start(first.0, first.1)],
            first.0,
            first.1,
            None,
        ));
        done.insert(order[0].0);
        loop {
            // Every way to reach a new component: a piece with a loop in a
            // built component and one in the new one.
            let mut ways: Vec<(GvId, usize, usize, usize)> = Vec::new();
            for &k in shell {
                let n = self.plan.loops[k].len();
                let Some(a) = (0..n).find(|&l| done.contains(&comp[&self.loop_start(k, l)])) else {
                    continue;
                };
                for l in 0..n {
                    let c = comp[&self.loop_start(k, l)];
                    if !done.contains(&c) {
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
            done.insert(c);
            order.push((c, pick.1, pick.2, Some((pick.1, pick.3))));
        }
        let mut region_out = region;
        for (c, k, l, host) in order {
            let r = self.component(shell, &comp, c, (k, l), host, region_out)?;
            region_out.get_or_insert(r);
            if let Some((hk, ha)) = host {
                self.settle(hk, ha, l)?;
            }
        }
        // Loops of one piece left on separate faces join it.
        for &k in shell {
            let n = self.plan.loops[k].len();
            let keep = self.face_of_loop(k, 0);
            for l in 1..n {
                let f = self.face_of_loop(k, l);
                if f != keep {
                    euler::kfmrh(&mut self.body, self.keys.next(), keep, f).map_err(Self::topo)?;
                }
            }
            self.faces[k] = Some(keep);
        }
        let mut used: BTreeSet<FaceId> = BTreeSet::new();
        for &k in shell {
            if !used.insert(self.faces[k].expect("built")) {
                return Err(Fail::Inconsistent);
            }
        }
        region_out.ok_or(Fail::Inconsistent)
    }

    /// Grows one component of a shell's edge graph, from the start of
    /// piece loop `start`, inside the face holding piece loop `host` (or as
    /// a new shell). Returns the shell's region.
    fn component(
        &mut self,
        shell: &[usize],
        comp: &BTreeMap<GvId, GvId>,
        c: GvId,
        start: (usize, usize),
        host: Option<(usize, usize)>,
        region: Option<RegionId>,
    ) -> Result<RegionId, Fail> {
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
            }
            Some((k, l)) => {
                let f = self.face_of_loop(k, l);
                let m = euler::mvr(&mut self.body, self.keys.next(), f, p)
                    .map_err(Self::topo)?
                    .0;
                self.vertices.insert(root, m.vertex);
            }
        }
        // The component's edges, the start loop's first arc first.
        let mut ges: Vec<GeId> = Vec::new();
        for &k in shell {
            for lp in &self.plan.loops[k] {
                if let Boundary::Arcs(a) = lp {
                    for &(g, _) in a {
                        if comp[&self.plan.table.edges[g].start] == c && !ges.contains(&g) {
                            ges.push(g);
                        }
                    }
                }
            }
        }
        ges.sort_unstable();
        if let Boundary::Arcs(a) = &self.plan.loops[start.0][start.1] {
            let g0 = a[0].0;
            ges.retain(|&g| g != g0);
            ges.insert(0, g0);
        }
        // A spanning tree, breadth first from the root, each vertex's edges
        // in the order above.
        let mut around: BTreeMap<GvId, Vec<GeId>> = BTreeMap::new();
        for &g in &ges {
            let e = &self.plan.table.edges[g];
            if e.start != e.end {
                around.entry(e.start).or_default().push(g);
                around.entry(e.end).or_default().push(g);
            }
        }
        let mut tree: BTreeSet<GeId> = BTreeSet::new();
        let mut reached: BTreeSet<GvId> = BTreeSet::from([root]);
        let mut queue = std::collections::VecDeque::from([root]);
        while let Some(u) = queue.pop_front() {
            for &g in around.get(&u).map_or(&[][..], |v| &v[..]) {
                let e = &self.plan.table.edges[g];
                let w = if e.start == u { e.end } else { e.start };
                if reached.contains(&w) {
                    continue;
                }
                let has_s = e.start == u;
                let at = self.corner(
                    u,
                    End {
                        ge: g,
                        start: has_s,
                    },
                )?;
                let curve = self.curve(g);
                let (pw, interval) = (self.point(w), e.interval);
                let key = self.keys.next();
                let m = if has_s {
                    euler::mev(&mut self.body, key, at, pw, curve, interval)
                } else {
                    euler::mev_reversed(&mut self.body, key, at, pw, curve, interval)
                }
                .map_err(Self::topo)?
                .0;
                self.edges.insert(g, m.edge);
                self.vertices.insert(w, m.vertex);
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
            let e = &self.plan.table.edges[g];
            let from = self.corner(e.start, End { ge: g, start: true })?;
            let to = self.corner(
                e.end,
                End {
                    ge: g,
                    start: false,
                },
            )?;
            if e.start == e.end && from == to {
                // A closed edge between the same two ends: MEF puts its end
                // before its start in the rotation.
                let rot = &self.plan.rotation[&e.start];
                let si = rot.iter().position(|x| *x == End { ge: g, start: true });
                let ei = rot.iter().position(|x| {
                    *x == End {
                        ge: g,
                        start: false,
                    }
                });
                let (Some(si), Some(ei)) = (si, ei) else {
                    return Err(Fail::Inconsistent);
                };
                if (ei + 1) % rot.len() != si && !matches!(from, Corner::Lone(_)) {
                    return Err(Fail::Inconsistent);
                }
            }
            let curve = self.curve(g);
            let (la, lb) = (self.corner_loop(from), self.corner_loop(to));
            let made = if la == lb {
                euler::mef(
                    &mut self.body,
                    self.keys.next(),
                    from,
                    to,
                    curve,
                    e.interval,
                )
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
                        return Err(Fail::Inconsistent);
                    };
                    euler::kfmrh(&mut self.body, self.keys.next(), keep, kill)
                        .map_err(Self::topo)?;
                }
                euler::mekr(
                    &mut self.body,
                    self.keys.next(),
                    from,
                    to,
                    curve,
                    e.interval,
                )
                .map_err(Self::topo)?
                .0
                .edge
            };
            self.edges.insert(g, made);
        }
        out_region.ok_or(Fail::Inconsistent)
    }

    fn corner_loop(&self, c: Corner) -> crate::topo::LoopId {
        match c {
            Corner::Before(f) => self.body.fin(f).expect("live").loop_,
            Corner::Lone(l) => l,
        }
    }

    /// Surfaces, pcurves and tolerances, once every shell is grown.
    fn finish(mut self) -> Result<Built, Fail> {
        let plan = self.plan;
        let table = plan.table;
        for k in 0..plan.kept.len() {
            let face = self.faces[k].ok_or(Fail::Inconsistent)?;
            let src = plan.kept[k].piece.face;
            let b = &plan.ops[src.op];
            let mut sense = b.faces[src.face].sense;
            if plan.kept[k].reversed {
                sense = sense.flipped();
            }
            let s = self.surface(k);
            euler::set_surface(&mut self.body, face, s, sense).map_err(Self::topo)?;
            for lp in &plan.loops[k] {
                if let Boundary::Arcs(a) = lp {
                    for &(g, sn) in a {
                        let fin = self.fin(g, sn);
                        let (c, map) = &table.edges[g].pcurves[&src];
                        let slot = (src, table.edges[g].members[0]);
                        let found = self
                            .curves2
                            .get(&slot)
                            .and_then(|list| list.iter().find(|x| x.0 == *c).map(|x| x.1));
                        let id = match found {
                            Some(id) => id,
                            None => {
                                let id = self.body.add_curve2(c.clone());
                                self.curves2.entry(slot).or_default().push((c.clone(), id));
                                id
                            }
                        };
                        let map = *map;
                        euler::set_pcurve(&mut self.body, fin, Some(Pcurve { curve: id, map }))
                            .map_err(Self::topo)?;
                    }
                }
            }
        }
        let tol = |t: f64| Tolerance::new(t.max(LINEAR_RESOLUTION)).map_err(|_| Fail::Inconsistent);
        let mut vertex_tol: BTreeMap<GvId, f64> = BTreeMap::new();
        for (&g, &e) in &self.edges {
            let ge = &table.edges[g];
            euler::set_edge_tolerance(&mut self.body, e, tol(ge.tolerance)?).map_err(Self::topo)?;
            for (t, v) in [(ge.interval[0], ge.start), (ge.interval[1], ge.end)] {
                let d = ge.curve.point(t).distance(table.vertices[v].point);
                let x = vertex_tol.entry(v).or_insert(0.0);
                *x = x.max(d * 1.0001 + 1e-12);
            }
        }
        for (&g, &v) in &self.vertices {
            let t = table.vertices[g]
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::boolean::table;
    use crate::geom::{Rigid, Vector3};
    use crate::mesh::{MeshParams, check_mesh, tessellate};
    use crate::name::OpKey;
    use crate::primitive;
    use crate::split::split_all;

    fn volume(b: &Body) -> f64 {
        let m = tessellate(b, &MeshParams::new(2e-3, 0.3)).unwrap();
        assert!(m.faults.is_empty(), "{:?}", m.faults);
        check_mesh(b, &m).unwrap();
        m.welded_triangles()
            .map(|t| {
                let [a, b, c] = t.map(|i| m.positions[i as usize].to_vector());
                a.dot(b.cross(c)) / 6.0
            })
            .sum()
    }

    /// A body built again from its own faces.
    fn rebuilt(b: &Body) -> Body {
        let ops = [b.clone()];
        let t = table::build(&ops).unwrap();
        let pieces = split_all(&ops, &t).unwrap();
        let kept: Vec<Kept> = pieces
            .into_iter()
            .map(|piece| Kept {
                piece,
                reversed: false,
            })
            .collect();
        let out = build(&ops, &t, &kept, OpKey(77)).unwrap().body;
        assert_eq!(crate::check::check(&out), [], "{}", out.dump());
        out
    }

    fn same(a: &Body) {
        let b = rebuilt(a);
        let (ca, cb) = (a.counts(), b.counts());
        assert_eq!(ca, cb);
        let (va, vb) = (volume(a), volume(&b));
        assert!((va - vb).abs() <= 1e-9 * va.abs().max(1.0), "{va} {vb}");
    }

    #[test]
    fn primitives_build_again() {
        let t = Rigid::rotation(Vector3::new(1.0, 2.0, 0.5).normalize().unwrap(), 0.7);
        let k = OpKey(1);
        same(&primitive::block(k, 1.0, 2.0, 3.0, &t).unwrap().0);
        same(&primitive::cylinder(k, 1.0, 2.0, &t).unwrap().0);
        same(&primitive::cone(k, 1.0, 2.0, &t).unwrap().0);
        same(&primitive::frustum(k, 1.0, 0.4, 2.0, &t).unwrap().0);
        same(&primitive::sphere(k, 1.0, &t).unwrap().0);
        same(&primitive::torus(k, 2.0, 0.5, &t).unwrap().0);
    }

    #[test]
    fn handles_and_cavities_build_again() {
        use crate::geom::{Axis, Circle2, Curve2, Frame, Frame2, Line2, Point2, Vector2};
        use crate::sweep::{
            Loop, LoopKey, Profile, Segment, SegmentKey, ValidProfile, extrude, revolve,
        };
        let square = |x: f64, y: f64, s: f64, key: u128| -> Vec<Segment> {
            let c = [
                Point2::new(x, y),
                Point2::new(x + s, y),
                Point2::new(x + s, y + s),
                Point2::new(x, y + s),
            ];
            (0..4)
                .map(|i| Segment {
                    key: SegmentKey(key + i as u128),
                    curve: Curve2::Line2(Line2::through(c[i], c[(i + 1) % 4]).unwrap()),
                    t0: 0.0,
                    t1: s,
                })
                .collect()
        };
        let circle = |x: f64, y: f64, r: f64, key: u128| Segment {
            key: SegmentKey(key),
            curve: Curve2::Circle2(
                Circle2::new(
                    Frame2::new(Point2::new(x, y), Vector2::new(1.0, 0.0), true).unwrap(),
                    r,
                )
                .unwrap(),
            ),
            t0: 0.0,
            t1: core::f64::consts::TAU,
        };
        // A plate with a square hole and a round one: genus two.
        let plate = ValidProfile::new(Profile {
            frame: Frame::WORLD,
            loops: vec![
                Loop {
                    key: LoopKey(1),
                    segments: square(0.0, 0.0, 4.0, 1),
                },
                Loop {
                    key: LoopKey(2),
                    segments: square(0.5, 0.5, 1.0, 10),
                },
                Loop {
                    key: LoopKey(3),
                    segments: vec![circle(3.0, 3.0, 0.5, 20)],
                },
            ],
        })
        .unwrap();
        same(
            &extrude(OpKey(2), &plate, Vector3::new(0.0, 0.0, 1.0), 0.0, 1.0)
                .unwrap()
                .0,
        );
        // A ring with a round hollow inside it: a cavity.
        let frame = Frame::new(
            crate::geom::Point3::ORIGIN,
            Vector3::new(0.0, -1.0, 0.0),
            Vector3::new(1.0, 0.0, 0.0),
        )
        .unwrap();
        let ring = ValidProfile::new(Profile {
            frame,
            loops: vec![
                Loop {
                    key: LoopKey(1),
                    segments: square(1.0, -0.5, 1.0, 1),
                },
                Loop {
                    key: LoopKey(2),
                    segments: vec![circle(1.5, 0.0, 0.25, 9)],
                },
            ],
        })
        .unwrap();
        let axis = Axis::new(crate::geom::Point3::ORIGIN, Vector3::new(0.0, 0.0, 1.0)).unwrap();
        let hollow = revolve(OpKey(3), &ring, axis, 0.0, core::f64::consts::TAU)
            .unwrap()
            .0;
        assert_eq!(hollow.counts().shells, 2);
        same(&hollow);
    }
}
