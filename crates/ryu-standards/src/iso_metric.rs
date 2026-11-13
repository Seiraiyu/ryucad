//! ISO metric screw threads (M profile): sizes, pitches and basic
//! diameters.
//!
//! Sources:
//! - Nominal sizes and their coarse and fine pitches are facts. To M64
//!   they are ISO 261's general-purpose selection, common knowledge as
//!   Wikipedia's "ISO metric screw thread" tabulates it (first, second and
//!   third choice sizes, each with its coarse pitch and its fine ones):
//!   so M1, M1.2, M1.4, M1.8, M2.2 and M4.5, and fine pitches such as
//!   M8×0.75, M10×1 and ×0.75, M12×1.5 and ×1, M14×1.25 and ×1, M16×1,
//!   M18×2 and ×1, up to M64×4, ×3 and ×1.5. Cross-checked against
//!   FED-STD-H28/21B (28 November 1984, public domain) Table XXI.1, its
//!   standard fastener series: every size and pitch it lists is here
//!   (`tests/h28_21.rs`); the rest of the selection it doesn't print, and
//!   was checked against the tabulation alone. Table XXI.1 is printed for
//!   the MJ profile, whose sizes and pitches are the M profile's. Sizes
//!   above M64 are XXI.1's, with only the pitches it prints. Fine pitches
//!   outside ISO 261's selection (H28/21B's Table XXI.2, such as M15×1)
//!   are not built in: a project's table adds them.
//! - Basic diameters are formulas: ISO 68-1 and ISO 724's basic profile,
//!   H = √3/2 · P, so D1 = D − 2 · 5H/8 = D − 5√3/8 · P. The result is
//!   rounded to the micrometre.
//!
//! Tolerances and limits are in `iso_metric_limits` (`internal_limits`).

use crate::exact::{isqrt, round_div};
use crate::{Nm, Source, Valued};

/// A nominal size and its pitches, coarse first.
#[derive(Debug, PartialEq, Eq)]
pub struct Size {
    pub name: &'static str,
    pub major: Nm,
    pub pitches: &'static [Nm],
}

const fn mm(whole: i64, thousandths: i64) -> Nm {
    whole * 1_000_000 + thousandths * 1_000
}

/// Every size, by major diameter: ISO 261's selection, coarse and fine,
/// to M64, and H28/21B Table XXI.1's sizes above it.
pub const SIZES: &[Size] = &[
    Size {
        name: "M1",
        major: mm(1, 0),
        pitches: &[mm(0, 250), mm(0, 200)],
    },
    Size {
        name: "M1.2",
        major: mm(1, 200),
        pitches: &[mm(0, 250), mm(0, 200)],
    },
    Size {
        name: "M1.4",
        major: mm(1, 400),
        pitches: &[mm(0, 300), mm(0, 200)],
    },
    Size {
        name: "M1.6",
        major: mm(1, 600),
        pitches: &[mm(0, 350), mm(0, 200)],
    },
    Size {
        name: "M1.8",
        major: mm(1, 800),
        pitches: &[mm(0, 350), mm(0, 200)],
    },
    Size {
        name: "M2",
        major: mm(2, 0),
        pitches: &[mm(0, 400), mm(0, 250)],
    },
    Size {
        name: "M2.2",
        major: mm(2, 200),
        pitches: &[mm(0, 450), mm(0, 250)],
    },
    Size {
        name: "M2.5",
        major: mm(2, 500),
        pitches: &[mm(0, 450), mm(0, 350)],
    },
    Size {
        name: "M3",
        major: mm(3, 0),
        pitches: &[mm(0, 500), mm(0, 350)],
    },
    Size {
        name: "M3.5",
        major: mm(3, 500),
        pitches: &[mm(0, 600), mm(0, 350)],
    },
    Size {
        name: "M4",
        major: mm(4, 0),
        pitches: &[mm(0, 700), mm(0, 500)],
    },
    Size {
        name: "M4.5",
        major: mm(4, 500),
        pitches: &[mm(0, 750), mm(0, 500)],
    },
    Size {
        name: "M5",
        major: mm(5, 0),
        pitches: &[mm(0, 800), mm(0, 500)],
    },
    Size {
        name: "M6",
        major: mm(6, 0),
        pitches: &[mm(1, 0), mm(0, 750)],
    },
    Size {
        name: "M7",
        major: mm(7, 0),
        pitches: &[mm(1, 0), mm(0, 750)],
    },
    Size {
        name: "M8",
        major: mm(8, 0),
        pitches: &[mm(1, 250), mm(1, 0), mm(0, 750)],
    },
    Size {
        name: "M10",
        major: mm(10, 0),
        pitches: &[mm(1, 500), mm(1, 250), mm(1, 0), mm(0, 750)],
    },
    Size {
        name: "M12",
        major: mm(12, 0),
        pitches: &[mm(1, 750), mm(1, 500), mm(1, 250), mm(1, 0)],
    },
    Size {
        name: "M14",
        major: mm(14, 0),
        pitches: &[mm(2, 0), mm(1, 500), mm(1, 250), mm(1, 0)],
    },
    Size {
        name: "M16",
        major: mm(16, 0),
        pitches: &[mm(2, 0), mm(1, 500), mm(1, 0)],
    },
    Size {
        name: "M18",
        major: mm(18, 0),
        pitches: &[mm(2, 500), mm(2, 0), mm(1, 500), mm(1, 0)],
    },
    Size {
        name: "M20",
        major: mm(20, 0),
        pitches: &[mm(2, 500), mm(2, 0), mm(1, 500), mm(1, 0)],
    },
    Size {
        name: "M22",
        major: mm(22, 0),
        pitches: &[mm(2, 500), mm(2, 0), mm(1, 500), mm(1, 0)],
    },
    Size {
        name: "M24",
        major: mm(24, 0),
        pitches: &[mm(3, 0), mm(2, 0), mm(1, 500), mm(1, 0)],
    },
    Size {
        name: "M27",
        major: mm(27, 0),
        pitches: &[mm(3, 0), mm(2, 0), mm(1, 500), mm(1, 0)],
    },
    Size {
        name: "M30",
        major: mm(30, 0),
        pitches: &[mm(3, 500), mm(3, 0), mm(2, 0), mm(1, 500), mm(1, 0)],
    },
    Size {
        name: "M33",
        major: mm(33, 0),
        pitches: &[mm(3, 500), mm(3, 0), mm(2, 0), mm(1, 500)],
    },
    Size {
        name: "M36",
        major: mm(36, 0),
        pitches: &[mm(4, 0), mm(3, 0), mm(2, 0), mm(1, 500)],
    },
    Size {
        name: "M39",
        major: mm(39, 0),
        pitches: &[mm(4, 0), mm(3, 0), mm(2, 0), mm(1, 500)],
    },
    Size {
        name: "M42",
        major: mm(42, 0),
        pitches: &[mm(4, 500), mm(4, 0), mm(3, 0), mm(2, 0), mm(1, 500)],
    },
    Size {
        name: "M45",
        major: mm(45, 0),
        pitches: &[mm(4, 500), mm(4, 0), mm(3, 0), mm(2, 0), mm(1, 500)],
    },
    Size {
        name: "M48",
        major: mm(48, 0),
        pitches: &[mm(5, 0), mm(4, 0), mm(3, 0), mm(2, 0), mm(1, 500)],
    },
    Size {
        name: "M52",
        major: mm(52, 0),
        pitches: &[mm(5, 0), mm(4, 0), mm(3, 0), mm(2, 0), mm(1, 500)],
    },
    Size {
        name: "M56",
        major: mm(56, 0),
        pitches: &[mm(5, 500), mm(4, 0), mm(3, 0), mm(2, 0), mm(1, 500)],
    },
    Size {
        name: "M60",
        major: mm(60, 0),
        pitches: &[mm(5, 500), mm(4, 0), mm(3, 0), mm(2, 0), mm(1, 500)],
    },
    Size {
        name: "M64",
        major: mm(64, 0),
        pitches: &[mm(6, 0), mm(4, 0), mm(3, 0), mm(2, 0), mm(1, 500)],
    },
    Size {
        name: "M72",
        major: mm(72, 0),
        pitches: &[mm(6, 0), mm(2, 0)],
    },
    Size {
        name: "M80",
        major: mm(80, 0),
        pitches: &[mm(6, 0), mm(2, 0)],
    },
    Size {
        name: "M90",
        major: mm(90, 0),
        pitches: &[mm(6, 0), mm(2, 0)],
    },
    Size {
        name: "M100",
        major: mm(100, 0),
        pitches: &[mm(6, 0), mm(2, 0)],
    },
];

/// The size named exactly so (`M6`, `M1.6`).
pub fn size(name: &str) -> Option<&'static Size> {
    SIZES.iter().find(|s| s.name == name)
}

/// ISO 724's basic minor diameter, D − 5√3/8 · P, to the micrometre.
pub fn basic_minor(major: Nm, pitch: Nm) -> Valued<Nm> {
    // 8 · (5√3/8 · P) in thousandths of a nanometre: √(75 P²) · 1000.
    let p = pitch as u128;
    let x8 = isqrt(75 * p * p * 1_000_000) as i128;
    let um = round_div(i128::from(major) * 8_000 - x8, 8_000_000);
    Valued::new(um as Nm * 1_000, Source::Formula("ISO 724 D1"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NM_PER_MM;

    #[test]
    fn sizes_and_their_pitches() {
        let m6 = size("M6").unwrap();
        assert_eq!((m6.major, m6.pitches[0]), (6 * NM_PER_MM, NM_PER_MM));
        assert_eq!(size("M8").unwrap().pitches, [1_250_000, 1_000_000, 750_000]);
        assert_eq!(
            size("m6"),
            None,
            "sizes are written as the standard writes them"
        );
        assert_eq!(size("M6.5"), None);
        assert_eq!(SIZES.first().unwrap().name, "M1");
        assert_eq!(SIZES.last().unwrap().name, "M100");
        // In order of size, each once, coarse pitch first and largest.
        for w in SIZES.windows(2) {
            assert!(w[0].major < w[1].major, "{}", w[1].name);
        }
        for s in SIZES {
            assert!(s.pitches.windows(2).all(|w| w[0] > w[1]), "{}", s.name);
        }
    }

    #[test]
    fn basic_minor_diameters_by_iso_724() {
        // D1 = D - 5√3/8 P, to the micrometre.
        for (name, pitch, d1) in [
            ("M6", 1_000_000, 4_917_000),
            ("M8", 1_250_000, 6_647_000),
            ("M10", 1_500_000, 8_376_000),
            ("M1", 250_000, 729_000),
            ("M64", 6_000_000, 57_505_000),
        ] {
            let s = size(name).unwrap();
            assert_eq!(basic_minor(s.major, pitch).value, d1, "{name}");
        }
        assert_eq!(
            basic_minor(6 * NM_PER_MM, NM_PER_MM).source,
            crate::Source::Formula("ISO 724 D1")
        );
    }
}
