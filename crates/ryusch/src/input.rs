//! What the engine reads: a block as data. ryueda builds it from a
//! project; nothing here knows of files.

use crate::geom::{Nm, Point, Side};
use crate::layout::{Instruction, Layout};
use std::collections::BTreeMap;

/// One block to lay out.
#[derive(Clone, Debug, Default)]
pub struct Block {
    /// What sheets' text calls it.
    pub name: String,
    /// The area of a sheet the contents must stay inside: the paper less
    /// the margin the PDF's border takes.
    pub sheet: [Nm; 2],
    pub parts: Vec<Part>,
    pub nets: Vec<Net>,
    /// The lasting instructions, by ID.
    pub instructions: BTreeMap<String, Instruction>,
    /// One-off moves: applied once, their results held as placements.
    pub moves: Vec<Instruction>,
    /// The layout the files hold, kept where it can be.
    pub previous: Option<Layout>,
    /// What to lay out from nothing, keeping the instructions.
    pub relayout: Relayout,
}

/// What a layout starts from nothing.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Relayout {
    /// Nothing: keep what the previous layout holds.
    #[default]
    None,
    /// The whole block.
    Block,
    /// One sheet, by ID.
    Sheet(String),
}

/// Something placed on a sheet: a symbol's unit, a child block's symbol,
/// or a port of the block.
#[derive(Clone, Debug, PartialEq)]
pub struct Part {
    /// `<component>`, `<component>:<unit>`, `instance:<id>`, `port:<id>`.
    pub key: String,
    pub kind: PartKind,
    /// The first line of text beside it: a designator, a block's name.
    pub refdes: String,
    /// The second: a value ("" for none).
    pub value: String,
    /// The body, in the part's own frame.
    pub graphics: Vec<Graphic>,
    pub pins: Vec<Pin>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PartKind {
    /// A component's unit; `prefix` is its designator's letters.
    Symbol { prefix: String },
    /// A child block, drawn as a box with its ports as pins.
    Block,
    /// A port of this block: where a signal enters or leaves it.
    Port { flow: Flow },
}

/// Which way a port carries what it carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Flow {
    In,
    Out,
    Bidirectional,
    Power,
    Passive,
}

/// KiCad's electrical pin types.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum PinKind {
    Input,
    Output,
    Bidirectional,
    TriState,
    Passive,
    Free,
    Unspecified,
    PowerIn,
    PowerOut,
    OpenCollector,
    OpenEmitter,
    NoConnect,
}

impl PinKind {
    /// Whether it drives the signal on its net.
    pub fn drives(self) -> bool {
        matches!(
            self,
            PinKind::Output | PinKind::TriState | PinKind::OpenCollector | PinKind::OpenEmitter
        )
    }

    /// Whether it takes the signal on its net.
    pub fn takes(self) -> bool {
        matches!(self, PinKind::Input)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Pin {
    pub number: String,
    /// Drawn inside the body; "" for none.
    pub name: String,
    /// The connection point, in the part's frame.
    pub at: Point,
    /// Which way it points from the body to its end.
    pub side: Side,
    pub length: Nm,
    pub kind: PinKind,
    /// The net it is on, by index into the block's nets.
    pub net: Option<usize>,
    /// Marked not connected.
    pub nc: bool,
    /// Whether its number is drawn (not on two-pin parts).
    pub show_number: bool,
}

/// A net of the block, a rail it uses, or a port's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Net {
    /// `<net id>`, `rail:<id>` or `port:<id>`: what a layout keys it by.
    pub key: String,
    /// What its labels say.
    pub name: String,
    pub kind: NetKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NetKind {
    Signal,
    /// A rail drawn by power ports pointing up.
    Supply,
    /// A rail drawn by ground symbols pointing down.
    Ground,
}

/// What a closed shape is filled with.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Fill {
    #[default]
    None,
    Body,
    Outline,
}

/// A piece of a part's body, in its own frame.
#[derive(Clone, Debug, PartialEq)]
pub enum Graphic {
    Rect {
        start: Point,
        end: Point,
        fill: Fill,
    },
    Polyline {
        points: Vec<Point>,
        fill: Fill,
    },
    /// Through three points.
    Arc {
        start: Point,
        mid: Point,
        end: Point,
    },
    Circle {
        center: Point,
        radius: Nm,
        fill: Fill,
    },
    /// A cubic Bézier.
    Bezier {
        points: [Point; 4],
    },
    Text {
        text: String,
        at: Point,
        size: Nm,
    },
}

impl Block {
    /// The part keyed `key`.
    pub fn part(&self, key: &str) -> Option<usize> {
        self.parts.iter().position(|p| p.key == key)
    }

    /// The net keyed `key`.
    pub fn net(&self, key: &str) -> Option<usize> {
        self.nets.iter().position(|n| n.key == key)
    }
}
