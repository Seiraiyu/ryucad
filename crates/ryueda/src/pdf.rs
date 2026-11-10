//! A PDF of sheets: one page per sheet at its paper's size, each with a
//! plain border and a heading, its drawing what `ryusch::paint` says the
//! sheet draws, written through `ryu_draw::pdf` (Inter subset and
//! embedded, links between pages and bookmarks; design 417 moved the
//! writer there, every byte as it was). No time stamp, no random ID: the
//! same pages give the same bytes everywhere.

use ryu_draw::pdf::{Canvas, Doc};
use ryu_draw::{Colour, P};
use ryusch::font::{Anchor, MIN_SIZE};
use ryusch::geom::{Nm, Point};
use ryusch::paint::{Item, LINE};
use std::collections::BTreeSet;

pub use ryu_draw::pdf::Mark;

/// Points per nanometre: 72 to the inch.
const PT: f64 = 72.0 / 25_400_000.0;
/// Between the paper's edge and the border.
pub const FRAME: Nm = 5_000_000;
/// The heading's text, in the band between the border and the sheet.
const HEADING: Nm = 2_500_000;

/// A link: a box on the page (in the sheet's frame) going to a page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    pub rect: ryusch::geom::Rect,
    pub to: usize,
}

/// One page: its paper, heading, where the sheet's frame sits on it, what
/// it draws (in the sheet's frame) and its links.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Page {
    pub paper: [Nm; 2],
    pub heading: String,
    /// The sheet's lower left corner on the paper.
    pub origin: Point,
    pub items: Vec<Item>,
    pub links: Vec<Link>,
}

/// A point of the sheet's frame on the paper, in nanometres: exact in
/// `f64` (every coordinate is far below 2^53).
fn on(origin: Point, q: Point) -> P {
    [(origin[0] + q[0]) as f64, (origin[1] + q[1]) as f64]
}

/// Every character the pages' text needs.
fn chars(pages: &[Page]) -> BTreeSet<char> {
    let mut out: BTreeSet<char> = BTreeSet::new();
    for p in pages {
        out.extend(p.heading.chars());
        for it in &p.items {
            if let Item::Text { text, .. } = it {
                out.extend(text.chars());
            }
        }
    }
    out
}

/// The circle through three points, and whether `a` to `c` through `b`
/// runs counterclockwise.
fn circle(a: Point, b: Point, c: Point) -> Option<([f64; 2], f64, bool)> {
    let f = |p: Point| [p[0] as f64, p[1] as f64];
    let (a, b, c) = (f(a), f(b), f(c));
    let d = 2.0 * (a[0] * (b[1] - c[1]) + b[0] * (c[1] - a[1]) + c[0] * (a[1] - b[1]));
    if d == 0.0 {
        return None;
    }
    let sq = |p: [f64; 2]| p[0] * p[0] + p[1] * p[1];
    let ux = (sq(a) * (b[1] - c[1]) + sq(b) * (c[1] - a[1]) + sq(c) * (a[1] - b[1])) / d;
    let uy = (sq(a) * (c[0] - b[0]) + sq(b) * (a[0] - c[0]) + sq(c) * (b[0] - a[0])) / d;
    let r = ryu_math::hypot(a[0] - ux, a[1] - uy);
    Some(([ux, uy], r, d > 0.0))
}

/// Draws one item at `origin` on the page.
fn draw(c: &mut Canvas, origin: Point, it: &Item) {
    let at = |q: Point| on(origin, q);
    let atf = |q: [f64; 2]| [origin[0] as f64 + q[0], origin[1] as f64 + q[1]];
    let stroke = |c: &mut Canvas, s: &str| c.stroke_colour(Colour::hex(s));
    let fill = |c: &mut Canvas, s: &str| c.fill_colour(Colour::hex(s));
    match it {
        Item::Polyline {
            points,
            stroke: s,
            fill: f,
        } => {
            let Some(first) = points.first() else { return };
            stroke(c, s);
            c.move_to(at(*first));
            for p in &points[1..] {
                c.line_to(at(*p));
            }
            match f {
                Some(f) => {
                    fill(c, f);
                    c.fill_and_stroke();
                }
                None => {
                    c.stroke();
                }
            }
        }
        Item::Circle {
            center,
            radius,
            stroke: s,
            fill: f,
        } => {
            let r = *radius as f64;
            let ctr = [center[0] as f64, center[1] as f64];
            let curves = ryu_draw::geom::arc_curves(ctr, r, 0.0, 2.0 * std::f64::consts::PI);
            c.move_to(atf([ctr[0] + r, ctr[1]]));
            for [a, b, e] in curves {
                c.cubic_to(atf(a), atf(b), atf(e));
            }
            c.close();
            if let Some(s) = s {
                stroke(c, s);
            }
            match (s, f) {
                (Some(_), Some(f)) => {
                    fill(c, f);
                    c.fill_and_stroke();
                }
                (Some(_), None) => {
                    c.stroke();
                }
                (None, Some(f)) => {
                    fill(c, f);
                    c.fill();
                }
                (None, None) => {}
            }
        }
        Item::Arc {
            start,
            mid,
            end,
            stroke: s,
        } => {
            stroke(c, s);
            let Some((ctr, r, ccw)) = circle(*start, *mid, *end) else {
                c.move_to(at(*start));
                c.line_to(at(*mid));
                c.line_to(at(*end));
                c.stroke();
                return;
            };
            let angle = |p: Point| ryu_math::atan2(p[1] as f64 - ctr[1], p[0] as f64 - ctr[0]);
            let (a0, a1) = (angle(*start), angle(*end));
            let tau = 2.0 * std::f64::consts::PI;
            let mut sweep = a1 - a0;
            if ccw {
                while sweep <= 0.0 {
                    sweep += tau;
                }
            } else {
                while sweep >= 0.0 {
                    sweep -= tau;
                }
            }
            c.move_to(at(*start));
            for [a, b, e] in ryu_draw::geom::arc_curves(ctr, r, a0, sweep) {
                c.cubic_to(atf(a), atf(b), atf(e));
            }
            c.stroke();
        }
        Item::Frame { rect, title_tab } => {
            let rect_path = |c: &mut Canvas, r: &ryusch::geom::Rect| {
                c.move_to(at([r.min[0], r.min[1]]));
                c.line_to(at([r.max[0], r.min[1]]));
                c.line_to(at([r.max[0], r.max[1]]));
                c.line_to(at([r.min[0], r.max[1]]));
                c.close();
            };
            stroke(c, ryusch::paint::FRAME);
            let [on_, off] = ryusch::paint::DASH;
            c.dash(&[on_ as f64, off as f64]);
            rect_path(c, rect);
            c.stroke();
            c.dash(&[]);
            fill(c, ryusch::paint::PAPER);
            rect_path(c, title_tab);
            c.fill_and_stroke();
        }
        Item::Bezier { points, stroke: s } => {
            stroke(c, s);
            let [a, b, d, e] = *points;
            c.move_to(at(a));
            c.cubic_to(at(b), at(d), at(e));
            c.stroke();
        }
        Item::Text {
            text,
            at: point,
            size,
            anchor,
            vertical,
            colour,
        } => {
            let w = ryusch::font::width(text, *size);
            let shift = match anchor {
                Anchor::Start => 0,
                Anchor::Middle => w / 2,
                Anchor::End => w,
            };
            // Along the text's own direction: up the page when vertical.
            let (px, py) = if *vertical {
                (point[0], point[1] - shift)
            } else {
                (point[0] - shift, point[1])
            };
            fill(c, colour);
            let dir = if *vertical { [0.0, 1.0] } else { [1.0, 0.0] };
            c.text(text, at([px, py]), *size as f64, dir);
        }
    }
}

/// The PDF of `pages`, with `marks` as its bookmarks and `title` its
/// document title.
pub fn write(title: &str, pages: &[Page], marks: &[Mark]) -> Vec<u8> {
    let doc = Doc {
        title,
        creator: "ryueda",
        pt: PT,
        chars: chars(pages),
    };
    let shapes: Vec<ryu_draw::pdf::Page> = pages
        .iter()
        .map(|p| ryu_draw::pdf::Page {
            size: [p.paper[0] as f64, p.paper[1] as f64],
            links: p
                .links
                .iter()
                .map(|l| ryu_draw::pdf::Link {
                    rect: [on(p.origin, l.rect.min), on(p.origin, l.rect.max)],
                    to: l.to,
                })
                .collect(),
        })
        .collect();
    ryu_draw::pdf::write(&doc, &shapes, marks, &mut |i, c| {
        let page = &pages[i];
        c.line_width(LINE as f64);
        c.content()
            .set_line_cap(pdf_writer::types::LineCapStyle::RoundCap);
        c.content()
            .set_line_join(pdf_writer::types::LineJoinStyle::RoundJoin);
        // The border, then the heading in the band above the sheet.
        c.stroke_colour(Colour::BLACK);
        c.rect(
            [FRAME as f64, FRAME as f64],
            [
                (page.paper[0] - 2 * FRAME) as f64,
                (page.paper[1] - 2 * FRAME) as f64,
            ],
        );
        c.stroke();
        draw(
            c,
            [0, 0],
            &Item::Text {
                text: page.heading.clone(),
                at: [
                    FRAME + MIN_SIZE,
                    page.paper[1] - FRAME - HEADING - MIN_SIZE / 2,
                ],
                size: HEADING,
                anchor: Anchor::Start,
                vertical: false,
                colour: "#202020",
            },
        );
        for it in &page.items {
            draw(c, page.origin, it);
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(text: &str, links: Vec<Link>) -> Page {
        Page {
            paper: [420_000_000, 297_000_000],
            heading: format!("design · top · sheet 1/2 · {text}"),
            origin: [10_000_000, 10_000_000],
            items: vec![
                Item::Polyline {
                    points: vec![[0, 0], [10_000_000, 0]],
                    stroke: "#1b6e3a",
                    fill: None,
                },
                Item::Arc {
                    start: [0, 0],
                    mid: [5_000_000, 5_000_000],
                    end: [10_000_000, 0],
                    stroke: "#7a1f1f",
                },
                Item::Circle {
                    center: [20_000_000, 0],
                    radius: 317_500,
                    stroke: None,
                    fill: Some("#1b6e3a"),
                },
                Item::Text {
                    text: "R1".into(),
                    at: [0, 20_000_000],
                    size: 1_270_000,
                    anchor: Anchor::Start,
                    vertical: true,
                    colour: "#202020",
                },
            ],
            links,
        }
    }

    #[test]
    fn pages_write_alike_with_their_text_links_and_bookmarks() {
        let pages = vec![
            page(
                "A",
                vec![Link {
                    rect: ryusch::geom::Rect::new([0, 0], [1_000_000, 1_000_000]),
                    to: 1,
                }],
            ),
            page("B", vec![]),
        ];
        let marks = vec![Mark {
            title: "top".into(),
            page: 0,
            children: vec![Mark {
                title: "Sheet 2".into(),
                page: 1,
                children: vec![],
            }],
        }];
        let a = write("design", &pages, &marks);
        assert_eq!(a, write("design", &pages, &marks));
        let text = String::from_utf8_lossy(&a);
        assert!(text.starts_with("%PDF-"));
        assert_eq!(text.matches("/Type /Page\n").count(), 2, "{text}");
        assert!(text.contains("/Subtype /Link"));
        assert!(text.contains("/GoTo"));
        assert!(text.contains("+Inter-Regular"));
        assert!(text.contains("/ToUnicode"));
        assert!(text.contains("/Title (Sheet 2)"));
        assert!(!text.contains("/CreationDate") && !text.contains("/ID"));
    }
}
