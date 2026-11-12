//! ISO metric internal thread tolerances and limits (classes 4H to 8H
//! and 4G to 8G).
//!
//! Sources:
//! - T_D1 and EI are ISO 965-1's formulas (grade 6: 433 P − 190 P^1.22
//!   below 1 mm, 230 P^0.7 from 1 mm; grades 4, 5, 7 and 8 by 0.63,
//!   0.8, 1.25 and 1.6; EI(G) = 15 + 11 P µm). The rounding is ISO 3's
//!   R40, then whole micrometres.
//! - No printed table is the arbiter. The public-domain FED-STD-H28/21B
//!   prints no limits of size (it refers to ASME B1.13M's tables), and
//!   nothing is copied from ISO's or ASME's paid tables (the owner's
//!   ruling, design 533). The formulas are checked by hand at M6×1, M8×1.25
//!   and M10×1.5 (the tests below).
//! - Where the formulas are known to round otherwise than ISO 965-1's own
//!   printed T_D1 table: at P = 0.5 mm and P = 2.5 mm (every grade), the
//!   nearest-R40 rounding of the formula falls one R40 step below the
//!   printed tolerance, so the maximum minor diameter of those pitches
//!   (M3, M20, M22 coarse) is that much smaller here: tighter, never
//!   looser. Pitches below 0.35 mm were not compared. A project that needs
//!   the printed values sets the hole's `tolerance` or its own table.
//!
//! External classes arrive with ryucad 28.B's external cosmetic threads.

use crate::exact::{Big, nearest_r40, prod, round_div, whole_um};
use crate::iso_metric::{Size, basic_minor};
use crate::{Nm, Source, Valued};

/// A tolerance position of an internal thread.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Position {
    G,
    H,
}

/// An internal tolerance class: a grade 4 to 8 and a position.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Class {
    pub grade: u8,
    pub position: Position,
}

impl Class {
    /// The class a size takes when none is written.
    pub const DEFAULT: Class = Class {
        grade: 6,
        position: Position::H,
    };

    pub fn parse(text: &str) -> Option<Class> {
        let b = text.as_bytes();
        let [g @ b'4'..=b'8', p] = b else { return None };
        let position = match p {
            b'G' => Position::G,
            b'H' => Position::H,
            _ => return None,
        };
        Some(Class {
            grade: g - b'0',
            position,
        })
    }

    pub fn text(self) -> String {
        let p = match self.position {
            Position::G => 'G',
            Position::H => 'H',
        };
        format!("{}{p}", self.grade)
    }

    /// Every class, for `known` lists.
    pub fn all() -> Vec<Class> {
        let mut out = Vec::new();
        for position in [Position::H, Position::G] {
            for grade in 4..=8 {
                out.push(Class { grade, position });
            }
        }
        out
    }
}

/// T_D1 for grade 6 in hundredths of a micrometre, R40-rounded.
fn td1_6(pitch: Nm) -> u64 {
    let p = pitch as u128;
    if p >= 1_000_000 {
        // 230 (p/10^6)^0.7 >= n/d  <=>  230^10 p^7 d^10 >= n^10 10^42.
        nearest_r40(|n, d| prod(&[(230, 10), (p, 7), (d, 10)]) >= prod(&[(n, 10), (10, 42)]))
    } else {
        // 433 P - n/d >= 190 P^1.22, both sides to the 50th power.
        nearest_r40(|n, d| {
            let (lhs, t) = (433 * p * d, n * 1_000_000);
            if lhs < t {
                return false;
            }
            let left: Big = prod(&[(lhs - t, 50), (10, 366)]);
            left >= prod(&[(190, 50), (p, 61), (1_000_000 * d, 50)])
        })
    }
}

/// ISO 965-1's T_D1 for `grade` (4 to 8) at `pitch`.
pub fn td1(grade: u8, pitch: Nm) -> Valued<Nm> {
    let six = u128::from(whole_um(td1_6(pitch)));
    let um = match grade {
        6 => six as u64,
        g => {
            let (num, den): (u128, u128) = match g {
                4 => (63, 100),
                5 => (80, 100),
                7 => (125, 100),
                8 => (160, 100),
                _ => panic!("grades 4 to 8 only"),
            };
            whole_um(nearest_r40(|n, d| num * six * d >= n * den))
        }
    };
    Valued::new(um as Nm * 1_000, Source::Formula("ISO 965-1 T_D1"))
}

/// ISO 965-1's fundamental deviation EI.
pub fn ei(position: Position, pitch: Nm) -> Valued<Nm> {
    let um = match position {
        Position::H => 0,
        // 15 + 11 P µm with P in mm: (15·10^6 + 11 p) / 10^6.
        Position::G => round_div(15_000_000 + 11 * i128::from(pitch), 1_000_000) as Nm,
    };
    Valued::new(um * 1_000, Source::Formula("ISO 965-1 EI"))
}

/// An internal thread's limits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// The minor diameter's least and greatest.
    pub minor: (Valued<Nm>, Valued<Nm>),
    /// The major diameter's least (internal threads have no greatest).
    pub major_min: Valued<Nm>,
}

/// Which printed limit a correction replaces (`unified::PRINTED`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Which {
    MinorMin,
    MinorMax,
    MajorMin,
}

/// The limits of `size` at `pitch` in `class`.
pub fn internal_limits(size: &Size, pitch: Nm, class: Class) -> Limits {
    let formula = Source::Formula("ISO 965-1");
    let e = ei(class.position, pitch).value;
    let d1 = basic_minor(size.major, pitch).value;
    Limits {
        minor: (
            Valued::new(d1 + e, formula),
            Valued::new(d1 + e + td1(class.grade, pitch).value, formula),
        ),
        major_min: Valued::new(size.major + e, formula),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::iso_metric::size;

    fn limits(name: &str, pitch: Nm, class: &str) -> (Nm, Nm, Nm) {
        let l = internal_limits(size(name).unwrap(), pitch, Class::parse(class).unwrap());
        (l.minor.0.value, l.minor.1.value, l.major_min.value)
    }

    #[test]
    fn tolerances_by_the_formulas() {
        // The formulas at P = 1 mm, worked by hand: 150, 190, 236, 300, 375.
        let got: Vec<Nm> = [4, 5, 6, 7, 8].map(|g| td1(g, 1_000_000).value).to_vec();
        assert_eq!(got, [150_000, 190_000, 236_000, 300_000, 375_000]);
        assert_eq!(td1(6, 1_250_000).value, 265_000);
        assert_eq!(td1(6, 1_500_000).value, 300_000);
        assert_eq!(ei(Position::G, 1_000_000).value, 26_000);
        assert_eq!(ei(Position::H, 1_000_000).value, 0);
        assert_eq!(
            td1(6, 1_000_000).source,
            crate::Source::Formula("ISO 965-1 T_D1")
        );
    }

    #[test]
    fn m6_6h_as_the_issue_has_it() {
        // M6×1-6H by hand: minor 4.917 to 5.153, major at least 6.000.
        assert_eq!(
            limits("M6", 1_000_000, "6H"),
            (4_917_000, 5_153_000, 6_000_000)
        );
        assert_eq!(
            limits("M6", 1_000_000, "6G"),
            (4_943_000, 5_179_000, 6_026_000)
        );
        assert_eq!(
            limits("M8", 1_250_000, "6H"),
            (6_647_000, 6_912_000, 8_000_000)
        );
        assert_eq!(
            limits("M10", 1_500_000, "6H"),
            (8_376_000, 8_676_000, 10_000_000)
        );
    }

    #[test]
    fn classes_read_and_write_back() {
        for t in ["4H", "5H", "6H", "7H", "8H", "4G", "6G", "8G"] {
            assert_eq!(Class::parse(t).unwrap().text(), t);
        }
        for t in ["6h", "6g", "3H", "9H", "6", "H6", "6HH", "5H6H"] {
            assert_eq!(Class::parse(t), None, "{t}");
        }
        assert_eq!(Class::DEFAULT.text(), "6H");
    }
}
