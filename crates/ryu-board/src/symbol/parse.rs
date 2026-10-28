//! ODB++ standard symbol names.

use super::{CornerSet, Corners, Plating, StandardSymbol, Thermal, ThermalForm};
use crate::geom::decimal::{self, Unit};
use crate::geom::{Mdeg, Nm};

/// The units of the numbers in a symbol's name: mils in an inch file,
/// microns in a millimetre one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NameUnits {
    Mil,
    Micron,
}

impl NameUnits {
    fn unit(self) -> Unit {
        match self {
            NameUnits::Mil => Unit::Mil,
            NameUnits::Micron => Unit::Micron,
        }
    }
}

/// Prefixes, longest of each family first, so `rect` is never read as `r`.
const PREFIXES: &[&str] = &[
    "oblong_ths",
    "sr_ths",
    "donut_rc",
    "donut_sr",
    "donut_s",
    "donut_r",
    "donut_o",
    "rc_ths",
    "rc_tho",
    "s_ths",
    "s_tho",
    "s_thr",
    "o_ths",
    "oval_h",
    "oval",
    "hex_l",
    "hex_s",
    "moire",
    "hole",
    "null",
    "rect",
    "oct",
    "tri",
    "thr",
    "ths",
    "bfr",
    "bfs",
    "di",
    "el",
    "r",
    "s",
];

struct Fields<'a> {
    tokens: Vec<&'a str>,
    units: NameUnits,
}

impl Fields<'_> {
    fn len(&self, i: usize) -> Option<Nm> {
        let v = decimal::nm(self.tokens.get(i)?, self.units.unit())
            .ok()?
            .value;
        (v >= 0).then_some(v)
    }

    fn angle(&self, i: usize) -> Option<Mdeg> {
        Some(decimal::mdeg(self.tokens.get(i)?).ok()?.0)
    }

    fn count(&self, i: usize) -> Option<u32> {
        let t = self.tokens.get(i)?;
        if t.is_empty() || !t.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        t.parse().ok()
    }

    /// Exactly `n` tokens.
    fn exactly(&self, n: usize) -> Option<()> {
        (self.tokens.len() == n).then_some(())
    }

    /// The optional `xr<rad>[x<corners>]`, or `xc<size>[x<corners>]` where
    /// `chamfers` allows it (rectangles only), after `n` fixed tokens, and
    /// nothing more.
    fn corners(&self, n: usize, chamfers: bool) -> Option<Corners> {
        if self.tokens.len() < n {
            return None;
        }
        let rest = &self.tokens[n..];
        let Some((first, more)) = rest.split_first() else {
            return Some(Corners::SHARP);
        };
        let which = match more {
            [] => CornerSet::ALL,
            [digits] => {
                let ds: Vec<u8> = digits.bytes().map(|b| b.wrapping_sub(b'0')).collect();
                if ds.is_empty() {
                    return None;
                }
                CornerSet::of(&ds)?
            }
            _ => return None,
        };
        let size = |t: &str| -> Option<Nm> {
            let v = decimal::nm(t, self.units.unit()).ok()?.value;
            (v >= 0).then_some(v)
        };
        if let Some(t) = first.strip_prefix('r') {
            Some(Corners::rounded(size(t)?, which))
        } else if let Some(t) = first.strip_prefix('c').filter(|_| chamfers) {
            Some(Corners::chamfered(size(t)?, which))
        } else {
            None
        }
    }
}

pub(super) fn parse(name: &str, units: NameUnits) -> Option<StandardSymbol> {
    let lower = name.to_ascii_lowercase();
    let prefix = PREFIXES.iter().find(|p| {
        lower
            .strip_prefix(**p)
            .and_then(|r| r.bytes().next())
            .is_some_and(|b| b.is_ascii_digit() || b == b'.')
    })?;
    let rest = &lower[prefix.len()..];
    let f = Fields {
        tokens: rest.split('x').collect(),
        units,
    };
    use StandardSymbol as S;
    let thermal = |form: ThermalForm, at: usize| -> Option<StandardSymbol> {
        Some(S::Thermal(Thermal {
            form,
            angle: f.angle(at)?,
            spokes: f.count(at + 1)?,
            gap: f.len(at + 2)?,
        }))
    };
    Some(match *prefix {
        "r" => f.exactly(1).and(Some(S::Round { d: f.len(0)? }))?,
        "s" => f.exactly(1).and(Some(S::Square { s: f.len(0)? }))?,
        "rect" => S::Rect {
            w: f.len(0)?,
            h: f.len(1)?,
            corners: f.corners(2, true)?,
        },
        "oval" => f.exactly(2).and(Some(S::Oval {
            w: f.len(0)?,
            h: f.len(1)?,
        }))?,
        "di" => f.exactly(2).and(Some(S::Diamond {
            w: f.len(0)?,
            h: f.len(1)?,
        }))?,
        "oct" => f.exactly(3).and(Some(S::Octagon {
            w: f.len(0)?,
            h: f.len(1)?,
            r: f.len(2)?,
        }))?,
        "donut_r" => f.exactly(2).and(Some(S::DonutRound {
            od: f.len(0)?,
            id: f.len(1)?,
        }))?,
        "donut_s" => S::DonutSquare {
            od: f.len(0)?,
            id: f.len(1)?,
            corners: f.corners(2, false)?,
        },
        "donut_sr" => f.exactly(2).and(Some(S::DonutSquareRound {
            od: f.len(0)?,
            id: f.len(1)?,
        }))?,
        "donut_rc" => S::DonutRect {
            w: f.len(0)?,
            h: f.len(1)?,
            lw: f.len(2)?,
            corners: f.corners(3, false)?,
        },
        "donut_o" => f.exactly(3).and(Some(S::DonutOval {
            w: f.len(0)?,
            h: f.len(1)?,
            lw: f.len(2)?,
        }))?,
        "hex_l" => f.exactly(3).and(Some(S::HexagonH {
            w: f.len(0)?,
            h: f.len(1)?,
            r: f.len(2)?,
        }))?,
        "hex_s" => f.exactly(3).and(Some(S::HexagonV {
            w: f.len(0)?,
            h: f.len(1)?,
            r: f.len(2)?,
        }))?,
        "bfr" => f.exactly(1).and(Some(S::ButterflyRound { d: f.len(0)? }))?,
        "bfs" => f
            .exactly(1)
            .and(Some(S::ButterflySquare { s: f.len(0)? }))?,
        "tri" => f.exactly(2).and(Some(S::Triangle {
            base: f.len(0)?,
            h: f.len(1)?,
        }))?,
        "oval_h" => f.exactly(2).and(Some(S::HalfOval {
            w: f.len(0)?,
            h: f.len(1)?,
        }))?,
        "el" => f.exactly(2).and(Some(S::Ellipse {
            w: f.len(0)?,
            h: f.len(1)?,
        }))?,
        "moire" => f.exactly(6).and(Some(S::Moire {
            ring_width: f.len(0)?,
            ring_gap: f.len(1)?,
            rings: f.count(2)?,
            line_width: f.len(3)?,
            line_length: f.len(4)?,
            line_angle: f.angle(5)?,
        }))?,
        "hole" => {
            f.exactly(4)?;
            let plating = match f.tokens[1] {
                "p" => Plating::Plated,
                "n" => Plating::NonPlated,
                "v" => Plating::Via,
                _ => return None,
            };
            S::Hole {
                d: f.len(0)?,
                plating,
                tol_plus: f.len(2)?,
                tol_minus: f.len(3)?,
            }
        }
        "null" => f.exactly(1).and(Some(S::Null { ext: f.count(0)? }))?,
        "thr" => {
            f.exactly(5)?;
            thermal(
                ThermalForm::RoundRounded {
                    od: f.len(0)?,
                    id: f.len(1)?,
                },
                2,
            )?
        }
        "ths" => {
            f.exactly(5)?;
            thermal(
                ThermalForm::RoundSquared {
                    od: f.len(0)?,
                    id: f.len(1)?,
                },
                2,
            )?
        }
        "s_ths" | "s_tho" => thermal(
            ThermalForm::Square {
                os: f.len(0)?,
                is: f.len(1)?,
                corners: f.corners(5, false)?,
                open: *prefix == "s_tho",
            },
            2,
        )?,
        "sr_ths" => {
            f.exactly(5)?;
            thermal(
                ThermalForm::SquareRound {
                    os: f.len(0)?,
                    id: f.len(1)?,
                },
                2,
            )?
        }
        "s_thr" => {
            f.exactly(5)?;
            thermal(
                ThermalForm::Line {
                    os: f.len(0)?,
                    is: f.len(1)?,
                },
                2,
            )?
        }
        "oblong_ths" => {
            f.exactly(7)?;
            let rounded = match f.tokens[6] {
                "r" => true,
                "s" => false,
                _ => return None,
            };
            thermal(
                ThermalForm::Oblong {
                    w: f.len(0)?,
                    h: f.len(1)?,
                    ring: f.len(5)?,
                    rounded,
                },
                2,
            )?
        }
        "rc_ths" | "rc_tho" => thermal(
            ThermalForm::Rect {
                w: f.len(0)?,
                h: f.len(1)?,
                ring: f.len(5)?,
                corners: f.corners(6, false)?,
                open: *prefix == "rc_tho",
            },
            2,
        )?,
        "o_ths" => {
            f.exactly(6)?;
            thermal(
                ThermalForm::Oval {
                    w: f.len(0)?,
                    h: f.len(1)?,
                    ring: f.len(5)?,
                },
                2,
            )?
        }
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mil(name: &str) -> Option<StandardSymbol> {
        parse(name, NameUnits::Mil)
    }

    fn um(name: &str) -> Option<StandardSymbol> {
        parse(name, NameUnits::Micron)
    }

    #[test]
    fn rounds_and_rectangles() {
        assert_eq!(
            mil("r39.3701"),
            Some(StandardSymbol::Round { d: 1_000_001 })
        );
        assert_eq!(um("r100"), Some(StandardSymbol::Round { d: 100_000 }));
        assert_eq!(um("s50"), Some(StandardSymbol::Square { s: 50_000 }));
        assert_eq!(
            mil("rect129.9213x90.5512"),
            Some(StandardSymbol::Rect {
                w: 3_300_001,
                h: 2_300_000,
                corners: Corners::SHARP
            })
        );
        assert_eq!(
            um("rect20x10xr2x13"),
            Some(StandardSymbol::Rect {
                w: 20_000,
                h: 10_000,
                corners: Corners::rounded(2_000, CornerSet::of(&[1, 3]).unwrap()),
            })
        );
        assert_eq!(
            um("rect20x10xc3"),
            Some(StandardSymbol::Rect {
                w: 20_000,
                h: 10_000,
                corners: Corners::chamfered(3_000, CornerSet::ALL),
            })
        );
        assert_eq!(um("RECT20X10"), um("rect20x10"));
    }

    #[test]
    fn thermals() {
        assert_eq!(
            mil("thr60x40x45x4x10"),
            Some(StandardSymbol::Thermal(Thermal {
                form: ThermalForm::RoundRounded {
                    od: 1_524_000,
                    id: 1_016_000
                },
                angle: Mdeg::new(45_000),
                spokes: 4,
                gap: 254_000,
            }))
        );
        let Some(StandardSymbol::Thermal(t)) = um("rc_tho100x60x0x4x10x8xr5") else {
            panic!()
        };
        assert_eq!(
            t.form,
            ThermalForm::Rect {
                w: 100_000,
                h: 60_000,
                ring: 8_000,
                corners: Corners::rounded(5_000, CornerSet::ALL),
                open: true,
            }
        );
        let thermal = |n: &str| match um(n) {
            Some(StandardSymbol::Thermal(t)) => Some(t.form),
            _ => None,
        };
        assert_eq!(
            thermal("s_thr100x60x45x4x10"),
            Some(ThermalForm::Line {
                os: 100_000,
                is: 60_000
            })
        );
        assert_eq!(
            thermal("sr_ths100x60x45x4x10"),
            Some(ThermalForm::SquareRound {
                os: 100_000,
                id: 60_000
            })
        );
        assert_eq!(
            thermal("oblong_ths100x60x0x2x10x8xr"),
            Some(ThermalForm::Oblong {
                w: 100_000,
                h: 60_000,
                ring: 8_000,
                rounded: true
            })
        );
        // Donuts and thermals round their corners; only rectangles chamfer.
        assert_eq!(um("donut_s50x30xc4"), None);
        assert_eq!(thermal("s_ths100x60x45x4x10xc5"), None);
        assert!(matches!(
            um("o_ths100x60x0x2x10x8"),
            Some(StandardSymbol::Thermal(_))
        ));
    }

    #[test]
    fn the_rest() {
        assert_eq!(
            um("donut_r50x30"),
            Some(StandardSymbol::DonutRound {
                od: 50_000,
                id: 30_000
            })
        );
        assert!(matches!(
            um("donut_s50x30xr4"),
            Some(StandardSymbol::DonutSquare { .. })
        ));
        assert!(matches!(
            um("donut_sr50x30"),
            Some(StandardSymbol::DonutSquareRound { .. })
        ));
        assert!(matches!(
            um("donut_rc50x30x5"),
            Some(StandardSymbol::DonutRect { .. })
        ));
        assert!(matches!(
            um("donut_o50x30x5"),
            Some(StandardSymbol::DonutOval { .. })
        ));
        assert!(matches!(
            um("oval_h50x30"),
            Some(StandardSymbol::HalfOval { .. })
        ));
        assert!(matches!(
            um("hex_l50x30x10"),
            Some(StandardSymbol::HexagonH { .. })
        ));
        assert!(matches!(
            um("moire10x5x3x2x80x45"),
            Some(StandardSymbol::Moire { rings: 3, .. })
        ));
        assert_eq!(
            um("hole30xpx1x2"),
            Some(StandardSymbol::Hole {
                d: 30_000,
                plating: Plating::Plated,
                tol_plus: 1_000,
                tol_minus: 2_000
            })
        );
        assert_eq!(um("null7"), Some(StandardSymbol::Null { ext: 7 }));
    }

    #[test]
    fn names_that_are_not_standard() {
        for n in [
            "",
            "r",
            "rect10",
            "rect10x5x",
            "rect10x5xq2",
            "rect10x5xr2x15",
            "thr1x2x3x4",
            "hole30xqx1x2",
            "my_pad",
            "oblong_ths1x2x0x2x1x1xq",
            "hplate10x5x1",
            "r-5",
            "rx5",
            "s5x5",
            "oval1x2x3",
            "rect10x5xr2x1x2",
        ] {
            assert_eq!(um(n), None, "{n}");
        }
    }
}
