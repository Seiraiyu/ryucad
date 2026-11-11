//! A PDF of sheets: one page per sheet at its paper's size, each with a
//! plain border and a title block (plan 2.E), its drawing what `ryusch::paint` says the
//! sheet draws, written through `ryu_draw::pdf` (Inter subset and
//! embedded, links between pages and bookmarks; design 417 moved the
//! writer there, every byte as it was). No time stamp, no random ID: the
//! same pages give the same bytes everywhere.

use ryu_draw::pdf::{Canvas, Doc};
use ryu_draw::{Colour, P};
use ryusch::font::Anchor;
use ryusch::geom::{Nm, Point};
use ryusch::paint::{DASH, Item, LINE};
use ryusch::style::Rgb;
use std::collections::BTreeSet;

pub use ryu_draw::pdf::Mark;

/// Points per nanometre: 72 to the inch.
const PT: f64 = 72.0 / 25_400_000.0;
/// Between the paper's edge and the border.
pub const FRAME: Nm = 5_000_000;

/// A link: a box on the page (in the sheet's frame) going to a page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    pub rect: ryusch::geom::Rect,
    pub to: usize,
}

/// One page: its paper, title block, where the sheet's frame sits on it,
/// what it draws (in the sheet's frame) and its links.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Page {
    pub paper: [Nm; 2],
    pub title: TitleBlock,
    /// The sheet's lower left corner on the paper.
    pub origin: Point,
    pub items: Vec<Item>,
    pub links: Vec<Link>,
}

/// What a page's title block says (plan 2.E): the style's fields filled
/// from properties, and the page's own design, block and sheet.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TitleBlock {
    pub title: String,
    pub number: String,
    pub organisation: String,
    /// `<design> · <block>` and the layer, when a stack's.
    pub design_block: String,
    pub revision: String,
    pub date: String,
    /// `n/m` through the document.
    pub sheet: String,
}

/// The title block's width and height.
pub const TITLE_W: Nm = 140_000_000;
pub const TITLE_H: Nm = 16_000_000;
const ROW: Nm = 7_000_000;
const LABEL: Nm = 1_500_000;
const VALUE: Nm = 2_500_000;
const TITLE_VALUE: Nm = 3_500_000;

/// The title block on a page of `paper`, in the drawing frame's lower
/// right: a top row (Title, Number) and a bottom row (Organisation,
/// Design · block, Rev, Date, Sheet), each cell's label at its top left
/// and its value under it.
pub fn title_items(paper: [Nm; 2], t: &TitleBlock) -> Vec<Item> {
    use ryusch::style::{Look, Role};
    let look = Look::default();
    let ink = look.role(Role::Text);
    let x1 = paper[0] - crate::schem::BORDER;
    let x0 = x1 - TITLE_W;
    let y0 = crate::schem::BORDER;
    let y1 = y0 + TITLE_H;
    let mid = y0 + ROW;
    let mm = |v: Nm| v * 1_000_000;
    let line = |pts: Vec<Point>| Item::Polyline {
        points: pts,
        stroke: ink,
        fill: None,
        width: LINE,
        dash: false,
    };
    let mut out = vec![
        line(vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1], [x0, y0]]),
        line(vec![[x0, mid], [x1, mid]]),
    ];
    let mut cell = |x: Nm, bottom: Nm, top: Nm, label: &str, value: &str, size: Nm, bold: bool| {
        if x > x0 {
            out.push(line(vec![[x, bottom], [x, top]]));
        }
        out.push(Item::Text {
            text: label.to_owned(),
            at: [x + mm(1), top - mm(1) - LABEL],
            size: LABEL,
            anchor: Anchor::Start,
            vertical: false,
            colour: look.role(Role::Name),
            bold: false,
        });
        out.push(Item::Text {
            text: value.to_owned(),
            at: [x + mm(1), bottom + mm(1) + VALUE / 4],
            size,
            anchor: Anchor::Start,
            vertical: false,
            colour: ink,
            bold,
        });
    };
    cell(x0, mid, y1, "Title", &t.title, TITLE_VALUE, true);
    cell(x0 + mm(90), mid, y1, "Number", &t.number, VALUE, false);
    let mut x = x0;
    for (w, label, value) in [
        (40, "Organisation", &t.organisation),
        (50, "Design · block", &t.design_block),
        (15, "Rev", &t.revision),
        (20, "Date", &t.date),
        (15, "Sheet", &t.sheet),
    ] {
        cell(x, y0, mid, label, value, VALUE, false);
        x += mm(w);
    }
    out
}

/// Row letters: ISO 5457 leaves out I and O.
const LETTERS: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ";
/// A zone's size, about.
const ZONE: Nm = 50_000_000;

/// How many zones a page of `paper` has across and down the drawing
/// frame (inside the border): about 50 mm each, at least 2.
pub fn zones(paper: [Nm; 2]) -> (usize, usize) {
    let b = crate::schem::BORDER;
    let n = |len: Nm| (((len + ZONE / 2) / ZONE) as usize).max(2);
    (n(paper[0] - 2 * b), n(paper[1] - 2 * b))
}

/// The zone at `at` on the page (its frame, y up): row letter from the
/// top, column number from the left; outside the frame, the nearest.
pub fn zone_at(paper: [Nm; 2], at: Point) -> (usize, usize) {
    let b = crate::schem::BORDER;
    let (cols, rows) = zones(paper);
    let (w, h) = (paper[0] - 2 * b, paper[1] - 2 * b);
    let col = ((at[0] - b).clamp(0, w - 1) as i128 * cols as i128 / w as i128) as usize;
    let row = ((paper[1] - b - at[1]).clamp(0, h - 1) as i128 * rows as i128 / h as i128) as usize;
    (row, col)
}

pub fn zone_text((row, col): (usize, usize)) -> String {
    format!("{}{}", LETTERS[row % LETTERS.len()] as char, col + 1)
}

pub fn zone_of(paper: [Nm; 2], at: Point) -> String {
    zone_text(zone_at(paper, at))
}

/// The drawing frame and the zone marks between it and the page's border:
/// a tick at each division, each zone's number (columns) or letter (rows)
/// in both margins.
pub fn zone_items(paper: [Nm; 2]) -> Vec<Item> {
    use ryusch::style::{Look, Role};
    let look = Look::default();
    let ink = look.role(Role::Name);
    let b = crate::schem::BORDER;
    let (cols, rows) = zones(paper);
    let (w, h) = (paper[0] - 2 * b, paper[1] - 2 * b);
    let size = 2_500_000;
    let line = |pts: Vec<Point>| Item::Polyline {
        points: pts,
        stroke: ink,
        fill: None,
        width: LINE,
        dash: false,
    };
    let text = |t: String, at: Point| Item::Text {
        text: t,
        at,
        size,
        anchor: Anchor::Middle,
        vertical: false,
        colour: ink,
        bold: false,
    };
    let mut out = vec![line(vec![
        [b, b],
        [b + w, b],
        [b + w, b + h],
        [b, b + h],
        [b, b],
    ])];
    let margin_mid = (FRAME + b) / 2;
    for i in 0..cols {
        let x = b + (w as i128 * i as i128 / cols as i128) as Nm;
        let next = b + (w as i128 * (i + 1) as i128 / cols as i128) as Nm;
        if i > 0 {
            out.push(line(vec![[x, FRAME], [x, b]]));
            out.push(line(vec![[x, paper[1] - b], [x, paper[1] - FRAME]]));
        }
        let mid = (x + next) / 2;
        for y in [margin_mid - size / 3, paper[1] - margin_mid - size / 3] {
            out.push(text((i + 1).to_string(), [mid, y]));
        }
    }
    for j in 0..rows {
        let top = paper[1] - b - (h as i128 * j as i128 / rows as i128) as Nm;
        let next = paper[1] - b - (h as i128 * (j + 1) as i128 / rows as i128) as Nm;
        if j > 0 {
            out.push(line(vec![[FRAME, top], [b, top]]));
            out.push(line(vec![[paper[0] - b, top], [paper[0] - FRAME, top]]));
        }
        let mid = (top + next) / 2 - size / 3;
        let l = (LETTERS[j % LETTERS.len()] as char).to_string();
        for x in [margin_mid, paper[0] - margin_mid] {
            out.push(text(l.clone(), [x, mid]));
        }
    }
    out
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
        for it in zone_items(p.paper)
            .into_iter()
            .chain(title_items(p.paper, &p.title))
        {
            if let Item::Text { text, .. } = it {
                out.extend(text.chars());
            }
        }
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

fn colour(c: Rgb) -> Colour {
    Colour(c.0, c.1, c.2)
}

/// Draws one item at `origin` on the page.
fn draw(c: &mut Canvas, origin: Point, it: &Item) {
    let at = |q: Point| on(origin, q);
    let atf = |q: [f64; 2]| [origin[0] as f64 + q[0], origin[1] as f64 + q[1]];
    let stroke = |c: &mut Canvas, s: Rgb, w: Nm| {
        c.line_width(w as f64);
        c.stroke_colour(colour(s));
    };
    let fill = |c: &mut Canvas, f: Rgb| c.fill_colour(colour(f));
    match it {
        Item::Bus {
            points,
            stroke: s,
            width,
        } => {
            let Some(first) = points.first() else { return };
            stroke(c, *s, *width);
            c.move_to(at(*first));
            for p in &points[1..] {
                c.line_to(at(*p));
            }
            c.stroke();
        }
        Item::Polyline {
            points,
            stroke: s,
            fill: f,
            width,
            dash,
        } => {
            let Some(first) = points.first() else { return };
            stroke(c, *s, *width);
            if *dash {
                c.dash(&[DASH[0] as f64, DASH[1] as f64]);
            }
            c.move_to(at(*first));
            for p in &points[1..] {
                c.line_to(at(*p));
            }
            match f {
                Some(f) => {
                    fill(c, *f);
                    c.fill_and_stroke();
                }
                None => {
                    c.stroke();
                }
            }
            if *dash {
                c.dash(&[]);
            }
        }
        Item::Circle {
            center,
            radius,
            stroke: s,
            fill: f,
            width,
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
                stroke(c, *s, *width);
            }
            match (s, f) {
                (Some(_), Some(f)) => {
                    fill(c, *f);
                    c.fill_and_stroke();
                }
                (Some(_), None) => {
                    c.stroke();
                }
                (None, Some(f)) => {
                    fill(c, *f);
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
            width,
        } => {
            stroke(c, *s, *width);
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
        Item::Frame {
            rect,
            title_tab,
            stroke: s,
            paper,
            width,
            frame,
        } => {
            let rect_path = |c: &mut Canvas, r: &ryusch::geom::Rect| {
                c.move_to(at([r.min[0], r.min[1]]));
                c.line_to(at([r.max[0], r.min[1]]));
                c.line_to(at([r.max[0], r.max[1]]));
                c.line_to(at([r.min[0], r.max[1]]));
                c.close();
            };
            stroke(c, *s, *width);
            if *frame {
                let [on_, off] = DASH;
                c.dash(&[on_ as f64, off as f64]);
                rect_path(c, rect);
                c.stroke();
                c.dash(&[]);
            }
            fill(c, *paper);
            rect_path(c, title_tab);
            c.fill_and_stroke();
        }
        Item::Bezier {
            points,
            stroke: s,
            width,
        } => {
            stroke(c, *s, *width);
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
            colour: col,
            // SemiBold from Task 22 (plan 2.E); Regular until then.
            bold: _,
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
            fill(c, *col);
            let dir = if *vertical { [0.0, 1.0] } else { [1.0, 0.0] };
            c.text(text, at([px, py]), *size as f64, dir);
        }
        Item::Begin(_) | Item::End => {}
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
        // The border, then the title block in the band at its foot.
        c.stroke_colour(Colour::BLACK);
        c.rect(
            [FRAME as f64, FRAME as f64],
            [
                (page.paper[0] - 2 * FRAME) as f64,
                (page.paper[1] - 2 * FRAME) as f64,
            ],
        );
        c.stroke();
        for it in zone_items(page.paper) {
            draw(c, [0, 0], &it);
        }
        for it in title_items(page.paper, &page.title) {
            draw(c, [0, 0], &it);
        }
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
            title: TitleBlock {
                title: "Sensor board".into(),
                number: "SB-1".into(),
                organisation: "Seiraiyu".into(),
                design_block: format!("design · top · {text}"),
                revision: "B".into(),
                date: "2026-11-11".into(),
                sheet: "1/2".into(),
            },
            origin: [10_000_000, 10_000_000],
            items: vec![
                Item::Polyline {
                    points: vec![[0, 0], [10_000_000, 0]],
                    stroke: Rgb(0x1f, 0x5f, 0xbf),
                    fill: None,
                    width: LINE,
                    dash: false,
                },
                Item::Arc {
                    start: [0, 0],
                    mid: [5_000_000, 5_000_000],
                    end: [10_000_000, 0],
                    stroke: Rgb(0x3a, 0x3f, 0x47),
                    width: LINE,
                },
                Item::Circle {
                    center: [20_000_000, 0],
                    radius: 317_500,
                    stroke: None,
                    fill: Some(Rgb(0x1f, 0x5f, 0xbf)),
                    width: LINE,
                },
                Item::Text {
                    text: "R1".into(),
                    at: [0, 20_000_000],
                    size: 1_270_000,
                    anchor: Anchor::Start,
                    vertical: true,
                    colour: Rgb(0x20, 0x23, 0x28),
                    bold: false,
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
        let items = title_items([420_000_000, 297_000_000], &pages[0].title);
        let texts: Vec<&str> = items
            .iter()
            .filter_map(|i| match i {
                Item::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        for t in [
            "Title",
            "Sensor board",
            "Number",
            "SB-1",
            "Rev",
            "B",
            "Date",
            "2026-11-11",
            "Sheet",
            "1/2",
        ] {
            assert!(texts.contains(&t), "{t} in {texts:?}");
        }
        // Inside the drawing frame's lower right, in the band.
        let r = items
            .iter()
            .filter_map(|i| match i {
                Item::Polyline { points, .. } => ryusch::geom::Rect::of(points.iter().copied()),
                _ => None,
            })
            .reduce(ryusch::geom::Rect::union)
            .unwrap();
        assert_eq!(
            r.max,
            [
                420_000_000 - crate::schem::BORDER,
                crate::schem::BORDER + TITLE_H
            ]
        );
        assert!(r.max[1] <= crate::schem::BORDER + crate::schem::TITLE_BAND);
    }

    #[test]
    fn zones_are_about_fifty_millimetres_lettered_from_the_top() {
        let a4 = [297_000_000, 210_000_000];
        let a3 = [420_000_000, 297_000_000];
        assert_eq!(zones(a4), (6, 4));
        assert_eq!(zones(a3), (8, 6));
        let b = crate::schem::BORDER;
        assert_eq!(zone_of(a4, [b + 1, 210_000_000 - b - 1]), "A1");
        assert_eq!(zone_of(a4, [297_000_000 - b - 1, b + 1]), "D6");
        assert_eq!(zone_of(a4, [0, 0]), "D1", "outside the frame clamps");
        let items = zone_items(a4);
        let labels: Vec<&str> = items
            .iter()
            .filter_map(|i| match i {
                Item::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        // Each number and letter twice (both margins).
        assert_eq!(labels.iter().filter(|t| **t == "6").count(), 2);
        assert_eq!(labels.iter().filter(|t| **t == "D").count(), 2);
        assert!(!labels.contains(&"I"));
    }
}
