//! A field's transforms (design 734), in a fixed order: trim, regex
//! capture, value table, case, unit for a bare number. `None` when the
//! field is empty after them: it isn't set.

use super::mapping::{Case, Field};

/// Whether `s` is a bare number: digits with an optional sign, point and
/// exponent, nothing else.
fn bare_number(s: &str) -> bool {
    !s.is_empty()
        && s.parse::<f64>().is_ok()
        && s.chars().all(|c| c.is_ascii_digit() || "+-.eE".contains(c))
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
}
