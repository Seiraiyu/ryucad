//! Clearance: pieces of different islands closer than a distance on a
//! layer, each with its exact distance. Candidates are pairs of items
//! within the distance (the items' positive bodies hold their pieces);
//! each pair of their pieces on different islands is judged on the
//! pieces' bodies, exact for plain ones and within their bound for
//! resolved ones. Pieces of one item are never a gap.

use std::collections::{BTreeMap, BTreeSet};
use std::hash::Hash;

use rayon::prelude::*;
use ryu_board::geom::Nm;

use super::{Copper, IslandId, Node};
use crate::measure::{self, Body, Distance, Verdict};

/// Two pieces of different islands closer than asked.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Gap<K> {
    pub layer: usize,
    /// The lesser piece first.
    pub a: (K, u32),
    pub b: (K, u32),
    /// Their islands, `a`'s first.
    pub islands: (IslandId<K>, IslandId<K>),
    pub distance: Distance,
    /// `Yes`, or `Uncertain` where a bound can't say whether the gap is
    /// under the distance.
    pub verdict: Verdict,
}

impl<K: Ord + Hash + Clone + Send + Sync> Copper<K> {
    /// Every pair of pieces of different islands on `layer` closer than
    /// `below`, closest first, then by pieces. Empty for a layer that isn't
    /// copper or a distance that isn't positive.
    pub fn clearance(&self, layer: usize, below: Nm) -> Vec<Gap<K>> {
        if below <= 0 || !self.layers.get(layer).is_some_and(|l| l.copper) {
            return Vec::new();
        }
        let pairs = self.bodies.pairs(layer, below);
        self.gaps_of(layer, below, pairs)
    }

    /// [`Copper::clearance`]'s gaps with an end in `keys` (ryueda 7.E).
    pub fn clearance_of(&self, layer: usize, below: Nm, keys: &BTreeSet<K>) -> Vec<Gap<K>> {
        if below <= 0 || !self.layers.get(layer).is_some_and(|l| l.copper) {
            return Vec::new();
        }
        let pairs = self.bodies.pairs_with(layer, below, keys);
        self.gaps_of(layer, below, pairs)
    }

    fn gaps_of(&self, layer: usize, below: Nm, pairs: Vec<crate::index::Pair<K>>) -> Vec<Gap<K>> {
        let resolved: BTreeSet<&K> = pairs
            .iter()
            .flat_map(|p| [&p.a, &p.b])
            .filter(|k| self.is_resolved(k))
            .collect();
        let made: BTreeMap<&K, Vec<(u32, Body)>> = resolved
            .into_par_iter()
            .map(|k| (k, self.piece_bodies(k)))
            .collect();
        let pieces = |k: &K| -> Vec<(u32, &Body)> {
            match made.get(k) {
                Some(v) => v.iter().map(|(i, b)| (*i, b)).collect(),
                None => self.own(k),
            }
        };
        let mut out: Vec<Gap<K>> = pairs
            .par_iter()
            .flat_map_iter(|p| {
                let mut found = Vec::new();
                for (i, a) in pieces(&p.a) {
                    for (j, b) in pieces(&p.b) {
                        let na = Node::Piece(p.a.clone(), i);
                        let nb = Node::Piece(p.b.clone(), j);
                        let (Some(ia), Some(ib)) = (self.island_of(&na), self.island_of(&nb))
                        else {
                            continue;
                        };
                        if ia == ib {
                            continue;
                        }
                        let verdict = measure::closer(a, b, below);
                        if verdict == Verdict::No {
                            continue;
                        }
                        let Some(distance) = measure::distance(a, b) else {
                            continue;
                        };
                        found.push(Gap {
                            layer,
                            a: (p.a.clone(), i),
                            b: (p.b.clone(), j),
                            islands: (ia, ib),
                            distance,
                            verdict,
                        });
                    }
                }
                found
            })
            .collect();
        out.sort_by(|x, y| (x.distance.nm, &x.a, &x.b).cmp(&(y.distance.nm, &y.a, &y.b)));
        out
    }
}

#[cfg(test)]
mod tests {
    use crate::connect::Copper;
    use crate::connect::fixtures::*;
    use crate::measure::BodyOptions;

    #[test]
    fn gaps_between_islands_and_not_within_one() {
        let mut l = board(1);
        l.items.insert("a", pad(0, p(0, 0), 300_000, Some("A")));
        l.items
            .insert("b", pad(0, p(700_000, 0), 300_000, Some("B")));
        // On a's island: its trace passes b closer than a does, but a
        // trace and its pad are never a gap.
        l.items
            .insert("t", trace(0, p(0, 0), p(0, 2_000_000), 100_000, Some("A")));
        let c = Copper::new(&l, BodyOptions::default());
        let g = c.clearance(0, 150_000);
        assert_eq!(g.len(), 1, "{g:?}");
        assert_eq!((g[0].a, g[0].b), (("a", 0), ("b", 0)));
        assert_eq!((g[0].distance.nm, g[0].distance.exact), (100_000, true));
        assert_ne!(g[0].islands.0, g[0].islands.1);
        // Closer than, not at: a gap of exactly 100 µm isn't under 100 µm.
        assert!(c.clearance(0, 100_000).is_empty());
        assert!(c.clearance(0, 0).is_empty());
        assert!(c.clearance(5, 150_000).is_empty());
    }
}
