//! PDFs read back by a reader of our own (design 417, plan 9.D): two
//! pages of their own sizes, each one's text read through the font's
//! ToUnicode map in the order drawn (symbols and Inter's ±, ×, °
//! included), the font file inflating, and the bookmarks depth first.

#[path = "common/pdf.rs"]
mod reader;

use reader::read;
use ryu_draw::geom::Path;
use ryu_draw::pdf::{Doc, Mark, PT_PER_MM, Page, chars, write};
use ryu_draw::style::{Colour, LineKind, Standard};
use ryu_draw::{Anchor, Item};

fn text(t: &str, at: [f64; 2]) -> Item {
    Item::Text {
        text: t.into(),
        at,
        size: 3.5,
        angle: 0.0,
        anchor: Anchor::Start,
        colour: Colour::BLACK,
    }
}

#[test]
fn a_pdf_reads_back_its_pages_text_fonts_and_bookmarks() {
    let pages_items = [
        vec![
            Item::Path {
                path: Path::circle([50.0, 40.0], 8.0),
                stroke: Some(Standard::Asme.stroke(LineKind::Visible)),
                fill: None,
            },
            text("FRONT", [10.0, 10.0]),
            text("40.00 ±0.10", [10.0, 20.0]),
        ],
        vec![text("SECTION A-A", [10.0, 10.0]), text("4× 90° (2:1)", [10.0, 30.0])],
    ];
    let all: Vec<Item> = pages_items.iter().flatten().cloned().collect();
    let doc = Doc {
        title: "Bracket — rev A",
        creator: "ryucad",
        pt: PT_PER_MM,
        chars: chars(&all),
    };
    let pages = [
        Page {
            size: [279.4, 215.9],
            links: vec![],
        },
        Page {
            size: [210.0, 297.0],
            links: vec![],
        },
    ];
    let marks = [
        Mark {
            title: "Sheet 1".into(),
            page: 0,
            children: vec![Mark {
                title: "v1 front".into(),
                page: 0,
                children: vec![],
            }],
        },
        Mark {
            title: "Sheet 2".into(),
            page: 1,
            children: vec![],
        },
    ];
    let bytes = write(&doc, &pages, &marks, &mut |i, c| c.items(&pages_items[i]));
    let r = read(&bytes).unwrap();
    assert_eq!(r.pages.len(), 2);
    let pt = |mm: f64| f64::from((mm * PT_PER_MM) as f32);
    assert!((r.pages[0].0[0] - pt(279.4)).abs() < 1e-3, "{:?}", r.pages[0].0);
    assert!((r.pages[1].0[1] - pt(297.0)).abs() < 1e-3);
    assert_eq!(r.pages[0].1, ["FRONT", "40.00 ±0.10"]);
    assert_eq!(r.pages[1].1, ["SECTION A-A", "4× 90° (2:1)"]);
    assert_eq!(r.fonts.len(), 1);
    assert!(r.fonts[0].0.ends_with("+Inter-Regular"), "{:?}", r.fonts);
    assert!(r.fonts[0].1 > 1000);
    assert_eq!(r.bookmarks, ["Sheet 1", "v1 front", "Sheet 2"]);
    assert_eq!(r.title.as_deref(), Some("Bracket — rev A"));
}

#[test]
fn a_damaged_pdf_is_refused() {
    let doc = Doc {
        title: "x",
        creator: "ryucad",
        pt: PT_PER_MM,
        chars: chars(&[text("X", [0.0, 0.0])]),
    };
    let pages = [Page {
        size: [100.0, 100.0],
        links: vec![],
    }];
    let mut bytes = write(&doc, &pages, &[], &mut |_, c| c.items(&[text("X", [0.0, 0.0])]));
    assert!(read(&bytes).is_ok());
    // An object moved by a byte no longer stands where the table says.
    let at = bytes.windows(7).position(|w| w == b"2 0 obj").unwrap();
    bytes.insert(at, b' ');
    assert!(read(&bytes).is_err());
}
