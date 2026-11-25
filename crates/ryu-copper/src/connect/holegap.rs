//! Hole clearance: copper closer than a distance to a hole's wall on a
//! layer of its span, where the hole doesn't join it (#946). A plated
//! hole's wall is a node of its island; copper of that island is never a
//! gap. An unplated hole joins nothing. Measured exactly on the pieces'
//! bodies (within their bound for resolved ones), as `clearance` is.

use std::hash::Hash;

use rayon::prelude::*;
use ryu_board::geom::Nm;

use super::{Copper, IslandId, Node};
use crate::measure::{self, Body, Distance, Verdict};

/// A piece of copper closer to a hole's wall than asked, on a layer
/// where the hole doesn't join it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct HoleGap<K> {
    pub layer: usize,
    pub hole: K,
    pub piece: (K, u32),
    /// The piece's island.
    pub island: IslandId<K>,
    /// The island the hole's plating joins, if it is plated and joins one.
    pub joined: Option<IslandId<K>>,
    /// From the piece (`a`) to the wall (`b`).
    pub distance: Distance,
    pub verdict: Verdict,
}

impl<K: Ord + Hash + Clone + Send + Sync> Copper<K> {
    /// Every piece on `layer` closer than `below` to the wall of a hole
    /// whose span holds the layer and whose plating doesn't join the
    /// piece's island; closest first, then by hole and piece. Empty for a
    /// layer that isn't copper or a distance that isn't positive.
    pub fn hole_clearance(&self, layer: usize, below: Nm) -> Vec<HoleGap<K>> {
        if below <= 0 || !self.layers.get(layer).is_some_and(|l| l.copper) {
            return Vec::new();
        }
        let holes: Vec<&K> = self
            .holes
            .iter()
            .filter(|(_, h)| h.span.0 <= layer && layer <= h.span.1)
            .map(|(k, _)| k)
            .collect();
        self.hole_gaps(layer, below, holes)
    }

    /// [`Copper::hole_clearance`]'s gaps whose hole or piece is in `keys`
    /// (ryueda 7.E): the walls of the holes in `keys`, and the walls near
    /// an item in `keys`, judged against that item alone.
    pub fn hole_clearance_of(
        &self,
        layer: usize,
        below: Nm,
        keys: &std::collections::BTreeSet<K>,
    ) -> Vec<HoleGap<K>> {
        if below <= 0 || !self.layers.get(layer).is_some_and(|l| l.copper) {
            return Vec::new();
        }
        let on = |k: &K| {
            self.holes
                .get(k)
                .is_some_and(|h| h.span.0 <= layer && layer <= h.span.1)
        };
        let mut holes: std::collections::BTreeSet<&K> = keys.iter().filter(|k| on(k)).collect();
        for k in keys {
            if self.bodies.layer(k) != Some(layer) {
                continue;
            }
            let Some(b) = self.bodies.body(k) else {
                continue;
            };
            for n in self.drills.near_body(0, b, below) {
                if let Some((hk, _)) = self.holes.get_key_value(&n.key)
                    && on(hk)
                {
                    holes.insert(hk);
                }
            }
        }
        let mut out = self.hole_gaps(layer, below, holes.into_iter().collect());
        out.retain(|g| keys.contains(&g.hole) || keys.contains(&g.piece.0));
        out
    }

    fn hole_gaps(&self, layer: usize, below: Nm, holes: Vec<&K>) -> Vec<HoleGap<K>> {
        let holes: Vec<(&K, &crate::layout::Hole)> =
            holes.into_iter().map(|k| (k, &self.holes[k])).collect();
        let mut out: Vec<HoleGap<K>> = holes
            .par_iter()
            .flat_map_iter(|(hk, h)| {
                let wall: Body = h.shape.body();
                // A back-drill takes the plating, and so the join, from the
                // layers it spans.
                let plated_here =
                    h.plated && !h.backdrilled.iter().any(|s| s.0 <= layer && layer <= s.1);
                let joined = plated_here
                    .then(|| self.island_of(&Node::Hole((*hk).clone())))
                    .flatten();
                let mut found = Vec::new();
                for near in self.bodies.near_body(layer, &wall, below) {
                    let made;
                    let pieces: Vec<(u32, &Body)> = if self.is_resolved(&near.key) {
                        made = self.piece_bodies(&near.key);
                        made.iter().map(|(i, b)| (*i, b)).collect()
                    } else {
                        self.own(&near.key)
                    };
                    for (i, body) in pieces {
                        let Some(island) = self.island_of(&Node::Piece(near.key.clone(), i)) else {
                            continue;
                        };
                        if joined.as_ref() == Some(&island) {
                            continue;
                        }
                        let verdict = measure::closer(body, &wall, below);
                        if verdict == Verdict::No {
                            continue;
                        }
                        let Some(distance) = measure::distance(body, &wall) else {
                            continue;
                        };
                        found.push(HoleGap {
                            layer,
                            hole: (*hk).clone(),
                            piece: (near.key.clone(), i),
                            island,
                            joined: joined.clone(),
                            distance,
                            verdict,
                        });
                    }
                }
                found
            })
            .collect();
        out.sort_by(|x, y| {
            (x.distance.nm, &x.hole, &x.piece).cmp(&(y.distance.nm, &y.hole, &y.piece))
        });
        out
    }
}

#[cfg(test)]
mod tests {
    use crate::connect::fixtures::*;
    use crate::connect::{Copper, Node};
    use crate::layout::{Hole, HoleShape};
    use crate::measure::BodyOptions;
    use ryu_board::model::Polarity;

    #[test]
    fn copper_near_a_wall_it_isnt_joined_to() {
        let mut l = board(2);
        // A via at the origin, 300 µm drill, with a pad on the top only.
        l.holes.insert("v", via(p(0, 0), 300_000, (0, 1)));
        l.items.insert("vp", pad(0, p(0, 0), 300_000, Some("A")));
        // On the bottom, another net's trace passes 100 µm from the wall:
        // its centre line at 150 + 100 + 50 µm.
        l.items.insert(
            "t",
            trace(
                1,
                p(-2_000_000, 300_000),
                p(2_000_000, 300_000),
                100_000,
                Some("B"),
            ),
        );
        // On the top, a trace of the via's own island touches its pad.
        l.items
            .insert("s", trace(0, p(0, 0), p(2_000_000, 0), 100_000, Some("A")));
        let c = Copper::new(&l, BodyOptions::default());
        let g = c.hole_clearance(1, 150_000);
        assert_eq!(g.len(), 1, "{g:?}");
        assert_eq!((g[0].hole, g[0].piece), ("v", ("t", 0)));
        assert_eq!((g[0].distance.nm, g[0].distance.exact), (100_000, true));
        assert_eq!(g[0].joined, c.island_of(&Node::Hole("v")));
        // Closer than, not at.
        assert!(c.hole_clearance(1, 100_000).is_empty());
        // The top: the pad and its trace are the hole's own island.
        assert!(c.hole_clearance(0, 150_000).is_empty());
        assert!(c.hole_clearance(7, 150_000).is_empty());
        assert!(c.hole_clearance(1, 0).is_empty());
    }

    #[test]
    fn spans_and_unplated_holes() {
        let mut l = board(4);
        // A blind via over layers 0-1: copper on layer 2 isn't near any hole.
        l.holes.insert("b", via(p(0, 0), 300_000, (0, 1)));
        l.items.insert(
            "t2",
            trace(
                2,
                p(-1_000_000, 200_000),
                p(1_000_000, 200_000),
                100_000,
                Some("B"),
            ),
        );
        l.items.insert(
            "t1",
            trace(
                1,
                p(-1_000_000, 250_000),
                p(1_000_000, 250_000),
                100_000,
                Some("B"),
            ),
        );
        // An unplated hole joins nothing: a pad round it is still a gap of 0.
        l.holes.insert(
            "n",
            Hole {
                shape: HoleShape::Round {
                    at: p(5_000_000, 0),
                    d: 1_000_000,
                },
                span: (0, 3),
                plated: false,
                backdrilled: vec![],
            },
        );
        l.items.insert(
            "np",
            trace(
                3,
                p(4_000_000, 600_000),
                p(6_000_000, 600_000),
                100_000,
                None,
            ),
        );
        let c = Copper::new(&l, BodyOptions::default());
        assert!(c.hole_clearance(2, 200_000).is_empty());
        let g1 = c.hole_clearance(1, 200_000);
        assert_eq!(g1.len(), 1);
        assert_eq!((g1[0].hole, g1[0].distance.nm), ("b", 50_000));
        let g3 = c.hole_clearance(3, 200_000);
        assert_eq!(g3.len(), 1);
        assert_eq!(
            (g3[0].hole, g3[0].joined, g3[0].distance.nm),
            ("n", None, 50_000)
        );
    }

    #[test]
    fn a_hole_inside_a_plane() {
        // An unplated hole strictly inside a plane: the plane reaches its
        // wall, a gap of 0. A plated one joins it: none.
        for (plated, gaps) in [(false, 1), (true, 0)] {
            let mut l = board(1);
            l.items.insert(
                "plane",
                block(
                    0,
                    -5_000_000,
                    -5_000_000,
                    5_000_000,
                    5_000_000,
                    Polarity::Positive,
                ),
            );
            l.holes.insert(
                "h",
                Hole {
                    shape: HoleShape::Round {
                        at: p(0, 0),
                        d: 1_000_000,
                    },
                    span: (0, 0),
                    plated,
                    backdrilled: vec![],
                },
            );
            let c = Copper::new(&l, BodyOptions::default());
            let g = c.hole_clearance(0, 200_000);
            assert_eq!(g.len(), gaps, "plated {plated}: {g:?}");
            if let Some(g) = g.first() {
                assert_eq!(g.distance.nm, 0);
            }
        }
    }

    #[test]
    fn a_back_drilled_span_joins_nothing() {
        // A via over three layers with its pad on the top, and a trace of
        // its net crossing its wall on the bottom. Plated there, the trace
        // is the via's island; back-drilled there, the hole cuts it in
        // two, each piece a gap of 0.
        for (back, gaps) in [(vec![], 0), (vec![(2, 2)], 2)] {
            let mut l = board(3);
            l.holes.insert(
                "v",
                Hole {
                    shape: HoleShape::Round {
                        at: p(0, 0),
                        d: 300_000,
                    },
                    span: (0, 2),
                    plated: true,
                    backdrilled: back.clone(),
                },
            );
            l.items.insert("vp", pad(0, p(0, 0), 300_000, Some("A")));
            l.items.insert(
                "t",
                trace(2, p(-1_000_000, 0), p(1_000_000, 0), 100_000, Some("A")),
            );
            let c = Copper::new(&l, BodyOptions::default());
            let g = c.hole_clearance(2, 200_000);
            assert_eq!(g.len(), gaps, "{back:?}: {g:?}");
        }
    }
}
