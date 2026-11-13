//! Every external unified limit against FED-STD-H28/2B's printed values
//! (`data/h28_2b_external.txt`): formula, or the printed correction.

use ryu_standards::unified::{ExtClass, SIZES, external_limits};

#[test]
fn every_printed_external_limit() {
    let text = include_str!("data/h28_2b_external.txt");
    let mut rows = 0;
    for line in text
        .lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
    {
        let f: Vec<&str> = line.split_whitespace().collect();
        let [major, tpi, _series, class, lo, hi] = f.as_slice() else {
            panic!("{line}")
        };
        let s = SIZES
            .iter()
            .find(|s| s.major == inch(major))
            .unwrap_or_else(|| panic!("{line}"));
        let l = external_limits(s, tpi.parse().unwrap(), ExtClass::parse(class).unwrap());
        assert_eq!(
            (l.major.0.value, l.major.1.value),
            (inch(lo), inch(hi)),
            "{line}"
        );
        rows += 1;
    }
    assert!(rows > 300, "{rows} rows");
}

/// Decimal inches to nanometres, exactly.
fn inch(t: &str) -> i64 {
    let (w, f) = t.split_once('.').unwrap_or((t, ""));
    let f = format!("{f:0<4}");
    (w.parse::<i64>().unwrap_or(0) * 10_000 + f[..4].parse::<i64>().unwrap()) * 2_540
}
