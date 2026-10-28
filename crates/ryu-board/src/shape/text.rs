//! Text: its characters' strokes in its font, scaled, placed and drawn
//! with its line width.

use std::collections::BTreeSet;

use super::stroke::line_outline;
use super::{Part, Piece, Shape, ShapeError};
use crate::geom::{Edge, Pen, Point, Stroke, div_round};
use crate::model::{Font, Polarity, Text};

fn scale(v: i64, to: i64, from: i64) -> i64 {
    div_round(i128::from(v) * i128::from(to), i128::from(from)) as i64
}

/// A text's shape, positive, in board coordinates. Each character's
/// strokes are scaled from the font's box to the text's character size
/// (x by `width / font.width`, y by `height / font.height`), moved along
/// by `k × (font.width + font.offset)` scaled the same way for the `k`th
/// character, drawn with the text's line width (each stroke keeping its
/// round or square pen) and placed by the text's placement. A character
/// the font lacks is a gap, listed in `missing` unless it is white space.
pub fn text_shape(text: &Text, font: &Font) -> Result<Shape, ShapeError> {
    if font.width <= 0 || font.height <= 0 {
        return Err(ShapeError::BadParameters("a font needs a size"));
    }
    let advance = scale(font.width + font.offset, text.width, font.width);
    let mut shape = Shape::default();
    let mut missing = BTreeSet::new();
    for (k, ch) in text.string.chars().enumerate() {
        let Some(strokes) = font.chars.get(&ch) else {
            if !ch.is_whitespace() {
                missing.insert(ch);
            }
            continue;
        };
        let dx = advance * k as i64;
        let at = |p: Point| {
            Point::new(
                scale(p.x, text.width, font.width) + dx,
                scale(p.y, text.height, font.height),
            )
        };
        for s in strokes {
            let pen = match s.pen {
                Pen::Round(_) => Pen::Round(text.line_width),
                Pen::Square(_) => Pen::Square(text.line_width),
            };
            let (a, b) = (at(s.a), at(s.b));
            let piece = match line_outline(a, b, pen) {
                Some(r) => Piece::Region(text.place.region(&r)),
                None => Piece::Stroke(text.place.stroke(&Stroke {
                    path: vec![Edge::line(a, b)],
                    pen,
                })),
            };
            shape.parts.push(Part {
                polarity: Polarity::Positive,
                piece,
            });
        }
    }
    shape.missing = missing.into_iter().collect();
    Ok(shape)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::Place;
    use crate::model::FontStroke;
    use std::collections::BTreeMap;

    fn font() -> Font {
        // A font in a 1000 × 2000 box: `I` one upright, `L` two strokes.
        let stroke = |a: (i64, i64), b: (i64, i64)| FontStroke {
            a: Point::new(a.0, a.1),
            b: Point::new(b.0, b.1),
            pen: Pen::Round(1),
        };
        let mut chars = BTreeMap::new();
        chars.insert('I', vec![stroke((500, 0), (500, 2000))]);
        chars.insert(
            'L',
            vec![stroke((0, 0), (0, 2000)), stroke((0, 0), (1000, 0))],
        );
        Font {
            name: "standard".into(),
            width: 1000,
            height: 2000,
            offset: 250,
            chars,
        }
    }

    fn text(s: &str) -> Text {
        Text {
            font: "standard".into(),
            place: Place::at(Point::new(10_000, 0)),
            width: 2000,
            height: 4000,
            line_width: 200,
            string: s.into(),
        }
    }

    #[test]
    fn characters_scale_and_advance() {
        let s = text_shape(&text("IL"), &font()).unwrap();
        assert_eq!(s.parts.len(), 3);
        let b = s.bounds().unwrap();
        // `L` starts one advance (1250 × 2 = 2500) along, from x 10,000.
        assert_eq!(b.min, Point::new(10_000 + 1000 - 100, -100));
        assert_eq!(b.max, Point::new(10_000 + 2500 + 2000 + 100, 4000 + 100));
        assert!(s.missing.is_empty());
    }

    #[test]
    fn missing_characters_are_gaps() {
        let s = text_shape(&text("I?I ?"), &font()).unwrap();
        assert_eq!(s.parts.len(), 2);
        assert_eq!(s.missing, vec!['?']);
    }
}
