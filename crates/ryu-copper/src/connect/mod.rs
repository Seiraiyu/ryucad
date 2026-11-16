//! Connectivity: the copper's islands, sets of pieces electrically joined
//! on copper layers and across them through plated holes, with negatives
//! and holes cutting what they cross.
//!
//! - **Pieces.** An item's positive solids fall into groups that touch
//!   (`Body::groups`): a trace is one, a thermal's spokes four. Each is a
//!   piece, a node of the contact graph, `Node::Piece(key, i)`. A plated
//!   hole is a node too, `Node::Hole(key)`, joined to what touches its
//!   wall on every copper layer of its span but those a back-drill took
//!   the plating from.
//! - **Contacts.** Two plain pieces on a layer are joined when they
//!   touch, exactly (`measure::touches`); a contact the bounds leave open
//!   is kept apart and reported ([`UncertainContact`]).
//! - **Soft and hard holes.** Every hole cuts the copper layers it spans.
//!   A hole is soft on a layer when a plain item has a solid holding it
//!   strictly inside (`hole::holds`, a via's pad) and every piece touching
//!   it reaches outside it (`hole::reaches_out`). Then it changes no
//!   contact: a piece touching it and reaching outside crosses its
//!   outline, which lies inside the holding solid, so whatever the hole
//!   cuts off a piece still touches that solid, and two pieces that met
//!   only inside the hole both touch it too. Any other hole is hard.
//! - **Resolved items.** An item resolves when it touches a negative part
//!   (drawn before or after it), a hard hole, or a resolved item in two or
//!   more pieces; a hole is hard when a touch with it is uncertain, a
//!   piece touching it doesn't reach outside it, or no plain item holds
//!   it. Resolving an item merges its own parts, the negative parts drawn
//!   after it that touch it, and the holes touching it (`merge_layer`);
//!   its pieces are the groups of the polygons left (`groups`). Which
//!   items resolve is the least fixed point of these rules, the same in
//!   whatever order it is found.
//! - **Contacts with resolved pieces.** A plain piece meets a resolved
//!   item (in one piece, else the plain one would resolve) where it meets
//!   the item's body: it touches no cutter, so that point is copper still.
//!   Two resolved pieces are judged on their polygons (`group_touch`). A
//!   plated wall joins a resolved piece when the item's body touches the
//!   hole and the piece lies within its bound and a nanometre of it.
//! - **Determinism.** Every decision is an exact predicate on integer
//!   nanometres; every answer is sorted by key; the graph's labels and
//!   search order never show.

mod edit;
#[cfg(test)]
pub(crate) mod fixtures;
mod graph;
mod hole;
mod resolve;

use std::collections::{BTreeMap, BTreeSet};
use std::hash::Hash;

use rayon::prelude::*;
use ryu_board::geom::{Nm, Point, Rect};
use ryu_board::model::Polarity;
use ryu_board::shape::{Part, Shape};

use crate::index::Index;
use crate::layout::{Hole, Item, Layer, Layout};
use crate::measure::{self, Body, BodyOptions, Verdict};
use crate::poly::{self, FlattenOptions, Polygons};
use crate::tree::grown;
use graph::Graph;
pub use resolve::{group_touch, groups};

/// A node of the contact graph.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Node<K> {
    /// An item's piece, by its index among the item's pieces.
    Piece(K, u32),
    /// A plated hole's wall.
    Hole(K),
}

/// An island's ID: its least piece.
pub type IslandId<K> = (K, u32);

/// Copper electrically joined.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Island<K> {
    pub id: IslandId<K>,
    /// Its pieces, sorted.
    pub pieces: Vec<(K, u32)>,
    /// The items its pieces belong to, sorted.
    pub items: Vec<K>,
    /// The layers they lie on, in stack order.
    pub layers: Vec<usize>,
    /// The plated holes joining it, sorted.
    pub holes: Vec<K>,
    /// The nets its items claim, sorted.
    pub claims: Vec<String>,
}

/// Two pieces the bounds can't say touch or not: kept apart.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct UncertainContact<K> {
    pub layer: usize,
    /// The lesser node first.
    pub a: Node<K>,
    pub b: Node<K>,
    /// How far the two may lie from their true copper, together.
    pub bound: Nm,
    /// Where: their boxes, each grown by the bound, overlapping.
    pub near: Rect,
}

/// One item as connectivity keeps it.
#[derive(Clone, Debug)]
struct Entry {
    layer: usize,
    item: Item,
    /// Its pieces' bodies when its solids fall into two groups or more;
    /// empty when one (the piece is the whole body) or none.
    split: Vec<Body>,
    /// Its negative parts' indices, as keyed in `Copper::negs`.
    negs: Vec<u32>,
    /// Its pieces' polygons when it is resolved.
    resolved: Option<Vec<Polygons>>,
}

/// An entry, its positive body and its negative parts' bodies.
type Made = (Entry, Body, Vec<(u32, Body)>);

impl Entry {
    fn make(item: &Item, opts: &BodyOptions) -> Made {
        let body = Body::of_shape(&item.shape, opts);
        let groups = body.groups();
        let split = if groups.len() > 1 {
            groups.iter().map(|g| body.subset(g)).collect()
        } else {
            Vec::new()
        };
        let negs: Vec<(u32, Body)> = item
            .shape
            .parts
            .iter()
            .enumerate()
            .filter(|(_, p)| p.polarity == Polarity::Negative)
            .map(|(k, p)| {
                let shape = Shape {
                    parts: vec![Part {
                        polarity: Polarity::Positive,
                        piece: p.piece.clone(),
                    }],
                    ..Shape::default()
                };
                (k as u32, Body::of_shape(&shape, opts))
            })
            .filter(|(_, b)| !b.is_empty())
            .collect();
        let entry = Entry {
            layer: item.layer,
            item: item.clone(),
            split,
            negs: negs.iter().map(|n| n.0).collect(),
            resolved: None,
        };
        (entry, body, negs)
    }
}

/// What finding a layer's least fixed point gave.
struct Modes<K> {
    resolved: BTreeMap<K, Vec<Polygons>>,
    hard: BTreeSet<K>,
}

impl<K> Default for Modes<K> {
    fn default() -> Modes<K> {
        Modes {
            resolved: BTreeMap::new(),
            hard: BTreeSet::new(),
        }
    }
}

/// Contacts judged.
struct Found<K> {
    edges: Vec<(Node<K>, Node<K>)>,
    uncertain: Vec<UncertainContact<K>>,
}

impl<K> Default for Found<K> {
    fn default() -> Found<K> {
        Found {
            edges: Vec::new(),
            uncertain: Vec::new(),
        }
    }
}

type Doubts<K> = BTreeMap<(Node<K>, Node<K>), UncertainContact<K>>;

/// Whether a hole spans a layer.
fn on(h: &Hole, layer: usize) -> bool {
    h.span.0 <= layer && layer <= h.span.1
}

/// Whether a hole's wall joins nothing on a layer: unplated, or its
/// plating drilled out there.
fn stripped(h: &Hole, layer: usize) -> bool {
    !h.plated || h.backdrilled.iter().any(|&(a, b)| a <= layer && layer <= b)
}

/// Two boxes, each grown by `by`, overlapping (a point between them when
/// they don't).
fn overlap(a: Option<Rect>, b: Option<Rect>, by: Nm) -> Rect {
    match (a, b) {
        (Some(a), Some(b)) => {
            let (a, b) = (grown(&a, by), grown(&b, by));
            Rect::new(
                Point::new(a.min.x.max(b.min.x), a.min.y.max(b.min.y)),
                Point::new(a.max.x.min(b.max.x), a.max.y.min(b.max.y)),
            )
        }
        (Some(r), None) | (None, Some(r)) => r,
        (None, None) => Rect::of_point(Point::ORIGIN),
    }
}

/// Records a verdict on two nodes: an edge, a doubt, or nothing.
fn judge<K: Ord>(
    layer: usize,
    v: Verdict,
    a: Node<K>,
    b: Node<K>,
    bound: Nm,
    near: Rect,
    out: &mut Found<K>,
) {
    let (a, b) = if a < b { (a, b) } else { (b, a) };
    match v {
        Verdict::Yes => out.edges.push((a, b)),
        Verdict::Uncertain => out.uncertain.push(UncertainContact {
            layer,
            a,
            b,
            bound,
            near,
        }),
        Verdict::No => {}
    }
}

/// A layout's copper connectivity, kept as items and holes come and go.
pub struct Copper<K> {
    layers: Vec<Layer>,
    opts: BodyOptions,
    entries: BTreeMap<K, Entry>,
    /// Items' positive bodies, per layer.
    bodies: Index<K>,
    /// Negative parts' bodies, by item and part, per layer.
    negs: Index<(K, u32)>,
    holes: BTreeMap<K, Hole>,
    /// Holes' bodies, all on one plane.
    drills: Index<K>,
    /// Hard holes, per layer.
    hard: Vec<BTreeSet<K>>,
    graph: Graph<Node<K>>,
    uncertain: Doubts<K>,
}

impl<K: Ord + Hash + Clone + Send + Sync> Copper<K> {
    /// The connectivity of a layout's copper layers: bodies made, each
    /// layer's fixed point found, and contacts judged, all in parallel.
    /// Items on other layers are left out; the layout's points are the
    /// netlist compare's.
    pub fn new(layout: &Layout<K>, opts: BodyOptions) -> Copper<K> {
        let n = layout.layers.len();
        let copper: Vec<bool> = layout.layers.iter().map(|l| l.copper).collect();
        let made: Vec<(K, Made)> = layout
            .items
            .par_iter()
            .filter(|(_, it)| copper.get(it.layer).copied().unwrap_or(false))
            .map(|(k, it)| (k.clone(), Entry::make(it, &opts)))
            .collect();
        let mut entries = BTreeMap::new();
        let mut bodies = Vec::with_capacity(made.len());
        let mut negs = Vec::new();
        for (k, (e, b, ns)) in made {
            bodies.push((k.clone(), e.layer, b));
            negs.extend(ns.into_iter().map(|(p, nb)| ((k.clone(), p), e.layer, nb)));
            entries.insert(k, e);
        }
        let drills = layout
            .holes
            .iter()
            .map(|(k, h)| (k.clone(), 0, h.shape.body()))
            .collect();
        let mut c = Copper {
            layers: layout.layers.clone(),
            opts,
            entries,
            bodies: Index::from_bodies(n, bodies, opts),
            negs: Index::from_bodies(n, negs, opts),
            holes: layout.holes.clone(),
            drills: Index::from_bodies(1, drills, opts),
            hard: vec![BTreeSet::new(); n],
            graph: Graph::new(),
            uncertain: BTreeMap::new(),
        };
        let modes: Vec<(usize, Modes<K>)> = (0..n)
            .into_par_iter()
            .filter(|&l| copper[l])
            .map(|l| {
                let items: BTreeSet<K> = c
                    .entries
                    .iter()
                    .filter(|(_, e)| e.layer == l)
                    .map(|(k, _)| k.clone())
                    .collect();
                let holes: BTreeSet<K> = c
                    .holes
                    .iter()
                    .filter(|(_, h)| on(h, l))
                    .map(|(k, _)| k.clone())
                    .collect();
                (l, c.settle_modes(l, items, holes, false))
            })
            .collect();
        for (l, m) in modes {
            c.apply(l, m);
        }
        let keys: Vec<K> = c.entries.keys().cloned().collect();
        for k in &keys {
            for node in c.nodes_of(k) {
                c.graph.add_node(node);
            }
        }
        for h in c.holes.keys() {
            c.graph.add_node(Node::Hole(h.clone()));
        }
        let found: Vec<Found<K>> = keys
            .par_iter()
            .map(|k| c.edges_of(k, &|_: &K| true))
            .collect();
        for f in found {
            c.absorb(f);
        }
        c
    }

    pub fn layers(&self) -> &[Layer] {
        &self.layers
    }

    pub fn options(&self) -> &BodyOptions {
        &self.opts
    }

    /// An item as given, if it lies on a copper layer.
    pub fn item(&self, key: &K) -> Option<&Item> {
        self.entries.get(key).map(|e| &e.item)
    }

    pub fn hole(&self, key: &K) -> Option<&Hole> {
        self.holes.get(key)
    }

    /// How many pieces an item has.
    pub fn pieces(&self, key: &K) -> u32 {
        match self.entries.get(key) {
            None => 0,
            Some(e) => match &e.resolved {
                Some(g) => g.len() as u32,
                None => self.own(key).len() as u32,
            },
        }
    }

    /// Whether an item's copper went through the boolean.
    pub fn is_resolved(&self, key: &K) -> bool {
        self.entries.get(key).is_some_and(|e| e.resolved.is_some())
    }

    /// The hard holes on a layer, sorted.
    pub fn hard_holes(&self, layer: usize) -> Vec<K> {
        self.hard
            .get(layer)
            .map(|s| s.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// The island a node lies on; `None` for a hole joining no copper.
    pub fn island_of(&self, node: &Node<K>) -> Option<IslandId<K>> {
        match self.graph.first(node)? {
            Node::Piece(k, i) => Some((k.clone(), *i)),
            Node::Hole(_) => None,
        }
    }

    /// The island a node lies on, whole.
    pub fn island(&self, node: &Node<K>) -> Option<Island<K>> {
        self.island_from(self.graph.component(node)?)
    }

    /// Every island, by ID.
    pub fn islands(&self) -> Vec<Island<K>> {
        let mut out: Vec<Island<K>> = self
            .graph
            .components()
            .filter_map(|c| self.island_from(c))
            .collect();
        out.sort_by(|a, b| a.id.cmp(&b.id));
        out
    }

    /// The nodes joined to a node, sorted.
    pub fn contacts(&self, node: &Node<K>) -> Vec<Node<K>> {
        self.graph.neighbours(node)
    }

    /// Every contact kept apart, by its nodes.
    pub fn uncertain(&self) -> Vec<UncertainContact<K>> {
        self.uncertain.values().cloned().collect()
    }

    fn island_from(&self, members: &BTreeSet<Node<K>>) -> Option<Island<K>> {
        let mut pieces = Vec::new();
        let mut holes = Vec::new();
        for n in members {
            match n {
                Node::Piece(k, i) => pieces.push((k.clone(), *i)),
                Node::Hole(h) => holes.push(h.clone()),
            }
        }
        let id = pieces.first()?.clone();
        let mut items: Vec<K> = pieces.iter().map(|p| p.0.clone()).collect();
        items.dedup();
        let mut layers: Vec<usize> = items.iter().map(|k| self.entries[k].layer).collect();
        layers.sort_unstable();
        layers.dedup();
        let mut claims: Vec<String> = items
            .iter()
            .filter_map(|k| self.entries[k].item.net.clone())
            .collect();
        claims.sort();
        claims.dedup();
        Some(Island {
            id,
            pieces,
            items,
            layers,
            holes,
            claims,
        })
    }

    /// An item's own pieces: its positive solids in groups that touch.
    fn own(&self, key: &K) -> Vec<(u32, &Body)> {
        let Some(e) = self.entries.get(key) else {
            return Vec::new();
        };
        if !e.split.is_empty() {
            return e
                .split
                .iter()
                .enumerate()
                .map(|(i, b)| (i as u32, b))
                .collect();
        }
        match self.bodies.body(key) {
            Some(b) if !b.is_empty() => vec![(0, b)],
            _ => Vec::new(),
        }
    }

    fn nodes_of(&self, key: &K) -> Vec<Node<K>> {
        (0..self.pieces(key))
            .map(|i| Node::Piece(key.clone(), i))
            .collect()
    }

    /// An item's resolved groups, found in `m` or kept.
    fn groups_of<'a>(&'a self, key: &K, m: &'a Modes<K>) -> Option<&'a Vec<Polygons>> {
        m.resolved
            .get(key)
            .or_else(|| self.entries.get(key).and_then(|e| e.resolved.as_ref()))
    }

    /// The holes spanning `layer` that touch an item's positive body, any
    /// verdict but `No`, sorted.
    fn holes_touching(&self, layer: usize, key: &K) -> Vec<K> {
        let Some(b) = self.bodies.body(key) else {
            return Vec::new();
        };
        self.drills
            .near_body(0, b, 0)
            .into_iter()
            .map(|n| n.key)
            .filter(|h| on(&self.holes[h], layer))
            .collect()
    }

    fn apply(&mut self, layer: usize, m: Modes<K>) {
        for (k, g) in m.resolved {
            if let Some(e) = self.entries.get_mut(&k) {
                e.resolved = Some(g);
            }
        }
        self.hard[layer].extend(m.hard);
    }

    fn absorb(&mut self, f: Found<K>) {
        for (a, b) in f.edges {
            self.graph.add_edge(&a, &b);
        }
        for u in f.uncertain {
            self.uncertain.insert((u.a.clone(), u.b.clone()), u);
        }
    }

    /// Which items on `layer` resolve and which holes are hard, from
    /// `items` and `holes` and the state outside them: the least fixed
    /// point, grown from the items that touch a negative part (and, when
    /// `deep`, those the kept state around them forces) and from holes
    /// hard as they lie. Each round's booleans run in parallel.
    fn settle_modes(
        &self,
        layer: usize,
        items: BTreeSet<K>,
        holes: BTreeSet<K>,
        deep: bool,
    ) -> Modes<K> {
        let mut m = Modes::default();
        let mut q: Vec<K> = items
            .par_iter()
            .filter(|k| self.must_resolve(layer, k, &m, deep))
            .cloned()
            .collect();
        let mut hq: Vec<K> = holes.into_iter().collect();
        loop {
            while let Some(h) = hq.pop() {
                if m.hard.contains(&h)
                    || self.hard[layer].contains(&h)
                    || !self.holes.contains_key(&h)
                {
                    continue;
                }
                if self.hole_is_hard(layer, &h, &m) {
                    let hb = self.drills.body(&h).expect("a hole's body");
                    q.extend(
                        self.bodies
                            .near_body(layer, hb, 0)
                            .into_iter()
                            .map(|n| n.key),
                    );
                    m.hard.insert(h);
                }
            }
            q.sort();
            q.dedup();
            q.retain(|k| self.groups_of(k, &m).is_none());
            if q.is_empty() {
                break;
            }
            let round: Vec<(K, Vec<Polygons>)> = q
                .par_iter()
                .map(|k| (k.clone(), self.resolve_item(layer, k)))
                .collect();
            q.clear();
            for (k, g) in round {
                // Its holes may have lost their holder; if it fell in two,
                // what touches it resolves too.
                hq.extend(self.holes_touching(layer, &k));
                if g.len() >= 2 {
                    q.extend(self.bodies.near(&k, 0).into_iter().map(|n| n.key));
                }
                m.resolved.insert(k, g);
            }
        }
        m
    }

    /// Whether an item must resolve before anything forces it: it touches
    /// a negative part, or (`deep`) a hard hole or an item in two or more
    /// resolved pieces.
    fn must_resolve(&self, layer: usize, key: &K, m: &Modes<K>, deep: bool) -> bool {
        let Some(body) = self.bodies.body(key) else {
            return false;
        };
        if body.is_empty() || self.groups_of(key, m).is_some() {
            return false;
        }
        if !self.negs.near_body(layer, body, 0).is_empty() {
            return true;
        }
        if !deep {
            return false;
        }
        self.holes_touching(layer, key)
            .iter()
            .any(|h| m.hard.contains(h) || self.hard[layer].contains(h))
            || self
                .bodies
                .near(key, 0)
                .iter()
                .any(|n| self.groups_of(&n.key, m).is_some_and(|g| g.len() >= 2))
    }

    /// Whether a hole is hard on a layer: it touches something, and a
    /// touch is uncertain, a piece touching it doesn't reach outside it,
    /// or no plain item holds it.
    fn hole_is_hard(&self, layer: usize, h: &K, m: &Modes<K>) -> bool {
        let hb = self.drills.body(h).expect("a hole's body");
        let shape = &self.holes[h].shape;
        let touching = self.bodies.near_body(layer, hb, 0);
        let mut held = false;
        for n in &touching {
            if n.verdict != Verdict::Yes {
                return true;
            }
            let plain = self.groups_of(&n.key, m).is_none();
            for (_, piece) in self.own(&n.key) {
                match measure::touches(piece, hb) {
                    Verdict::No => continue,
                    Verdict::Uncertain => return true,
                    Verdict::Yes => {}
                }
                if !piece.solids.iter().any(|s| hole::reaches_out(s, shape)) {
                    return true;
                }
                held |= plain && piece.solids.iter().any(|s| hole::holds(s, shape));
            }
        }
        !touching.is_empty() && !held
    }

    /// An item's copper through the boolean: its own parts, then the
    /// negative parts drawn after it (greater keys) that touch it, less
    /// the holes touching it; in groups.
    fn resolve_item(&self, layer: usize, key: &K) -> Vec<Polygons> {
        let e = &self.entries[key];
        let fo = FlattenOptions::new(self.opts.bound());
        let mut parts = poly::flatten_shape(&e.item.shape, &fo);
        let body = self.bodies.body(key).expect("an item's body");
        for n in self.negs.near_body(layer, body, 0) {
            let (owner, part) = &n.key;
            if owner > key {
                let shape = &self.entries[owner].item.shape;
                let one = Shape {
                    parts: vec![shape.parts[*part as usize].clone()],
                    ..Shape::default()
                };
                parts.extend(poly::flatten_shape(&one, &fo));
            }
        }
        let holes: Vec<_> = self
            .holes_touching(layer, key)
            .iter()
            .map(|h| poly::flatten_hole(&self.holes[h].shape, &fo))
            .collect();
        groups(&poly::merge_layer(&parts, &holes))
    }

    /// An item's contacts: with every item touching it (once per pair
    /// inside `in_scope`, from the lesser key), and with the plated walls
    /// of the holes touching it.
    fn edges_of(&self, key: &K, in_scope: &(dyn Fn(&K) -> bool + Sync)) -> Found<K> {
        let mut out = Found::default();
        let Some(e) = self.entries.get(key) else {
            return out;
        };
        let layer = e.layer;
        for n in self.bodies.near(key, 0) {
            if in_scope(&n.key) && n.key < *key {
                continue;
            }
            self.pair(layer, key, &n.key, n.verdict, &mut out);
        }
        for h in self.holes_touching(layer, key) {
            self.wall(layer, &h, key, &mut out);
        }
        out
    }

    /// Two items' pieces' contacts; `v` is their bodies' touch.
    fn pair(&self, layer: usize, x: &K, y: &K, v: Verdict, out: &mut Found<K>) {
        match (&self.entries[x].resolved, &self.entries[y].resolved) {
            (None, None) => {
                let (px, py) = (self.own(x), self.own(y));
                let whole = px.len() == 1 && py.len() == 1;
                for &(i, a) in &px {
                    for &(j, b) in &py {
                        let v = if whole { v } else { measure::touches(a, b) };
                        let bound = a.bound + b.bound;
                        let near = overlap(a.bounds(), b.bounds(), bound);
                        let (na, nb) = (Node::Piece(x.clone(), i), Node::Piece(y.clone(), j));
                        judge(layer, v, na, nb, bound, near, out);
                    }
                }
            }
            (None, Some(g)) => self.plain_resolved(layer, x, y, g, out),
            (Some(g), None) => self.plain_resolved(layer, y, x, g, out),
            (Some(gx), Some(gy)) => {
                let fo = FlattenOptions::new(self.opts.bound());
                for (i, a) in gx.iter().enumerate() {
                    for (j, b) in gy.iter().enumerate() {
                        let bound = a.bound + b.bound;
                        let near = overlap(a.bounds(), b.bounds(), bound);
                        let na = Node::Piece(x.clone(), i as u32);
                        let nb = Node::Piece(y.clone(), j as u32);
                        judge(layer, group_touch(a, b, &fo), na, nb, bound, near, out);
                    }
                }
            }
        }
    }

    /// A plain item `x` against a resolved item `y`: `x` touches no cutter
    /// (else it would resolve), so where it meets `y`'s body is `y`'s
    /// copper still, and `y` is in one piece (else `x` would resolve).
    /// Were `y` in more, every contact would be kept apart, never guessed.
    fn plain_resolved(&self, layer: usize, x: &K, y: &K, gy: &[Polygons], out: &mut Found<K>) {
        let yb = self.bodies.body(y).expect("an item's body");
        for (i, a) in self.own(x) {
            for (j, g) in gy.iter().enumerate() {
                let v = if gy.len() == 1 {
                    measure::touches(a, yb)
                } else {
                    Verdict::Uncertain
                };
                let bound = a.bound + g.bound;
                let near = overlap(a.bounds(), g.bounds(), bound);
                let (na, nb) = (Node::Piece(x.clone(), i), Node::Piece(y.clone(), j as u32));
                judge(layer, v, na, nb, bound, near, out);
            }
        }
    }

    /// A plated wall's contacts with an item's pieces on a layer.
    fn wall(&self, layer: usize, h: &K, key: &K, out: &mut Found<K>) {
        let hole = &self.holes[h];
        if stripped(hole, layer) {
            return;
        }
        let hb = self.drills.body(h).expect("a hole's body");
        let node = Node::Hole(h.clone());
        match &self.entries[key].resolved {
            None => {
                for (i, p) in self.own(key) {
                    if measure::touches(p, hb) == Verdict::Yes {
                        out.edges.push((Node::Piece(key.clone(), i), node.clone()));
                    }
                }
            }
            Some(gs) => {
                let body = self.bodies.body(key).expect("an item's body");
                if measure::touches(body, hb) != Verdict::Yes {
                    return;
                }
                for (i, g) in gs.iter().enumerate() {
                    if measure::within(&g.body(), hb, g.bound + 1) == Verdict::Yes {
                        out.edges
                            .push((Node::Piece(key.clone(), i as u32), node.clone()));
                    }
                }
            }
        }
    }

    /// A hole's wall's contacts with everything touching it on a layer.
    fn walls_of(&self, layer: usize, h: &K) -> Found<K> {
        let mut out = Found::default();
        if let Some(hb) = self.drills.body(h) {
            for n in self.bodies.near_body(layer, hb, 0) {
                self.wall(layer, h, &n.key, &mut out);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::*;
    use super::*;

    fn ids(c: &Copper<&'static str>) -> Vec<Vec<(&'static str, u32)>> {
        c.islands().into_iter().map(|i| i.pieces).collect()
    }

    #[test]
    fn plated_holes_join_layers_unless_stripped() {
        let mut l = board(2);
        l.items.insert("a.pad", pad(0, p(0, 0), 300_000, None));
        l.items
            .insert("a.trace", trace(0, p(0, 0), p(2_000_000, 0), 100_000, None));
        l.items.insert("b.pad", pad(1, p(0, 0), 300_000, None));
        l.holes.insert("v", via(p(0, 0), 200_000, (0, 1)));
        let c = Copper::new(&l, BodyOptions::default());
        assert_eq!(
            ids(&c),
            vec![vec![("a.pad", 0), ("a.trace", 0), ("b.pad", 0)]]
        );
        let all = c.islands();
        assert_eq!(all[0].holes, ["v"]);
        assert_eq!(all[0].layers, [0, 1]);
        assert_eq!(all[0].items, ["a.pad", "a.trace", "b.pad"]);
        // The via cuts nothing that matters: its pad holds it.
        assert!(c.hard_holes(0).is_empty() && !c.is_resolved(&"a.trace"));
        // Unplated, or its plating drilled out of the bottom: two islands.
        let strips: [fn(&mut Hole); 2] = [|h| h.plated = false, |h| h.backdrilled = vec![(1, 1)]];
        for strip in strips {
            let mut l = l.clone();
            strip(l.holes.get_mut("v").unwrap());
            let c = Copper::new(&l, BodyOptions::default());
            assert_eq!(
                ids(&c),
                vec![vec![("a.pad", 0), ("a.trace", 0)], vec![("b.pad", 0)]]
            );
        }
    }

    #[test]
    fn a_hole_through_a_trace_opens_it() {
        let mut l = board(1);
        l.items.insert(
            "t",
            trace(0, p(-2_000_000, 0), p(2_000_000, 0), 100_000, Some("N")),
        );
        l.holes.insert(
            "h",
            Hole {
                plated: false,
                ..via(p(0, 0), 300_000, (0, 0))
            },
        );
        let c = Copper::new(&l, BodyOptions::default());
        // Nothing holds the hole: it is hard, and the trace resolved, in two.
        assert_eq!(c.hard_holes(0), ["h"]);
        assert!(c.is_resolved(&"t"));
        assert_eq!(c.pieces(&"t"), 2);
        assert_eq!(ids(&c), vec![vec![("t", 0)], vec![("t", 1)]]);
        // Plated, its wall joins the halves again.
        l.holes.get_mut("h").unwrap().plated = true;
        let c = Copper::new(&l, BodyOptions::default());
        assert_eq!(ids(&c), vec![vec![("t", 0), ("t", 1)]]);
        assert_eq!(c.islands()[0].holes, ["h"]);
        assert_eq!(c.islands()[0].claims, ["N"]);
    }

    #[test]
    fn negatives_cut_what_was_drawn_before() {
        // A plane, then a negative strip across it: two halves.
        let mut l = board(1);
        let plane = block(0, 0, 0, 10_000_000, 10_000_000, Polarity::Positive);
        let strip = block(
            0,
            4_000_000,
            -1_000_000,
            5_000_000,
            11_000_000,
            Polarity::Negative,
        );
        l.items.insert("a", plane.clone());
        l.items.insert("b", strip.clone());
        let c = Copper::new(&l, BodyOptions::default());
        assert_eq!(ids(&c), vec![vec![("a", 0)], vec![("a", 1)]]);
        assert_eq!(c.pieces(&"b"), 0);
        // Drawn first, the strip clears nothing: one plane, resolved all
        // the same (it touches a negative part).
        let mut first = board(1);
        first.items.insert("a", strip);
        first.items.insert("b", plane);
        let c = Copper::new(&first, BodyOptions::default());
        assert_eq!(ids(&c), vec![vec![("b", 0)]]);
        assert!(c.is_resolved(&"b"));
        // A trace drawn over the cut bridges the halves.
        l.items.insert(
            "c",
            trace(
                0,
                p(3_000_000, 5_000_000),
                p(6_000_000, 5_000_000),
                200_000,
                None,
            ),
        );
        let c = Copper::new(&l, BodyOptions::default());
        assert_eq!(ids(&c), vec![vec![("a", 0), ("a", 1), ("c", 0)]]);
        assert!(c.uncertain().is_empty());
    }

    #[test]
    fn contacts_the_bounds_cannot_settle_stay_apart() {
        // Two squares sharing an edge, each cut by a negative disc drawn
        // after it: both resolved, their polygons meeting along the edge
        // within their bounds.
        let mut l = board(1);
        l.items.insert(
            "a",
            block(0, 0, 0, 1_000_000, 1_000_000, Polarity::Positive),
        );
        l.items.insert(
            "b",
            block(0, 1_000_000, 0, 2_000_000, 1_000_000, Polarity::Positive),
        );
        l.items.insert("x1", blot(0, p(500_000, 500_000), 100_000));
        l.items
            .insert("x2", blot(0, p(1_500_000, 500_000), 100_000));
        let c = Copper::new(&l, BodyOptions::default());
        assert_eq!(ids(&c), vec![vec![("a", 0)], vec![("b", 0)]]);
        let u = c.uncertain();
        assert_eq!(u.len(), 1);
        assert_eq!(u[0].layer, 0);
        assert_eq!(
            (&u[0].a, &u[0].b),
            (&Node::Piece("a", 0), &Node::Piece("b", 0))
        );
        assert!(u[0].bound > 0 && u[0].bound <= 2 * 52, "{}", u[0].bound);
        assert!(u[0].near.contains(p(1_000_000, 500_000)));
        // Without the negatives the squares are plain, and touch exactly.
        l.items.retain(|k, _| !k.starts_with('x'));
        let c = Copper::new(&l, BodyOptions::default());
        assert_eq!(ids(&c), vec![vec![("a", 0), ("b", 0)]]);
        assert!(c.uncertain().is_empty());
    }

    #[test]
    fn an_item_in_parts_is_a_piece_per_part() {
        // Two squares apart in one item, a trace touching the right one.
        let mut l = board(1);
        l.items.insert(
            "p",
            Item {
                layer: 0,
                shape: Shape::regions(
                    Polarity::Positive,
                    [square(0, 0, 1_000, 1_000), square(5_000, 0, 6_000, 1_000)],
                ),
                net: None,
            },
        );
        l.items
            .insert("t", trace(0, p(6_000, 500), p(9_000, 500), 200, None));
        let c = Copper::new(&l, BodyOptions::default());
        assert_eq!(c.pieces(&"p"), 2);
        assert_eq!(ids(&c), vec![vec![("p", 0)], vec![("p", 1), ("t", 0)]]);
        assert_eq!(c.island_of(&Node::Piece("t", 0)), Some(("p", 1)));
        assert_eq!(c.contacts(&Node::Piece("t", 0)), vec![Node::Piece("p", 1)]);
    }
}
