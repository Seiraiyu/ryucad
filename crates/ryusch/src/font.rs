//! Text measured as Inter Regular sets it: the sum of its characters'
//! advance widths, in integers, so a text's box is the same bits
//! everywhere. Kerning is left out; SVG gives each text its measured width
//! (`textLength`), so what is drawn fills the box reserved for it.

use crate::geom::{Nm, Point, Rect};
use crate::widths::{ADVANCES, UNITS_PER_EM};

/// The smallest text drawn: 1 mm high.
pub const MIN_SIZE: Nm = 1_000_000;
/// Designators, values, labels and names: 50 mil.
pub const SIZE: Nm = 1_270_000;

/// A character's advance, in font units: a character Inter lacks
/// measures as `0` does.
fn advance(c: char) -> i64 {
    let at = |c: char| ADVANCES.binary_search_by(|(k, _)| k.cmp(&c)).ok();
    let i = at(c).or_else(|| at('0')).expect("the table has 0");
    i64::from(ADVANCES[i].1)
}

/// `text`'s width at `size` (the em), rounded half up.
pub fn width(text: &str, size: Nm) -> Nm {
    let units: i64 = text.chars().map(advance).sum();
    (units * size + UNITS_PER_EM / 2) / UNITS_PER_EM
}

/// Which way a text runs from its anchor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Anchor {
    /// The anchor is the start of the baseline.
    Start,
    /// The middle of the baseline.
    Middle,
    /// The end of the baseline.
    End,
}

/// The box a text takes, horizontal, its baseline through `at`: from a
/// quarter of the size below the baseline to the size above it.
pub fn text_box(text: &str, size: Nm, at: Point, anchor: Anchor) -> Rect {
    let w = width(text, size);
    let x0 = match anchor {
        Anchor::Start => at[0],
        Anchor::Middle => at[0] - w / 2,
        Anchor::End => at[0] - w,
    };
    Rect::new([x0, at[1] - size / 4], [x0 + w, at[1] + size])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texts_measure_by_their_characters() {
        assert_eq!(width("", SIZE), 0);
        let one = width("R", SIZE);
        assert!(one > 0 && one < SIZE);
        assert_eq!(width("RR", SIZE), (2 * advance('R') * SIZE + 1024) / 2048);
        // Unknown characters measure as 0 does.
        assert_eq!(width("\u{4e00}", SIZE), width("0", SIZE));
        let b = text_box("R1", SIZE, [0, 0], Anchor::End);
        assert_eq!(b.max[0], 0);
        assert_eq!(b.height(), SIZE + SIZE / 4);
    }
}
