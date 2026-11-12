//! Tap drills by ryucad's rule, a formula and not a copied table
//! (design 533, Deriving sizes).
//!
//! Sources:
//! - Metric: D − P, then the smallest standard drill at or above it
//!   within the class's minor limits.
//! - Unified: the standard drill nearest D − 9√3/16 · p (75% of the
//!   sharp-V depth 3√3/4 · p, the common shop formula), within the
//!   class's minor limits.
//! - Engagement e%: D − 5√3/8 · P · e/100 (ISO 724's H1 scaled), then the
//!   smallest standard drill at or above it.
//!
//! Every comparison against a √3 target is exact. Pipe threads have no
//! rule: their tap drill is a project's.

use crate::Nm;
use crate::drill::{self, Drill};
use crate::thread::{Standard, Thread};

/// A tap drill: a standard drill, or the computed size when none
/// qualifies.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TapDrill {
    pub name: String,
    pub nm: Nm,
    pub standard: bool,
}

impl TapDrill {
    fn of(d: &Drill) -> TapDrill {
        TapDrill {
            name: d.name.clone(),
            nm: d.nm,
            standard: true,
        }
    }

    fn computed(nm: Nm) -> TapDrill {
        TapDrill {
            name: crate::mm_decimal(nm),
            nm,
            standard: false,
        }
    }
}

/// Whether c·√3 ≥ x for c ≥ 0, exactly: 3c² ≥ x² when x > 0.
fn root3_at_least(c_num: i128, c_den: i128, x: i128) -> bool {
    // (c_num/c_den)·√3 >= x  <=>  3 c_num² >= x² c_den² (x > 0).
    x <= 0 || 3 * c_num * c_num >= x * x * c_den * c_den
}

/// The default rule; none for pipe threads.
pub fn rule(t: &Thread) -> Option<TapDrill> {
    let major = i128::from(t.major?.value);
    let (lo, hi) = t.minor.map(|(a, b)| (a.value, b.value))?;
    let (pn, pd) = t.pitch.value.nm();
    let within = |d: &&Drill| lo <= d.nm && d.nm <= hi;
    match t.standard {
        Standard::IsoMetric => {
            let target = (major - pn / pd) as Nm;
            Some(
                drill::of(false)
                    .filter(within)
                    .find(|d| d.nm >= target)
                    .map_or_else(|| TapDrill::computed(target), TapDrill::of),
            )
        }
        Standard::Unc | Standard::Unf | Standard::Unef | Standard::Un => {
            // t = D - 9√3/16 · p, with p = pn/pd. Drill a is nearer than
            // b (a < b) iff t < m = (a + b)/2 iff 2(D - m) < 9√3·pn/(8 pd)
            // iff x2 <= 0 or 64 x2² pd² < 243 pn², with x2 = 2D - a - b
            // (both sides squared; √3 is irrational, so never equal).
            let nearer_lower = |a: Nm, b: Nm| {
                let x2 = 2 * major - i128::from(a) - i128::from(b);
                x2 <= 0 || 64 * x2 * x2 * pd * pd < 243 * pn * pn
            };
            let cands: Vec<&Drill> = drill::of(true).filter(within).collect();
            let Some((&first, rest)) = cands.split_first() else {
                // No inch drill falls within the limits (fine pitches from
                // 9/16 in, whose window is narrower than 1/64 in): the
                // target itself, to 0.0001 in, as H28/2B writes such
                // drills (`0.5203`).
                let x = crate::exact::isqrt((243 * pn * pn / (256 * pd * pd)) as u128) as i128;
                let nm = (crate::exact::round_div(major - x, 2_540) * 2_540) as Nm;
                return Some(TapDrill {
                    name: crate::in_decimal(nm).unwrap_or_else(|| crate::mm_decimal(nm)),
                    nm,
                    standard: false,
                });
            };
            let mut best = first;
            for &d in rest {
                if !nearer_lower(best.nm, d.nm) {
                    best = d;
                }
            }
            Some(TapDrill::of(best))
        }
        _ => None,
    }
}

/// The drill for e% engagement (50 to 100).
pub fn engagement(t: &Thread, percent: u32) -> Option<TapDrill> {
    let major = i128::from(t.major?.value);
    let (pn, pd) = t.pitch.value.nm();
    let e = i128::from(percent);
    let inch = t.standard.inch();
    // d >= D - 5√3/8 · P · e/100  <=>  (5 pn e)√3 >= (D - d) · 800 pd.
    let ok = |d: &&Drill| root3_at_least(5 * pn * e, 800 * pd, major - i128::from(d.nm));
    let found = drill::of(inch).find(ok);
    Some(found.map_or_else(
        || {
            // 5√3/8 · P · e/100 to the nanometre, from below.
            let x = crate::exact::isqrt((75 * pn * pn * e * e / (640_000 * pd * pd)) as u128);
            TapDrill::computed((major - x as i128) as Nm)
        },
        TapDrill::of,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thread::{Standard, lookup};

    fn rule_of(s: Standard, size: &str) -> (String, i64, bool) {
        let t = lookup(s, size, None, None).unwrap();
        let d = rule(&t).unwrap();
        (d.name.clone(), d.nm, d.standard)
    }

    #[test]
    fn metric_rule_is_d_minus_p_at_or_above() {
        assert_eq!(
            rule_of(Standard::IsoMetric, "M6"),
            ("5".into(), 5_000_000, true)
        );
        assert_eq!(
            rule_of(Standard::IsoMetric, "M8"),
            ("6.8".into(), 6_800_000, true)
        );
        assert_eq!(
            rule_of(Standard::IsoMetric, "M10"),
            ("8.5".into(), 8_500_000, true)
        );
        assert_eq!(rule_of(Standard::IsoMetric, "M12").1, 10_300_000);
    }

    #[test]
    fn unified_rule_is_the_nearest_75_percent_drill() {
        assert_eq!(rule_of(Standard::Unc, "1/4").0, "#7");
        assert_eq!(rule_of(Standard::Unc, "#10").0, "#25");
        assert_eq!(rule_of(Standard::Unf, "#10").0, "#21");
        assert_eq!(rule_of(Standard::Unc, "1/2").0, "27/64");
    }

    #[test]
    fn every_builtin_size_drills_within_its_limits() {
        let mut computed = Vec::new();
        for s in [
            Standard::IsoMetric,
            Standard::Unc,
            Standard::Unf,
            Standard::Unef,
        ] {
            for size in crate::thread::sizes(s) {
                let t = lookup(s, size, None, None).unwrap();
                let d = rule(&t).unwrap();
                let (lo, hi) = t.minor.unwrap();
                assert!(lo.value <= d.nm && d.nm <= hi.value, "{size}: {}", d.name);
                if !d.standard {
                    computed.push(format!("{} {size}", s.key()));
                }
            }
        }
        // No standard drill falls within these sizes' limits, so the rule
        // gives the computed size: metric above 65 mm (the series' end),
        // and unified fine pitches whose window is narrower than 1/64 in.
        assert_eq!(
            computed,
            [
                "iso_metric M72",
                "iso_metric M80",
                "iso_metric M90",
                "iso_metric M100",
                "unf 9/16",
                "unf 5/8",
                "unef 9/16",
                "unef 5/8",
                "unef 11/16",
                "unef 1-1/16",
                "unef 1-1/8",
                "unef 1-3/16",
                "unef 1-1/4",
                "unef 1-5/16",
                "unef 1-3/8",
                "unef 1-7/16",
                "unef 1-1/2",
                "unef 1-9/16",
                "unef 1-5/8",
                "unef 1-11/16",
            ]
        );
    }

    #[test]
    fn computed_unified_drills_are_in_inches() {
        // 9/16-18: 0.5625 - 9√3/16 / 18 = 0.50837 in.
        let t = lookup(Standard::Unf, "9/16", None, None).unwrap();
        let d = rule(&t).unwrap();
        assert_eq!((d.name.as_str(), d.standard), ("0.5084", false));
    }

    #[test]
    fn engagement_rounds_up_to_a_drill() {
        let m6 = lookup(Standard::IsoMetric, "M6", None, None).unwrap();
        // 6 - 1.0825 · 0.75 = 5.188: the next drill is 5.2.
        assert_eq!(engagement(&m6, 75).unwrap().nm, 5_200_000);
        assert_eq!(engagement(&m6, 100).unwrap().nm, 5_000_000); // 4.917 → 5.0
        assert!(rule(&lookup(Standard::Npt, "1/8", None, None).unwrap()).is_none());
    }
}
