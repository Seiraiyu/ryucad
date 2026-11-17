//! The built-in stroke font: printable ASCII drawn for this crate on an
//! 8 × 12 grid (capitals 12 tall, lower case 8), every glyph inside its
//! box, round pens. Drawn by us, so there is no licence to carry. Space
//! has no strokes; [`super::text_shape`] leaves it as a gap without
//! listing it missing.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use crate::geom::{Pen, Point};
use crate::model::{Font, FontStroke};

/// The font's name, as a [`crate::model::Text`] names it.
pub const BUILTIN_FONT: &str = "ryu-stroke";

/// Nanometres per grid unit in the font's own box.
const UNIT: i64 = 1_000;

/// Each glyph as polylines separated by `;`, points `x,y` by spaces.
#[rustfmt::skip]
const GLYPHS: &[(char, &str)] = &[
    ('!', "4,12 4,4; 4,1 4,0"),
    ('"', "2,12 2,9; 6,12 6,9"),
    ('#', "2,0 3,12; 5,0 6,12; 0,4 8,4; 0,8 8,8"),
    ('$', "8,10 2,10 0,8 2,6 6,6 8,4 6,2 0,2; 4,12 4,0"),
    ('%', "0,0 8,12; 1,12 3,12 3,10 1,10 1,12; 5,2 7,2 7,0 5,0 5,2"),
    ('&', "8,0 1,9 1,11 2,12 4,12 5,11 5,9 0,4 0,2 2,0 5,0 8,4"),
    ('\'', "4,12 4,9"),
    ('(', "6,12 4,10 4,2 6,0"),
    (')', "2,12 4,10 4,2 2,0"),
    ('*', "4,10 4,2; 1,8 7,4; 1,4 7,8"),
    ('+', "4,9 4,3; 1,6 7,6"),
    (',', "4,2 3,0"),
    ('-', "1,6 7,6"),
    ('.', "4,1 4,0"),
    ('/', "0,0 8,12"),
    ('0', "2,0 0,2 0,10 2,12 6,12 8,10 8,2 6,0 2,0; 2,3 6,9"),
    ('1', "2,10 4,12 4,0; 2,0 6,0"),
    ('2', "0,10 2,12 6,12 8,10 8,8 0,0 8,0"),
    ('3', "0,10 2,12 6,12 8,10 8,8 6,6 2,6; 6,6 8,4 8,2 6,0 2,0 0,2"),
    ('4', "6,0 6,12 0,4 8,4"),
    ('5', "8,12 0,12 0,7 6,7 8,5 8,2 6,0 0,0"),
    ('6', "8,10 6,12 2,12 0,10 0,2 2,0 6,0 8,2 8,5 6,7 0,7"),
    ('7', "0,12 8,12 2,0"),
    ('8', "2,6 0,8 0,10 2,12 6,12 8,10 8,8 6,6 2,6 0,4 0,2 2,0 6,0 8,2 8,4 6,6"),
    ('9', "8,5 2,5 0,7 0,10 2,12 6,12 8,10 8,2 6,0 2,0 0,2"),
    (':', "4,8 4,7; 4,1 4,0"),
    (';', "4,8 4,7; 4,2 3,0"),
    ('<', "8,11 0,6 8,1"),
    ('=', "1,8 7,8; 1,4 7,4"),
    ('>', "0,11 8,6 0,1"),
    ('?', "0,10 2,12 6,12 8,10 8,8 4,5 4,3; 4,1 4,0"),
    ('@', "6,4 6,8 3,8 2,6 3,4 6,4 8,6 8,10 6,12 2,12 0,10 0,2 2,0 8,0"),
    ('A', "0,0 0,8 4,12 8,8 8,0; 0,5 8,5"),
    ('B', "0,0 0,12 6,12 8,10 8,8 6,6 0,6; 6,6 8,4 8,2 6,0 0,0"),
    ('C', "8,10 6,12 2,12 0,10 0,2 2,0 6,0 8,2"),
    ('D', "0,0 0,12 5,12 8,9 8,3 5,0 0,0"),
    ('E', "8,12 0,12 0,0 8,0; 0,6 6,6"),
    ('F', "8,12 0,12 0,0; 0,6 6,6"),
    ('G', "8,10 6,12 2,12 0,10 0,2 2,0 6,0 8,2 8,6 4,6"),
    ('H', "0,0 0,12; 8,0 8,12; 0,6 8,6"),
    ('I', "2,12 6,12; 4,12 4,0; 2,0 6,0"),
    ('J', "4,12 8,12; 6,12 6,2 4,0 2,0 0,2"),
    ('K', "0,0 0,12; 8,12 0,4; 3,7 8,0"),
    ('L', "0,12 0,0 8,0"),
    ('M', "0,0 0,12 4,6 8,12 8,0"),
    ('N', "0,0 0,12 8,0 8,12"),
    ('O', "2,0 0,2 0,10 2,12 6,12 8,10 8,2 6,0 2,0"),
    ('P', "0,0 0,12 6,12 8,10 8,8 6,6 0,6"),
    ('Q', "2,0 0,2 0,10 2,12 6,12 8,10 8,2 6,0 2,0; 5,3 8,0"),
    ('R', "0,0 0,12 6,12 8,10 8,8 6,6 0,6; 4,6 8,0"),
    ('S', "8,10 6,12 2,12 0,10 0,8 2,6 6,6 8,4 8,2 6,0 2,0 0,2"),
    ('T', "0,12 8,12; 4,12 4,0"),
    ('U', "0,12 0,2 2,0 6,0 8,2 8,12"),
    ('V', "0,12 4,0 8,12"),
    ('W', "0,12 2,0 4,6 6,0 8,12"),
    ('X', "0,12 8,0; 0,0 8,12"),
    ('Y', "0,12 4,6 8,12; 4,6 4,0"),
    ('Z', "0,12 8,12 0,0 8,0"),
    ('[', "6,12 3,12 3,0 6,0"),
    ('\\', "0,12 8,0"),
    (']', "2,12 5,12 5,0 2,0"),
    ('^', "1,8 4,12 7,8"),
    ('_', "0,0 8,0"),
    ('`', "3,12 5,10"),
    ('a', "1,8 6,8 8,6 8,0; 8,5 2,5 0,3 0,2 2,0 8,0"),
    ('b', "0,12 0,0 6,0 8,2 8,6 6,8 0,8"),
    ('c', "8,8 2,8 0,6 0,2 2,0 8,0"),
    ('d', "8,12 8,0 2,0 0,2 0,6 2,8 8,8"),
    ('e', "0,4 8,4 8,6 6,8 2,8 0,6 0,2 2,0 8,0"),
    ('f', "8,12 5,12 3,10 3,0; 0,7 7,7"),
    ('g', "8,3 2,3 0,5 0,6 2,8 8,8 8,1 7,0 1,0"),
    ('h', "0,12 0,0; 0,6 2,8 6,8 8,6 8,0"),
    ('i', "4,8 4,0; 4,11 4,10"),
    ('j', "5,8 5,1 4,0 1,0; 5,11 5,10"),
    ('k', "0,12 0,0; 7,8 0,3; 3,5 8,0"),
    ('l', "3,12 4,12 4,1 5,0 6,0"),
    ('m', "0,0 0,8; 0,6 2,8 3,8 4,6 4,0; 4,6 6,8 7,8 8,6 8,0"),
    ('n', "0,0 0,8; 0,6 2,8 6,8 8,6 8,0"),
    ('o', "2,0 0,2 0,6 2,8 6,8 8,6 8,2 6,0 2,0"),
    ('p', "0,0 0,8 6,8 8,6 8,5 6,3 0,3"),
    ('q', "8,0 8,8 2,8 0,6 0,5 2,3 8,3"),
    ('r', "0,0 0,8; 0,5 3,8 8,8"),
    ('s', "8,8 2,8 0,6 2,4 6,4 8,2 6,0 0,0"),
    ('t', "3,12 3,1 4,0 7,0; 0,8 7,8"),
    ('u', "0,8 0,2 2,0 6,0 8,2; 8,8 8,0"),
    ('v', "0,8 4,0 8,8"),
    ('w', "0,8 2,0 4,5 6,0 8,8"),
    ('x', "0,8 8,0; 0,0 8,8"),
    ('y', "0,8 0,5 2,3 8,3; 8,8 8,1 7,0 1,0"),
    ('z', "0,8 8,8 0,0 8,0"),
    ('{', "6,12 5,12 4,11 4,7 3,6 4,5 4,1 5,0 6,0"),
    ('|', "4,12 4,0"),
    ('}', "2,12 3,12 4,11 4,7 5,6 4,5 4,1 3,0 2,0"),
    ('~', "0,6 2,8 4,6 6,4 8,6"),
];

fn point(s: &str) -> Point {
    let (x, y) = s.split_once(',').expect("x,y");
    Point::new(
        x.parse::<i64>().expect("a grid x") * UNIT,
        y.parse::<i64>().expect("a grid y") * UNIT,
    )
}

fn strokes(spec: &str) -> Vec<FontStroke> {
    let mut out = Vec::new();
    for line in spec.split(';') {
        let pts: Vec<Point> = line.split_whitespace().map(point).collect();
        for w in pts.windows(2) {
            out.push(FontStroke {
                a: w[0],
                b: w[1],
                pen: Pen::Round(1),
            });
        }
    }
    out
}

/// The built-in font: a box 8 × 12 units, 3 units between characters.
pub fn builtin_font() -> &'static Font {
    static FONT: OnceLock<Font> = OnceLock::new();
    FONT.get_or_init(|| Font {
        name: BUILTIN_FONT.into(),
        width: 8 * UNIT,
        height: 12 * UNIT,
        offset: 3 * UNIT,
        chars: GLYPHS
            .iter()
            .map(|(c, s)| (*c, strokes(s)))
            .collect::<BTreeMap<_, _>>(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::Place;
    use crate::model::Text;
    use crate::shape::text_shape;

    #[test]
    fn every_printable_character_but_space_has_strokes() {
        let f = builtin_font();
        for c in '!'..='~' {
            let s = f.chars.get(&c).unwrap_or_else(|| panic!("no glyph {c:?}"));
            assert!(!s.is_empty(), "{c:?}");
        }
        assert!(!f.chars.contains_key(&' '));
        assert_eq!(f.chars.len(), 94);
    }

    #[test]
    fn glyphs_lie_inside_their_box_and_differ() {
        let f = builtin_font();
        for (c, s) in &f.chars {
            for st in s {
                for p in [st.a, st.b] {
                    assert!(
                        (0..=f.width).contains(&p.x) && (0..=f.height).contains(&p.y),
                        "{c:?} {p:?}"
                    );
                }
                assert_ne!(st.a, st.b, "{c:?} has a stroke of no length");
            }
        }
        let mut seen = std::collections::BTreeMap::new();
        for (c, s) in &f.chars {
            if let Some(other) = seen.insert(format!("{s:?}"), *c) {
                panic!("{c:?} draws as {other:?}");
            }
        }
    }

    #[test]
    fn a_designator_draws_with_no_gap() {
        let t = Text {
            font: BUILTIN_FONT.into(),
            place: Place::IDENTITY,
            width: 800_000,
            height: 1_000_000,
            line_width: 150_000,
            string: "R10 C1_x".into(),
        };
        let s = text_shape(&t, builtin_font()).unwrap();
        assert!(s.missing.is_empty());
        let b = s.bounds().unwrap();
        // Half the line width beyond the box, and a nanometre of the pen
        // outline's rounding.
        assert!(b.max.y <= 1_000_000 + 75_001 && b.min.y >= -75_001, "{b:?}");
        let t = Text {
            string: "Ω".into(),
            ..t
        };
        assert_eq!(text_shape(&t, builtin_font()).unwrap().missing, vec!['Ω']);
    }
}
