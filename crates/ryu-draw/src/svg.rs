//! SVG of one sheet: its items in order, in millimetres with y down as
//! SVG has it (the drawing's y up flipped about the sheet's height), each
//! number written to four decimals (a tenth of a micrometre), text in
//! Inter by name. The same items give the same bytes everywhere.

use crate::geom::{P, Path, Piece};
use crate::item::{Anchor, Item};
use crate::style::Colour;
use std::fmt::Write as _;

/// A number to four decimals, `-0` written `0`, trailing zeros dropped.
pub fn num(v: f64) -> String {
    let r = (v * 10_000.0).round() / 10_000.0;
    let r = if r == 0.0 { 0.0 } else { r };
    let mut s = format!("{r:.4}");
    while s.contains('.') && (s.ends_with('0') || s.ends_with('.')) {
        s.pop();
    }
    s
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}

/// A path's `d`, y flipped about `h`.
fn d(path: &Path, h: f64) -> String {
    let q = |p: P| format!("{} {}", num(p[0]), num(h - p[1]));
    let mut out = format!("M{}", q(path.start));
    for piece in path.cubics() {
        match piece {
            Piece::Line(p) => {
                let _ = write!(out, " L{}", q(p));
            }
            Piece::Cubic([a, b, e]) => {
                let _ = write!(out, " C{} {} {}", q(a), q(b), q(e));
            }
        }
    }
    if path.closed {
        out.push_str(" Z");
    }
    out
}

/// The SVG of a sheet `size` (in millimetres) drawing `items` on
/// `background` (none: transparent).
pub fn write(size: P, items: &[Item], background: Option<Colour>) -> String {
    let (w, h) = (size[0], size[1]);
    let mut out = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}mm\" height=\"{h}mm\" viewBox=\"0 0 {w} {h}\">\n",
        w = num(w),
        h = num(h)
    );
    if let Some(b) = background {
        let _ = writeln!(
            out,
            "<rect x=\"0\" y=\"0\" width=\"{}\" height=\"{}\" fill=\"{}\"/>",
            num(w),
            num(h),
            b.to_hex()
        );
    }
    for it in items {
        match it {
            Item::Path { path, stroke, fill } => {
                let mut attrs = match fill {
                    Some(f) => format!(" fill=\"{}\"", f.to_hex()),
                    None => " fill=\"none\"".to_owned(),
                };
                if let Some(s) = stroke {
                    let _ = write!(
                        attrs,
                        " stroke=\"{}\" stroke-width=\"{}\" stroke-linecap=\"round\" stroke-linejoin=\"round\"",
                        s.colour.to_hex(),
                        num(s.width)
                    );
                    if !s.dash.is_empty() {
                        let dash: Vec<String> = s.dash.iter().map(|v| num(*v)).collect();
                        let _ = write!(attrs, " stroke-dasharray=\"{}\"", dash.join(" "));
                    }
                }
                let _ = writeln!(out, "<path d=\"{}\"{attrs}/>", d(path, h));
            }
            Item::Text {
                text,
                at,
                size,
                angle,
                anchor,
                colour,
            } => {
                let anchor = match anchor {
                    Anchor::Start => "start",
                    Anchor::Middle => "middle",
                    Anchor::End => "end",
                };
                let (x, y) = (num(at[0]), num(h - at[1]));
                let turn = if *angle == 0.0 {
                    String::new()
                } else {
                    format!(
                        " transform=\"rotate({} {x} {y})\"",
                        num(-angle.to_degrees())
                    )
                };
                let _ = writeln!(
                    out,
                    "<text x=\"{x}\" y=\"{y}\" font-family=\"Inter\" font-size=\"{}\" text-anchor=\"{anchor}\" fill=\"{}\"{turn}>{}</text>",
                    num(*size),
                    colour.to_hex(),
                    escape(text)
                );
            }
        }
    }
    out.push_str("</svg>\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::{LineKind, Standard};

    #[test]
    fn numbers_are_short_and_never_minus_zero() {
        assert_eq!(num(1.0), "1");
        assert_eq!(num(-0.00001), "0");
        assert_eq!(num(2.5), "2.5");
        assert_eq!(num(1.23456), "1.2346");
    }

    #[test]
    fn a_sheet_draws_its_paths_and_text_flipped() {
        let items = vec![
            Item::Path {
                path: Path::polyline(&[[0.0, 0.0], [10.0, 0.0]], false),
                stroke: Some(Standard::Iso.stroke(LineKind::Hidden)),
                fill: None,
            },
            Item::Text {
                text: "A<B".into(),
                at: [5.0, 20.0],
                size: 3.5,
                angle: std::f64::consts::FRAC_PI_2,
                anchor: Anchor::Middle,
                colour: Colour::BLACK,
            },
        ];
        let s = write([210.0, 297.0], &items, Some(Colour::WHITE));
        assert!(s.contains("viewBox=\"0 0 210 297\""), "{s}");
        assert!(s.contains("d=\"M0 297 L10 297\""), "{s}");
        assert!(s.contains("stroke-dasharray=\"3 0.75\""), "{s}");
        assert!(s.contains("y=\"277\""), "{s}");
        assert!(s.contains("rotate(-90 5 277)"), "{s}");
        assert!(s.contains(">A&lt;B</text>"), "{s}");
        assert_eq!(s, write([210.0, 297.0], &items, Some(Colour::WHITE)));
    }
}
