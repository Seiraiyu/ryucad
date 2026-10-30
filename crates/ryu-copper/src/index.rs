//! What is near what, per layer: a packed R-tree of every solid's box,
//! bulk-loaded in Hilbert order, and beside it the changes since: inserted
//! items in packed trees of doubling sizes (one per bit of their count, as
//! a binary counter carries), removed ones dropped as they are met. The
//! layer is packed again when the changes pass an eighth of it.
//!
//! A removed item's slot is held until its layer is packed again (until
//! then the trees may still hold its entries), then numbered again for the
//! next insert, so the slots grow with the items, not with the edits.
//!
//! Every answer is sorted by key, so the trees' shapes (which depend on
//! the order of edits) never show; one thread and many give the same
//! answers.

use std::collections::HashMap;
use std::hash::Hash;

use rayon::prelude::*;
use ryu_board::geom::{Nm, Point, Rect};
use ryu_board::shape::Shape;

use crate::layout::Layout;
use crate::measure::{self, Body, BodyOptions, Distance, MAX_REACH, Verdict};
use crate::tree::{Packed, grown};

/// Changes a layer takes before it is packed again, at least.
const MIN_CHANGES: usize = 64;

/// One layer's trees.
#[derive(Clone, Debug, Default)]
struct Tree {
    base: Packed,
    /// Inserted since: `added[k]` holds about `2^k` items' entries.
    added: Vec<Option<Packed>>,
    /// Entries in `added`.
    extra: usize,
    /// Entries of removed items still in the trees.
    dead: usize,
    /// The largest bound of any body entered since the layer was last
    /// packed, or live when it was: queries reach that much further.
    bound: Nm,
    /// Slots removed from this layer whose entries the trees may still
    /// hold: free once the layer is packed again.
    held: Vec<u32>,
}

impl Tree {
    fn query(&self, q: &Rect, out: &mut Vec<u32>) {
        self.base.query(q, out);
        for p in self.added.iter().flatten() {
            p.query(q, out);
        }
    }

    fn add(&mut self, entries: Vec<(Rect, u32)>) {
        self.extra += entries.len();
        let mut carry = entries;
        for k in 0.. {
            if k == self.added.len() {
                self.added.push(None);
            }
            match self.added[k].take() {
                None => {
                    self.added[k] = Some(Packed::new(carry));
                    return;
                }
                Some(p) => carry.extend(p.entries),
            }
        }
    }

    fn crowded(&self) -> bool {
        let limit = (self.base.len() / 8).max(MIN_CHANGES);
        self.extra > limit || self.dead > limit
    }
}

struct Slot<K> {
    key: K,
    layer: usize,
    body: Body,
}

/// Two items within a distance of each other on a layer.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Pair<K> {
    /// The lesser key first.
    pub a: K,
    pub b: K,
    /// [`Verdict::Yes`], or [`Verdict::Uncertain`] where a bounded body
    /// can't settle it.
    pub verdict: Verdict,
}

/// An item near another.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Near<K> {
    pub key: K,
    pub verdict: Verdict,
}

/// The spatial index of a layout's items.
pub struct Index<K> {
    opts: BodyOptions,
    slots: Vec<Option<Slot<K>>>,
    /// Slots no tree names, numbered again by inserts.
    free: Vec<u32>,
    keys: HashMap<K, u32>,
    trees: Vec<Tree>,
}

impl<K: Ord + Hash + Clone + Send + Sync> Index<K> {
    /// An empty index of `layers` layers.
    pub fn new(layers: usize, opts: BodyOptions) -> Index<K> {
        Index {
            opts,
            slots: Vec::new(),
            free: Vec::new(),
            keys: HashMap::new(),
            trees: vec![Tree::default(); layers],
        }
    }

    /// The index of a layout's items, their bodies made in parallel and
    /// each layer packed in parallel.
    pub fn of_layout(layout: &Layout<K>, opts: BodyOptions) -> Index<K> {
        let bodies: Vec<(K, usize, Body)> = layout
            .items
            .par_iter()
            .map(|(k, it)| (k.clone(), it.layer, Body::of_shape(&it.shape, &opts)))
            .collect();
        Index::from_bodies(layout.layers.len(), bodies, opts)
    }

    /// The index of bodies already made (a later key replaces an earlier
    /// one), each layer packed in parallel.
    pub fn from_bodies(
        layers: usize,
        bodies: Vec<(K, usize, Body)>,
        opts: BodyOptions,
    ) -> Index<K> {
        let mut index = Index::new(layers, opts);
        for (key, layer, body) in bodies {
            if let Some(old) = index.keys.insert(key.clone(), index.slots.len() as u32) {
                // No tree holds it yet.
                index.slots[old as usize] = None;
                index.free.push(old);
            }
            index.slots.push(Some(Slot { key, layer, body }));
        }
        let packed: Vec<Tree> = (0..layers)
            .into_par_iter()
            .map(|l| index.packed(l))
            .collect();
        index.trees = packed;
        index
    }

    pub fn options(&self) -> &BodyOptions {
        &self.opts
    }

    /// The number of items.
    pub fn len(&self) -> usize {
        self.keys.len()
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    fn live(&self, slot: u32) -> Option<&Slot<K>> {
        self.slots[slot as usize].as_ref()
    }

    /// A layer's live entries packed into one tree, with their largest
    /// bound.
    fn packed(&self, layer: usize) -> Tree {
        let mut entries = Vec::new();
        let mut bound = 0;
        for (s, slot) in self.slots.iter().enumerate() {
            if let Some(slot) = slot
                && slot.layer == layer
            {
                entries.extend(slot.body.entries().into_iter().map(|(r, _)| (r, s as u32)));
                bound = bound.max(slot.body.bound);
            }
        }
        Tree {
            base: Packed::new(entries),
            bound,
            ..Tree::default()
        }
    }

    /// An item's body.
    pub fn body(&self, key: &K) -> Option<&Body> {
        self.keys
            .get(key)
            .and_then(|&s| self.live(s))
            .map(|s| &s.body)
    }

    /// An item's layer.
    pub fn layer(&self, key: &K) -> Option<usize> {
        self.keys
            .get(key)
            .and_then(|&s| self.live(s))
            .map(|s| s.layer)
    }

    /// Adds an item's shape (replacing any item of that key).
    pub fn insert_shape(&mut self, key: K, layer: usize, shape: &Shape) {
        let body = Body::of_shape(shape, &self.opts);
        self.insert(key, layer, body);
    }

    /// Adds an item's body (replacing any item of that key).
    pub fn insert(&mut self, key: K, layer: usize, body: Body) {
        self.remove(&key);
        let slot = match self.free.pop() {
            Some(s) => s,
            None => {
                self.slots.push(None);
                (self.slots.len() - 1) as u32
            }
        };
        let entries: Vec<(Rect, u32)> =
            body.entries().into_iter().map(|(r, _)| (r, slot)).collect();
        let tree = &mut self.trees[layer];
        tree.bound = tree.bound.max(body.bound);
        tree.add(entries);
        self.keys.insert(key.clone(), slot);
        self.slots[slot as usize] = Some(Slot { key, layer, body });
        self.tidy(layer);
    }

    /// Removes an item, giving back its body.
    pub fn remove(&mut self, key: &K) -> Option<Body> {
        let slot = self.keys.remove(key)?;
        let s = self.slots[slot as usize].take().expect("a live slot");
        let tree = &mut self.trees[s.layer];
        tree.dead += s.body.entries().len();
        tree.held.push(slot);
        self.tidy(s.layer);
        Some(s.body)
    }

    /// Packs a layer again once its changes pass an eighth of it.
    fn tidy(&mut self, layer: usize) {
        if self.trees[layer].crowded() {
            let held = std::mem::take(&mut self.trees[layer].held);
            self.trees[layer] = self.packed(layer);
            // No tree names these slots now.
            self.free.extend(held);
        }
    }

    fn slots_meeting(&self, layer: usize, q: &Rect) -> Vec<u32> {
        let mut out = Vec::new();
        self.trees[layer].query(q, &mut out);
        out.sort_unstable();
        out.dedup();
        out.retain(|&s| self.live(s).is_some_and(|x| x.layer == layer));
        out
    }

    fn keys_of(&self, slots: impl IntoIterator<Item = u32>) -> Vec<K> {
        let mut out: Vec<K> = slots
            .into_iter()
            .map(|s| self.live(s).expect("live").key.clone())
            .collect();
        out.sort();
        out
    }

    /// Items with a solid whose box meets `q`.
    pub fn in_box(&self, layer: usize, q: &Rect) -> Vec<K> {
        self.keys_of(self.slots_meeting(layer, q))
    }

    /// Candidate slots within `d` of a body's boxes, and as far again as
    /// its bound and the layer's largest, where a bounded answer may still
    /// be [`Verdict::Uncertain`].
    fn candidates(&self, layer: usize, body: &Body, d: Nm) -> Vec<u32> {
        let mut out = Vec::new();
        let reach = d
            .saturating_add(body.bound)
            .saturating_add(self.trees[layer].bound)
            .min(2 * MAX_REACH);
        for (r, _) in body.entries() {
            self.trees[layer].query(&grown(&r, reach), &mut out);
        }
        out.sort_unstable();
        out.dedup();
        out.retain(|&s| self.live(s).is_some_and(|x| x.layer == layer));
        out
    }

    /// Items on `layer` within `d` of a body, by key.
    pub fn near_body(&self, layer: usize, body: &Body, d: Nm) -> Vec<Near<K>> {
        let d = d.clamp(0, MAX_REACH);
        let mut out: Vec<Near<K>> = self
            .candidates(layer, body, d)
            .into_iter()
            .filter_map(|s| {
                let x = self.live(s)?;
                let verdict = measure::within(body, &x.body, d);
                (verdict != Verdict::No).then(|| Near {
                    key: x.key.clone(),
                    verdict,
                })
            })
            .collect();
        out.sort_by(|a, b| a.key.cmp(&b.key));
        out
    }

    /// Other items on an item's layer within `d` of it, by key.
    pub fn near(&self, key: &K, d: Nm) -> Vec<Near<K>> {
        let Some(&slot) = self.keys.get(key) else {
            return Vec::new();
        };
        let x = self.live(slot).expect("live");
        let mut out = self.near_body(x.layer, &x.body, d);
        out.retain(|n| &n.key != key);
        out
    }

    /// Every pair of items on `layer` within `d` of each other, in
    /// parallel by item, sorted by keys.
    pub fn pairs(&self, layer: usize, d: Nm) -> Vec<Pair<K>> {
        let d = d.clamp(0, MAX_REACH);
        let mine: Vec<u32> = self.slots_meeting(
            layer,
            &Rect {
                min: Point::new(-(1 << 41), -(1 << 41)),
                max: Point::new(1 << 41, 1 << 41),
            },
        );
        let mut out: Vec<Pair<K>> = mine
            .par_iter()
            .flat_map_iter(|&s| {
                let x = self.live(s).expect("live");
                self.candidates(layer, &x.body, d)
                    .into_iter()
                    .filter(move |&t| t > s)
                    .filter_map(move |t| {
                        let y = self.live(t)?;
                        let verdict = measure::within(&x.body, &y.body, d);
                        if verdict == Verdict::No {
                            return None;
                        }
                        let (a, b) = if x.key < y.key {
                            (x.key.clone(), y.key.clone())
                        } else {
                            (y.key.clone(), x.key.clone())
                        };
                        Some(Pair { a, b, verdict })
                    })
            })
            .collect();
        out.sort_by(|p, q| (&p.a, &p.b).cmp(&(&q.a, &q.b)));
        out
    }

    /// The item on `layer` nearest a point, by its distance rounded to the
    /// nanometre, ties by key; `None` on an empty layer.
    pub fn nearest(&self, layer: usize, p: Point) -> Option<(K, Distance)> {
        let at = Body::point(p);
        let mut reach: Nm = 1_000;
        loop {
            let found = self.slots_meeting(layer, &grown(&Rect::of_point(p), reach));
            if !found.is_empty() {
                // Anything nearer than the best found lies in the box of
                // that radius about the point.
                let best = self.best_of(&found, &at)?;
                let r = best.1.nm + best.1.bound + 1;
                let all = self.slots_meeting(layer, &grown(&Rect::of_point(p), r));
                return self.best_of(&all, &at);
            }
            if reach >= MAX_REACH {
                return None;
            }
            reach = (reach * 4).min(MAX_REACH);
        }
    }

    fn best_of(&self, slots: &[u32], at: &Body) -> Option<(K, Distance)> {
        slots
            .iter()
            .filter_map(|&s| {
                let x = self.live(s)?;
                Some((x.key.clone(), measure::distance(&x.body, at)?))
            })
            .min_by(|a, b| (a.1.nm, &a.0).cmp(&(b.1.nm, &b.0)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(x: i64, y: i64) -> Point {
        Point::new(x, y)
    }

    fn dot(x: i64, y: i64) -> Body {
        Body::stroke(p(x, y), p(x, y), 100)
    }

    #[test]
    fn near_pairs_and_nearest() {
        let mut ix: Index<&str> = Index::new(2, BodyOptions::default());
        ix.insert("a", 0, dot(0, 0));
        ix.insert("b", 0, dot(250, 0));
        ix.insert("c", 0, dot(1000, 0));
        ix.insert("d", 1, dot(0, 0));
        // a and b are 150 apart, b and c 650.
        let near: Vec<_> = ix.near(&"a", 150).into_iter().map(|n| n.key).collect();
        assert_eq!(near, ["b"]);
        assert!(ix.near(&"a", 149).is_empty());
        let pairs: Vec<_> = ix.pairs(0, 650).into_iter().map(|q| (q.a, q.b)).collect();
        assert_eq!(pairs, [("a", "b"), ("b", "c")]);
        let (k, d) = ix.nearest(0, p(700, 10)).unwrap();
        assert_eq!((k, d.nm), ("c", 250));
        assert_eq!(ix.nearest(1, p(5_000_000, 0)).unwrap().0, "d");
        assert_eq!(ix.in_box(0, &Rect::new(p(-10, -10), p(10, 10))), ["a"]);
        // Removing and moving.
        assert!(ix.remove(&"b").is_some());
        assert!(ix.pairs(0, 650).is_empty());
        ix.insert("c", 0, dot(300, 0));
        let pairs: Vec<_> = ix.pairs(0, 200).into_iter().map(|q| (q.a, q.b)).collect();
        assert_eq!(pairs, [("a", "c")]);
        assert_eq!(ix.len(), 3);
        assert_eq!(ix.layer(&"d"), Some(1));
        // A shape goes in as its body.
        let disc =
            ryu_board::geom::Region::solid(ryu_board::shape::circle(p(5_000, 0), 50).unwrap());
        ix.insert_shape(
            "e",
            0,
            &Shape::regions(ryu_board::model::Polarity::Positive, [disc]),
        );
        assert_eq!(ix.nearest(0, p(5_000, 200)).unwrap().0, "e");
        assert_eq!(ix.body(&"e").unwrap().solids.len(), 1);
        assert!(ix.remove(&"zz").is_none());
    }

    fn bounded(x: i64, y: i64, bound: Nm) -> Body {
        let mut b = Body::point(p(x, y));
        b.bound = bound;
        b
    }

    /// The verdicts `within` gives, as the index should answer them.
    fn judged(pairs: &[(&'static str, &Body)], of: &Body, d: Nm) -> Vec<Near<&'static str>> {
        pairs
            .iter()
            .filter_map(|&(key, b)| {
                let verdict = measure::within(of, b, d);
                (verdict != Verdict::No).then_some(Near { key, verdict })
            })
            .collect()
    }

    #[test]
    fn bounded_bodies_past_the_distance_stay_uncertain() {
        // Solids 100 apart: within 95 is open while the bounds reach 5.
        for (bx, by) in [(0, 5), (5, 0), (2, 3)] {
            let x = bounded(0, 0, bx);
            let y = bounded(100, 0, by);
            assert_eq!(measure::within(&x, &y, 95), Verdict::Uncertain);
            assert_eq!(measure::within(&x, &y, 94), Verdict::No);
            // A far item the bounds never reach.
            let z = bounded(10_000, 0, 0);
            let mut ix: Index<&str> = Index::new(1, BodyOptions::default());
            ix.insert("x", 0, x.clone());
            ix.insert("y", 0, y.clone());
            ix.insert("z", 0, z.clone());
            for d in [94, 95, 99, 100] {
                let others = [("y", &y), ("z", &z)];
                assert_eq!(ix.near(&"x", d), judged(&others, &x, d), "near at {d}");
                // The same body asked about from outside the index.
                let all = [("x", &x), ("y", &y), ("z", &z)];
                assert_eq!(
                    ix.near_body(0, &x, d),
                    judged(&all, &x, d),
                    "near_body at {d}"
                );
                let pairs: Vec<_> = ix.pairs(0, d);
                let want: Vec<_> = judged(&[("y", &y)], &x, d)
                    .into_iter()
                    .map(|n| Pair {
                        a: "x",
                        b: n.key,
                        verdict: n.verdict,
                    })
                    .collect();
                assert_eq!(pairs, want, "pairs at {d}");
            }
            assert_eq!(ix.near(&"x", 95)[0].verdict, Verdict::Uncertain);
            // Packed from bodies, the layer keeps the largest bound.
            let packed = Index::from_bodies(
                1,
                vec![("x", 0, x.clone()), ("y", 0, y.clone()), ("z", 0, z)],
                BodyOptions::default(),
            );
            assert_eq!(packed.pairs(0, 95), ix.pairs(0, 95));
        }
    }

    #[test]
    fn slots_grow_with_the_items_not_the_edits() {
        let mut ix: Index<u32> = Index::new(2, BodyOptions::default());
        for k in 0..100 {
            ix.insert(k, (k % 2) as usize, dot(1_000 * k as i64, 0));
        }
        for round in 0..200i64 {
            for k in 0..100u32 {
                // Moves, replacements and removals, on both layers.
                match (round + i64::from(k)) % 3 {
                    0 => ix.insert(k, (k % 2) as usize, dot(1_000 * k as i64, round)),
                    1 => {
                        ix.remove(&k);
                        ix.insert(k, ((k + 1) % 2) as usize, dot(1_000 * k as i64, -round));
                    }
                    _ => {
                        ix.remove(&k);
                    }
                }
            }
            for k in 0..100u32 {
                if ix.layer(&k).is_none() {
                    ix.insert(k, (k % 2) as usize, dot(1_000 * k as i64, 0));
                }
            }
        }
        assert_eq!(ix.len(), 100);
        // Live items, plus what each layer holds until it is packed again.
        assert!(
            ix.slots.len() <= 100 + 2 * (MIN_CHANGES + 1),
            "{} slots",
            ix.slots.len()
        );
        // A reused slot never answers for its old item's boxes.
        for k in 0..100u32 {
            let layer = ix.layer(&k).unwrap();
            let r = ix.body(&k).unwrap().bounds().unwrap();
            assert_eq!(ix.in_box(layer, &r), [k]);
            assert!(ix.in_box(1 - layer, &r).is_empty());
        }
    }
}
