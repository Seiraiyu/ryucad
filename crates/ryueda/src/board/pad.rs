//! A footprint pad's copper and drill: its shape through `ryu-board`'s
//! symbols (`circle`, `rect`, `oval`, `trapezoid`, rounded and chamfered
//! rectangles) or, for a custom pad, its anchor and primitives; placed by
//! the pad's own offset and turn in the footprint's frame.

use super::graphic;
use crate::footprint::{Anchor, Corner, Drill, Pad, PadKind, PadShape, Shape as Drawn};
use ryu_board::geom::{Mdeg, Nm, Place, Point};
use ryu_board::model::Polarity;
use ryu_board::shape::{Shape, ShapeError, symbol_shape};
use ryu_board::symbol::{CornerSet, Corners, StandardSymbol, pad as pads};
use ryu_copper::layout::HoleShape;

/// The pad's place in the footprint's frame.
pub fn pad_place(p: &Pad) -> Place {
    Place {
        at: Point::new(p.at[0], p.at[1]),
        rot: Mdeg::new(p.rotation_mdeg),
        mirror: false,
        scale: None,
    }
}

/// ODB++'s corner numbers for ryueda's corners: 1 top right, 2 top left,
/// 3 bottom left, 4 bottom right.
fn corner_number(c: Corner) -> u8 {
    match c {
        Corner::TopRight => 1,
        Corner::TopLeft => 2,
        Corner::BottomLeft => 3,
        Corner::BottomRight => 4,
    }
}

/// The standard symbol of a pad of `size` and `shape` (none for custom).
pub fn symbol(shape: &PadShape, w: Nm, h: Nm) -> Option<StandardSymbol> {
    Some(match shape {
        PadShape::Circle => StandardSymbol::Round { d: w.min(h) },
        PadShape::Rect => StandardSymbol::Rect {
            w,
            h,
            corners: Corners::SHARP,
        },
        PadShape::Oval => StandardSymbol::Oval { w, h },
        PadShape::Trapezoid { delta } => pads::trapezoid(w, h, delta[0], delta[1]),
        PadShape::Roundrect { ratio_ppm } => pads::roundrect(w, h, *ratio_ppm),
        PadShape::Chamfered {
            ratio_ppm,
            corners,
            round_ratio_ppm,
        } => {
            let nums: Vec<u8> = corners.iter().copied().map(corner_number).collect();
            pads::chamfered(
                w,
                h,
                *ratio_ppm,
                CornerSet::of(&nums).expect("1 to 4"),
                *round_ratio_ppm,
            )
        }
        PadShape::Custom { .. } => return None,
    })
}

/// The pad's copper in its own frame (before its offset and turn), grown
/// by `grow` on every side (negative shrinks), for standard shapes
/// exactly: their sizes change by twice `grow`, and a circle's diameter
/// likewise. A custom pad isn't grown here (`None` when `grow` isn't 0):
/// [`super::shapes`] offsets it through `ryu_copper::poly`.
pub fn own_shape(p: &Pad, grow: Nm) -> Result<Option<Shape>, ShapeError> {
    let (w, h) = (p.size[0] + 2 * grow, p.size[1] + 2 * grow);
    if w <= 0 || h <= 0 {
        return Ok(Some(Shape::default()));
    }
    match &p.shape {
        PadShape::Custom { anchor, primitives } => {
            if grow != 0 {
                return Ok(None);
            }
            let anchor = match anchor {
                Anchor::Circle => StandardSymbol::Round { d: w.min(h) },
                Anchor::Rect => StandardSymbol::Rect {
                    w,
                    h,
                    corners: Corners::SHARP,
                },
            };
            let mut s = symbol_shape(&anchor)?;
            for prim in primitives {
                // A custom pad's closed primitives are copper whether or
                // not they are marked filled, as KiCad draws them.
                let mut prim = prim.clone();
                if let Drawn::Rect { fill, .. }
                | Drawn::Circle { fill, .. }
                | Drawn::Polygon { fill, .. } = &mut prim
                {
                    *fill = true;
                }
                let mut d = graphic::drawn(&prim);
                s.parts.append(&mut d.parts);
                s.approximate = s.approximate.max(d.approximate);
            }
            Ok(Some(s))
        }
        shape => {
            let sym = symbol(shape, w, h).expect("standard");
            Ok(Some(symbol_shape(&sym)?))
        }
    }
}

/// The pad's copper in the footprint's frame.
pub fn copper(p: &Pad) -> Result<Shape, ShapeError> {
    Ok(own_shape(p, 0)?
        .expect("ungrown")
        .placed(&pad_place(p), Polarity::Positive))
}

/// Which copper layers the pad is on: `(top, bottom)`, and whether it
/// names `copper.inner` (which stage 5 can't build).
pub fn copper_sides(p: &Pad) -> (bool, bool, bool) {
    let has = |l: &str| p.layers.iter().any(|x| x == l);
    let all = has("copper.all") || matches!(p.kind, PadKind::Through);
    (
        all || has("copper.top"),
        all || has("copper.bottom"),
        has("copper.inner"),
    )
}

/// The pad's drill in the footprint's frame, and whether it is plated.
pub fn hole(p: &Pad) -> Option<(HoleShape, bool)> {
    let d = p.drill.as_ref()?;
    let place = pad_place(p);
    let plated = matches!(p.kind, PadKind::Through);
    Some(match d {
        Drill::Round {
            diameter_nm,
            offset,
        } => (
            HoleShape::Round {
                at: place.point(Point::new(offset[0], offset[1])),
                d: *diameter_nm,
            },
            plated,
        ),
        Drill::Slot { size, offset } => {
            // A slot's length runs along its longer side, in the pad's frame.
            let (w, h) = (size[0], size[1]);
            let d = w.min(h);
            let reach = (w.max(h) - d) / 2;
            let (dx, dy) = if w >= h { (reach, 0) } else { (0, reach) };
            let o = Point::new(offset[0], offset[1]);
            let a = place.point(o.offset(-dx, -dy));
            let b = place.point(o.offset(dx, dy));
            if a == b {
                (HoleShape::Round { at: a, d }, plated)
            } else {
                (HoleShape::Slot { a, b, d }, plated)
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::footprint::tests::r0402;
    use ryu_board::shape::Piece;

    fn area2(s: &Shape) -> f64 {
        s.parts
            .iter()
            .map(|p| match &p.piece {
                Piece::Region(r) => r.outer.area2(),
                _ => 0.0,
            })
            .sum()
    }

    #[test]
    fn pads_take_their_footprints_sizes_and_places() {
        let f = r0402();
        let p1 = f
            .pads
            .values()
            .find(|p| p.number.as_deref() == Some("1"))
            .unwrap();
        let c = copper(p1).unwrap();
        let b = c.bounds().unwrap();
        assert_eq!(b.min, Point::new(-510_000 - 270_000, -320_000));
        assert_eq!(b.max, Point::new(-510_000 + 270_000, 320_000));
        // A rounded rectangle: less than its box, more than its box less
        // the corners' squares.
        let a = area2(&c) / 2.0;
        let full = 540_000.0 * 640_000.0;
        assert!(a < full && a > full - 4.0 * 135_000.0 * 135_000.0, "{a}");
        assert_eq!(copper_sides(p1), (true, false, false));
        assert!(hole(p1).is_none());
    }

    #[test]
    fn turned_pads_and_slots() {
        let mut p = r0402().pads.into_values().next().unwrap();
        p.shape = PadShape::Rect;
        p.at = [0, 0];
        p.rotation_mdeg = 90_000;
        let b = copper(&p).unwrap().bounds().unwrap();
        assert_eq!((b.max.x - b.min.x, b.max.y - b.min.y), (640_000, 540_000));
        p.kind = PadKind::Through;
        p.drill = Some(Drill::Slot {
            size: [300_000, 100_000],
            offset: [0, 0],
        });
        let (h, plated) = hole(&p).unwrap();
        assert!(plated);
        assert_eq!(
            h,
            HoleShape::Slot {
                a: Point::new(0, -100_000),
                b: Point::new(0, 100_000),
                d: 100_000
            }
        );
        assert_eq!(copper_sides(&p), (true, true, false));
    }

    #[test]
    fn custom_pads_fill_their_closed_primitives() {
        let mut p = r0402().pads.into_values().next().unwrap();
        p.at = [0, 0];
        let tri: Drawn = serde_json::from_value(serde_json::json!(
            {"type": "polygon", "points": [[0, 0], [1_000_000, 0], [0, 1_000_000]], "width_nm": 0}
        ))
        .unwrap();
        p.shape = PadShape::Custom {
            anchor: Anchor::Circle,
            primitives: vec![tri],
        };
        let c = copper(&p).unwrap();
        assert_eq!(c.parts.len(), 2, "{c:?}");
        assert!(matches!(c.parts[1].piece, Piece::Region(_)));
    }

    #[test]
    fn standard_pads_grow_exactly() {
        let mut p = r0402().pads.into_values().next().unwrap();
        p.shape = PadShape::Rect;
        let s = own_shape(&p, 50_000).unwrap().unwrap();
        let b = s.bounds().unwrap();
        assert_eq!(b.max.x - b.min.x, 540_000 + 100_000);
        assert!(own_shape(&p, -400_000).unwrap().unwrap().parts.is_empty());
        p.shape = PadShape::Custom {
            anchor: Anchor::Rect,
            primitives: vec![],
        };
        assert!(own_shape(&p, 10).unwrap().is_none());
    }
}
