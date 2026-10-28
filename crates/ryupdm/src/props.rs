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
use ryucad_doc::doc::assembly::snap_origin;
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
/// The newest format: 2 adds `amount`. A file without one stays at 1.
pub const FORMAT: u64 = 2;

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
        let format = if self.values.contains_key("amount") {
            FORMAT
        } else {
            1
        };
        v.insert("format".to_owned(), json!(format));
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

    /// The unit of measure it is counted in: `each` unless set.
    pub fn uom(&self) -> &str {
        self.values
            .get("uom")
            .and_then(Value::as_str)
            .unwrap_or("each")
    }
}

/// The units an `amount` may be written in for each unit of measure, with
/// their size in that unit of measure. A bare number is in the unit of
/// measure itself.
const AMOUNT_UNITS: [(&str, &[(&str, f64)]); 5] = [
    (
        "m",
        &[
            ("m", 1.0),
            ("cm", 0.01),
            ("mm", 0.001),
            ("um", 1e-6),
            ("in", 0.0254),
            ("ft", 0.3048),
        ],
    ),
    (
        "m2",
        &[
            ("m2", 1.0),
            ("cm2", 1e-4),
            ("mm2", 1e-6),
            ("in2", 0.00064516),
            ("ft2", 0.09290304),
        ],
    ),
    (
        "m3",
        &[
            ("m3", 1.0),
            ("l", 1e-3),
            ("ml", 1e-6),
            ("cm3", 1e-6),
            ("mm3", 1e-9),
            ("in3", 1.6387064e-5),
            ("ft3", 0.028316846592),
        ],
    ),
    (
        "l",
        &[
            ("l", 1.0),
            ("ml", 1e-3),
            ("m3", 1e3),
            ("cm3", 1e-3),
            ("mm3", 1e-6),
        ],
    ),
    ("kg", &[("kg", 1.0), ("g", 1e-3), ("lb", 0.45359237)]),
];

/// An `amount` as a number in `uom`: `"600 mm"` with `uom` `m` is 0.6.
/// A positive decimal number, then optionally a unit `uom` takes (a space
/// between them or not); else why not.
pub fn amount(text: &str, uom: &str) -> std::result::Result<f64, String> {
    let Some((_, units)) = AMOUNT_UNITS.iter().find(|(u, _)| *u == uom) else {
        return Err(format!(
            "a part counted {uom} has no amount: amount is for parts counted in m, m2, m3, l or kg"
        ));
    };
    let t = text.trim();
    let split = t
        .find(|c: char| {
            !(c.is_ascii_digit() || c == '.' || c == 'e' || c == 'E' || c == '+' || c == '-')
        })
        .unwrap_or(t.len());
    let (number, unit) = (t[..split].trim(), t[split..].trim());
    let names: Vec<&str> = units.iter().map(|(n, _)| *n).collect();
    let factor = if unit.is_empty() {
        1.0
    } else {
        units
            .iter()
            .find(|(n, _)| *n == unit)
            .map(|(_, f)| *f)
            .ok_or_else(|| format!("{unit:?} isn't a unit of {uom}: use {}", names.join(", ")))?
    };
    match number.parse::<f64>() {
        // At its shortest decimal within rounding: `600 mm` is 0.6 m.
        Ok(x) if x.is_finite() && x > 0.0 => Ok(snap_origin(x * factor)),
        _ => Err(format!(
            "expected a positive amount of {uom}, such as \"2.5 {}\"",
            names[0]
        )),
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

/// `value` as `decl`'s type stores it: a string that spells a whole
/// number, a number or a boolean becomes one, so a command line can give
/// every value as text. Anything else is left for [`fits`] to judge.
pub fn coerce(decl: &Decl, value: Value) -> Value {
    let Some(s) = value.as_str().map(str::trim) else {
        return value;
    };
    let typed = match decl.ty {
        Type::Integer => s.parse::<i64>().ok().map(Value::from),
        Type::Number => s
            .parse::<f64>()
            .ok()
            .and_then(serde_json::Number::from_f64)
            .map(Value::Number),
        Type::Boolean => match s {
            "true" => Some(Value::Bool(true)),
            "false" => Some(Value::Bool(false)),
            _ => None,
        },
        _ => None,
    };
    typed.unwrap_or(value)
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
        if name == "amount"
            && let Some(text) = value.as_str()
            && let Err(why) = amount(text, props.uom())
        {
            faults.push(
                Error::new(Code::PropertyInvalid, format!("part {part}: amount: {why}"))
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

/// The properties `props` lacks that a release needs: each required one
/// under `config`, then `amount` for a part counted in anything but
/// `each`.
pub fn missing(config: &Config, props: &Properties) -> Vec<String> {
    let mut out: Vec<String> = config
        .decls()
        .into_iter()
        .filter(|(name, decl)| decl.required && !props.values.contains_key(name))
        .map(|(name, _)| name)
        .collect();
    if props.uom() != "each" && !props.values.contains_key("amount") {
        out.push("amount".to_owned());
    }
    out
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
        Pdm::of(
            src,
            project.parts.keys().cloned(),
            project.manifest.default_unit,
        )
    }

    /// [`Pdm::read`] for the parts `parts` of the project `src` holds,
    /// whose default unit is `unit`: for files no project was loaded from
    /// (a merge's sides).
    pub fn of(src: &dyn Source, parts: impl IntoIterator<Item = Id>, unit: Unit) -> Pdm {
        let config = Config::read(src);
        let mut parts: BTreeMap<Id, PartProperties> = parts
            .into_iter()
            .map(|id| {
                let file = read(src, &id);
                let faults = match (&config, &file) {
                    (Ok(c), Ok(p)) => check(c, unit, &id, p),
                    _ => Vec::new(),
                };
                (id, PartProperties { file, faults })
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
            Properties::from_json(&json!({"format": 3, "kind": KIND}))
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
    fn text_becomes_the_declared_type() {
        let c = config();
        let decl = |n: &str| c.decl(n).unwrap();
        assert_eq!(coerce(&decl("count"), json!(" 12 ")), json!(12));
        assert_eq!(coerce(&decl("ratio"), json!("0.25")), json!(0.25));
        assert_eq!(coerce(&decl("coated"), json!("true")), json!(true));
        assert_eq!(coerce(&decl("count"), json!("twelve")), json!("twelve"));
        assert_eq!(coerce(&decl("ratio"), json!("inf")), json!("inf"));
        // Text stays text: a part number of digits is still a string.
        assert_eq!(coerce(&decl("number"), json!("100")), json!("100"));
        assert_eq!(coerce(&decl("count"), json!(3)), json!(3));
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

    #[test]
    fn an_amount_is_how_much_of_its_unit_of_measure_one_occurrence_uses() {
        assert_eq!(amount("600 mm", "m"), Ok(0.6));
        assert_eq!(amount("0.6", "m"), Ok(0.6));
        assert_eq!(amount("2in", "m"), Ok(0.0508));
        assert_eq!(amount("250 ml", "l"), Ok(0.25));
        assert_eq!(amount("5 l", "m3"), Ok(0.005));
        assert_eq!(amount("12 g", "kg"), Ok(0.012));
        assert!(
            amount("12 g", "m")
                .unwrap_err()
                .contains("isn't a unit of m")
        );
        assert!(amount("-1 m", "m").unwrap_err().contains("positive"));
        assert!(amount("1", "each").unwrap_err().contains("counted each"));
        let config = Config::default();
        let p = |v: Value| Properties::from_json(&v).unwrap();
        let tube = p(json!({"format": 2, "kind": KIND, "uom": "m", "amount": "600 mm"}));
        assert!(check(&config, Unit::Mm, &id("t"), &tube).is_empty());
        assert_eq!(tube.to_json()["format"], 2);
        // On a part counted each, or in the wrong unit: invalid.
        let each = p(json!({"format": 2, "kind": KIND, "amount": "600 mm"}));
        let faults = check(&config, Unit::Mm, &id("t"), &each);
        assert_eq!(faults[0].code, Code::PropertyInvalid);
        assert_eq!(faults[0].details["property"], "amount");
        // Without one, the file stays at format 1; a part counted in m
        // needs one to release.
        let bare = p(json!({"format": 1, "kind": KIND, "uom": "m"}));
        assert_eq!(bare.to_json()["format"], 1);
        assert_eq!(missing(&config, &bare), ["number", "amount"]);
        assert_eq!(missing(&config, &tube), ["number"]);
    }
}
