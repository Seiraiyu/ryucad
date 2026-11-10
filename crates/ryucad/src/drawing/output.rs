//! A built drawing written out: a sheet as SVG or PNG (resvg over the
//! SVG, with the bundled Inter alone, as ryueda's schematics render), and
//! every sheet as one PDF, a page each at its paper with bookmarks per
//! sheet and view. The same drawing gives the same bytes everywhere.

use super::{Built, SheetBuilt};
use crate::error::{Code, Error, Result};
use ryu_draw::pdf::{Doc, Mark, PT_PER_MM, Page};
use ryu_draw::{Colour, svg};

/// The paper drawings are drawn on.
pub const PAPER: Colour = Colour::WHITE;

/// Pixels to the millimetre a PNG takes unless asked: about 100 dpi.
pub const PX_PER_MM: f32 = 4.0;

/// A sheet's SVG, in millimetres on white.
pub fn sheet_svg(sheet: &SheetBuilt) -> String {
    svg::write(sheet.paper, &sheet.items, Some(PAPER))
}

/// A sheet's PNG at `px_per_mm` (0.5 to 40).
pub fn sheet_png(sheet: &SheetBuilt, px_per_mm: f32) -> Result<Vec<u8>> {
    if !(0.5..=40.0).contains(&px_per_mm) {
        return Err(Error::new(
            Code::InvalidParam,
            format!("a scale is 0.5 to 40 pixels a millimetre, not {px_per_mm}"),
        )
        .with("param", "scale"));
    }
    png(&sheet_svg(sheet), px_per_mm)
}

/// SVG to PNG at `px_per_mm`, with Inter alone for text.
pub fn png(svg: &str, px_per_mm: f32) -> Result<Vec<u8>> {
    use resvg::{tiny_skia, usvg};
    let mut opt = usvg::Options {
        font_family: "Inter".into(),
        ..usvg::Options::default()
    };
    std::sync::Arc::make_mut(&mut opt.fontdb).load_font_data(ryu_draw::font::INTER.to_vec());
    let tree = usvg::Tree::from_str(svg, &opt)
        .map_err(|e| Error::new(Code::Internal, format!("the SVG doesn't read: {e}")))?;
    // usvg reads millimetres at 96 per inch.
    let k = px_per_mm / (96.0 / 25.4);
    let size = tree.size();
    let (w, h) = (
        (size.width() * k).ceil() as u32,
        (size.height() * k).ceil() as u32,
    );
    if w == 0 || h == 0 || u64::from(w) * u64::from(h) > 200_000_000 {
        return Err(Error::new(
            Code::InvalidParam,
            format!("{w}x{h} pixels is no image to make; choose another scale"),
        )
        .with("param", "scale"));
    }
    let mut pixmap = tiny_skia::Pixmap::new(w, h).expect("a size checked above");
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(k, k),
        &mut pixmap.as_mut(),
    );
    pixmap
        .encode_png()
        .map_err(|e| Error::new(Code::Internal, format!("PNG: {e}")))
}

/// Every sheet as one PDF: a page each, its paper's size, bookmarked by
/// sheet and, under each sheet, by view.
pub fn pdf(built: &Built) -> Vec<u8> {
    let chars = built
        .sheets
        .iter()
        .flat_map(|s| ryu_draw::pdf::chars(&s.items))
        .collect();
    let title = format!("{} · {}", built.part, built.name);
    let doc = Doc {
        title: &title,
        creator: "ryucad",
        pt: PT_PER_MM,
        chars,
    };
    let pages: Vec<Page> = built
        .sheets
        .iter()
        .map(|s| Page {
            size: s.paper,
            links: Vec::new(),
        })
        .collect();
    let marks: Vec<Mark> = built
        .sheets
        .iter()
        .enumerate()
        .map(|(i, s)| Mark {
            title: format!("{} {}", s.key, s.name),
            page: i,
            children: s
                .views
                .iter()
                .map(|v| Mark {
                    title: format!("{} ({})", v.key, v.kind),
                    page: i,
                    children: Vec::new(),
                })
                .collect(),
        })
        .collect();
    ryu_draw::pdf::write(&doc, &pages, &marks, &mut |i, canvas| {
        let s = &built.sheets[i];
        canvas.fill_colour(PAPER);
        canvas.rect([0.0, 0.0], s.paper);
        canvas.fill();
        canvas.items(&s.items);
    })
}
