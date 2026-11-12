//! Text measured as Inter Regular or SemiBold sets it: the sum of its characters'
//! advance widths, in integers, so a text's box is the same bits
//! everywhere. Kerning is left out; SVG gives each text its measured width
//! (`textLength`), so what is drawn fills the box reserved for it.

use crate::geom::{Nm, Point, Rect};
use crate::widths::UNITS_PER_EM;

/// The smallest text drawn: 1 mm high.
pub const MIN_SIZE: Nm = 1_000_000;
/// Designators, values, labels and names: 50 mil.
pub const SIZE: Nm = 1_270_000;

/// Which face a text is set in: Regular, or SemiBold for designators,
/// titles and emphasis (plan 2.E).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Weight {
    #[default]
    Regular,
    SemiBold,
}

/// A text's size and face.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Font {
    pub size: Nm,
    pub weight: Weight,
}

impl Font {
    /// Values, labels and names.
    pub const REGULAR: Font = Font {
        size: SIZE,
        weight: Weight::Regular,
    };
    /// Designators and titles.
    pub const DESIGNATOR: Font = Font {
        size: SIZE,
        weight: Weight::SemiBold,
    };
}

impl Default for Font {
    fn default() -> Font {
        Font::REGULAR
    }
}

const _: () = assert!(crate::widths_semibold::UNITS_PER_EM == UNITS_PER_EM);

/// A character's advance in `weight`, in font units: a character Inter
/// lacks measures as `0` does.
fn advance(c: char, weight: Weight) -> i64 {
    let table = match weight {
        Weight::Regular => crate::widths::ADVANCES,
        Weight::SemiBold => crate::widths_semibold::ADVANCES,
    };
    let at = |c: char| table.binary_search_by(|(k, _)| k.cmp(&c)).ok();
    let i = at(c).or_else(|| at('0')).expect("the table has 0");
    i64::from(table[i].1)
}

/// `text`'s width in `font`, rounded half up.
pub fn width_in(text: &str, font: Font) -> Nm {
    let units: i64 = text.chars().map(|c| advance(c, font.weight)).sum();
    (units * font.size + UNITS_PER_EM / 2) / UNITS_PER_EM
}

/// `text`'s width at `size` (the em) in Regular, rounded half up.
pub fn width(text: &str, size: Nm) -> Nm {
    width_in(
        text,
        Font {
            size,
            weight: Weight::Regular,
        },
    )
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

/// The box a text in `font` takes, horizontal, its baseline through `at`:
/// from a quarter of the size below the baseline to the size above it.
pub fn text_box_in(text: &str, font: Font, at: Point, anchor: Anchor) -> Rect {
    let w = width_in(text, font);
    let x0 = match anchor {
        Anchor::Start => at[0],
        Anchor::Middle => at[0] - w / 2,
        Anchor::End => at[0] - w,
    };
    Rect::new([x0, at[1] - font.size / 4], [x0 + w, at[1] + font.size])
}

/// [`text_box_in`] in Regular at `size`.
pub fn text_box(text: &str, size: Nm, at: Point, anchor: Anchor) -> Rect {
    text_box_in(
        text,
        Font {
            size,
            weight: Weight::Regular,
        },
        at,
        anchor,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texts_measure_by_their_characters() {
        assert_eq!(width("", SIZE), 0);
        let one = width("R", SIZE);
        assert!(one > 0 && one < SIZE);
        assert_eq!(
            width("RR", SIZE),
            (2 * advance('R', Weight::Regular) * SIZE + 1024) / 2048
        );
        // Unknown characters measure as 0 does.
        assert_eq!(width("\u{4e00}", SIZE), width("0", SIZE));
        let b = text_box("R1", SIZE, [0, 0], Anchor::End);
        assert_eq!(b.max[0], 0);
        assert_eq!(b.height(), SIZE + SIZE / 4);
    }

    #[test]
    fn semibold_measures_wider_and_fonts_say_their_size() {
        let r = Font::REGULAR;
        let b = Font::DESIGNATOR;
        assert_eq!((r.size, r.weight), (SIZE, Weight::Regular));
        assert_eq!((b.size, b.weight), (SIZE, Weight::SemiBold));
        assert!(width_in("U12", b) > width_in("U12", r));
        assert_eq!(width_in("U12", r), width("U12", SIZE));
        let big = Font {
            size: 2 * SIZE,
            weight: Weight::SemiBold,
        };
        assert_eq!(
            text_box_in("U1", big, [0, 0], Anchor::Start).height(),
            2 * SIZE + SIZE / 2
        );
    }
}
