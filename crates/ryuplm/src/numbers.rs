//! Part numbers against the catalogue's numbering schemes (design 734):
//! a number matching none that applies is a warning, never an error, so
//! legacy numbers are kept as written; `next` fills a scheme's template
//! from the highest number in use.

use crate::error::{Code, Error, Result};
use crate::id::Id;
use crate::load::Catalogue;
use ryupdm::config::{Config, NumberScheme};

/// Whether `scheme` applies to an item of `class`.
pub fn applies(scheme: &NumberScheme, class: Option<&str>) -> bool {
    scheme.class.is_none() || scheme.class.as_deref() == class
}

/// `part_number_unschemed` for each item whose number matches no scheme
/// that applies to it; none when there are no schemes.
pub fn unschemed(cat: &Catalogue, config: &Config) -> Vec<Error> {
    if config.schemes.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for (id, item) in &cat.items {
        let class = item.class.as_deref();
        let schemes: Vec<&NumberScheme> = config.schemes.iter().filter(|s| applies(s, class)).collect();
        if !schemes.iter().any(|s| s.pattern.matches(&item.number)) {
            let names: Vec<&str> = schemes.iter().map(|s| s.name.as_str()).collect();
            out.push(
                Error::new(
                    Code::PartNumberUnschemed,
                    format!("item {id}: number {:?} matches no numbering scheme ({})", item.number, names.join(", ")),
                )
                .with("item", id)
                .with("pointer", "/number")
                .with("value", &item.number)
                .with("schemes", &names),
            );
        }
    }
    out
}

/// The scheme `name` names, or the one `class` takes, or the only one.
pub fn scheme<'a>(config: &'a Config, name: Option<&str>, class: Option<&str>) -> Result<&'a NumberScheme> {
    if config.schemes.is_empty() {
        return Err(Error::new(Code::InvalidParam, "ryupdm.json declares no numbering schemes: add part_number.schemes"));
    }
    if let Some(n) = name {
        return config.schemes.iter().find(|s| s.name == n).ok_or_else(|| {
            Error::new(Code::InvalidParam, format!("no numbering scheme {n:?}")).with("scheme", n)
        });
    }
    let with_next: Vec<&NumberScheme> = config.schemes.iter().filter(|s| s.next.is_some()).collect();
    if let Some(c) = class
        && let Some(s) = with_next.iter().find(|s| s.class.as_deref() == Some(c))
    {
        return Ok(s);
    }
    match with_next.as_slice() {
        [one] => Ok(one),
        _ => Err(Error::new(Code::Usage, "several numbering schemes: name one with --next <scheme>")
            .with("schemes", config.schemes.iter().map(|s| s.name.as_str()).collect::<Vec<_>>())),
    }
}

/// The next number of the scheme chosen as [`scheme`] chooses: one more
/// than the highest sequence any item's number or previous number reads
/// back through its template.
pub fn next(cat: &Catalogue, config: &Config, name: Option<&str>, class: Option<&str>) -> Result<String> {
    let s = scheme(config, name, class)?;
    let t = s.next.as_ref().ok_or_else(|| {
        Error::new(Code::InvalidParam, format!("scheme {:?} has no next template", s.name)).with("scheme", &s.name)
    })?;
    let highest = cat
        .items
        .values()
        .flat_map(|i| std::iter::once(&i.number).chain(&i.previous_numbers))
        .filter_map(|n| t.read(n))
        .max()
        .unwrap_or(0);
    Ok(t.fill(highest + 1))
}

/// Unused here; keeps `Id` imported for Task 14's helpers.
#[allow(dead_code)]
fn _id(_: &Id) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::item::Item;
    use crate::judge::tests::{codes, files};
    use serde_json::json;

    fn config() -> serde_json::Value {
        json!({"format": 5, "kind": "ryupdm.config", "part_number": {"schemes": [
            {"name": "mech", "class": "machined", "pattern": "MECH-\\d{5}", "next": "MECH-{seq:5}"},
            {"name": "elec", "pattern": "E\\d{4}", "next": "E{seq:4}"}
        ]}})
    }

    fn item(n: &str, class: Option<&str>) -> Item {
        let mut i = Item::new(n);
        i.class = class.map(str::to_owned);
        i
    }

    #[test]
    fn unschemed_numbers_are_warnings() {
        let f = files(
            &[("machined", None), ("label", None)],
            Some(config()),
            &[
                ("0000000000001", item("MECH-00001", Some("machined"))),
                ("0000000000002", item("MECH-00002", Some("label"))),
                ("0000000000003", item("E0007", Some("label"))),
                ("0000000000004", item("100-0042 rev", None)),
            ],
        );
        assert_eq!(
            codes(&f),
            [
                ("part_number_unschemed".into(), "warning", "0000000000002".into()),
                ("part_number_unschemed".into(), "warning", "0000000000004".into()),
            ]
        );
        // No schemes, no warnings.
        let plain = files(&[], None, &[("0000000000004", item("100-0042 rev", None))]);
        assert!(codes(&plain).is_empty());
    }

    #[test]
    fn the_next_number_follows_the_highest_in_use() {
        let f = files(
            &[("machined", None)],
            Some(config()),
            &[
                ("0000000000001", item("MECH-00007", Some("machined"))),
                ("0000000000002", {
                    let mut i = item("MECH-00009", Some("machined"));
                    i.previous_numbers.push("MECH-00041".into());
                    i
                }),
                ("0000000000003", item("E0002", None)),
            ],
        );
        let cat = crate::load::Catalogue::load(&f).unwrap();
        let c = ryupdm::config::Config::read_in(&f, "ryuplm").unwrap();
        assert_eq!(next(&cat, &c, Some("mech"), None).unwrap(), "MECH-00042");
        assert_eq!(next(&cat, &c, None, Some("machined")).unwrap(), "MECH-00042");
        assert_eq!(next(&cat, &c, Some("elec"), None).unwrap(), "E0003");
        assert_eq!(next(&cat, &c, None, None).unwrap_err().code, Code::Usage);
        assert_eq!(next(&cat, &c, Some("none"), None).unwrap_err().code, Code::InvalidParam);
        let empty = files(&[], Some(json!({"format": 1, "kind": "ryupdm.config"})), &[]);
        let cat = crate::load::Catalogue::load(&empty).unwrap();
        let c = ryupdm::config::Config::read_in(&empty, "ryuplm").unwrap();
        assert_eq!(next(&cat, &c, None, None).unwrap_err().code, Code::InvalidParam);
    }
}
