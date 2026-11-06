//! UTC seconds and RFC 3339 text, by integer arithmetic alone (Howard
//! Hinnant's civil-date algorithms): catalogues date themselves so.

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

fn num(s: &str) -> Option<i64> {
    (!s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
        .then(|| s.parse().ok())
        .flatten()
}

/// `YYYY-MM-DDTHH:MM:SS`, any fraction of a second dropped, then `Z` or
/// `±HH:MM`, as UTC seconds; `None` for anything else or before 1970.
pub fn parse_rfc3339(s: &str) -> Option<u64> {
    let s = s.trim();
    let (date, rest) = s.split_once(['T', 't', ' '])?;
    let mut d = date.split('-');
    let (y, mo, da) = (num(d.next()?)?, num(d.next()?)?, num(d.next()?)?);
    if d.next().is_some() || !(1..=12).contains(&mo) || !(1..=31).contains(&da) {
        return None;
    }
    let (clock, offset) = if let Some(c) = rest.strip_suffix(['Z', 'z']) {
        (c, 0)
    } else {
        let at = rest.rfind(['+', '-'])?;
        let (c, o) = rest.split_at(at);
        let sign = if o.starts_with('-') { -1 } else { 1 };
        let (oh, om) = o[1..].split_once(':')?;
        let (oh, om) = (num(oh)?, num(om)?);
        if oh > 23 || om > 59 {
            return None;
        }
        (c, sign * (oh * 3600 + om * 60))
    };
    let clock = clock.split_once('.').map_or(clock, |(c, _)| c);
    let mut c = clock.split(':');
    let (h, mi, se) = (num(c.next()?)?, num(c.next()?)?, num(c.next()?)?);
    if c.next().is_some() || h > 23 || mi > 59 || se > 60 {
        return None;
    }
    let t = days_from_civil(y, mo, da) * 86_400 + h * 3600 + mi * 60 + se - offset;
    u64::try_from(t).ok()
}

/// UTC seconds as `YYYY-MM-DDTHH:MM:SSZ`.
pub fn rfc3339(unix: u64) -> String {
    let t = unix as i64;
    let (y, m, d) = civil_from_days(t.div_euclid(86_400));
    let s = t.rem_euclid(86_400);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        s / 3600,
        s % 3600 / 60,
        s % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc3339_reads_and_writes() {
        let t = parse_rfc3339("2026-11-06T11:33:30+00:00").unwrap();
        assert_eq!(t, 1_793_964_810);
        assert_eq!(rfc3339(t), "2026-11-06T11:33:30Z");
        assert_eq!(parse_rfc3339("2026-11-06T13:33:30+02:00"), Some(t));
        assert_eq!(parse_rfc3339("2026-11-06T11:33:30.123456Z"), Some(t));
        assert_eq!(parse_rfc3339("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(parse_rfc3339("2024-02-29T00:00:00Z").map(rfc3339).as_deref(), Some("2024-02-29T00:00:00Z"));
        for bad in ["", "x", "1969-12-31T23:59:59Z", "2026-13-01T00:00:00Z", "2026-11-06", "2026-11-06T11:33:30"] {
            assert_eq!(parse_rfc3339(bad), None, "{bad}");
        }
    }
}
