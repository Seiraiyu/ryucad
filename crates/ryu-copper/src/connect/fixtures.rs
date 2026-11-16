//! Hand-made boards for connectivity's unit tests.

use ryu_board::geom::{Edge, Nm, Pen, Point, Region, Stroke};
use ryu_board::model::{Polarity, Side};
use ryu_board::shape::{Part, Piece, Shape, circle, polygon};

use crate::layout::{Hole, HoleShape, Item, Layer, Layout};

pub(crate) fn p(x: i64, y: i64) -> Point {
    Point::new(x, y)
}

/// `n` copper layers, top to bottom.
pub(crate) fn board(n: usize) -> Layout<&'static str> {
    let side = |k: usize| {
        if k == 0 {
            Side::Top
        } else if k + 1 == n {
            Side::Bottom
        } else {
            Side::Inner
        }
    };
    Layout {
        layers: (0..n)
            .map(|k| Layer {
                name: format!("l{k}"),
                side: side(k),
                copper: true,
            })
            .collect(),
        ..Layout::default()
    }
}

/// A trace from `a` to `b` with a round pen `w` wide.
pub(crate) fn trace(layer: usize, a: Point, b: Point, w: Nm, net: Option<&str>) -> Item {
    Item {
        layer,
        shape: Shape {
            parts: vec![Part {
                polarity: Polarity::Positive,
                piece: Piece::Stroke(Stroke {
                    path: vec![Edge::line(a, b)],
                    pen: Pen::Round(w),
                }),
            }],
            ..Shape::default()
        },
        net: net.map(String::from),
    }
}

/// A round pad of radius `r`.
pub(crate) fn pad(layer: usize, c: Point, r: Nm, net: Option<&str>) -> Item {
    Item {
        layer,
        shape: Shape::regions(Polarity::Positive, [Region::solid(circle(c, r).unwrap())]),
        net: net.map(String::from),
    }
}

/// A negative disc of radius `r`.
pub(crate) fn blot(layer: usize, c: Point, r: Nm) -> Item {
    Item {
        layer,
        shape: Shape::regions(Polarity::Negative, [Region::solid(circle(c, r).unwrap())]),
        net: None,
    }
}

/// The box `[x0, x1] × [y0, y1]` as a region.
pub(crate) fn square(x0: i64, y0: i64, x1: i64, y1: i64) -> Region {
    Region::solid(polygon(&[p(x0, y0), p(x1, y0), p(x1, y1), p(x0, y1)]).unwrap())
}

/// A box as an item of one polarity.
pub(crate) fn block(layer: usize, x0: i64, y0: i64, x1: i64, y1: i64, polarity: Polarity) -> Item {
    Item {
        layer,
        shape: Shape::regions(polarity, [square(x0, y0, x1, y1)]),
        net: None,
    }
}

/// A plated round hole.
pub(crate) fn via(at: Point, d: Nm, span: (usize, usize)) -> Hole {
    Hole {
        shape: HoleShape::Round { at, d },
        span,
        plated: true,
        backdrilled: vec![],
    }
}
