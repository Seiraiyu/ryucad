//! `ryusch` measures text with Inter's advance widths, a table generated
//! from the font ryueda bundles (`fonts/Inter-Regular.ttf`), so it reads
//! no font itself. The table must be the font's: after changing the font,
//! regenerate it with `UPDATE_WIDTHS=1 cargo test -p ryueda --test
//! font_widths` and review the diff.

/// The characters measured: printable ASCII and Latin-1, and the few
/// others schematics write (Ω, arrows, dashes, the ellipsis).
fn characters() -> Vec<char> {
    let mut out: Vec<char> = (0x20u32..0x7f)
        .chain(0xa0..0x100)
        .filter_map(char::from_u32)
        .collect();
    out.extend([
        '\u{3a9}', '\u{2013}', '\u{2014}', '\u{2026}', '\u{2126}', '\u{2190}', '\u{2192}',
    ]);
    out
}

fn table() -> String {
    let bytes = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/fonts/Inter-Regular.ttf"
    ))
    .unwrap();
    let face = ttf_parser::Face::parse(&bytes, 0).unwrap();
    let mut rows = String::new();
    for c in characters() {
        let Some(g) = face.glyph_index(c) else {
            continue;
        };
        let adv = face.glyph_hor_advance(g).unwrap();
        rows.push_str(&format!("    ({:?}, {adv}),\n", c));
    }
    format!(
        "//! Inter Regular's advance widths, in font units: generated from\n\
         //! `crates/ryueda/fonts/Inter-Regular.ttf` by `UPDATE_WIDTHS=1 cargo test\n\
         //! -p ryueda --test font_widths`. Don't edit by hand.\n\
         \n\
         /// The font's units per em.\n\
         pub const UNITS_PER_EM: i64 = {};\n\
         \n\
         /// `(character, advance)`, sorted by character.\n\
         pub const ADVANCES: &[(char, u16)] = &[\n{rows}];\n",
        face.units_per_em()
    )
}

#[test]
fn ryusch_measures_with_the_bundled_font() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../ryusch/src/widths.rs");
    let want = table();
    if std::env::var("UPDATE_WIDTHS").is_ok_and(|v| v == "1") {
        std::fs::write(path, &want).unwrap();
        return;
    }
    let have = std::fs::read_to_string(path).unwrap_or_default();
    assert!(
        have == want,
        "ryusch's widths aren't the bundled font's; regenerate with UPDATE_WIDTHS=1"
    );
}
