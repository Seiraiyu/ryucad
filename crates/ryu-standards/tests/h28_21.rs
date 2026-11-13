//! Every size and pitch FED-STD-H28/21B's Table XXI.1 prints is built in,
//! with its coarse pitch first. H28/21B prints no limits of size, so the
//! limits rest on ISO 965-1's formulas alone (`iso_metric_limits`).

use ryu_standards::iso_metric::size;

const DATA: &str = include_str!("data/h28_21_sizes.txt");

fn nm(mm: &str) -> i64 {
    let (whole, frac) = mm.split_once('.').unwrap_or((mm, ""));
    let frac = format!("{frac:0<6}");
    whole.parse::<i64>().unwrap() * 1_000_000 + frac.parse::<i64>().unwrap()
}

#[test]
fn every_printed_size_and_pitch_is_built_in() {
    let mut wrong = Vec::new();
    let mut rows = 0;
    for line in DATA
        .lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
    {
        let w: Vec<&str> = line.split_whitespace().collect();
        let [name, coarse, fine] = w[..] else {
            panic!("bad row {line}")
        };
        rows += 1;
        let Some(s) = size(&format!("M{name}")) else {
            wrong.push(format!("{line}: M{name} is not built in"));
            continue;
        };
        if s.major != nm(name) {
            wrong.push(format!("{line}: major {}", s.major));
        }
        if coarse != "-" && s.pitches[0] != nm(coarse) {
            wrong.push(format!("{line}: coarse pitch {}", s.pitches[0]));
        }
        if fine != "-" && !s.pitches.contains(&nm(fine)) {
            wrong.push(format!("{line}: no {fine} mm pitch"));
        }
    }
    assert_eq!(rows, 31, "Table XXI.1 has 31 sizes");
    assert!(
        wrong.is_empty(),
        "{} rows differ:\n{}",
        wrong.len(),
        wrong.join("\n")
    );
}
