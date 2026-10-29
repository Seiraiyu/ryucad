//! Stroke fonts (`fonts/standard` and any other under `fonts/`).
//!
//! A font is `XSIZE`, `YSIZE` and `OFFSET`, then a block per character:
//! `CHAR c`, its `LINE xs ys xe ye pol shape width` strokes, `ECHAR`. The
//! units don't matter, since text scales the font's box to its own size;
//! the numbers are read as inches, for room.

use std::collections::BTreeMap;

use ryu_board::geom::Pen;
use ryu_board::model::{Font, FontStroke};

use super::text::{Bad, lines, tokens};
use super::units::{Nums, Units};

/// Reads a font named `name`. Negative strokes, which the model's fonts
/// don't hold, are counted in the second value and left out.
pub fn font(name: &str, text: &str) -> Result<(Font, usize), Bad> {
    let mut nums = Nums::new(Units::Inch);
    let (mut w, mut h, mut offset) = (None, None, None);
    let mut chars = BTreeMap::new();
    let mut open: Option<(char, Vec<FontStroke>, u32)> = None;
    let mut negative = 0;
    for (n, line) in lines(text) {
        if let Some(rest) = line.strip_prefix("CHAR") {
            if open.is_some() {
                return Err(Bad::new(n, 1, "CHAR inside a character"));
            }
            // `CHAR c`, `CHARc`, and `CHAR ` (a space, trimmed away).
            let rest = rest.strip_prefix(' ').unwrap_or(rest);
            let mut it = rest.chars();
            let c = it.next().unwrap_or(' ');
            if it.next().is_some() {
                return Err(Bad::new(n, 6, format!("CHAR {rest}: not one character")));
            }
            open = Some((c, Vec::new(), n));
            continue;
        }
        let toks = tokens(line).map_err(|b| Bad::new(n, b.column, b.message))?;
        let head = toks[0].text;
        let size = |nums: &mut Nums| -> Result<i64, Bad> {
            let t = toks
                .get(1)
                .ok_or_else(|| Bad::new(n, 1, format!("{head} without a size")))?;
            nums.coord(t, n)
        };
        match head {
            "XSIZE" => w = Some(size(&mut nums)?),
            "YSIZE" => h = Some(size(&mut nums)?),
            "OFFSET" => offset = Some(size(&mut nums)?),
            "LINE" => {
                let Some((_, strokes, _)) = &mut open else {
                    return Err(Bad::new(n, 1, "LINE outside a character"));
                };
                if toks.len() != 8 {
                    return Err(Bad::new(n, 1, "LINE takes seven fields"));
                }
                let a = nums.point(&toks[1], &toks[2], n)?;
                let b = nums.point(&toks[3], &toks[4], n)?;
                let width = nums.coord(&toks[7], n)?;
                let pen = match toks[6].text {
                    "R" => Pen::Round(width),
                    "S" => Pen::Square(width),
                    other => {
                        return Err(Bad::new(n, toks[6].column, format!("{other}: not R or S")));
                    }
                };
                match toks[5].text {
                    "P" => strokes.push(FontStroke { a, b, pen }),
                    "N" => negative += 1,
                    other => {
                        return Err(Bad::new(n, toks[5].column, format!("{other}: not P or N")));
                    }
                }
            }
            "ECHAR" => {
                let Some((c, strokes, _)) = open.take() else {
                    return Err(Bad::new(n, 1, "ECHAR without CHAR"));
                };
                chars.insert(c, strokes);
            }
            other => return Err(Bad::new(n, 1, format!("{other}: not a font record"))),
        }
    }
    if let Some((c, _, at)) = open {
        return Err(Bad::new(at, 1, format!("CHAR {c} without ECHAR")));
    }
    let need = |v: Option<i64>, what: &str| v.ok_or_else(|| Bad::new(1, 1, format!("no {what}")));
    let font = Font {
        name: name.to_string(),
        width: need(w, "XSIZE")?,
        height: need(h, "YSIZE")?,
        offset: need(offset, "OFFSET")?,
        chars,
    };
    Ok((font, negative))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ryu_board::geom::Point;

    fn p(x: i64, y: i64) -> Point {
        Point::new(x, y)
    }

    #[test]
    fn reads_the_standard_font() {
        let t = "XSIZE  0.302000\r\nYSIZE  0.302000\r\nOFFSET 0.000000\r\nCHAR !\r\nLINE 0.000000 0.000000 0.000000 0.200000 P R 0.012000\r\nLINE 0.000000 -0.100000 0.000000 -0.100000 P R 0.012000\r\nECHAR\r\nCHAR\nLINE 0 0 0.1 0 N S 0.01\nECHAR\nCHAR~\nLINE -0.1 0.15 -0.05 0.2 P S 0.012\nECHAR\nCHAR '\nECHAR\n";
        let (f, negative) = font("standard", t).unwrap();
        assert_eq!((f.width, f.height, f.offset), (7_670_800, 7_670_800, 0));
        assert_eq!(negative, 1);
        assert_eq!(f.chars.len(), 4);
        assert!(f.chars.contains_key(&'\''));
        assert_eq!(
            f.chars[&'!'][0],
            FontStroke {
                a: p(0, 0),
                b: p(0, 5_080_000),
                pen: Pen::Round(304_800)
            }
        );
        assert!(f.chars[&' '].is_empty());
        assert_eq!(f.chars[&'~'][0].pen, Pen::Square(304_800));
    }

    #[test]
    fn faults() {
        let at = |t: &str| font("f", t).unwrap_err().message;
        assert_eq!(at("XSIZE 1\nYSIZE 1\n"), "no OFFSET");
        assert_eq!(at("CHAR a\nLINE 0 0 1 1 P R\n"), "LINE takes seven fields");
        assert_eq!(at("CHAR a\nCHAR b\n"), "CHAR inside a character");
        assert_eq!(at("CHAR ab\n"), "CHAR ab: not one character");
        assert_eq!(at("LINE 0 0 1 1 P R 1\n"), "LINE outside a character");
        assert_eq!(at("CHAR a\nLINE 0 0 1 1 P X 1\n"), "X: not R or S");
        assert_eq!(at("CHAR a\n"), "CHAR a without ECHAR");
        assert_eq!(at("WHAT 1\n"), "WHAT: not a font record");
    }
}
