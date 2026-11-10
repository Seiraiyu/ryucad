//! PDFs of pages: each page its own size, what it draws written through a
//! [`Canvas`], its text real (Inter, subset and embedded as a CID font
//! with a ToUnicode map, so it can be searched and selected), with links
//! between pages and bookmarks. No time stamp, no random ID: the same
//! pages give the same bytes everywhere.
//!
//! ryueda's schematic PDFs (design 236) were written by this code before
//! it moved here, and keep their bytes: objects are numbered in the same
//! order, and every coordinate is `(v · pt) as f32` for a value `v` in the
//! drawing's unit and `pt` points to the unit, as it was for nanometres.

use crate::font::{INTER, Subset};
use crate::geom::{P, Piece};
use crate::item::{Anchor, Item};
use crate::style::Colour;
use pdf_writer::types::{ActionType, AnnotationType, CidFontType, FontFlags, SystemInfo};
use pdf_writer::{Content, Filter, Finish, Name, Pdf, Rect, Ref, Str, TextStr};
use std::collections::BTreeSet;

/// Points to the millimetre: 72 to the inch.
pub const PT_PER_MM: f64 = 72.0 / 25.4;

/// What every page shares.
pub struct Doc<'a> {
    /// The document's title, in its information dictionary.
    pub title: &'a str,
    /// The program that wrote it, likewise.
    pub creator: &'a str,
    /// Points to the drawing's unit.
    pub pt: f64,
    /// Every character any page's text holds: the font's subset.
    pub chars: BTreeSet<char>,
}

/// A link: a box on its page going to a page.
#[derive(Clone, Debug, PartialEq)]
pub struct Link {
    pub rect: [P; 2],
    pub to: usize,
}

/// One page: its size and its links.
#[derive(Clone, Debug, PartialEq)]
pub struct Page {
    pub size: P,
    pub links: Vec<Link>,
}

/// A bookmark: its title, the page it opens and the bookmarks under it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mark {
    pub title: String,
    pub page: usize,
    pub children: Vec<Mark>,
}

/// A page's content, in the drawing's unit.
pub struct Canvas<'a> {
    content: Content,
    font: &'a Subset,
    pt: f64,
}

impl Canvas<'_> {
    /// `v` in points.
    pub fn p(&self, v: f64) -> f32 {
        (v * self.pt) as f32
    }

    /// The content stream itself, for what the methods here don't say.
    pub fn content(&mut self) -> &mut Content {
        &mut self.content
    }

    pub fn line_width(&mut self, w: f64) {
        let w = self.p(w);
        self.content.set_line_width(w);
    }

    pub fn dash(&mut self, on_off: &[f64]) {
        let d: Vec<f32> = on_off.iter().map(|v| self.p(*v)).collect();
        self.content.set_dash_pattern(d, 0.0);
    }

    pub fn stroke_colour(&mut self, c: Colour) {
        let (r, g, b) = c.unit();
        self.content.set_stroke_rgb(r, g, b);
    }

    pub fn fill_colour(&mut self, c: Colour) {
        let (r, g, b) = c.unit();
        self.content.set_fill_rgb(r, g, b);
    }

    pub fn move_to(&mut self, q: P) {
        let (x, y) = (self.p(q[0]), self.p(q[1]));
        self.content.move_to(x, y);
    }

    pub fn line_to(&mut self, q: P) {
        let (x, y) = (self.p(q[0]), self.p(q[1]));
        self.content.line_to(x, y);
    }

    pub fn cubic_to(&mut self, a: P, b: P, e: P) {
        let v = [a[0], a[1], b[0], b[1], e[0], e[1]].map(|v| self.p(v));
        self.content.cubic_to(v[0], v[1], v[2], v[3], v[4], v[5]);
    }

    pub fn close(&mut self) {
        self.content.close_path();
    }

    /// A rectangle's path from its corner `at` and its size.
    pub fn rect(&mut self, at: P, size: P) {
        let v = [at[0], at[1], size[0], size[1]].map(|v| self.p(v));
        self.content.rect(v[0], v[1], v[2], v[3]);
    }

    pub fn stroke(&mut self) {
        self.content.stroke();
    }

    pub fn fill(&mut self) {
        self.content.fill_nonzero();
    }

    pub fn fill_and_stroke(&mut self) {
        self.content.fill_nonzero_and_stroke();
    }

    /// `text` in Inter at `size`, its baseline's start at `at`, along
    /// `dir` (a unit vector: `[1, 0]` across the page, `[0, 1]` up it), in
    /// the fill colour.
    pub fn text(&mut self, text: &str, at: P, size: f64, dir: [f32; 2]) {
        let (x, y, s) = (self.p(at[0]), self.p(at[1]), self.p(size));
        self.content.begin_text();
        self.content.set_font(Name(b"F1"), s);
        // `0.0 - y`, not `-y`: across the page that is `0`, never `-0`.
        self.content
            .set_text_matrix([dir[0], dir[1], 0.0 - dir[1], dir[0], x, y]);
        let bytes = self.font.encode(text);
        self.content.show(Str(&bytes));
        self.content.end_text();
    }

    /// The path of `pieces` from `start`.
    fn path(&mut self, start: P, pieces: &[Piece], closed: bool) {
        self.move_to(start);
        for piece in pieces {
            match piece {
                Piece::Line(q) => self.line_to(*q),
                Piece::Cubic([a, b, e]) => self.cubic_to(*a, *b, *e),
            }
        }
        if closed {
            self.close();
        }
    }

    /// Draws `items` in order: a path's stroke sets its width, pattern
    /// and colour each time, a fill its colour; text is filled, turned
    /// about its point.
    pub fn items(&mut self, items: &[Item]) {
        for it in items {
            match it {
                Item::Path { path, stroke, fill } => {
                    if stroke.is_none() && fill.is_none() {
                        continue;
                    }
                    if let Some(s) = stroke {
                        self.line_width(s.width);
                        self.dash(&s.dash);
                        self.stroke_colour(s.colour);
                    }
                    if let Some(f) = fill {
                        self.fill_colour(*f);
                    }
                    self.path(path.start, &path.cubics(), path.closed);
                    match (stroke, fill) {
                        (Some(_), Some(_)) => self.fill_and_stroke(),
                        (Some(_), None) => self.stroke(),
                        _ => self.fill(),
                    }
                }
                Item::Text {
                    text,
                    at,
                    size,
                    angle,
                    anchor,
                    colour,
                } => {
                    let w = crate::font::width(text, *size);
                    let shift = match anchor {
                        Anchor::Start => 0.0,
                        Anchor::Middle => w / 2.0,
                        Anchor::End => w,
                    };
                    let (s, c) = ryu_math::sin_cos(*angle);
                    self.fill_colour(*colour);
                    self.text(
                        text,
                        [at[0] - c * shift, at[1] - s * shift],
                        *size,
                        [c as f32, s as f32],
                    );
                }
            }
        }
    }
}

/// Every character `items`' text holds.
pub fn chars(items: &[Item]) -> BTreeSet<char> {
    items
        .iter()
        .filter_map(|it| match it {
            Item::Text { text, .. } => Some(text.chars()),
            Item::Path { .. } => None,
        })
        .flatten()
        .collect()
}

/// The PDF of `pages`, each page's content what `draw` writes on its
/// canvas, with `marks` as its bookmarks.
pub fn write(
    doc: &Doc,
    pages: &[Page],
    marks: &[Mark],
    draw: &mut dyn FnMut(usize, &mut Canvas),
) -> Vec<u8> {
    let font = Subset::new(&doc.chars);
    let p = |v: f64| (v * doc.pt) as f32;
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
        .title(TextStr(doc.title))
        .creator(TextStr(doc.creator));
    for (i, page) in pages.iter().enumerate() {
        let annots: Vec<Ref> = page.links.iter().map(|_| id()).collect();
        let mut w = pdf.page(page_ids[i]);
        w.media_box(Rect::new(0.0, 0.0, p(page.size[0]), p(page.size[1])));
        w.parent(tree);
        w.contents(content_ids[i]);
        if !annots.is_empty() {
            w.annotations(annots.iter().copied());
        }
        w.resources().fonts().pair(Name(b"F1"), type0);
        w.finish();
        for (link, aid) in page.links.iter().zip(&annots) {
            let [lo, hi] = link.rect;
            let mut a = pdf.annotation(*aid);
            a.subtype(AnnotationType::Link);
            a.rect(Rect::new(p(lo[0]), p(lo[1]), p(hi[0]), p(hi[1])));
            a.border(0.0, 0.0, 0.0, None);
            a.action()
                .action_type(ActionType::GoTo)
                .destination()
                .page(page_ids[link.to])
                .fit();
            a.finish();
        }
        let mut canvas = Canvas {
            content: Content::new(),
            font: &font,
            pt: doc.pt,
        };
        draw(i, &mut canvas);
        pdf.stream(content_ids[i], &canvas.content.finish());
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
    let units = font.units;
    let scale = |v: f64| (v * 1000.0 / units) as f32;
    let name = format!("{}+Inter-Regular", font.tag());
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
    let packed = miniz_oxide::deflate::compress_to_vec_zlib(&data, 10);
    pdf.stream(file, &packed).filter(Filter::FlateDecode);
    let mut cmap = pdf_writer::types::UnicodeCmap::new(Name(b"Custom"), system);
    for (c, g) in &font.gids {
        cmap.pair(*g, *c);
    }
    pdf.cmap(unicode, &cmap.finish());
    pdf.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::Path;
    use crate::style::{LineKind, Standard};

    fn sheet() -> Vec<Item> {
        vec![
            Item::Path {
                path: Path::polyline(&[[10.0, 10.0], [100.0, 10.0], [100.0, 60.0]], false),
                stroke: Some(Standard::Asme.stroke(LineKind::Visible)),
                fill: None,
            },
            Item::Path {
                path: Path::circle([50.0, 40.0], 8.0),
                stroke: Some(Standard::Asme.stroke(LineKind::Hidden)),
                fill: None,
            },
            Item::Text {
                text: "FRONT".into(),
                at: [50.0, 5.0],
                size: 3.5,
                angle: 0.0,
                anchor: Anchor::Middle,
                colour: Colour::BLACK,
            },
        ]
    }

    #[test]
    fn items_write_alike_with_their_text_and_dashes() {
        let items = sheet();
        let doc = Doc {
            title: "bracket",
            creator: "ryucad",
            pt: PT_PER_MM,
            chars: chars(&items),
        };
        let pages = [Page {
            size: [279.4, 215.9],
            links: vec![],
        }];
        let write_it = || write(&doc, &pages, &[], &mut |_, c| c.items(&items));
        let a = write_it();
        assert_eq!(a, write_it());
        let text = String::from_utf8_lossy(&a);
        assert!(text.starts_with("%PDF-"));
        assert!(text.contains("/MediaBox [0 0 792 612]"), "{text}");
        assert!(text.contains("+Inter-Regular"));
        assert!(text.contains("/Creator (ryucad)"));
        assert!(!text.contains("/CreationDate") && !text.contains("/ID"));
    }
}
