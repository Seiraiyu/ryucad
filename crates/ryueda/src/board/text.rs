//! Text on a board: a footprint's reference and value fields and its
//! text graphics, through `ryu-board`'s built-in stroke font. KiCad places
//! text by its centre, ryu-board by its first character's bottom left:
//! the origin moves back by half the drawn width and height in the text's
//! own frame.

use ryu_board::geom::{Mdeg, Place, Point};
use ryu_board::model::Text;
use ryu_board::shape::{BUILTIN_FONT, Shape, ShapeError, builtin_font, text_shape};

/// A centred text in the footprint's frame.
pub fn centred(
    string: &str,
    at: [i64; 2],
    rotation_mdeg: i64,
    size_nm: [i64; 2],
    thickness_nm: i64,
) -> Result<Shape, ShapeError> {
    let f = builtin_font();
    let n = string.chars().count() as i128;
    if n == 0 {
        return Ok(Shape::default());
    }
    let [w, h] = size_nm;
    // Each character advances (width + offset) scaled to w; the last has
    // no offset after it.
    let advance = i128::from(f.width + f.offset) * i128::from(w) / i128::from(f.width);
    let drawn = advance * (n - 1) + i128::from(w);
    let turn = Place { at: Point::new(at[0], at[1]), rot: Mdeg::new(rotation_mdeg), mirror: false, scale: None };
    let origin = turn.point(Point::new(-(drawn / 2) as i64, -h / 2));
    let text = Text {
        font: BUILTIN_FONT.into(),
        place: Place { at: origin, ..turn },
        width: w,
        height: h,
        line_width: thickness_nm.max(1),
        string: string.to_owned(),
    };
    text_shape(&text, f)
}

/// `${REFERENCE}` and `${VALUE}` in a footprint's text, filled in.
pub fn substitute(text: &str, refdes: &str, value: &str) -> String {
    text.replace("${REFERENCE}", refdes).replace("${VALUE}", value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_is_centred_on_its_place() {
        // Centred by the characters' boxes, as KiCad does: `H` fills its
        // box, so its ink is centred too.
        let s = centred("HH", [1_000_000, 2_000_000], 0, [1_000_000, 1_000_000], 150_000).unwrap();
        let b = s.bounds().unwrap();
        let cx = (b.min.x + b.max.x) / 2;
        let cy = (b.min.y + b.max.y) / 2;
        assert!((cx - 1_000_000).abs() <= 1, "{b:?}");
        assert!((cy - 2_000_000).abs() <= 1, "{b:?}");
        let turned = centred("R1", [0, 0], 90_000, [1_000_000, 1_000_000], 150_000).unwrap();
        let t = turned.bounds().unwrap();
        assert!(t.max.y - t.min.y > t.max.x - t.min.x, "turned text runs up: {t:?}");
    }

    #[test]
    fn fields_are_substituted() {
        assert_eq!(substitute("${REFERENCE} ${VALUE}", "R1", "4k7"), "R1 4k7");
    }
}
