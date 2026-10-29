//! Clusters: parts placed together, rigidly, as one. A part alone is a
//! cluster of one; an idiom or a `group` instruction makes larger ones.

use crate::geom::{Place, Point, Rect};
use crate::input::{Block, NetKind, PartKind};
use crate::layout::{IdiomKind, LabelKind};
use crate::shape::Shape;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cluster {
    /// Each member part and its place relative to the cluster's origin.
    pub members: Vec<(usize, Place)>,
    /// The idiom it draws, if it is one.
    pub idiom: Option<IdiomKind>,
}

impl Cluster {
    pub fn single(part: usize, place: Place) -> Cluster {
        Cluster {
            members: vec![(part, place)],
            idiom: None,
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
    /// members' text where text goes first, and the power ports its rail
    /// pins take.
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
                r = r.union(crate::label::label_box(
                    kind,
                    &block.nets[n].name,
                    at.end,
                    side,
                ));
            }
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
