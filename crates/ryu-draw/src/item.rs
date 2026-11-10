//! What a drawing draws: stroked or filled paths, and text.

use crate::geom::{P, Path};
use crate::style::{Colour, Stroke};

/// Where a text's point is along it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Anchor {
    Start,
    Middle,
    End,
}

/// One thing drawn, in the drawing's unit.
#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    /// A path stroked, filled (by the nonzero rule), or both.
    Path {
        path: Path,
        stroke: Option<Stroke>,
        fill: Option<Colour>,
    },
    /// One line of Inter: `at` on its baseline at `anchor`, `size` its
    /// em, turned counterclockwise by `angle` radians about `at`.
    Text {
        text: String,
        at: P,
        size: f64,
        angle: f64,
        anchor: Anchor,
        colour: Colour,
    },
}

impl Item {
    /// A box holding what the item draws (a text's from Inter's advance
    /// widths, its ascender and descender).
    pub fn bounds(&self) -> [P; 2] {
        match self {
            Item::Path { path, stroke, .. } => {
                let [lo, hi] = path.bounds();
                let w = stroke.as_ref().map_or(0.0, |s| s.width / 2.0);
                [[lo[0] - w, lo[1] - w], [hi[0] + w, hi[1] + w]]
            }
            Item::Text {
                text,
                at,
                size,
                angle,
                anchor,
                ..
            } => {
                let w = crate::font::width(text, *size);
                let x0 = match anchor {
                    Anchor::Start => 0.0,
                    Anchor::Middle => -w / 2.0,
                    Anchor::End => -w,
                };
                let (asc, desc) = crate::font::extent(*size);
                let (s, c) = ryu_math::sin_cos(*angle);
                let corners = [[x0, desc], [x0 + w, desc], [x0 + w, asc], [x0, asc]]
                    .map(|q| [at[0] + c * q[0] - s * q[1], at[1] + s * q[0] + c * q[1]]);
                let mut lo = corners[0];
                let mut hi = corners[0];
                for q in corners {
                    lo = [lo[0].min(q[0]), lo[1].min(q[1])];
                    hi = [hi[0].max(q[0]), hi[1].max(q[1])];
                }
                [lo, hi]
            }
        }
    }
}
