//! Where a netlist point lands: on the outer copper layer of its side, or
//! through a plated hole's wall; nowhere when it lies on no copper or on
//! copper drilled away; uncertain when the bounds can't say, or two
//! islands hold it. Never guessed.

use std::collections::BTreeSet;
use std::hash::Hash;

use ryu_board::geom::Point;

use super::{Copper, IslandId, Node, on, stripped};
use crate::layout::PointSide;
use crate::measure::{self, Body, Verdict};

/// Where a point lands.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Placement<K> {
    On(IslandId<K>),
    /// On no copper, or on copper drilled away.
    Off,
    /// The bounds can't say which island, or whether any.
    Uncertain,
}

impl<K: Ord + Hash + Clone + Send + Sync> Copper<K> {
    /// A side's outer copper layer: the first copper layer in stack order
    /// for the top, the last for the bottom.
    pub fn outer(&self, side: PointSide) -> Option<usize> {
        let mut copper = (0..self.layers.len()).filter(|&l| self.layers[l].copper);
        match side {
            PointSide::Top => copper.next(),
            PointSide::Bottom => copper.next_back(),
        }
    }

    /// Where a point lands. With a side, on that side's outer copper
    /// layer: through the wall of a hole that holds it and spans the layer
    /// (nowhere if the hole's plating doesn't reach it), else on the
    /// pieces that hold it there. With no side, through the wall of a
    /// plated hole that holds it.
    pub fn place(&self, at: Point, side: Option<PointSide>) -> Placement<K> {
        let layer = match side {
            Some(s) => match self.outer(s) {
                Some(l) => Some(l),
                None => return Placement::Off,
            },
            None => None,
        };
        let pt = Body::point(at);
        let mut found: BTreeSet<IslandId<K>> = BTreeSet::new();
        let mut doubt = false;
        let mut drilled = false;
        for n in self.drills.near_body(0, &pt, 0) {
            let h = &self.holes[&n.key];
            if let Some(l) = layer
                && !on(h, l)
            {
                continue;
            }
            if n.verdict != Verdict::Yes {
                doubt = true;
                continue;
            }
            drilled = true;
            let joins = match layer {
                Some(l) => !stripped(h, l),
                None => h.plated,
            };
            if joins && let Some(id) = self.island_of(&Node::Hole(n.key.clone())) {
                found.insert(id);
            }
        }
        if !drilled && let Some(l) = layer {
            for n in self.bodies.near_body(l, &pt, 0) {
                let mut mark = |v: Verdict, node: Node<K>| match v {
                    Verdict::Yes => {
                        if let Some(id) = self.island_of(&node) {
                            found.insert(id);
                        }
                    }
                    Verdict::Uncertain => doubt = true,
                    Verdict::No => {}
                };
                match &self.entries[&n.key].resolved {
                    None => {
                        for (i, b) in self.own(&n.key) {
                            mark(measure::contains(b, at), Node::Piece(n.key.clone(), i));
                        }
                    }
                    Some(gs) => {
                        for (i, g) in gs.iter().enumerate() {
                            mark(g.contains(at), Node::Piece(n.key.clone(), i as u32));
                        }
                    }
                }
            }
        }
        if doubt || found.len() > 1 {
            return Placement::Uncertain;
        }
        found
            .into_iter()
            .next()
            .map_or(Placement::Off, Placement::On)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connect::fixtures::*;
    use crate::measure::BodyOptions;

    #[test]
    fn points_land_on_copper_or_through_holes() {
        let mut l = board(2);
        l.items.insert("a.pad", pad(0, p(0, 0), 300_000, None));
        l.items.insert("b.pad", pad(1, p(0, 0), 300_000, None));
        l.items
            .insert("c.pad", pad(1, p(2_000_000, 0), 300_000, None));
        l.holes.insert("v", via(p(0, 0), 200_000, (0, 1)));
        let c = Copper::new(&l, BodyOptions::default());
        let (top, bottom) = (Some(PointSide::Top), Some(PointSide::Bottom));
        assert_eq!(c.outer(PointSide::Top), Some(0));
        assert_eq!(c.outer(PointSide::Bottom), Some(1));
        assert_eq!(c.place(p(250_000, 0), top), Placement::On(("a.pad", 0)));
        // In the hole: through its wall, from either side or none.
        assert_eq!(c.place(p(0, 0), top), Placement::On(("a.pad", 0)));
        assert_eq!(c.place(p(50_000, 0), bottom), Placement::On(("a.pad", 0)));
        assert_eq!(c.place(p(0, 0), None), Placement::On(("a.pad", 0)));
        assert_eq!(
            c.place(p(2_000_000, 0), bottom),
            Placement::On(("c.pad", 0))
        );
        // The top has no copper there, and there is no hole.
        assert_eq!(c.place(p(2_000_000, 0), top), Placement::Off);
        assert_eq!(c.place(p(2_000_000, 0), None), Placement::Off);
        // Unplated, the hole holds no copper; its pad round it still does.
        l.holes.get_mut("v").unwrap().plated = false;
        let c = Copper::new(&l, BodyOptions::default());
        assert_eq!(c.place(p(0, 0), top), Placement::Off);
        assert_eq!(c.place(p(0, 0), None), Placement::Off);
        assert_eq!(c.place(p(250_000, 0), top), Placement::On(("a.pad", 0)));
    }
}
