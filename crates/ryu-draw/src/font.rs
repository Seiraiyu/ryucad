//! The one face every ryu drawing writes: Inter Regular 4.1 (OFL 1.1;
//! `fonts/LICENSES.md`), its advance widths and extent, and the subset
//! a PDF embeds.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::OnceLock;

/// Inter Regular, static: what SVG names, PNGs are rasterized with and
/// PDFs embed a subset of.
pub const INTER: &[u8] = include_bytes!("../fonts/Inter-Regular.ttf");

fn face() -> ttf_parser::Face<'static> {
    ttf_parser::Face::parse(INTER, 0).expect("the bundled font reads")
}

struct Metrics {
    units: f64,
    ascender: f64,
    descender: f64,
}

fn metrics() -> &'static Metrics {
    static M: OnceLock<Metrics> = OnceLock::new();
    M.get_or_init(|| {
        let f = face();
        Metrics {
            units: f64::from(f.units_per_em()),
            ascender: f64::from(f.ascender()),
            descender: f64::from(f.descender()),
        }
    })
}

/// A character's advance in font units, read once a process.
fn advance(c: char) -> u16 {
    static A: OnceLock<std::sync::Mutex<HashMap<char, u16>>> = OnceLock::new();
    let table = A.get_or_init(Default::default);
    let mut t = table.lock().unwrap_or_else(|e| e.into_inner());
    *t.entry(c).or_insert_with(|| {
        let f = face();
        f.glyph_index(c)
            .and_then(|g| f.glyph_hor_advance(g))
            .unwrap_or(0)
    })
}

/// How wide `text` is at `size` (its em), by Inter's advance widths: no
/// kerning, a character Inter lacks counting 0.
pub fn width(text: &str, size: f64) -> f64 {
    let units: u32 = text.chars().map(|c| u32::from(advance(c))).sum();
    f64::from(units) * size / metrics().units
}

/// How far Inter reaches above its baseline and below it (negative) at
/// `size`.
pub fn extent(size: f64) -> (f64, f64) {
    let m = metrics();
    (m.ascender * size / m.units, m.descender * size / m.units)
}

/// The glyphs a PDF holds: each character's glyph, renumbered in the
/// order of the characters.
pub struct Subset {
    pub(crate) units: f64,
    pub(crate) gids: BTreeMap<char, u16>,
    pub(crate) remap: subsetter::GlyphRemapper,
}

impl Subset {
    pub fn new(chars: &BTreeSet<char>) -> Subset {
        let face = face();
        let mut remap = subsetter::GlyphRemapper::new();
        let mut gids = BTreeMap::new();
        for c in chars {
            let g = face.glyph_index(*c).map_or(0, |g| g.0);
            gids.insert(*c, remap.remap(g));
        }
        Subset {
            units: f64::from(face.units_per_em()),
            gids,
            remap,
        }
    }

    /// `text` as two-byte CIDs.
    pub fn encode(&self, text: &str) -> Vec<u8> {
        text.chars()
            .flat_map(|c| self.gids.get(&c).copied().unwrap_or(0).to_be_bytes())
            .collect()
    }

    /// The six letters a subset font's name starts with: from the glyphs
    /// it holds, so the same pages name it alike.
    pub fn tag(&self) -> String {
        let mut h = blake3::Hasher::new();
        for (c, g) in &self.gids {
            h.update(&u32::from(*c).to_le_bytes());
            h.update(&g.to_le_bytes());
        }
        h.finalize().as_bytes()[..6]
            .iter()
            .map(|b| char::from(b'A' + b % 26))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn widths_and_extent_are_the_fonts() {
        let f = face();
        let g = f.glyph_index('M').unwrap();
        let m = f64::from(f.glyph_hor_advance(g).unwrap());
        let em = f64::from(f.units_per_em());
        assert_eq!(width("MM", 10.0), 2.0 * m * 10.0 / em);
        assert_eq!(width("", 10.0), 0.0);
        let (a, d) = extent(10.0);
        assert!(a > 5.0 && d < 0.0);
    }

    #[test]
    fn a_subset_numbers_its_glyphs_and_names_itself_by_them() {
        let s = Subset::new(&"AB".chars().collect());
        assert_eq!(s.encode("BA").len(), 4);
        assert_eq!(s.tag().len(), 6);
        assert_eq!(s.tag(), Subset::new(&"AB".chars().collect()).tag());
        assert_ne!(s.tag(), Subset::new(&"AC".chars().collect()).tag());
    }
}
