//! Mask and paste openings: a mask opening is the pad grown by its own
//! margin, else its footprint's, else the preset's; a paste opening is the
//! pad changed by its margin and its ratio of the pad's size (ryueda's
//! fields, as KiCad's). Standard shapes change by their sizes exactly;
//! custom pads through `ryu_copper::poly::grow` and `shrink` at the
//! 10 nm bound.

use super::pad::{own_shape, pad_place};
use super::preset::Preset;
use crate::footprint::{Footprint, Pad};
use ryu_board::geom::{Nm, Region, div_round};
use ryu_board::model::Polarity;
use ryu_board::shape::{Shape, ShapeError, polygon};
use ryu_copper::poly::{self, FillRule, FlattenOptions, Rings, flatten_shape};

/// How finely custom pads are offset, in nm.
pub const BOUND_NM: Nm = 10;

pub fn mask_margin(f: &Footprint, p: &Pad, preset: &Preset) -> Nm {
    p.margins.mask_margin_nm.or(f.margins.mask_margin_nm).unwrap_or(preset.mask_expansion_nm)
}

/// The paste opening's change on each side: the margin plus the ratio
/// of the pad's smaller side, rounded to the nanometre.
pub fn paste_margin(f: &Footprint, p: &Pad, preset: &Preset) -> Nm {
    let margin = p.margins.paste_margin_nm.or(f.margins.paste_margin_nm).unwrap_or(preset.paste_margin_nm);
    let ratio = p.margins.paste_ratio_ppm.or(f.margins.paste_ratio_ppm).unwrap_or(0);
    let side = p.size[0].min(p.size[1]);
    margin + div_round(i128::from(side) * i128::from(ratio), 1_000_000) as Nm
}

/// `p`'s copper changed by `by` on every side, in the footprint's frame.
pub fn changed(p: &Pad, by: Nm) -> Result<Shape, ShapeError> {
    if let Some(s) = own_shape(p, by)? {
        return Ok(s.placed(&pad_place(p), Polarity::Positive));
    }
    // A custom pad: flatten, offset, and back to regions.
    let base = own_shape(p, 0)?.expect("ungrown");
    let opts = FlattenOptions::new(BOUND_NM);
    let mut rings = Vec::new();
    for (_, r) in flatten_shape(&base, &opts) {
        rings.extend(r.rings);
    }
    let merged = poly::simplify(&Rings::new(rings, opts.bound()), FillRule::NonZero);
    let moved = if by >= 0 { poly::grow(&merged, by, &opts) } else { poly::shrink(&merged, -by, &opts) };
    let mut out = Shape::default();
    for pg in &moved.polygons {
        let outer = polygon(&pg.outer)?;
        let holes = pg
            .holes
            .iter()
            .map(|h| polygon(h))
            .collect::<Result<Vec<_>, _>>()?;
        out.parts.push(ryu_board::shape::Part {
            polarity: Polarity::Positive,
            piece: ryu_board::shape::Piece::Region(Region { outer, holes }),
        });
    }
    out.approximate = Some(opts.bound());
    Ok(out.placed(&pad_place(p), Polarity::Positive))
}

/// The pad's mask opening on a side it names a mask layer for.
pub fn mask(f: &Footprint, p: &Pad, preset: &Preset) -> Result<Shape, ShapeError> {
    changed(p, mask_margin(f, p, preset))
}

/// The pad's paste opening; empty when the change consumes it.
pub fn paste(f: &Footprint, p: &Pad, preset: &Preset) -> Result<Shape, ShapeError> {
    changed(p, paste_margin(f, p, preset))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::footprint::tests::r0402;
    use super::super::preset::JLC_2L;

    #[test]
    fn masks_grow_by_the_preset_unless_the_pad_says() {
        let f = r0402();
        let p = f.pads.values().next().unwrap().clone();
        // jlc-2l opens the mask 1:1 with the pad.
        assert_eq!(JLC_2L.mask_expansion_nm, 0);
        let b = mask(&f, &p, &JLC_2L).unwrap().bounds().unwrap();
        assert_eq!(b.max.x - b.min.x, 540_000);
        let mut q = p.clone();
        q.margins.mask_margin_nm = Some(50_000);
        let b = mask(&f, &q, &JLC_2L).unwrap().bounds().unwrap();
        assert_eq!(b.max.x - b.min.x, 540_000 + 100_000);
        // The footprint's margin, where the pad has none.
        let mut g = f.clone();
        g.margins.mask_margin_nm = Some(25_000);
        let b = mask(&g, &p, &JLC_2L).unwrap().bounds().unwrap();
        assert_eq!(b.max.x - b.min.x, 540_000 + 50_000);
    }

    #[test]
    fn paste_shrinks_by_margin_and_ratio() {
        let f = r0402();
        let mut p = f.pads.values().next().unwrap().clone();
        p.margins.paste_margin_nm = Some(-10_000);
        p.margins.paste_ratio_ppm = Some(-100_000);
        // -10 µm, and a tenth of the smaller side (540 µm): -64 µm a side.
        assert_eq!(paste_margin(&f, &p, &JLC_2L), -64_000);
        let b = paste(&f, &p, &JLC_2L).unwrap().bounds().unwrap();
        assert_eq!(b.max.x - b.min.x, 540_000 - 128_000);
    }

    #[test]
    fn custom_pads_grow_through_the_engine() {
        use crate::footprint::{Anchor, PadShape};
        let f = r0402();
        let mut p = f.pads.values().next().unwrap().clone();
        p.at = [0, 0];
        p.shape = PadShape::Custom { anchor: Anchor::Rect, primitives: vec![] };
        let s = changed(&p, 50_000).unwrap();
        let b = s.bounds().unwrap();
        // Round joins at the corners stay within the box grown by 50 µm.
        assert!((b.max.x - b.min.x - (540_000 + 100_000)).abs() <= 2 * BOUND_NM, "{b:?}");
        assert_eq!(s.approximate, Some(BOUND_NM));
    }
}
