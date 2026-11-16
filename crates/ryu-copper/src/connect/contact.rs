//! Point contacts: pairs of items on a layer that an island joins where
//! their copper is thin, a point, an edge or a sliver within the bounds.
//! Fabricated copper and KiCad both join them; agents are told of them.
//!
//! Two bodies of round pens only, with no bound, are judged exactly: each
//! pen narrowed by 2 nm, they still touch only if a disc of radius 1 nm
//! lies in a solid of each. Otherwise the two items' copper (a plain
//! item's flattened parts, a resolved one's polygons) is clipped to the
//! place they meet and intersected; the contact is thin if that shrinks to
//! nothing by both bounds and a nanometre. So it over-reports by the
//! bounds, never misses a point contact.

use std::collections::{BTreeMap, BTreeSet};
use std::hash::Hash;

use rayon::prelude::*;
use ryu_board::geom::Rect;

use super::{Copper, Island, Node, overlap};
use crate::measure::{self, Body, Solid, Verdict};
use crate::poly::{self, FillRule, FlattenOptions, Op, Rings};
use crate::tree::grown;

/// Two items on a layer joined where their copper is thin.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PointContact<K> {
    pub layer: usize,
    /// The lesser key first.
    pub a: K,
    pub b: K,
    /// Where: their boxes' overlap.
    pub near: Rect,
}

/// A body of round pens only, each narrowed by 2 nm (pens under 2 nm
/// wide dropped); `None` when any solid is a region.
fn narrowed(b: &Body) -> Option<Body> {
    let mut solids = Vec::new();
    for s in &b.solids {
        match s {
            Solid::Thick { core, w } => {
                if *w >= 2 {
                    solids.push(Solid::Thick {
                        core: *core,
                        w: w - 2,
                    });
                }
            }
            Solid::Region(_) => return None,
        }
    }
    Some(Body::new(solids, 0))
}

impl<K: Ord + Hash + Clone + Send + Sync> Copper<K> {
    /// The point contacts within an island, by layer and items.
    pub fn point_contacts(&self, island: &Island<K>) -> Vec<PointContact<K>> {
        let mut pairs: BTreeSet<(K, K)> = BTreeSet::new();
        for (k, i) in &island.pieces {
            let layer = self.entries[k].layer;
            for n in self.graph.neighbours(&Node::Piece(k.clone(), *i)) {
                if let Node::Piece(m, _) = n
                    && m != *k
                    && self.entries.get(&m).is_some_and(|e| e.layer == layer)
                {
                    pairs.insert(if *k < m {
                        (k.clone(), m)
                    } else {
                        (m, k.clone())
                    });
                }
            }
        }
        let fo = FlattenOptions::new(self.opts.bound());
        let items: BTreeSet<&K> = pairs.iter().flat_map(|(a, b)| [a, b]).collect();
        let rings: BTreeMap<&K, Rings> = items
            .into_par_iter()
            .map(|k| (k, self.item_rings(k, &fo)))
            .collect();
        let mut out: Vec<PointContact<K>> = pairs
            .par_iter()
            .filter_map(|(a, b)| self.thin(a, b, &rings, &fo))
            .collect();
        out.sort_by(|x, y| (x.layer, &x.a, &x.b).cmp(&(y.layer, &y.a, &y.b)));
        out
    }

    /// An item's copper as rings: a resolved item's polygons, a plain
    /// item's parts merged.
    fn item_rings(&self, key: &K, fo: &FlattenOptions) -> Rings {
        let e = &self.entries[key];
        match &e.resolved {
            Some(g) => {
                let mut r = Rings::default();
                for p in g {
                    let x = p.rings();
                    r.bound = r.bound.max(x.bound);
                    r.rings.extend(x.rings);
                }
                r
            }
            None => poly::merge_layer(&poly::flatten_shape(&e.item.shape, fo), &[]).rings(),
        }
    }

    fn thin(
        &self,
        a: &K,
        b: &K,
        rings: &BTreeMap<&K, Rings>,
        fo: &FlattenOptions,
    ) -> Option<PointContact<K>> {
        let layer = self.entries[a].layer;
        let (ba, bb) = (self.bodies.body(a)?, self.bodies.body(b)?);
        let near = overlap(ba.bounds(), bb.bounds(), 0);
        let found = Some(PointContact {
            layer,
            a: a.clone(),
            b: b.clone(),
            near,
        });
        let plain = !self.is_resolved(a) && !self.is_resolved(b);
        if plain
            && ba.bound == 0
            && bb.bound == 0
            && let (Some(na), Some(nb)) = (narrowed(ba), narrowed(bb))
        {
            return if measure::touches(&na, &nb) == Verdict::Yes {
                None
            } else {
                found
            };
        }
        let (ra, rb) = (&rings[a], &rings[b]);
        let window = grown(&near, ra.bound + rb.bound + 2);
        let both = poly::boolean(
            &poly::clip(ra, window),
            FillRule::NonZero,
            &poly::clip(rb, window),
            FillRule::NonZero,
            Op::Intersection,
        );
        let wide = !both.is_empty() && !poly::shrink(&both, ra.bound + rb.bound + 1, fo).is_empty();
        if wide { None } else { found }
    }
}

#[cfg(test)]
mod tests {
    use crate::connect::fixtures::*;
    use crate::connect::{Copper, Node};
    use crate::measure::BodyOptions;

    fn contacts(l: &crate::layout::Layout<&'static str>) -> Vec<(&'static str, &'static str)> {
        let c = Copper::new(l, BodyOptions::default());
        let mut out = Vec::new();
        for i in c.islands() {
            for pc in c.point_contacts(&i) {
                out.push((pc.a, pc.b));
            }
        }
        out
    }

    #[test]
    fn points_and_edges_are_thin_overlaps_are_not() {
        // Two round pads touching at (300 µm, 0).
        let mut l = board(1);
        l.items.insert("a", pad(0, p(0, 0), 300_000, None));
        l.items.insert("b", pad(0, p(600_000, 0), 300_000, None));
        assert_eq!(contacts(&l), [("a", "b")]);
        // Two traces side by side, touching along an edge.
        let mut l = board(1);
        l.items
            .insert("a", trace(0, p(0, 0), p(1_000_000, 0), 100_000, None));
        l.items.insert(
            "b",
            trace(0, p(0, 100_000), p(1_000_000, 100_000), 100_000, None),
        );
        assert_eq!(contacts(&l), [("a", "b")]);
        // A trace into a pad's centre, and two traces crossing: overlaps.
        let mut l = board(1);
        l.items.insert("a", pad(0, p(0, 0), 300_000, None));
        l.items
            .insert("b", trace(0, p(0, 0), p(1_000_000, 0), 100_000, None));
        l.items.insert(
            "c",
            trace(0, p(500_000, -500_000), p(500_000, 500_000), 100_000, None),
        );
        assert!(contacts(&l).is_empty());
        let c = Copper::new(&l, BodyOptions::default());
        assert_eq!(c.islands().len(), 1);
        assert_eq!(c.contacts(&Node::Piece("b", 0)).len(), 2);
    }
}
