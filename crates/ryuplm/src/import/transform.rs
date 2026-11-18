//! A field's transforms (designs 734 and 935), in a fixed order: trim,
//! regex capture, value table, case, unit for a bare number, and a date
//! read by its patterns. `None` when the field is empty after them: it
//! isn't set.

use super::mapping::{Case, Field};

/// Whether `s` is a bare number: digits with an optional sign, point and
/// exponent, nothing else.
fn bare_number(s: &str) -> bool {
    !s.is_empty()
        && s.parse::<f64>().is_ok()
        && s.chars().all(|c| c.is_ascii_digit() || "+-.eE".contains(c))
}

/// One step of a date pattern.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tok {
    Year,
    Month2,
    Month,
    Day2,
    Day,
    Hour,
    Minute,
    Second,
    Lit(char),
}

/// A date pattern (design 935): `YYYY`, `MM`, `M`, `DD`, `D`, `hh`, `mm`,
/// `ss` and literal characters, or `serial`, a spreadsheet's day number.
#[derive(Clone, Debug)]
pub struct DatePattern {
    /// As the mapping wrote it.
    pub text: String,
    /// `None` for `serial`.
    toks: Option<Vec<Tok>>,
}

impl PartialEq for DatePattern {
    fn eq(&self, o: &DatePattern) -> bool {
        self.text == o.text
    }
}

/// The day a spreadsheet's serial 0 or 1 counts from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Dates {
    /// 1 is 1900-01-01, with Lotus's 29 February 1900 at 60.
    #[default]
    S1900,
    /// 0 is 1904-01-01 (older Mac workbooks).
    S1904,
}

const TOKENS: [(&str, Tok); 8] = [
    ("YYYY", Tok::Year),
    ("MM", Tok::Month2),
    ("M", Tok::Month),
    ("DD", Tok::Day2),
    ("D", Tok::Day),
    ("hh", Tok::Hour),
    ("mm", Tok::Minute),
    ("ss", Tok::Second),
];

impl DatePattern {
    pub fn parse(s: &str) -> Result<DatePattern, String> {
        if s == "serial" {
            return Ok(DatePattern {
                text: s.to_owned(),
                toks: None,
            });
        }
        let mut toks = Vec::new();
        let mut rest = s;
        while let Some(c) = rest.chars().next() {
            let (t, n) = TOKENS
                .iter()
                .find(|(p, _)| rest.starts_with(p))
                .map_or((Tok::Lit(c), c.len_utf8()), |(p, t)| (*t, p.len()));
            toks.push(t);
            rest = &rest[n..];
        }
        let count = |group: &[Tok]| toks.iter().filter(|t| group.contains(t)).count();
        for group in [
            &[Tok::Year][..],
            &[Tok::Month2, Tok::Month],
            &[Tok::Day2, Tok::Day],
            &[Tok::Hour],
            &[Tok::Minute],
            &[Tok::Second],
        ] {
            if count(group) > 1 {
                return Err(format!("date pattern {s:?} names one part twice"));
            }
        }
        if count(&[Tok::Year]) == 0
            || count(&[Tok::Month2, Tok::Month]) == 0
            || count(&[Tok::Day2, Tok::Day]) == 0
        {
            return Err(format!(
                "date pattern {s:?} needs YYYY, MM or M, and DD or D; or is serial"
            ));
        }
        if count(&[Tok::Minute, Tok::Second]) > 0 && count(&[Tok::Hour]) == 0 {
            return Err(format!(
                "date pattern {s:?} has minutes or seconds but no hh"
            ));
        }
        Ok(DatePattern {
            text: s.to_owned(),
            toks: Some(toks),
        })
    }

    /// `v` as ISO 8601 when this pattern reads it.
    fn read(&self, v: &str, dates: Dates) -> Option<String> {
        let Some(toks) = &self.toks else {
            return serial(v, dates);
        };
        let (mut y, mut mo, mut d, mut h) = (None, None, None, None);
        let (mut mi, mut se) = (0, 0);
        let mut i = 0;
        for t in toks {
            match *t {
                Tok::Lit(c) => {
                    if !v[i..].starts_with(c) {
                        return None;
                    }
                    i += c.len_utf8();
                }
                Tok::Year => y = Some(i64::from(digits(v, &mut i, 4, 4)?)),
                Tok::Month2 => mo = Some(digits(v, &mut i, 2, 2)?),
                Tok::Month => mo = Some(digits(v, &mut i, 1, 2)?),
                Tok::Day2 => d = Some(digits(v, &mut i, 2, 2)?),
                Tok::Day => d = Some(digits(v, &mut i, 1, 2)?),
                Tok::Hour => h = Some(digits(v, &mut i, 2, 2)?),
                Tok::Minute => mi = digits(v, &mut i, 2, 2)?,
                Tok::Second => se = digits(v, &mut i, 2, 2)?,
            }
        }
        let (y, mo, d) = (y?, mo?, d?);
        let ok = i == v.len()
            && (1..=12).contains(&mo)
            && d >= 1
            && d <= days_in(y, mo)
            && h.is_none_or(|h| h <= 23)
            && mi <= 59
            && se <= 59;
        ok.then(|| iso(y, mo, d, h.map(|h| (h, mi, se))))
    }
}

/// `min` to `max` ASCII digits at byte `i` of `s`, moving `i` past them.
fn digits(s: &str, i: &mut usize, min: usize, max: usize) -> Option<u32> {
    let b = s.as_bytes();
    let from = *i;
    while *i < b.len() && *i - from < max && b[*i].is_ascii_digit() {
        *i += 1;
    }
    if *i - from < min {
        return None;
    }
    s[from..*i].parse().ok()
}

fn leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

fn days_in(y: i64, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if leap(y) => 29,
        _ => 28,
    }
}

fn iso(y: i64, m: u32, d: u32, time: Option<(u32, u32, u32)>) -> String {
    match time {
        None => format!("{y:04}-{m:02}-{d:02}"),
        Some((h, mi, s)) => format!("{y:04}-{m:02}-{d:02}T{h:02}:{mi:02}:{s:02}"),
    }
}

/// The civil date `n` days after 1970-01-01 (Howard Hinnant's
/// `civil_from_days`, in integers).
fn civil(n: i64) -> (i64, u32, u32) {
    let z = n + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

/// A spreadsheet's serial: whole days, and a fraction of a day as a time
/// rounded to the second, computed in integers so every platform agrees.
fn serial(v: &str, dates: Dates) -> Option<String> {
    let (int, frac) = match v.split_once('.') {
        Some((_, "")) => return None,
        Some(x) => x,
        None => (v, ""),
    };
    if int.is_empty()
        || int.len() > 7
        || frac.len() > 15
        || !int.bytes().all(|b| b.is_ascii_digit())
        || !frac.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let n: i64 = int.parse().ok()?;
    let mut secs: i64 = if frac.is_empty() {
        0
    } else {
        let num: u128 = frac.parse().ok()?;
        let den = 10u128.pow(frac.len() as u32);
        ((num * 86_400 * 2 + den) / (2 * den)) as i64
    };
    let mut carry = 0;
    if secs == 86_400 {
        secs = 0;
        carry = 1;
    }
    let day = match dates {
        Dates::S1900 => match n {
            0 | 60 => return None,
            1..=59 => n - 25_568,
            _ => n - 25_569,
        },
        Dates::S1904 => n - 24_107,
    } + carry;
    let (y, m, d) = civil(day);
    if !(1..=9999).contains(&y) {
        return None;
    }
    let time = (secs > 0).then(|| {
        let s = secs as u32;
        (s / 3600, s / 60 % 60, s % 60)
    });
    Some(iso(y, m, d, time))
}

/// `v`, trimmed, read by the first of `patterns` that reads it, as ISO
/// 8601; why not, when none does.
pub fn read_date(v: &str, patterns: &[DatePattern], dates: Dates) -> Result<String, String> {
    let v = v.trim();
    patterns
        .iter()
        .find_map(|p| p.read(v, dates))
        .ok_or_else(|| {
            format!(
                "{v:?} isn't a date as {} reads one",
                patterns
                    .iter()
                    .map(|p| format!("{:?}", p.text))
                    .collect::<Vec<_>>()
                    .join(" or ")
            )
        })
}

pub fn apply(f: &Field, cell: &str) -> Option<String> {
    let mut v: String = if f.trim {
        cell.trim().to_owned()
    } else {
        cell.to_owned()
    };
    if let Some(re) = &f.regex {
        let caps = re.captures(&v)?;
        v = caps
            .get(1)
            .or_else(|| caps.get(0))
            .map(|m| m.as_str().to_owned())?;
    }
    if let Some(to) = f.values.get(&v) {
        v = to.clone();
    }
    match f.case {
        Some(Case::Upper) => v = v.to_uppercase(),
        Some(Case::Lower) => v = v.to_lowercase(),
        None => {}
    }
    if let Some(u) = &f.unit
        && bare_number(v.trim())
    {
        v = format!("{} {u}", v.trim());
    }
    (!v.is_empty()).then_some(v)
}

/// [`apply`], then the field's date patterns: `Err` with why when the
/// value isn't a date they read (refused at its cell).
pub fn apply_in(f: &Field, cell: &str, dates: Dates) -> Result<Option<String>, String> {
    let Some(v) = apply(f, cell) else {
        return Ok(None);
    };
    match &f.date {
        None => Ok(Some(v)),
        Some(p) => read_date(&v, p, dates).map(Some),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::import::mapping::Field;

    #[test]
    fn transforms_apply_in_order() {
        let mut f = Field::from("c");
        assert_eq!(
            apply(&f, "  x "),
            Some("  x ".into()),
            "as written without trim"
        );
        f.trim = true;
        assert_eq!(apply(&f, "  x "), Some("x".into()));
        assert_eq!(apply(&f, "   "), None);
        f.regex = Some(regex_lite::Regex::new(r"\b(M\d+)\b").unwrap());
        assert_eq!(apply(&f, "Screw M3 x 8"), Some("M3".into()));
        assert_eq!(apply(&f, "Washer"), None);
        f.regex = None;
        f.values.insert("Y".into(), "true".into());
        f.case = Some(Case::Upper);
        assert_eq!(
            apply(&f, "Y"),
            Some("TRUE".into()),
            "the table, then the case"
        );
        assert_eq!(apply(&f, "n"), Some("N".into()));
        let mut u = Field::from("c");
        u.unit = Some("mm".into());
        assert_eq!(apply(&u, "5"), Some("5 mm".into()));
        assert_eq!(apply(&u, "2.5e1"), Some("2.5e1 mm".into()));
        assert_eq!(
            apply(&u, "5 in"),
            Some("5 in".into()),
            "a unit already given stays"
        );
    }

    #[test]
    fn date_patterns_are_checked() {
        for bad in ["MM/YYYY", "DD/MM", "YYYY-MM-DD mm", "YYYY-MM-DD-DD"] {
            assert!(DatePattern::parse(bad).is_err(), "{bad}");
        }
        assert_eq!(DatePattern::parse("YYYY-MM-DD").unwrap().text, "YYYY-MM-DD");
        assert_eq!(DatePattern::parse("serial").unwrap().text, "serial");
    }

    #[test]
    fn dates_read_by_their_patterns() {
        let p = |s: &str| DatePattern::parse(s).unwrap();
        let both = [p("DD/MM/YYYY"), p("YYYY-MM-DD")];
        let read = |v: &str| read_date(v, &both, Dates::S1900);
        assert_eq!(read("07/03/2019").unwrap(), "2019-03-07");
        assert_eq!(read(" 2020-11-02 ").unwrap(), "2020-11-02");
        assert_eq!(read("29/02/2020").unwrap(), "2020-02-29");
        for bad in [
            "31/02/2019",
            "29/02/2019",
            "07/13/2019",
            "7/3/2019",
            "07/03/19",
            "07/03/2019x",
            "",
        ] {
            assert!(read(bad).is_err(), "{bad}");
        }
        assert!(read("soon").unwrap_err().contains("DD/MM/YYYY"));
        let one = |pat: &str, v: &str| read_date(v, &[p(pat)], Dates::S1900).unwrap();
        assert_eq!(one("M/D/YYYY", "3/7/2019"), "2019-03-07");
        assert_eq!(one("YYYYMMDD hhmm", "20190307 1405"), "2019-03-07T14:05:00");
        assert_eq!(
            one("YYYY.MM.DD hh:mm:ss", "2019.03.07 14:05:09"),
            "2019-03-07T14:05:09"
        );
        assert!(
            read_date(
                "2019.03.07 24:00:00",
                &[p("YYYY.MM.DD hh:mm:ss")],
                Dates::S1900
            )
            .is_err()
        );
    }

    #[test]
    fn serials_count_from_1900_or_1904() {
        let s = [DatePattern::parse("serial").unwrap()];
        let read = |v: &str, d: Dates| read_date(v, &s, d);
        assert_eq!(read("43531", Dates::S1900).unwrap(), "2019-03-07");
        assert_eq!(read("42069", Dates::S1904).unwrap(), "2019-03-07");
        assert_eq!(read("1", Dates::S1900).unwrap(), "1900-01-01");
        assert_eq!(read("59", Dates::S1900).unwrap(), "1900-02-28");
        assert_eq!(read("61", Dates::S1900).unwrap(), "1900-03-01");
        assert!(
            read("60", Dates::S1900).is_err(),
            "Lotus's 29 February 1900"
        );
        assert!(read("0", Dates::S1900).is_err());
        assert_eq!(read("0", Dates::S1904).unwrap(), "1904-01-01");
        assert_eq!(
            read("43531.5", Dates::S1900).unwrap(),
            "2019-03-07T12:00:00"
        );
        assert_eq!(
            read("43531.586805555555", Dates::S1900).unwrap(),
            "2019-03-07T14:05:00"
        );
        assert_eq!(read("43531.000", Dates::S1900).unwrap(), "2019-03-07");
        assert_eq!(read("43531.99999999", Dates::S1900).unwrap(), "2019-03-08");
        for bad in ["-1", "1e3", "4353 1", "12345678", "43531.", ".5"] {
            assert!(read(bad, Dates::S1900).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_date_reads_after_the_other_transforms() {
        let mut f = Field::from("c");
        f.trim = true;
        f.values.insert("n/a".into(), String::new());
        f.date = Some(vec![DatePattern::parse("DD/MM/YYYY").unwrap()]);
        assert_eq!(
            apply_in(&f, " 07/03/2019 ", Dates::S1900),
            Ok(Some("2019-03-07".into()))
        );
        assert_eq!(
            apply_in(&f, "n/a", Dates::S1900),
            Ok(None),
            "the table empties it first"
        );
        assert!(apply_in(&f, "March", Dates::S1900).is_err());
        assert_eq!(
            apply_in(&Field::from("c"), "x", Dates::S1900),
            Ok(Some("x".into()))
        );
    }
}
