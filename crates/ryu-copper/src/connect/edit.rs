//! Edits: items and holes inserted and removed, connectivity kept.
//!
//! An edit gathers what it touches before and after (items; the items its
//! negative parts or hole may cut; holes) and the resolved cluster joined
//! to that: resolved items reached through touching resolved items and
//! hard holes. Their pieces leave the graph; they are reset to plain and
//! the cluster's hard holes to soft; the change is made; the least fixed
//! point is found again from the cluster, the edited item, what it cuts,
//! and the holes round them all; every item whose pieces changed gets new
//! nodes and its contacts judged again; and the components the removals
//! touched are settled. Everything else keeps its pieces, contacts and
//! modes: none of them depends on what the edit changed.

use std::collections::BTreeSet;
use std::hash::Hash;

use rayon::prelude::*;

use super::{Copper, Entry, Found, Node, on};
use crate::layout::{Hole, Item};

/// What an edit touches on a layer.
struct Touch<K> {
    /// Items touching the item's body or its negative parts, or the hole.
    items: BTreeSet<K>,
    /// Items its negative parts or the hole may cut.
    cut: BTreeSet<K>,
    /// Holes touching the item, or the hole itself.
    holes: BTreeSet<K>,
}

impl<K> Default for Touch<K> {
    fn default() -> Touch<K> {
        Touch {
            items: BTreeSet::new(),
            cut: BTreeSet::new(),
            holes: BTreeSet::new(),
        }
    }
}

/// A resolved cluster: its items and its hard holes.
type Cluster<K> = (BTreeSet<K>, BTreeSet<K>);

impl<K: Ord + Hash + Clone + Send + Sync> Copper<K> {
    /// Adds an item, or replaces the one of that key. An item on a layer
    /// that isn't copper is left out (and any old one of its key removed).
    pub fn insert_item(&mut self, key: K, item: Item) {
        let mut layers: Vec<usize> = self
            .entries
            .get(&key)
            .map(|e| e.layer)
            .into_iter()
            .collect();
        if self.is_copper(item.layer) {
            layers.push(item.layer);
        }
        layers.sort_unstable();
        layers.dedup();
        let k = key.clone();
        self.edit(&layers, Some(&key), None, move |c| {
            c.unset_item(&k);
            if c.is_copper(item.layer) {
                c.set_item(k, &item);
            }
        });
    }

    /// Removes an item, giving it back.
    pub fn remove_item(&mut self, key: &K) -> Option<Item> {
        let e = self.entries.get(key)?;
        let (layer, item) = (e.layer, e.item.clone());
        self.edit(&[layer], Some(key), None, |c| c.unset_item(key));
        Some(item)
    }

    /// Adds a hole, or replaces the one of that key.
    pub fn insert_hole(&mut self, key: K, hole: Hole) {
        let mut layers: Vec<usize> = self
            .holes
            .get(&key)
            .map(|h| self.copper_in(h))
            .unwrap_or_default();
        layers.extend(self.copper_in(&hole));
        layers.sort_unstable();
        layers.dedup();
        let k = key.clone();
        self.edit(&layers, None, Some(&key), move |c| {
            c.drills.insert(k.clone(), 0, hole.shape.body());
            c.holes.insert(k, hole);
        });
    }

    /// Removes a hole, giving it back.
    pub fn remove_hole(&mut self, key: &K) -> Option<Hole> {
        let hole = self.holes.get(key)?.clone();
        let layers = self.copper_in(&hole);
        self.edit(&layers, None, Some(key), |c| {
            c.drills.remove(key);
            c.holes.remove(key);
            for s in &mut c.hard {
                s.remove(key);
            }
        });
        Some(hole)
    }

    fn is_copper(&self, layer: usize) -> bool {
        self.layers.get(layer).is_some_and(|l| l.copper)
    }

    /// The copper layers a hole spans.
    fn copper_in(&self, h: &Hole) -> Vec<usize> {
        (h.span.0..=h.span.1)
            .filter(|&l| self.is_copper(l))
            .collect()
    }

    fn set_item(&mut self, key: K, item: &Item) {
        let (e, body, negs) = Entry::make(item, &self.opts);
        self.bodies.insert(key.clone(), e.layer, body);
        for (p, nb) in negs {
            self.negs.insert((key.clone(), p), e.layer, nb);
        }
        self.entries.insert(key, e);
    }

    fn unset_item(&mut self, key: &K) {
        if let Some(e) = self.entries.remove(key) {
            self.bodies.remove(key);
            for p in e.negs {
                self.negs.remove(&(key.clone(), p));
            }
        }
    }

    /// Removes nodes and the doubts about them, giving back the nodes they
    /// touched.
    fn forget(&mut self, gone: &[Node<K>]) -> Vec<Node<K>> {
        if gone.is_empty() {
            return Vec::new();
        }
        let set: BTreeSet<&Node<K>> = gone.iter().collect();
        self.uncertain
            .retain(|(a, b), _| !set.contains(a) && !set.contains(b));
        self.graph.remove_nodes(gone)
    }

    fn touch(&self, layer: usize, item: Option<&K>, hole: Option<&K>) -> Touch<K> {
        let mut t = Touch::default();
        if let Some(k) = item
            && let Some(e) = self.entries.get(k)
            && e.layer == layer
        {
            t.items
                .extend(self.bodies.near(k, 0).into_iter().map(|n| n.key));
            t.holes.extend(self.holes_touching(layer, k));
            for &p in &e.negs {
                if let Some(nb) = self.negs.body(&(k.clone(), p)) {
                    for n in self.bodies.near_body(layer, nb, 0) {
                        t.cut.insert(n.key.clone());
                        t.items.insert(n.key);
                    }
                }
            }
        }
        if let Some(h) = hole
            && let Some(x) = self.holes.get(h)
            && on(x, layer)
        {
            let hb = self.drills.body(h).expect("a hole's body");
            for n in self.bodies.near_body(layer, hb, 0) {
                t.cut.insert(n.key.clone());
                t.items.insert(n.key);
            }
            t.holes.insert(h.clone());
        }
        t
    }

    /// The resolved items and hard holes joined to `items` and `holes` on
    /// a layer through touching resolved items and hard holes.
    fn cluster(&self, layer: usize, items: &BTreeSet<K>, holes: &BTreeSet<K>) -> Cluster<K> {
        let mut r = BTreeSet::new();
        let mut hs = BTreeSet::new();
        let mut q: Vec<K> = items
            .iter()
            .filter(|k| self.is_resolved(k))
            .cloned()
            .collect();
        let mut hq: Vec<K> = holes
            .iter()
            .filter(|h| self.hard[layer].contains(*h))
            .cloned()
            .collect();
        loop {
            if let Some(h) = hq.pop() {
                if hs.insert(h.clone()) {
                    let hb = self.drills.body(&h).expect("a hole's body");
                    q.extend(
                        self.bodies
                            .near_body(layer, hb, 0)
                            .into_iter()
                            .map(|n| n.key)
                            .filter(|k| self.is_resolved(k)),
                    );
                }
                continue;
            }
            let Some(k) = q.pop() else {
                break;
            };
            if !r.insert(k.clone()) {
                continue;
            }
            q.extend(
                self.bodies
                    .near(&k, 0)
                    .into_iter()
                    .map(|n| n.key)
                    .filter(|x| self.is_resolved(x)),
            );
            hq.extend(
                self.holes_touching(layer, &k)
                    .into_iter()
                    .filter(|h| self.hard[layer].contains(h)),
            );
        }
        (r, hs)
    }

    /// An edit on `layers` of an item or a hole, made by `change`.
    fn edit(
        &mut self,
        layers: &[usize],
        item: Option<&K>,
        hole: Option<&K>,
        change: impl FnOnce(&mut Self),
    ) {
        // Before: what the edit touches, and the clusters joined to it.
        let before: Vec<Touch<K>> = layers.iter().map(|&l| self.touch(l, item, hole)).collect();
        let olds: Vec<Cluster<K>> = layers
            .iter()
            .zip(&before)
            .map(|(&l, t)| {
                let mut seeds = t.items.clone();
                if let Some(k) = item {
                    seeds.insert(k.clone());
                }
                self.cluster(l, &seeds, &t.holes)
            })
            .collect();
        let mut gone: Vec<Node<K>> = Vec::new();
        if let Some(k) = item {
            gone.extend(self.nodes_of(k));
        }
        if let Some(h) = hole
            && self.holes.contains_key(h)
        {
            gone.push(Node::Hole(h.clone()));
        }
        for (r, _) in &olds {
            for k in r {
                gone.extend(self.nodes_of(k));
            }
        }
        let mut seeds = self.forget(&gone);
        for (&l, (r, hs)) in layers.iter().zip(&olds) {
            for k in r {
                if let Some(e) = self.entries.get_mut(k) {
                    e.resolved = None;
                }
            }
            for h in hs {
                self.hard[l].remove(h);
            }
        }
        change(self);
        let mut fresh: Vec<Node<K>> = Vec::new();
        if let Some(h) = hole
            && self.holes.contains_key(h)
        {
            let node = Node::Hole(h.clone());
            self.graph.add_node(node.clone());
            fresh.push(node);
        }
        for (i, &l) in layers.iter().enumerate() {
            let after = self.touch(l, item, hole);
            let (r_old, h_old) = &olds[i];
            let here = item.filter(|k| self.entries.get(*k).is_some_and(|e| e.layer == l));
            let mut items: BTreeSet<K> = r_old.clone();
            items.extend(after.cut.iter().cloned());
            items.extend(here.cloned());
            let mut holes: BTreeSet<K> = h_old.clone();
            holes.extend(before[i].holes.iter().cloned());
            holes.extend(after.holes.iter().cloned());
            for k in r_old {
                holes.extend(self.holes_touching(l, k));
            }
            let m = self.settle_modes(l, items, holes, true);
            // Plain items resolving now lose their plain pieces.
            let newly: Vec<Node<K>> = m
                .resolved
                .keys()
                .filter(|k| !r_old.contains(*k) && Some(*k) != here)
                .flat_map(|k| self.nodes_of(k))
                .collect();
            seeds.extend(self.forget(&newly));
            let mut p: BTreeSet<K> = r_old.clone();
            p.extend(m.resolved.keys().cloned());
            p.extend(here.cloned());
            self.apply(l, m);
            for k in &p {
                for node in self.nodes_of(k) {
                    self.graph.add_node(node.clone());
                    fresh.push(node);
                }
            }
            let this: &Self = self;
            let found: Vec<Found<K>> = p
                .par_iter()
                .map(|k| this.edges_of(k, &|x: &K| p.contains(x)))
                .collect();
            for f in found {
                self.absorb(f);
            }
            if let Some(h) = hole {
                let f = self.walls_of(l, h);
                self.absorb(f);
            }
        }
        seeds.extend(fresh);
        self.graph.settle(&seeds);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connect::fixtures::*;
    use crate::layout::Layout;
    use crate::measure::BodyOptions;
    use ryu_board::model::Polarity;

    type L = Layout<&'static str>;
    type C = Copper<&'static str>;

    /// What an edit leaves is what building again gives.
    fn same(c: &C, l: &L) {
        let fresh = Copper::new(l, BodyOptions::default());
        assert_eq!(c.islands(), fresh.islands());
        assert_eq!(c.uncertain(), fresh.uncertain());
        for k in l.items.keys() {
            assert_eq!(c.is_resolved(k), fresh.is_resolved(k), "{k}");
            assert_eq!(c.pieces(k), fresh.pieces(k), "{k}");
        }
        for layer in 0..l.layers.len() {
            assert_eq!(
                c.hard_holes(layer),
                fresh.hard_holes(layer),
                "layer {layer}"
            );
        }
    }

    fn put(l: &mut L, c: &mut C, k: &'static str, it: Item) {
        l.items.insert(k, it.clone());
        c.insert_item(k, it);
        same(c, l);
    }

    fn take(l: &mut L, c: &mut C, k: &'static str) {
        l.items.remove(k);
        assert!(c.remove_item(&k).is_some());
        same(c, l);
    }

    fn drill(l: &mut L, c: &mut C, k: &'static str, h: Hole) {
        l.holes.insert(k, h.clone());
        c.insert_hole(k, h);
        same(c, l);
    }

    fn undrill(l: &mut L, c: &mut C, k: &'static str) {
        l.holes.remove(k);
        assert!(c.remove_hole(&k).is_some());
        same(c, l);
    }

    #[test]
    fn edits_match_a_rebuild() {
        let mut l = board(2);
        let mut c = Copper::new(&l, BodyOptions::default());
        put(&mut l, &mut c, "a.pad", pad(0, p(0, 0), 300_000, None));
        put(&mut l, &mut c, "b.pad", pad(1, p(0, 0), 300_000, None));
        drill(&mut l, &mut c, "v", via(p(0, 0), 200_000, (0, 1)));
        assert_eq!(c.islands().len(), 1);
        let t = |layer| trace(layer, p(0, 0), p(3_000_000, 0), 100_000, None);
        put(&mut l, &mut c, "t", t(0));
        // A negative strip across the trace, then gone.
        let strip = block(
            0,
            1_400_000,
            -500_000,
            1_600_000,
            500_000,
            Polarity::Negative,
        );
        put(&mut l, &mut c, "u", strip);
        assert!(c.is_resolved(&"t") && c.pieces(&"t") == 2);
        take(&mut l, &mut c, "u");
        assert!(!c.is_resolved(&"t"));
        // An unplated hole through the trace, then gone.
        let w = Hole {
            plated: false,
            ..via(p(2_000_000, 0), 300_000, (0, 1))
        };
        drill(&mut l, &mut c, "w", w);
        assert_eq!(c.pieces(&"t"), 2);
        undrill(&mut l, &mut c, "w");
        // A stub wholly inside the via's hole makes it hard, then gone.
        put(
            &mut l,
            &mut c,
            "s",
            trace(0, p(-20_000, 0), p(20_000, 0), 40_000, None),
        );
        assert_eq!(c.hard_holes(0), ["v"]);
        assert!(c.hard_holes(1).is_empty());
        take(&mut l, &mut c, "s");
        // The via replaced by a wider one; the pad that held it gone.
        drill(&mut l, &mut c, "v", via(p(0, 0), 250_000, (0, 1)));
        take(&mut l, &mut c, "a.pad");
        assert_eq!(c.hard_holes(0), ["v"]);
        // The trace moved to the bottom layer, and back.
        put(&mut l, &mut c, "t", t(1));
        put(&mut l, &mut c, "t", t(0));
        // Everything goes.
        take(&mut l, &mut c, "b.pad");
        take(&mut l, &mut c, "t");
        undrill(&mut l, &mut c, "v");
        assert!(c.islands().is_empty() && c.uncertain().is_empty());
    }
}
