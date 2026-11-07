//! A PDF of sheets: one page per sheet at its paper's size, each with a
//! plain border and a heading, its drawing what `ryusch::paint` says the
//! sheet draws, its text real (Inter, subset and embedded as a CID font
//! with a ToUnicode map, so it can be searched and selected), with links
//! between pages and bookmarks. No time stamp, no random ID: the same
//! pages give the same bytes everywhere.

use crate::schemcmd::INTER;
use pdf_writer::types::{
    ActionType, AnnotationType, CidFontType, FontFlags, LineCapStyle, LineJoinStyle, SystemInfo,
};
use pdf_writer::{Content, Filter, Finish, Name, Pdf, Rect, Ref, Str, TextStr};
use ryusch::font::{Anchor, MIN_SIZE};
use ryusch::geom::{Nm, Point};
use ryusch::paint::{Item, LINE};
use std::collections::{BTreeMap, BTreeSet};

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

/// A bookmark: its title, the page it opens and the bookmarks under it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mark {
    pub title: String,
    pub page: usize,
    pub children: Vec<Mark>,
}

fn pt(v: Nm) -> f32 {
    (v as f64 * PT) as f32
}

fn rgb(hex: &str) -> (f32, f32, f32) {
    let c = |i: usize| {
        let v = u8::from_str_radix(hex.get(i..i + 2).unwrap_or("00"), 16).unwrap_or(0);
        f32::from(v) / 255.0
    };
    (c(1), c(3), c(5))
}

/// The font as the PDF holds it: each character's glyph, renumbered.
struct Font {
    face_units: f64,
    gids: BTreeMap<char, u16>,
    remap: subsetter::GlyphRemapper,
}

impl Font {
    fn new(chars: &BTreeSet<char>) -> Font {
        let face = ttf_parser::Face::parse(INTER, 0).expect("the bundled font reads");
        let mut remap = subsetter::GlyphRemapper::new();
        let mut gids = BTreeMap::new();
        for c in chars {
            let g = face.glyph_index(*c).map_or(0, |g| g.0);
            gids.insert(*c, remap.remap(g));
        }
        Font {
            face_units: f64::from(face.units_per_em()),
            gids,
            remap,
        }
    }

    /// `text` as two-byte CIDs.
    fn encode(&self, text: &str) -> Vec<u8> {
        text.chars()
            .flat_map(|c| self.gids.get(&c).copied().unwrap_or(0).to_be_bytes())
            .collect()
    }
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

/// The cubic Béziers of the arc about `centre` of radius `r` from angle
/// `from` turning by `sweep`, each a quarter turn at most, as
/// `(control, control, end)`.
fn arc_curves(centre: [f64; 2], r: f64, from: f64, sweep: f64) -> Vec<[[f64; 2]; 3]> {
    let n = (sweep.abs() / (std::f64::consts::FRAC_PI_2))
        .ceil()
        .max(1.0) as usize;
    let step = sweep / n as f64;
    let k = 4.0 / 3.0 * ryu_math::tan(step / 4.0);
    let at = |t: f64| {
        let (s, c) = ryu_math::sin_cos(t);
        ([centre[0] + r * c, centre[1] + r * s], [-s, c])
    };
    (0..n)
        .map(|i| {
            let (t0, t1) = (from + step * i as f64, from + step * (i + 1) as f64);
            let ((p0, d0), (p1, d1)) = (at(t0), at(t1));
            [
                [p0[0] + k * r * d0[0], p0[1] + k * r * d0[1]],
                [p1[0] - k * r * d1[0], p1[1] - k * r * d1[1]],
                p1,
            ]
        })
        .collect()
}

/// Draws one item at `origin` on the page.
fn draw(c: &mut Content, font: &Font, origin: Point, it: &Item) {
    let x = |v: Nm| pt(origin[0] + v);
    let y = |v: Nm| pt(origin[1] + v);
    let xf = |v: f64| ((origin[0] as f64 + v) * PT) as f32;
    let yf = |v: f64| ((origin[1] as f64 + v) * PT) as f32;
    let stroke = |c: &mut Content, s: &str| {
        let (r, g, b) = rgb(s);
        c.set_stroke_rgb(r, g, b);
    };
    let fill = |c: &mut Content, s: &str| {
        let (r, g, b) = rgb(s);
        c.set_fill_rgb(r, g, b);
    };
    match it {
        Item::Polyline {
            points,
            stroke: s,
            fill: f,
        } => {
            let Some(first) = points.first() else { return };
            stroke(c, s);
            c.move_to(x(first[0]), y(first[1]));
            for p in &points[1..] {
                c.line_to(x(p[0]), y(p[1]));
            }
            match f {
                Some(f) => {
                    fill(c, f);
                    c.fill_nonzero_and_stroke();
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
            let curves = arc_curves(ctr, r, 0.0, 2.0 * std::f64::consts::PI);
            c.move_to(xf(ctr[0] + r), yf(ctr[1]));
            for [a, b, e] in curves {
                c.cubic_to(xf(a[0]), yf(a[1]), xf(b[0]), yf(b[1]), xf(e[0]), yf(e[1]));
            }
            c.close_path();
            if let Some(s) = s {
                stroke(c, s);
            }
            match (s, f) {
                (Some(_), Some(f)) => {
                    fill(c, f);
                    c.fill_nonzero_and_stroke();
                }
                (Some(_), None) => {
                    c.stroke();
                }
                (None, Some(f)) => {
                    fill(c, f);
                    c.fill_nonzero();
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
                c.move_to(x(start[0]), y(start[1]));
                c.line_to(x(mid[0]), y(mid[1]));
                c.line_to(x(end[0]), y(end[1]));
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
            c.move_to(x(start[0]), y(start[1]));
            for [a, b, e] in arc_curves(ctr, r, a0, sweep) {
                c.cubic_to(xf(a[0]), yf(a[1]), xf(b[0]), yf(b[1]), xf(e[0]), yf(e[1]));
            }
            c.stroke();
        }
        Item::Frame { rect, title_tab } => {
            let rect_path = |c: &mut Content, r: &ryusch::geom::Rect| {
                c.move_to(x(r.min[0]), y(r.min[1]));
                c.line_to(x(r.max[0]), y(r.min[1]));
                c.line_to(x(r.max[0]), y(r.max[1]));
                c.line_to(x(r.min[0]), y(r.max[1]));
                c.close_path();
            };
            stroke(c, ryusch::paint::FRAME);
            let [on, off] = ryusch::paint::DASH;
            c.set_dash_pattern([pt(on), pt(off)], 0.0);
            rect_path(c, rect);
            c.stroke();
            c.set_dash_pattern([], 0.0);
            fill(c, ryusch::paint::PAPER);
            rect_path(c, title_tab);
            c.fill_nonzero_and_stroke();
        }
        Item::Bezier { points, stroke: s } => {
            stroke(c, s);
            let [a, b, d, e] = *points;
            c.move_to(x(a[0]), y(a[1]));
            c.cubic_to(x(b[0]), y(b[1]), x(d[0]), y(d[1]), x(e[0]), y(e[1]));
            c.stroke();
        }
        Item::Text {
            text,
            at,
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
                (at[0], at[1] - shift)
            } else {
                (at[0] - shift, at[1])
            };
            fill(c, colour);
            c.begin_text();
            c.set_font(Name(b"F1"), pt(*size));
            let m = if *vertical {
                [0.0, 1.0, -1.0, 0.0, x(px), y(py)]
            } else {
                [1.0, 0.0, 0.0, 1.0, x(px), y(py)]
            };
            c.set_text_matrix(m);
            c.show(Str(&font.encode(text)));
            c.end_text();
        }
    }
}

/// The six letters a subset font's name starts with: from the glyphs it
/// holds, so the same pages name it alike.
fn subset_tag(font: &Font) -> String {
    let mut h = blake3::Hasher::new();
    for (c, g) in &font.gids {
        h.update(&u32::from(*c).to_le_bytes());
        h.update(&g.to_le_bytes());
    }
    h.finalize().as_bytes()[..6]
        .iter()
        .map(|b| char::from(b'A' + b % 26))
        .collect()
}

/// The PDF of `pages`, with `marks` as its bookmarks and `title` its
/// document title.
pub fn write(title: &str, pages: &[Page], marks: &[Mark]) -> Vec<u8> {
    let font = Font::new(&chars(pages));
    let mut pdf = Pdf::new();
    let mut next = 1;
    let mut id = || {
        let r = Ref::new(next);
        next += 1;
        r
    };
    let catalog = id();
    let tree = id();
    let outline = id();
    let type0 = id();
    let cid = id();
    let descriptor = id();
    let file = id();
    let unicode = id();
    let info = id();
    let page_ids: Vec<Ref> = pages.iter().map(|_| id()).collect();
    let content_ids: Vec<Ref> = pages.iter().map(|_| id()).collect();
    let mut cat = pdf.catalog(catalog);
    cat.pages(tree);
    if !marks.is_empty() {
        cat.outlines(outline);
    }
    cat.finish();
    pdf.pages(tree)
        .kids(page_ids.iter().copied())
        .count(pages.len() as i32);
    pdf.document_info(info)
        .title(TextStr(title))
        .creator(TextStr("ryueda"));
    for (i, page) in pages.iter().enumerate() {
        let annots: Vec<Ref> = page.links.iter().map(|_| id()).collect();
        let mut w = pdf.page(page_ids[i]);
        w.media_box(Rect::new(0.0, 0.0, pt(page.paper[0]), pt(page.paper[1])));
        w.parent(tree);
        w.contents(content_ids[i]);
        if !annots.is_empty() {
            w.annotations(annots.iter().copied());
        }
        w.resources().fonts().pair(Name(b"F1"), type0);
        w.finish();
        for (link, aid) in page.links.iter().zip(&annots) {
            let r = link.rect;
            let o = page.origin;
            let mut a = pdf.annotation(*aid);
            a.subtype(AnnotationType::Link);
            a.rect(Rect::new(
                pt(o[0] + r.min[0]),
                pt(o[1] + r.min[1]),
                pt(o[0] + r.max[0]),
                pt(o[1] + r.max[1]),
            ));
            a.border(0.0, 0.0, 0.0, None);
            a.action()
                .action_type(ActionType::GoTo)
                .destination()
                .page(page_ids[link.to])
                .fit();
            a.finish();
        }
        let mut c = Content::new();
        c.set_line_width(pt(LINE));
        c.set_line_cap(LineCapStyle::RoundCap);
        c.set_line_join(LineJoinStyle::RoundJoin);
        // The border, then the heading in the band above the sheet.
        c.set_stroke_rgb(0.0, 0.0, 0.0);
        c.rect(
            pt(FRAME),
            pt(FRAME),
            pt(page.paper[0] - 2 * FRAME),
            pt(page.paper[1] - 2 * FRAME),
        );
        c.stroke();
        draw(
            &mut c,
            &font,
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
            draw(&mut c, &font, page.origin, it);
        }
        pdf.stream(content_ids[i], &c.finish());
    }
    // Bookmarks: depth first, each item's links to its parent and siblings.
    if !marks.is_empty() {
        fn count(m: &[Mark]) -> i32 {
            m.iter().map(|x| 1 + count(&x.children)).sum()
        }
        let mut refs: Vec<Ref> = Vec::new();
        fn assign(m: &[Mark], id: &mut dyn FnMut() -> Ref, out: &mut Vec<Ref>) {
            for x in m {
                out.push(id());
                assign(&x.children, id, out);
            }
        }
        assign(marks, &mut id, &mut refs);
        let mut at = 0;
        #[allow(clippy::too_many_arguments)]
        fn emit(
            pdf: &mut Pdf,
            m: &[Mark],
            parent: Ref,
            refs: &[Ref],
            at: &mut usize,
            pages: &[Ref],
        ) -> Vec<Ref> {
            let mine: Vec<(usize, &Mark)> = m
                .iter()
                .map(|x| {
                    let i = *at;
                    *at += 1 + count(&x.children) as usize;
                    (i, x)
                })
                .collect();
            let ids: Vec<Ref> = mine.iter().map(|(i, _)| refs[*i]).collect();
            for (k, (i, x)) in mine.iter().enumerate() {
                let mut sub_at = i + 1;
                let kids = emit(pdf, &x.children, refs[*i], refs, &mut sub_at, pages);
                let mut o = pdf.outline_item(refs[*i]);
                o.title(TextStr(&x.title)).parent(parent);
                if k > 0 {
                    o.prev(ids[k - 1]);
                }
                if k + 1 < ids.len() {
                    o.next(ids[k + 1]);
                }
                if let (Some(f), Some(l)) = (kids.first(), kids.last()) {
                    o.first(*f).last(*l).count(-(kids.len() as i32));
                }
                o.dest().page(pages[x.page]).fit();
                o.finish();
            }
            ids
        }
        let top = emit(&mut pdf, marks, outline, &refs, &mut at, &page_ids);
        let mut o = pdf.outline(outline);
        o.first(top[0])
            .last(*top.last().expect("some"))
            .count(count(marks));
        o.finish();
    }
    // The font: Type 0 over a CID font of the subset, Identity-H, with a
    // ToUnicode map so text reads back.
    let face = ttf_parser::Face::parse(INTER, 0).expect("the bundled font reads");
    let units = font.face_units;
    let scale = |v: f64| (v * 1000.0 / units) as f32;
    let name = format!("{}+Inter-Regular", subset_tag(&font));
    let system = SystemInfo {
        registry: Str(b"Adobe"),
        ordering: Str(b"Identity"),
        supplement: 0,
    };
    pdf.type0_font(type0)
        .base_font(Name(name.as_bytes()))
        .encoding_predefined(Name(b"Identity-H"))
        .descendant_font(cid)
        .to_unicode(unicode);
    let mut widths: Vec<(u16, f32)> = Vec::new();
    for g in font.remap.remapped_gids() {
        let new = font.remap.get(g).expect("remapped");
        let adv = face
            .glyph_hor_advance(ttf_parser::GlyphId(g))
            .map_or(0.0, |a| scale(f64::from(a)));
        widths.push((new, adv));
    }
    widths.sort_by_key(|(n, _)| *n);
    let mut cf = pdf.cid_font(cid);
    cf.subtype(CidFontType::Type2)
        .base_font(Name(name.as_bytes()))
        .system_info(system)
        .font_descriptor(descriptor)
        .default_width(0.0)
        .cid_to_gid_map_predefined(Name(b"Identity"));
    cf.widths().consecutive(0, widths.iter().map(|(_, w)| *w));
    cf.finish();
    let bb = face.global_bounding_box();
    pdf.font_descriptor(descriptor)
        .name(Name(name.as_bytes()))
        .flags(FontFlags::NON_SYMBOLIC)
        .bbox(Rect::new(
            scale(f64::from(bb.x_min)),
            scale(f64::from(bb.y_min)),
            scale(f64::from(bb.x_max)),
            scale(f64::from(bb.y_max)),
        ))
        .italic_angle(0.0)
        .ascent(scale(f64::from(face.ascender())))
        .descent(scale(f64::from(face.descender())))
        .cap_height(scale(f64::from(
            face.capital_height().unwrap_or(face.ascender()),
        )))
        .stem_v(80.0)
        .font_file2(file);
    let data = subsetter::subset(INTER, 0, &font.remap).expect("the bundled font subsets");
    let packed = miniz(&data);
    pdf.stream(file, &packed).filter(Filter::FlateDecode);
    let mut cmap = pdf_writer::types::UnicodeCmap::new(Name(b"Custom"), system);
    for (c, g) in &font.gids {
        cmap.pair(*g, *c);
    }
    pdf.cmap(unicode, &cmap.finish());
    pdf.finish()
}

/// Deflated, as PDF's FlateDecode reads: zlib, its best level, the same
/// bytes everywhere.
fn miniz(data: &[u8]) -> Vec<u8> {
    miniz_oxide::deflate::compress_to_vec_zlib(data, 10)
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
