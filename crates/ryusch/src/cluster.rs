//! Clusters: parts placed together, rigidly, as one. A part alone is a
//! cluster of one; an idiom or a `group` instruction makes larger ones.

use crate::geom::{Nm, Place, Point, Rect};
use crate::input::{Block, NetKind, PartKind};
use crate::layout::{IdiomKind, LabelKind};
use crate::shape::Shape;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cluster {
    /// Each member part and its place relative to the cluster's origin.
    pub members: Vec<(usize, Place)>,
    /// The idiom it draws, if it is one.
    pub idiom: Option<IdiomKind>,
    /// A region's margin (`region::FRAME`), or 0 for an ordinary
    /// cluster. A framed cluster is a whole region placed on its sheet:
    /// its room is its members' with a label on every pin whose net
    /// leaves it, grown by the margin, with the title band on top.
    pub frame: Nm,
}

impl Cluster {
    pub fn single(part: usize, place: Place) -> Cluster {
        Cluster {
            members: vec![(part, place)],
            idiom: None,
            frame: 0,
        }
    }

    /// Each member's place with the cluster's origin at `origin`.
    pub fn places(&self, origin: Point) -> Vec<(usize, Place)> {
        self.members
            .iter()
            .map(|(p, rel)| {
                (
                    *p,
                    Place::new(
                        [rel.at[0] + origin[0], rel.at[1] + origin[1]],
                        rel.rotation,
                        rel.mirror,
                    ),
                )
            })
            .collect()
    }

    /// Its extent with its origin at `origin`.
    pub fn extent(&self, block: &Block, origin: Point) -> Rect {
        self.places(origin)
            .into_iter()
            .map(|(p, place)| Shape::of(&block.parts[p], place).extent)
            .reduce(Rect::union)
            .expect("a cluster has a member")
    }

    /// The room it takes with its origin at `origin`: its extent, its
    /// members' text where text goes first, and the power ports (or the
    /// labels) its rail pins take.
    pub fn room(&self, block: &Block, origin: Point) -> Rect {
        let mut r = self.extent(block, origin);
        for (p, place) in self.places(origin) {
            let part = &block.parts[p];
            let shape = Shape::of(part, place);
            let lines: Vec<&str> = [part.refdes.as_str(), part.value.as_str()]
                .into_iter()
                .filter(|t| !t.is_empty())
                .collect();
            if !lines.is_empty() && !matches!(part.kind, PartKind::Port { .. }) {
                let at = crate::text::candidates(shape.body, &lines)
                    .into_iter()
                    .find(|c| {
                        crate::text::boxes(&lines, *c)
                            .iter()
                            .all(|b| !b.grow(crate::geom::GRID / 2).overlaps(&shape.extent))
                    });
                if let Some(at) = at {
                    for b in crate::text::boxes(&lines, at) {
                        r = r.union(b);
                    }
                }
            }
            for (k, pin) in part.pins.iter().enumerate() {
                let Some(n) = pin.net else { continue };
                let kind = match block.nets[n].kind {
                    NetKind::Supply => LabelKind::Power,
                    NetKind::Ground => LabelKind::Ground,
                    NetKind::Signal => continue,
                };
                let at = shape.pins[k];
                let side = crate::label::pointing(kind, at.side);
                // Out two steps where the port turns from its pin.
                let anchor = if side == at.side {
                    at.end
                } else {
                    let d = at.side.step();
                    [at.end[0] + 2 * d[0], at.end[1] + 2 * d[1]]
                };
                let name = &block.label_text(p, k);
                // A rail may be drawn by labels instead: room for either.
                r = r
                    .union(crate::label::label_box(kind, name, anchor, side))
                    .union(crate::label::label_box(
                        LabelKind::Net,
                        name,
                        at.end,
                        at.side,
                    ));
            }
        }
        r
    }

    /// Its room with a label on every signal pin besides: what a part
    /// placed among others reserves, since its nets may take labels.
    pub fn room_with_labels(&self, block: &Block, origin: Point) -> Rect {
        self.room_labelled(block, origin, |_| true)
    }

    /// Its room with a label on each signal pin whose net `labelled` says
    /// will take one.
    pub fn room_labelled(
        &self,
        block: &Block,
        origin: Point,
        labelled: impl Fn(usize) -> bool,
    ) -> Rect {
        let mut r = self.room(block, origin);
        for (p, place) in self.places(origin) {
            let part = &block.parts[p];
            let shape = Shape::of(part, place);
            for (k, pin) in part.pins.iter().enumerate() {
                if let Some(n) = pin.drawn_net()
                    && block.nets[n].kind == NetKind::Signal
                    && (labelled(n) || pin.stacked())
                {
                    let at = shape.pins[k];
                    r = r.union(crate::label::label_box(
                        LabelKind::Net,
                        &block.label_text(p, k),
                        at.end,
                        at.side,
                    ));
                }
            }
        }
        if self.frame > 0 {
            let inside: std::collections::BTreeSet<usize> =
                self.members.iter().map(|(p, _)| *p).collect();
            for (p, place) in self.places(origin) {
                let shape = Shape::of(&block.parts[p], place);
                for (k, pin) in block.parts[p].pins.iter().enumerate() {
                    let Some(n) = pin.drawn_net() else { continue };
                    let leaves = block.nets[n].kind == NetKind::Signal
                        && block.parts.iter().enumerate().any(|(q, part)| {
                            !inside.contains(&q) && part.pins.iter().any(|x| x.nets().contains(&n))
                        });
                    if leaves {
                        let at = shape.pins[k];
                        r = r.union(crate::label::label_box(
                            LabelKind::Net,
                            &block.label_text(p, k),
                            at.end,
                            at.side,
                        ));
                    }
                }
            }
            r = r.grow(self.frame);
            r.max[1] += crate::region::TITLE;
        }
        r
    }

    /// The cluster's first member: what flow and ordering read it by.
    pub fn first(&self) -> usize {
        self.members[0].0
    }
}

/// Whether a part is a connector: its designator is `J`, `P`, `CN` or `CON` with
/// at least two pins, or what a `connector` idiom names.
pub fn is_connector(block: &Block, part: usize) -> bool {
    match &block.parts[part].kind {
        PartKind::Symbol { prefix } => {
            matches!(prefix.as_str(), "J" | "P" | "CN" | "CON") && block.parts[part].pins.len() >= 2
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::GRID;
    use crate::input::NetKind;
    use crate::testing::{net, resistor};

    #[test]
    fn a_framed_cluster_reserves_its_frame_and_title() {
        // R2 beside it on both nets: both leave the cluster.
        let block = Block {
            parts: vec![
                resistor("r1", "R1", "10k", [Some(0), Some(1)]),
                resistor("r2", "R2", "10k", [Some(0), Some(1)]),
            ],
            nets: vec![
                net("a", "A", NetKind::Signal),
                net("b", "B", NetKind::Signal),
            ],
            sheet: [400_000_000, 277_000_000],
            ..Block::default()
        };
        let bare = Cluster::single(0, Place::new([0, 0], 0, false));
        let mut framed = bare.clone();
        framed.frame = crate::region::FRAME;
        let r = bare.room_labelled(&block, [0, 0], |_| true);
        let f = framed.room_labelled(&block, [0, 0], |_| false);
        assert_eq!(f.min, [r.min[0] - 2 * GRID, r.min[1] - 2 * GRID]);
        assert_eq!(
            f.max,
            [
                r.max[0] + 2 * GRID,
                r.max[1] + 2 * GRID + crate::region::TITLE
            ],
            "labels on every net leaving it, its margin, its title band"
        );
    }
}
