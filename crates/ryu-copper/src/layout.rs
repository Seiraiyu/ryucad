//! The engine's input: a generic layout any product can build. ryudfm
//! builds one from a `ryu-board` board step (plan 1.D); ryueda will build
//! one from its own boards.
//!
//! Items and holes are keyed by the caller's `K` (ordered and hashable:
//! ryudfm's feature references, ryueda's stable IDs), so every answer
//! names what the caller knows.

use std::collections::BTreeMap;

use ryu_board::geom::{Nm, Point};
use ryu_board::model::Side;
use ryu_board::shape::Shape;

/// A layer of the stack, in stack order.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Layer {
    pub name: String,
    pub side: Side,
    /// Whether the layer is copper. Other layers (mask, paste, silk) may
    /// be measured and merged but take no part in connectivity.
    pub copper: bool,
}

/// One feature on a layer.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Item {
    /// The layer's index in [`Layout::layers`].
    pub layer: usize,
    /// Its shape, its parts with their polarity in drawing order.
    pub shape: Shape,
    /// The net the source claims for it, if any.
    pub net: Option<String>,
}

/// A drilled hole's outline.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HoleShape {
    Round {
        at: Point,
        d: Nm,
    },
    /// A slot from `a` to `b`, `d` wide, with round ends.
    Slot {
        a: Point,
        b: Point,
        d: Nm,
    },
}

/// A drilled hole.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Hole {
    pub shape: HoleShape,
    /// The first and last copper layers it passes, as layer indices.
    pub span: (usize, usize),
    pub plated: bool,
    /// Spans whose plating a back-drill removes.
    pub backdrilled: Vec<(usize, usize)>,
}

/// The side a netlist point is reached from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PointSide {
    Top,
    Bottom,
}

/// A point of the netlist to compare the copper against.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct NetPoint<K> {
    pub net: String,
    pub at: Point,
    /// `None` for a point at a hole, reached through its plating.
    pub side: Option<PointSide>,
    pub key: K,
}

/// Everything the engine measures and connects.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout<K: Ord> {
    pub layers: Vec<Layer>,
    pub items: BTreeMap<K, Item>,
    pub holes: BTreeMap<K, Hole>,
    pub points: Vec<NetPoint<K>>,
}

impl<K: Ord> Default for Layout<K> {
    fn default() -> Layout<K> {
        Layout {
            layers: Vec::new(),
            items: BTreeMap::new(),
            holes: BTreeMap::new(),
            points: Vec::new(),
        }
    }
}

impl HoleShape {
    /// The hole's outline as a body: a disc, or a slot with round ends.
    pub fn body(&self) -> crate::measure::Body {
        match *self {
            HoleShape::Round { at, d } => crate::measure::Body::stroke(at, at, d),
            HoleShape::Slot { a, b, d } => crate::measure::Body::stroke(a, b, d),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::measure::{self, Body};

    #[test]
    fn holes_are_discs_and_slots() {
        let round = HoleShape::Round {
            at: Point::new(0, 0),
            d: 300,
        }
        .body();
        let slot = HoleShape::Slot {
            a: Point::new(1000, 0),
            b: Point::new(2000, 0),
            d: 200,
        }
        .body();
        // 1000 - 150 - 100.
        assert_eq!(measure::distance(&round, &slot).unwrap().nm, 750);
        assert!(measure::contains(&slot, Point::new(1500, 100)).is_yes());
        assert!(!measure::contains(&slot, Point::new(1500, 101)).is_yes());
        let l: Layout<u32> = Layout::default();
        assert!(l.items.is_empty() && Body::point(Point::ORIGIN).bounds().is_some());
    }
}
