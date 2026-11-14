//! Standard drill series.
//!
//! Sources: facts. Metric drills run 0.20 to 3.00 mm by 0.05, 3.0 to
//! 20.0 mm by 0.1, and 20.0 to 65.0 mm by 0.5. Fractional drills run
//! 1/64 to 1-3/4 in by 1/64. The number (#80 to #1) and letter (A to Z)
//! gauge sizes are the common gauge, as Wikipedia's "Drill bit sizes"
//! tabulates it, cross-checked against the drill columns of FED-STD-H28/2B
//! (public domain) Table II.A.1, which agree for every gauge they list
//! (#2 to #56, F to Z).

use crate::{NM_PER_IN, Nm};
use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Series {
    Metric,
    Fractional,
    Number,
    Letter,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Drill {
    /// `5`, `6.8`; `27/64`, `1-11/32`; `#7`; `F`.
    pub name: String,
    pub nm: Nm,
    pub series: Series,
}

/// Number gauge #1 to #80, in ten-thousandths of an inch.
const NUMBER: [i64; 80] = [
    2280, 2210, 2130, 2090, 2055, 2040, 2010, 1990, 1960, 1935, 1910, 1890, 1850, 1820, 1800, 1770,
    1730, 1695, 1660, 1610, 1590, 1570, 1540, 1520, 1495, 1470, 1440, 1405, 1360, 1285, 1200, 1160,
    1130, 1110, 1100, 1065, 1040, 1015, 995, 980, 960, 935, 890, 860, 820, 810, 785, 760, 730, 700,
    670, 635, 595, 550, 520, 465, 430, 420, 410, 400, 390, 380, 370, 360, 350, 330, 320, 310, 292,
    280, 260, 250, 240, 225, 210, 200, 180, 160, 145, 135,
];

/// Letter gauge A to Z, in ten-thousandths of an inch.
const LETTER: [i64; 26] = [
    2340, 2380, 2420, 2460, 2500, 2570, 2610, 2660, 2720, 2770, 2810, 2900, 2950, 3020, 3160, 3230,
    3320, 3390, 3480, 3580, 3680, 3770, 3860, 3970, 4040, 4130,
];

/// Every standard drill: metric by size, then inch drills by size.
pub fn all() -> &'static [Drill] {
    static ALL: OnceLock<Vec<Drill>> = OnceLock::new();
    ALL.get_or_init(|| {
        let mut out = Vec::new();
        let metric = |out: &mut Vec<Drill>, from: i64, to: i64, step: i64| {
            let mut nm = from;
            while nm <= to {
                if out.last().is_none_or(|d: &Drill| d.nm < nm) {
                    out.push(Drill {
                        name: crate::mm_decimal(nm),
                        nm,
                        series: Series::Metric,
                    });
                }
                nm += step;
            }
        };
        metric(&mut out, 200_000, 3_000_000, 50_000);
        metric(&mut out, 3_000_000, 20_000_000, 100_000);
        metric(&mut out, 20_000_000, 65_000_000, 500_000);
        let mut inch = Vec::new();
        for n in 1..=112i64 {
            let (w, r) = (n / 64, n % 64);
            let g = gcd(r, 64);
            let name = match (w, r) {
                (w, 0) => w.to_string(),
                (0, r) => format!("{}/{}", r / g, 64 / g),
                (w, r) => format!("{w}-{}/{}", r / g, 64 / g),
            };
            inch.push(Drill {
                name,
                nm: NM_PER_IN * n / 64,
                series: Series::Fractional,
            });
        }
        for (i, t) in NUMBER.iter().enumerate() {
            inch.push(Drill {
                name: format!("#{}", i + 1),
                nm: t * 2_540,
                series: Series::Number,
            });
        }
        for (i, t) in LETTER.iter().enumerate() {
            let name = char::from(b'A' + i as u8).to_string();
            inch.push(Drill {
                name,
                nm: t * 2_540,
                series: Series::Letter,
            });
        }
        // By size; where two series share a size, fractional, then
        // number, then letter (the stable sort keeps that order).
        inch.sort_by_key(|d| d.nm);
        out.extend(inch);
        out
    })
}

fn gcd(a: i64, b: i64) -> i64 {
    if b == 0 { a } else { gcd(b, a % b) }
}

/// The drills of one system: metric, or inch.
pub fn of(inch: bool) -> impl Iterator<Item = &'static Drill> {
    all()
        .iter()
        .filter(move |d| (d.series != Series::Metric) == inch)
}

/// The standard drill of exactly this size in that system, if any.
pub fn standard(nm: Nm, inch: bool) -> Option<&'static Drill> {
    of(inch).find(|d| d.nm == nm)
}

pub fn by_name(name: &str) -> Option<&'static Drill> {
    all().iter().find(|d| d.name == name)
}

/// The standard drill of that system nearest `nm`, a tie going to the
/// larger (a tap cuts a thread more easily in it).
pub fn nearest(nm: Nm, inch: bool) -> &'static Drill {
    of(inch)
        .min_by_key(|d| ((d.nm - nm).abs(), core::cmp::Reverse(d.nm)))
        .expect("every system has drills")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_series() {
        assert_eq!(standard(5_000_000, false).unwrap().name, "5");
        assert_eq!(standard(6_750_000, false), None, "0.1 mm steps above 3 mm");
        assert_eq!(standard(2_050_000, false).unwrap().name, "2.05");
        for (name, inch) in [
            ("#7", "0.201"),
            ("#29", "0.136"),
            ("#36", "0.1065"),
            ("#43", "0.089"),
            ("#21", "0.159"),
            ("#25", "0.1495"),
            ("#3", "0.213"),
            ("#1", "0.228"),
            ("#80", "0.0135"),
            ("A", "0.234"),
            ("F", "0.257"),
            ("Q", "0.332"),
            ("U", "0.368"),
            ("Z", "0.413"),
        ] {
            let d = by_name(name).unwrap();
            assert_eq!(crate::in_decimal(d.nm).unwrap(), inch, "{name}");
        }
        // 27/64 in is no whole number of hundred-thousandths.
        assert_eq!(by_name("27/64").unwrap().nm, crate::NM_PER_IN * 27 / 64);
        let inch: Vec<&Drill> = all()
            .iter()
            .filter(|d| d.series != Series::Metric)
            .collect();
        assert!(inch.windows(2).all(|w| w[0].nm <= w[1].nm));
    }

    #[test]
    fn the_nearest_drill() {
        // 5.04 mm: 5.0 is nearer than 5.1.
        assert_eq!(nearest(5_040_000, false).name, "5");
        // Halfway between 5.0 and 5.1 (the series steps 0.1 mm from 3 to
        // 20 mm): the larger, which a tap prefers.
        assert_eq!(nearest(5_050_000, false).name, "5.1");
        assert_eq!(nearest(5_075_000, false).name, "5.1");
        // Inch: 0.2015 in is nearer #7 (0.2010) than 13/64 (0.2031).
        assert_eq!(nearest(5_118_100, true).name, "#7");
    }
}
