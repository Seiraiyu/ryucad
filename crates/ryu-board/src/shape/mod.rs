//! Exact shapes: everything on a board as contours of integer segments and
//! arcs. Shapes are never merged with their neighbours here; resolving
//! negatives and merging copper are stage 1's booleans.

mod build;

pub use build::{circle, polygon, rect};

use crate::geom::{Edge, Nm, Pen, Place, Rect, Region, Stroke};
use crate::model::Polarity;

/// One piece of a shape.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Piece {
    Region(Region),
    /// A path and a round or square pen, where its outline isn't a simple
    /// region (an arc drawn with a pen wider than its radius, a square pen
    /// along an arc): stage 1 sweeps it.
    Stroke(Stroke),
    /// An edge drawn with a pen that is a shape of its own (a line drawn
    /// with a symbol that isn't round or square), the pen's regions in its
    /// own coordinates: stage 1 sweeps it.
    Swept {
        path: Edge,
        pen: Vec<Region>,
    },
}

/// A piece and its polarity.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Part {
    pub polarity: Polarity,
    pub piece: Piece,
}

/// What a feature or a symbol is, in drawing order.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Shape {
    pub parts: Vec<Part>,
    /// Set when some piece only approximates the true outline (the
    /// ellipse), to within this many nanometres.
    pub approximate: Option<Nm>,
    /// Characters a text's font doesn't have, left as gaps.
    pub missing: Vec<char>,
    /// Set for a barcode, which has no shape here.
    pub skipped: bool,
}

/// Why something has no shape.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ShapeError {
    /// Parameters no shape fits (a rounding wider than its side, a hole
    /// larger than its ring), saying which.
    BadParameters(&'static str),
    /// A symbol name the board doesn't have.
    UnknownSymbol(String),
    /// A user symbol on a cycle, with every symbol of the cycle.
    SymbolCycle(Vec<String>),
    /// A text's font the board doesn't have.
    UnknownFont(String),
    /// A coordinate past ±2^40 nm once placed.
    OutOfRange,
}

impl Shape {
    pub fn regions(polarity: Polarity, regions: impl IntoIterator<Item = Region>) -> Shape {
        Shape {
            parts: regions
                .into_iter()
                .map(|r| Part {
                    polarity,
                    piece: Piece::Region(r),
                })
                .collect(),
            ..Shape::default()
        }
    }

    /// This shape placed, its polarity multiplied by `polarity`.
    pub fn placed(&self, place: &Place, polarity: Polarity) -> Shape {
        Shape {
            parts: self
                .parts
                .iter()
                .map(|p| Part {
                    polarity: p.polarity.times(polarity),
                    piece: match &p.piece {
                        Piece::Region(r) => Piece::Region(place.region(r)),
                        Piece::Stroke(s) => Piece::Stroke(place.stroke(s)),
                        Piece::Swept { path, pen } => Piece::Swept {
                            path: place.edge(path),
                            pen: pen.iter().map(|r| place_pen(place, r)).collect(),
                        },
                    },
                })
                .collect(),
            approximate: self.approximate,
            missing: self.missing.clone(),
            skipped: self.skipped,
        }
    }

    /// The smallest integer box holding every piece, or `None` for an
    /// empty shape.
    pub fn bounds(&self) -> Option<Rect> {
        self.parts
            .iter()
            .map(|p| piece_bounds(&p.piece))
            .reduce(Rect::union)
    }
}

/// A pen keeps its own origin: only its turn, mirror and scale apply.
fn place_pen(place: &Place, r: &Region) -> Region {
    Place {
        at: crate::geom::Point::ORIGIN,
        ..*place
    }
    .region(r)
}

fn grow(r: Rect, by: Nm) -> Rect {
    Rect {
        min: r.min.offset(-by, -by),
        max: r.max.offset(by, by),
    }
}

fn piece_bounds(p: &Piece) -> Rect {
    match p {
        Piece::Region(r) => r.bounds(),
        Piece::Stroke(s) => {
            let path = s
                .path
                .iter()
                .map(Rect::of_edge)
                .reduce(Rect::union)
                .expect("a path has edges");
            match s.pen {
                Pen::Round(w) => grow(path, ceil_half(w)),
                // A square pen kept square reaches half its side each way.
                Pen::Square(w) => grow(path, ceil_half(w)),
            }
        }
        Piece::Swept { path, pen } => {
            let p = Rect::of_edge(path);
            let pen = pen
                .iter()
                .map(Region::bounds)
                .reduce(Rect::union)
                .expect("a pen has regions");
            Rect {
                min: p.min.offset(pen.min.x, pen.min.y),
                max: p.max.offset(pen.max.x, pen.max.y),
            }
        }
    }
}

fn ceil_half(w: Nm) -> Nm {
    (w + 1) / 2
}
