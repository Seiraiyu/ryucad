//! A part's properties, `parts/<part>/properties.json`: one flat object of
//! property name to value, checked against the project's settings. The
//! file is optional; with none, no property is set.
//!
//! ```json
//! {
//!   "format": 1,
//!   "kind": "ryupdm.properties",
//!   "material": "6061-T6",
//!   "name": "Motor bracket",
//!   "number": "100-0042"
//! }
//! ```
//!
//! A property the settings don't know, a value that doesn't fit its
//! declaration, a number off the pattern and two parts with one number are
//! faults of the part: it still rebuilds, but can't be released.

use crate::config::{Config, Decl, Type};
use ryucad_doc::doc::files::header;
use ryucad_doc::doc::id::Id;
use ryucad_doc::doc::project::{PROPERTIES, Project};
use ryucad_doc::doc::source::{Source, join};
use ryucad_doc::error::{Code, Error, Result};
use ryucad_doc::expr::{Expr, no_parameters};
use ryucad_doc::units::Unit;
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

pub const KIND: &str = "ryupdm.properties";
pub const FORMAT: u64 = 1;

/// A part's properties as written.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Properties {
    pub values: BTreeMap<String, Value>,
}

impl Properties {
    pub fn to_json(&self) -> Value {
        let mut v: Map<String, Value> = self
            .values
            .iter()
            .map(|(k, x)| (k.clone(), x.clone()))
            .collect();
        v.insert("format".to_owned(), json!(FORMAT));
        v.insert("kind".to_owned(), json!(KIND));
        Value::Object(v)
    }

    /// Reads the file's shape: its kind and format, then every other key
    /// as a property. Whether they fit the settings is [`check`]'s.
    pub fn from_json(v: &Value) -> Result<Properties> {
        header(v, KIND, FORMAT)?;
        let values = v
            .as_object()
            .expect("header checked it is an object")
            .iter()
            .filter(|(k, _)| *k != "format" && *k != "kind")
            .map(|(k, x)| (k.clone(), x.clone()))
            .collect();
        Ok(Properties { values })
    }

    /// The part number, when it is set to a string.
    pub fn number(&self) -> Option<&str> {
        self.values.get("number").and_then(Value::as_str)
    }
}

/// Where a part's properties live, relative to the project.
pub fn path(part: &Id) -> String {
    join(&join("parts", part.as_str()), PROPERTIES)
}

/// The part's properties: none when it has no file.
pub fn read(src: &dyn Source, part: &Id) -> Result<Properties> {
    let at = path(part);
    match crate::read_json(src, &at)? {
        None => Ok(Properties::default()),
        Some(v) => Properties::from_json(&v).map_err(|e| e.with("path", &at)),
    }
}

/// Checks one value against its declaration, `unit` being the project's
/// default unit (for a quantity's bare number). `None` when it fits, else
/// why not.
pub fn fits(decl: &Decl, value: &Value, unit: Unit) -> Option<String> {
    let string = |multiline: bool| match value.as_str() {
        None => Some("expected a string".to_owned()),
        Some("") => Some("expected text, not an empty string".to_owned()),
        Some(s)
            if s.chars()
                .any(|c| c.is_control() && !(multiline && (c == '\n' || c == '\t'))) =>
        {
            Some(if multiline {
                "control characters other than new lines and tabs aren't allowed".to_owned()
            } else {
                "expected one line, with no control characters".to_owned()
            })
        }
        Some(_) => None,
    };
    match &decl.ty {
        Type::String => string(false),
        Type::Text => string(true),
        Type::Integer => value
            .as_i64()
            .is_none()
            .then(|| "expected a whole number".to_owned()),
        Type::Number => (!value.is_number()).then(|| "expected a number".to_owned()),
        Type::Boolean => (!value.is_boolean()).then(|| "expected true or false".to_owned()),
        Type::Enum(values) => match value.as_str() {
            Some(s) if values.iter().any(|v| v == s) => None,
            _ => Some(format!("expected one of {}", values.join(", "))),
        },
        Type::Quantity(dim) => {
            let Some(s) = value.as_str() else {
                return Some(format!(
                    "expected a {} as a string, such as \"2 mm\"",
                    dim.name()
                ));
            };
            // A bare number takes the project's unit, as in feature files;
            // anything else is an expression that reads no parameters.
            let q = ryucad_doc::units::Quantity::parse(s, unit)
                .ok()
                .filter(|q| q.dim() == *dim)
                .map(|_| ());
            if q.is_some() {
                return None;
            }
            match Expr::parse(s).and_then(|e| e.eval(&no_parameters)) {
                Ok(v) if v.dim == *dim => None,
                Ok(v) => Some(format!("expected a {}, not a {}", dim.name(), v.dim.name())),
                Err(e) => Some(e.message),
            }
        }
    }
}

/// The faults of one part's properties against `config`, by property name:
/// unknown properties, values that don't fit, a number off the pattern.
/// Numbers shared with other parts are [`duplicates`]'.
pub fn check(config: &Config, unit: Unit, part: &Id, props: &Properties) -> Vec<Error> {
    let mut faults = Vec::new();
    for (name, value) in &props.values {
        let at = format!("/{name}");
        let Some(decl) = config.decl(name) else {
            faults.push(
                Error::new(
                    Code::PropertyUnknown,
                    format!(
                        "part {part}: {name} is neither a core property nor declared in ryupdm.json"
                    ),
                )
                .with("part", part)
                .with("property", name)
                .with("pointer", &at),
            );
            continue;
        };
        if let Some(why) = fits(&decl, value, unit) {
            faults.push(
                Error::new(Code::PropertyInvalid, format!("part {part}: {name}: {why}"))
                    .with("part", part)
                    .with("property", name)
                    .with("pointer", &at)
                    .with("value", value),
            );
            continue;
        }
        if name == "number"
            && let (Some(p), Some(n)) = (&config.pattern, value.as_str())
            && !p.matches(n)
        {
            faults.push(
                Error::new(
                    Code::PartNumberInvalid,
                    format!(
                        "part {part}: number {n:?} doesn't match the project's pattern {:?}",
                        p.text()
                    ),
                )
                .with("part", part)
                .with("property", name)
                .with("pointer", &at)
                .with("value", n)
                .with("pattern", p.text()),
            );
        }
    }
    faults
}

/// `part_number_duplicate` for each part whose number another part has
/// too, naming the others.
pub fn duplicates<'a>(numbers: impl IntoIterator<Item = (&'a Id, &'a str)>) -> BTreeMap<Id, Error> {
    let mut by_number: BTreeMap<&str, Vec<&Id>> = BTreeMap::new();
    for (part, n) in numbers {
        by_number.entry(n).or_default().push(part);
    }
    let mut out = BTreeMap::new();
    for (n, parts) in by_number.into_iter().filter(|(_, p)| p.len() > 1) {
        for part in &parts {
            let others: Vec<&Id> = parts.iter().copied().filter(|p| p != part).collect();
            let list = others
                .iter()
                .map(|p| p.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            out.insert(
                (*part).clone(),
                Error::new(
                    Code::PartNumberDuplicate,
                    format!("part {part}: number {n:?} is also part {list}'s"),
                )
                .with("part", part)
                .with("property", "number")
                .with("pointer", "/number")
                .with("value", n)
                .with("parts", &others),
            );
        }
    }
    out
}

/// One part's properties as the project has them.
#[derive(Clone, Debug)]
pub struct PartProperties {
    /// The properties, or why the file can't be read.
    pub file: Result<Properties>,
    /// What is wrong with them, in name order, a duplicate number last.
    pub faults: Vec<Error>,
}

/// Every part's properties in a project, checked against its settings.
#[derive(Clone, Debug)]
pub struct Pdm {
    /// The settings, or why `ryupdm.json` can't be read. With broken
    /// settings, properties are read but not judged.
    pub config: Result<Config>,
    pub parts: BTreeMap<Id, PartProperties>,
}

impl Pdm {
    /// Reads `project`'s settings and its parts' properties from `src`, the
    /// files `project` was loaded from.
    pub fn read(src: &dyn Source, project: &Project) -> Pdm {
        let config = Config::read(src);
        let unit = project.manifest.default_unit;
        let mut parts: BTreeMap<Id, PartProperties> = project
            .parts
            .keys()
            .map(|id| {
                let file = read(src, id);
                let faults = match (&config, &file) {
                    (Ok(c), Ok(p)) => check(c, unit, id, p),
                    _ => Vec::new(),
                };
                (id.clone(), PartProperties { file, faults })
            })
            .collect();
        if config.is_ok() {
            let numbers: Vec<(Id, String)> = parts
                .iter()
                .filter_map(|(id, p)| {
                    let n = p.file.as_ref().ok()?.number()?;
                    // A number that is itself faulty isn't compared.
                    (!p.faults.iter().any(|f| f.details["property"] == "number"))
                        .then(|| (id.clone(), n.to_owned()))
                })
                .collect();
            let dups = duplicates(numbers.iter().map(|(id, n)| (id, n.as_str())));
            for (id, e) in dups {
                parts.get_mut(&id).expect("a part").faults.push(e);
            }
        }
        Pdm { config, parts }
    }

    /// Every fault of every part, the files' own first, in part order.
    pub fn faults(&self) -> Vec<Error> {
        self.parts
            .values()
            .flat_map(|p| {
                p.file
                    .as_ref()
                    .err()
                    .cloned()
                    .into_iter()
                    .chain(p.faults.iter().cloned())
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Pattern;
    use ryucad_doc::doc::canonical;

    fn id(s: &str) -> Id {
        Id::parse(s, "part").unwrap()
    }

    fn props(v: Value) -> Properties {
        let mut v = v;
        v["format"] = json!(1);
        v["kind"] = json!(KIND);
        Properties::from_json(&v).unwrap()
    }

    fn config() -> Config {
        Config::from_json(&json!({
            "format": 1, "kind": "ryupdm.config",
            "part_number": {"pattern": "\\d{3}-\\d{4}"},
            "properties": {
                "finish": {"type": "enum", "values": ["anodized", "bare"]},
                "count": {"type": "integer"},
                "ratio": {"type": "number"},
                "coated": {"type": "boolean"},
                "area": {"type": "quantity", "dim": "length^2"},
                "gap": {"type": "quantity", "dim": "length"},
                "draft": {"type": "quantity", "dim": "angle"}
            }
        }))
        .unwrap()
    }

    fn codes(p: Value) -> Vec<(Code, String)> {
        check(&config(), Unit::Mm, &id("p"), &props(p))
            .into_iter()
            .map(|e| (e.code, e.details["property"].as_str().unwrap().to_owned()))
            .collect()
    }

    #[test]
    fn round_trips_as_written() {
        let v = json!({"format": 1, "kind": "ryupdm.properties", "number": "100-0042",
                       "count": 3, "gap": "2 mm"});
        let p = Properties::from_json(&v).unwrap();
        assert_eq!(p.number(), Some("100-0042"));
        assert_eq!(p.to_json(), v);
        assert_eq!(
            Properties::from_json(&json!({"format": 1, "kind": "ryucad.part"}))
                .unwrap_err()
                .code,
            Code::InvalidDocument
        );
        assert_eq!(
            Properties::from_json(&json!({"format": 2, "kind": KIND}))
                .unwrap_err()
                .code,
            Code::FormatTooNew
        );
    }

    #[test]
    fn values_fit_their_types() {
        let fine = json!({
            "number": "100-0042", "name": "Bracket", "description": "Two\nlines\tand a tab",
            "material": "6061-T6", "uom": "kg", "make_buy": "buy", "finish": "bare",
            "count": -3, "ratio": 0.5, "coated": false, "area": "4 mm^2", "gap": "2",
            "draft": "1.5 deg"
        });
        assert_eq!(codes(fine), []);
        let bad = json!({
            "name": "", "description": 3, "material": "one\ntwo", "uom": "each ", "make_buy": "rent",
            "finish": "Bare", "count": 1.5, "ratio": "1", "coated": "yes", "area": "4 mm",
            "gap": "wall", "draft": 3, "colour": "red", "number": "1000-42"
        });
        let mut got = codes(bad);
        got.sort_by(|a, b| a.1.cmp(&b.1));
        let mut want: Vec<(Code, String)> = [
            "area",
            "coated",
            "count",
            "description",
            "draft",
            "finish",
            "gap",
            "make_buy",
            "material",
            "name",
            "ratio",
            "uom",
        ]
        .iter()
        .map(|n| (Code::PropertyInvalid, (*n).to_owned()))
        .chain([
            (Code::PropertyUnknown, "colour".to_owned()),
            (Code::PartNumberInvalid, "number".to_owned()),
        ])
        .collect();
        want.sort_by(|a, b| a.1.cmp(&b.1));
        assert_eq!(got, want);
    }

    #[test]
    fn faults_say_where_and_why() {
        let e = &check(
            &config(),
            Unit::Mm,
            &id("p"),
            &props(json!({"area": "4 mm"})),
        )[0];
        assert_eq!(e.details["pointer"], "/area");
        assert_eq!(e.details["part"], "p");
        assert!(e.message.contains("length^2"), "{}", e.message);
        let mut c = config();
        c.pattern = Some(Pattern::new("[A-Z]+").unwrap());
        let e = &check(
            &c,
            Unit::Mm,
            &id("p"),
            &props(json!({"number": "100-0042"})),
        )[0];
        assert_eq!(
            (e.code, &e.details["pattern"]),
            (Code::PartNumberInvalid, &json!("[A-Z]+"))
        );
        // With no pattern, any non-empty line is a number.
        let c = Config::default();
        assert!(
            check(
                &c,
                Unit::Mm,
                &id("p"),
                &props(json!({"number": "anything"}))
            )
            .is_empty()
        );
    }

    #[test]
    fn duplicate_numbers_are_reported_on_every_part() {
        let (a, b, c, d) = (id("a"), id("b"), id("c"), id("d"));
        let dups = duplicates([(&a, "1"), (&b, "2"), (&c, "1"), (&d, "1")]);
        assert_eq!(dups.keys().collect::<Vec<_>>(), [&a, &c, &d]);
        assert_eq!(dups[&a].details["parts"], json!(["c", "d"]));
        assert_eq!(dups[&c].code, Code::PartNumberDuplicate);
    }

    fn project(files: &[(&str, Value)]) -> (Project, BTreeMap<String, Vec<u8>>) {
        let mut src: BTreeMap<String, Vec<u8>> = BTreeMap::new();
        src.insert(
            "ryucad.json".into(),
            canonical::to_string(
                &json!({"default_unit": "mm", "format": 1, "kind": "ryucad.project"}),
            )
            .into_bytes(),
        );
        for p in ["a", "b", "c"] {
            src.insert(
                format!("parts/{p}/part.json"),
                canonical::to_string(&json!({"features": [], "format": 1, "kind": "ryucad.part"}))
                    .into_bytes(),
            );
        }
        for (at, v) in files {
            src.insert((*at).to_owned(), canonical::to_string(v).into_bytes());
        }
        let p = Project::load_from(std::path::Path::new("/p"), &src).unwrap();
        (p, src)
    }

    #[test]
    fn a_project_is_read_and_judged_whole() {
        let number = |n: &str| json!({"format": 1, "kind": KIND, "number": n});
        let (p, src) = project(&[
            ("parts/a/properties.json", number("100-0001")),
            ("parts/b/properties.json", number("100-0001")),
            (
                "parts/c/properties.json",
                json!({"format": 1, "kind": KIND, "colour": "red"}),
            ),
        ]);
        let pdm = Pdm::read(&src, &p);
        assert_eq!(pdm.config, Ok(Config::default()));
        let codes: Vec<(String, Code)> = pdm
            .faults()
            .iter()
            .map(|e| (e.details["part"].as_str().unwrap().to_owned(), e.code))
            .collect();
        assert_eq!(
            codes,
            [
                ("a".into(), Code::PartNumberDuplicate),
                ("b".into(), Code::PartNumberDuplicate),
                ("c".into(), Code::PropertyUnknown)
            ]
        );
        // A broken file is the part's fault; broken settings judge nothing.
        let (p, mut src) = project(&[("ryupdm.json", json!({"format": 1, "kind": "x"}))]);
        src.insert("parts/a/properties.json".into(), b"{".to_vec());
        let pdm = Pdm::read(&src, &p);
        assert_eq!(
            pdm.config.as_ref().unwrap_err().details["path"],
            "ryupdm.json"
        );
        assert_eq!(pdm.faults()[0].code, Code::InvalidJson);
        assert_eq!(pdm.faults().len(), 1);
        assert_eq!(pdm.parts[&id("b")].file, Ok(Properties::default()));
    }
}
