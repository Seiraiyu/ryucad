//! Every internal minor-diameter limit FED-STD-H28/2B prints to 1-3/4 in,
//! against the formulas (and `PRINTED`, one row). A difference is listed,
//! never silently passed. The rows were read by eye from a scan, so the
//! formulas check the reading too: all but one follow them exactly, and a
//! misread digit fails here.

use ryu_standards::unified::{Class, SIZES, Series, UN_TPI, internal_limits, size, tpi};

const DATA: &str = include_str!("data/h28_2b_internal.txt");

/// Inches as printed (up to four decimals) to nanometres, exactly.
fn nm(inches: &str) -> i64 {
    let (whole, frac) = inches.split_once('.').unwrap_or((inches, ""));
    let whole = if whole.is_empty() {
        0
    } else {
        whole.parse::<i64>().unwrap()
    };
    let frac = format!("{frac:0<4}");
    whole * 25_400_000 + frac.parse::<i64>().unwrap() * 2_540
}

#[test]
fn every_printed_internal_limit_matches() {
    let mut wrong = Vec::new();
    let mut rows = 0;
    for line in DATA
        .lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
    {
        let w: Vec<&str> = line.split_whitespace().collect();
        let [major, n, series, class, lo, hi] = w[..] else {
            panic!("bad row {line}")
        };
        rows += 1;
        let Some(s) = SIZES.iter().find(|s| s.major == nm(major)) else {
            wrong.push(format!("{line}: no size of major {major} in"));
            continue;
        };
        let n: u16 = n.parse().unwrap();
        // The series printed is the one the built-in sizes give, so a
        // misread size or TPI shows here as well as in the limits.
        let series_tpi = match series {
            "UNC" => tpi(Series::Unc, s),
            "UNF" => tpi(Series::Unf, s),
            "UNEF" => tpi(Series::Unef, s),
            "UN" => UN_TPI.contains(&n).then_some(n),
            _ => None,
        };
        if series_tpi != Some(n) {
            wrong.push(format!("{line}: not {series} by the built-in sizes"));
        }
        if nm(lo) >= nm(hi) {
            wrong.push(format!("{line}: minimum not below maximum"));
        }
        let l = internal_limits(s, n, Class::parse(class).unwrap());
        let got = [l.minor.0.value, l.minor.1.value];
        let want = [nm(lo), nm(hi)];
        if got != want {
            wrong.push(format!(
                "{line}: got {:.4} {:.4}",
                got[0] as f64 / 25.4e6,
                got[1] as f64 / 25.4e6
            ));
        }
    }
    assert!(
        wrong.is_empty(),
        "{} rows differ:\n{}",
        wrong.len(),
        wrong.join("\n")
    );
    assert_eq!(rows, 340, "both tables to 1.750 in");
}

#[test]
fn by_hand() {
    // 1/4-20 2B: 0.1959 + 0.0115 = 0.2074, printed 0.196 to 0.207.
    let l = internal_limits(size("1/4").unwrap(), 20, Class::B2);
    assert_eq!((l.minor.0.value, l.minor.1.value), (4_978_400, 5_257_800));
    // #8-32 3B: 0.130171 + 0.0087 = 0.1389.
    let l = internal_limits(size("#8").unwrap(), 32, Class::B3);
    assert_eq!((l.minor.0.value, l.minor.1.value), (3_302_000, 3_528_060));
    // 1-1/2-18 2B is printed otherwise, and says so.
    let l = internal_limits(size("1-1/2").unwrap(), 18, Class::B2);
    assert_eq!(l.minor.1.value, 36_880_800);
    assert_eq!(
        l.minor.1.source,
        ryu_standards::Source::Public("FED-STD-H28/2B Table II.A.1")
    );
}
