//! What a pad, or a line's pen, is drawn with.
//!
//! Standard symbols carry their parameters in nanometres and millidegrees.
//! [`StandardSymbol::parse`] reads ODB++'s names.

mod parse;

use crate::geom::{Mdeg, Nm};

pub use parse::NameUnits;

/// Which corners of a rectangle a rounding or chamfer applies to, by
/// ODB++'s numbers: 1 top right, 2 top left, 3 bottom left, 4 bottom
/// right. A name that lists none means all four.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CornerSet(u8);

impl CornerSet {
    pub const ALL: CornerSet = CornerSet(0b1111);
    pub const NONE: CornerSet = CornerSet(0);

    /// Corners by number, 1 to 4.
    pub fn of(corners: &[u8]) -> Option<CornerSet> {
        let mut bits = 0;
        for &c in corners {
            if !(1..=4).contains(&c) {
                return None;
            }
            bits |= 1 << (c - 1);
        }
        Some(CornerSet(bits))
    }

    pub fn has(self, corner: u8) -> bool {
        (1..=4).contains(&corner) && self.0 & (1 << (corner - 1)) != 0
    }
}

/// How a rectangle's corners are cut: rounded with radius `round`, or
/// chamfered `chamfer` along each side, each on its own corners. ODB++
/// names give one or the other; ryueda's chamfered pads may round the
/// corners they don't chamfer. A corner in both sets is refused when its
/// shape is built.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Corners {
    pub round: Nm,
    pub rounded: CornerSet,
    pub chamfer: Nm,
    pub chamfered: CornerSet,
}

impl Corners {
    pub const SHARP: Corners = Corners {
        round: 0,
        rounded: CornerSet::NONE,
        chamfer: 0,
        chamfered: CornerSet::NONE,
    };

    pub fn rounded(r: Nm, which: CornerSet) -> Corners {
        Corners {
            round: r,
            rounded: which,
            ..Corners::SHARP
        }
    }

    pub fn chamfered(c: Nm, which: CornerSet) -> Corners {
        Corners {
            chamfer: c,
            chamfered: which,
            ..Corners::SHARP
        }
    }

    pub fn is_sharp(&self) -> bool {
        self.rounded == CornerSet::NONE && self.chamfered == CornerSet::NONE
    }
}

/// A drilled hole's plating.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Plating {
    Plated,
    NonPlated,
    Via,
}

/// The ring a thermal is cut from. Rounded corners (`xr`) round the
/// outside only; the inside stays square.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ThermalForm {
    /// `thr`: a round ring, its pieces' ends rounded.
    RoundRounded { od: Nm, id: Nm },
    /// `ths`: a round ring, its pieces' ends cut square.
    RoundSquared { od: Nm, id: Nm },
    /// `s_ths` (`open` false) and `s_tho` (`open` true): a square ring.
    Square {
        os: Nm,
        is: Nm,
        corners: Corners,
        open: bool,
    },
    /// `sr_ths`: a square outside, a round inside.
    SquareRound { os: Nm, id: Nm },
    /// `s_thr`, the line thermal: four lines along a square ring's sides,
    /// their ends rounded.
    Line { os: Nm, is: Nm },
    /// `rc_ths` and `rc_tho`: a rectangular ring `ring` wide.
    Rect {
        w: Nm,
        h: Nm,
        ring: Nm,
        corners: Corners,
        open: bool,
    },
    /// `o_ths`: an oval ring `ring` wide, its pieces' ends cut square.
    Oval { w: Nm, h: Nm, ring: Nm },
    /// `oblong_ths`: an oval ring `ring` wide, its pieces' ends rounded
    /// (`r`) or square (`s`).
    Oblong {
        w: Nm,
        h: Nm,
        ring: Nm,
        rounded: bool,
    },
}

/// A thermal: its ring less `spokes` gaps `gap` wide, the first at `angle`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Thermal {
    pub form: ThermalForm,
    pub angle: Mdeg,
    pub spokes: u32,
    pub gap: Nm,
}

/// Every standard symbol of ODB++ 8.1 this crate draws, and the trapezoid
/// ryueda's pads need. Sizes are whole widths and heights, centred on the
/// symbol's origin.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StandardSymbol {
    Round {
        d: Nm,
    },
    Square {
        s: Nm,
    },
    Rect {
        w: Nm,
        h: Nm,
        corners: Corners,
    },
    Oval {
        w: Nm,
        h: Nm,
    },
    Diamond {
        w: Nm,
        h: Nm,
    },
    Octagon {
        w: Nm,
        h: Nm,
        r: Nm,
    },
    DonutRound {
        od: Nm,
        id: Nm,
    },
    DonutSquare {
        od: Nm,
        id: Nm,
        corners: Corners,
    },
    DonutSquareRound {
        od: Nm,
        id: Nm,
    },
    DonutRect {
        w: Nm,
        h: Nm,
        lw: Nm,
        corners: Corners,
    },
    DonutOval {
        w: Nm,
        h: Nm,
        lw: Nm,
    },
    HexagonH {
        w: Nm,
        h: Nm,
        r: Nm,
    },
    HexagonV {
        w: Nm,
        h: Nm,
        r: Nm,
    },
    ButterflyRound {
        d: Nm,
    },
    ButterflySquare {
        s: Nm,
    },
    Triangle {
        base: Nm,
        h: Nm,
    },
    HalfOval {
        w: Nm,
        h: Nm,
    },
    Ellipse {
        w: Nm,
        h: Nm,
    },
    Moire {
        ring_width: Nm,
        ring_gap: Nm,
        rings: u32,
        line_width: Nm,
        line_length: Nm,
        line_angle: Mdeg,
    },
    Hole {
        d: Nm,
        plating: Plating,
        tol_plus: Nm,
        tol_minus: Nm,
    },
    Null {
        ext: u32,
    },
    Thermal(Thermal),
    /// ryueda's trapezoid: the left side `h + dx` tall and the right
    /// `h - dx`, the top `w + dy` wide and the bottom `w - dy`.
    Trapezoid {
        w: Nm,
        h: Nm,
        dx: Nm,
        dy: Nm,
    },
}

impl StandardSymbol {
    /// Reads an ODB++ standard symbol name in the units of the file that
    /// names it: mils in an inch file, microns in a millimetre one. `None`
    /// for a name that isn't a standard symbol (a user symbol, or nothing).
    pub fn parse(name: &str, units: NameUnits) -> Option<StandardSymbol> {
        parse::parse(name, units)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corners_by_number() {
        let c = CornerSet::of(&[1, 3]).unwrap();
        assert!(c.has(1) && c.has(3) && !c.has(2) && !c.has(4));
        assert!(!c.has(0) && !c.has(5));
        assert_eq!(CornerSet::of(&[1, 2, 3, 4]), Some(CornerSet::ALL));
        assert_eq!(CornerSet::of(&[]), Some(CornerSet::NONE));
        assert_eq!(CornerSet::of(&[5]), None);
        assert!(Corners::SHARP.is_sharp());
        assert!(!Corners::rounded(5, c).is_sharp());
        assert_eq!(Corners::chamfered(5, c).chamfer, 5);
    }
}
