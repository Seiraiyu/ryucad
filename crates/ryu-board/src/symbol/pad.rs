//! ryueda's footprint pads (design 157) in the standard vocabulary, exactly.
//!
//! `circle`, `rect` and `oval` are `Round`, `Rect` and `Oval`; `custom`
//! pads are user symbols, built by ryueda from their primitives.

use super::{CornerSet, Corners, StandardSymbol};
use crate::geom::{Nm, div_round};

/// A ratio in millionths of the pad's smaller side, in nanometres.
fn of_smaller(w: Nm, h: Nm, ratio: i64) -> Nm {
    div_round(i128::from(w.min(h)) * i128::from(ratio), 1_000_000) as Nm
}

/// A `roundrect` pad: every corner rounded by `ratio` (millionths) of the
/// smaller side.
pub fn roundrect(w: Nm, h: Nm, ratio: i64) -> StandardSymbol {
    StandardSymbol::Rect {
        w,
        h,
        corners: Corners::rounded(of_smaller(w, h, ratio), CornerSet::ALL),
    }
}

/// A `chamfered` pad: `chamfered` corners cut by `chamfer_ratio` of the
/// smaller side, and the others rounded by `round_ratio` (0 for none).
pub fn chamfered(
    w: Nm,
    h: Nm,
    chamfer_ratio: i64,
    chamfered: CornerSet,
    round_ratio: i64,
) -> StandardSymbol {
    let rest = CornerSet::of(&(1..=4).filter(|c| !chamfered.has(*c)).collect::<Vec<u8>>())
        .expect("corners are 1 to 4");
    let round = of_smaller(w, h, round_ratio);
    StandardSymbol::Rect {
        w,
        h,
        corners: Corners {
            round,
            rounded: if round > 0 { rest } else { CornerSet::NONE },
            chamfer: of_smaller(w, h, chamfer_ratio),
            chamfered,
        },
    }
}

/// A `trapezoid` pad with KiCad's delta: the left side `h + dx` tall and
/// the right `h - dx`, the top `w + dy` wide and the bottom `w - dy`.
pub fn trapezoid(w: Nm, h: Nm, dx: Nm, dy: Nm) -> StandardSymbol {
    StandardSymbol::Trapezoid { w, h, dx, dy }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ratios_resolve_against_the_smaller_side() {
        assert_eq!(
            roundrect(2_000_000, 1_000_000, 250_000),
            StandardSymbol::Rect {
                w: 2_000_000,
                h: 1_000_000,
                corners: Corners::rounded(250_000, CornerSet::ALL)
            }
        );
        let StandardSymbol::Rect { corners, .. } = chamfered(
            1_000_000,
            3_000_000,
            200_000,
            CornerSet::of(&[1, 2]).unwrap(),
            100_000,
        ) else {
            panic!()
        };
        assert_eq!(corners.chamfer, 200_000);
        assert_eq!(corners.round, 100_000);
        assert!(corners.rounded.has(3) && corners.rounded.has(4) && !corners.rounded.has(1));
        let StandardSymbol::Rect { corners, .. } =
            chamfered(1_000_000, 3_000_000, 200_000, CornerSet::ALL, 0)
        else {
            panic!()
        };
        assert_eq!(corners.rounded, CornerSet::NONE);
    }
}
