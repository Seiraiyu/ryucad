//! A mapping file, `imports/<name>.json` (kind `ryuplm.import`, design
//! 734): how one outside source's rows become items. Supporting a new
//! layout means writing one of these, never code.

use super::text::Encoding;
use crate::catalogue::Manifest;
use crate::error::{Code, Error, Result};
use ryu_doc::files::{Obj, header_in, invalid};
use ryupdm::config::Config;
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};

pub const KIND: &str = "ryuplm.import";
pub const FORMAT: u64 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReaderKind {
    Csv,
    Xlsx,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reader {
    pub kind: ReaderKind,
    pub file: Option<String>,
    pub encoding: Option<Encoding>,
    pub delimiter: Option<char>,
    pub sheet: Option<String>,
    pub header_row: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Case {
    Upper,
    Lower,
}

/// One target's source column and transforms, applied in the order the
/// fields are declared here.
#[derive(Clone, Debug)]
pub struct Field {
    pub from: String,
    pub trim: bool,
    pub regex: Option<regex_lite::Regex>,
    pub values: BTreeMap<String, String>,
    pub case: Option<Case>,
    pub unit: Option<String>,
}

impl PartialEq for Field {
    fn eq(&self, o: &Field) -> bool {
        self.from == o.from
            && self.trim == o.trim
            && self.regex.as_ref().map(regex_lite::Regex::as_str)
                == o.regex.as_ref().map(regex_lite::Regex::as_str)
            && self.values == o.values
            && self.case == o.case
            && self.unit == o.unit
    }
}

impl Field {
    pub fn from(column: &str) -> Field {
        Field {
            from: column.to_owned(),
            trim: false,
            regex: None,
            values: BTreeMap::new(),
            case: None,
            unit: None,
        }
    }
}

/// A class rule: when every named column's trimmed cell equals its value
/// (always, with none), the item takes `class`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rule {
    pub when: BTreeMap<String, String>,
    pub class: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Bom {
    pub parent: Option<String>,
    pub level: Option<String>,
    pub root: Option<String>,
    pub quantity: String,
    pub find: Option<String>,
    pub refdes: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Numbers {
    Keep,
    /// The scheme to take the next number of; the item's class's when `None`.
    Assign(Option<String>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Mapping {
    pub name: String,
    pub reader: Reader,
    pub key: String,
    pub fields: BTreeMap<String, Field>,
    pub class: Vec<Rule>,
    pub bom: Option<Bom>,
    pub numbers: Numbers,
}

/// Whether `n` may name a mapping: 1 to 63 of a-z, 0-9, `_` and `-`.
pub fn name_ok(n: &str) -> bool {
    ryupdm::config::scheme_name(n)
}

/// The item fields a mapping may set, besides `properties.<name>` and
/// `aml.<n>.<part>`.
pub const PLAIN: [&str; 5] = ["number", "name", "description", "unit", "make_buy"];
pub const AML_PARTS: [&str; 3] = ["manufacturer", "mpn", "status"];

/// What a target is: a plain field, a declared property, or one part of
/// one AML entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target<'a> {
    Plain(&'a str),
    Property(&'a str),
    Aml(usize, &'a str),
}

pub fn target(t: &str) -> Option<Target<'_>> {
    if PLAIN.contains(&t) {
        return Some(Target::Plain(t));
    }
    if let Some(p) = t.strip_prefix("properties.") {
        return (!p.is_empty()).then_some(Target::Property(p));
    }
    let rest = t.strip_prefix("aml.")?;
    let (n, part) = rest.split_once('.')?;
    let n: usize = n.parse().ok().filter(|n: &usize| *n < 10)?;
    AML_PARTS.contains(&part).then_some(Target::Aml(n, part))
}

fn text(v: Value, at: &str) -> Result<String> {
    v.as_str()
        .map(str::to_owned)
        .ok_or_else(|| invalid(at, "expected a string"))
}

fn column(v: Value, at: &str) -> Result<String> {
    let s = text(v, at)?;
    if s.trim().is_empty() {
        return Err(invalid(at, "a column's name"));
    }
    Ok(s)
}

impl Mapping {
    pub fn from_json(name: &str, v: &Value) -> Result<Mapping> {
        let mut o = header_in(v, KIND, FORMAT, crate::catalogue::NAME)?;
        let r = o.take("reader")?;
        let mut ro = Obj::new(&r, "/reader")?;
        let kind = match text(ro.take("type")?, "/reader/type")?.as_str() {
            "csv" => ReaderKind::Csv,
            "xlsx" => ReaderKind::Xlsx,
            _ => return Err(invalid("/reader/type", "type is csv or xlsx")),
        };
        let file = ro
            .take_opt("file")
            .map(|f| text(f, "/reader/file"))
            .transpose()?;
        let encoding = ro
            .take_opt("encoding")
            .map(|e| {
                let s = text(e, "/reader/encoding")?;
                Encoding::parse(&s).ok_or_else(|| {
                    invalid(
                        "/reader/encoding",
                        "encoding is utf-8, utf-16le, utf-16be, latin1 or windows-1252",
                    )
                })
            })
            .transpose()?;
        let delimiter = ro
            .take_opt("delimiter")
            .map(|d| {
                let s = text(d, "/reader/delimiter")?;
                let mut cs = s.chars();
                match (cs.next(), cs.next()) {
                    (Some(c), None) if c != '"' && c != '\n' && c != '\r' => Ok(c),
                    _ => Err(invalid(
                        "/reader/delimiter",
                        "a delimiter is one character, not a quote or a line break",
                    )),
                }
            })
            .transpose()?;
        let sheet = ro
            .take_opt("sheet")
            .map(|s| text(s, "/reader/sheet"))
            .transpose()?;
        let header_row = match ro.take_opt("header_row") {
            None => 1,
            Some(h) => h
                .as_u64()
                .filter(|h| (1..=1_000_000).contains(h))
                .ok_or_else(|| invalid("/reader/header_row", "a row number from 1"))?,
        };
        ro.finish()?;
        if kind == ReaderKind::Xlsx && (encoding.is_some() || delimiter.is_some()) {
            return Err(invalid(
                "/reader",
                "an xlsx reader takes sheet and header_row, not encoding or delimiter",
            ));
        }
        if kind == ReaderKind::Csv && sheet.is_some() {
            return Err(invalid("/reader/sheet", "a csv reader has no sheets"));
        }
        let reader = Reader {
            kind,
            file,
            encoding,
            delimiter,
            sheet,
            header_row,
        };
        let key = column(o.take("key")?, "/key")?;
        let mut fields = BTreeMap::new();
        if let Some(f) = o.take_opt("fields") {
            let map = f
                .as_object()
                .ok_or_else(|| invalid("/fields", "fields must be an object"))?;
            for (t, body) in map {
                let at = format!("/fields/{t}");
                if target(t).is_none() {
                    return Err(invalid(
                        &at,
                        "a target is number, name, description, unit, make_buy, properties.<name> or aml.<0-9>.manufacturer|mpn|status",
                    ));
                }
                let mut fo = Obj::new(body, &at)?;
                let mut field = Field::from(&column(fo.take("from")?, &format!("{at}/from"))?);
                if let Some(x) = fo.take_opt("trim") {
                    field.trim = x
                        .as_bool()
                        .ok_or_else(|| invalid(&format!("{at}/trim"), "true or false"))?;
                }
                if let Some(x) = fo.take_opt("regex") {
                    let s = text(x, &format!("{at}/regex"))?;
                    field.regex = Some(regex_lite::Regex::new(&s).map_err(|e| {
                        invalid(&format!("{at}/regex"), &format!("not a regex: {e}"))
                    })?);
                }
                if let Some(x) = fo.take_opt("values") {
                    let m = x.as_object().ok_or_else(|| {
                        invalid(&format!("{at}/values"), "an object of value to value")
                    })?;
                    for (k, v) in m {
                        field.values.insert(
                            k.clone(),
                            v.as_str()
                                .ok_or_else(|| {
                                    invalid(&format!("{at}/values/{k}"), "expected a string")
                                })?
                                .to_owned(),
                        );
                    }
                }
                if let Some(x) = fo.take_opt("case") {
                    field.case = Some(match text(x, &format!("{at}/case"))?.as_str() {
                        "upper" => Case::Upper,
                        "lower" => Case::Lower,
                        _ => return Err(invalid(&format!("{at}/case"), "case is upper or lower")),
                    });
                }
                if let Some(x) = fo.take_opt("unit") {
                    field.unit = Some(text(x, &format!("{at}/unit"))?);
                }
                fo.finish()?;
                fields.insert(t.clone(), field);
            }
        }
        let mut class = Vec::new();
        match o.take_opt("class") {
            None => {}
            Some(Value::String(c)) => class.push(Rule {
                when: BTreeMap::new(),
                class: c,
            }),
            Some(Value::Array(list)) => {
                for (i, r) in list.iter().enumerate() {
                    let at = format!("/class/{i}");
                    let mut co = Obj::new(r, &at)?;
                    let mut when = BTreeMap::new();
                    if let Some(w) = co.take_opt("when") {
                        let m = w.as_object().ok_or_else(|| {
                            invalid(&format!("{at}/when"), "an object of column to value")
                        })?;
                        for (k, v) in m {
                            when.insert(
                                k.clone(),
                                v.as_str()
                                    .ok_or_else(|| {
                                        invalid(&format!("{at}/when/{k}"), "expected a string")
                                    })?
                                    .to_owned(),
                            );
                        }
                    }
                    let c = text(co.take("class")?, &format!("{at}/class"))?;
                    co.finish()?;
                    class.push(Rule { when, class: c });
                }
            }
            Some(_) => {
                return Err(invalid(
                    "/class",
                    "a class, or a list of {when, class} rules",
                ));
            }
        }
        let bom = match o.take_opt("bom") {
            None => None,
            Some(b) => {
                let mut bo = Obj::new(&b, "/bom")?;
                let mut opt = |k: &str| -> Result<Option<String>> {
                    bo.take_opt(k)
                        .map(|x| column(x, &format!("/bom/{k}")))
                        .transpose()
                };
                let (parent, level, root, find, refdes) = (
                    opt("parent")?,
                    opt("level")?,
                    opt("root")?,
                    opt("find")?,
                    opt("refdes")?,
                );
                let quantity = column(bo.take("quantity")?, "/bom/quantity")?;
                bo.finish()?;
                if parent.is_some() && level.is_some() {
                    return Err(invalid(
                        "/bom",
                        "a BOM has a parent column or a level column, not both",
                    ));
                }
                if parent.is_none() && level.is_none() && root.is_none() {
                    return Err(invalid(
                        "/bom",
                        "a BOM needs a parent column, a level column or a root",
                    ));
                }
                Some(Bom {
                    parent,
                    level,
                    root,
                    quantity,
                    find,
                    refdes,
                })
            }
        };
        let numbers = match o.take_opt("numbers").as_ref().map(Value::as_str) {
            None | Some(Some("keep")) => Numbers::Keep,
            Some(Some("assign")) => Numbers::Assign(None),
            _ => return Err(invalid("/numbers", "numbers is keep or assign")),
        };
        let numbers = match (numbers, o.take_opt("scheme")) {
            (Numbers::Assign(_), Some(s)) => Numbers::Assign(Some(text(s, "/scheme")?)),
            (Numbers::Keep, Some(_)) => {
                return Err(invalid("/scheme", "a scheme is for numbers: assign"));
            }
            (n, None) => n,
        };
        match (&numbers, fields.contains_key("number")) {
            (Numbers::Keep, false) => {
                return Err(invalid("/fields", "numbers: keep needs fields.number"));
            }
            (Numbers::Assign(_), true) => {
                return Err(invalid(
                    "/fields/number",
                    "numbers: assign takes no fields.number",
                ));
            }
            _ => {}
        }
        o.finish()?;
        Ok(Mapping {
            name: name.to_owned(),
            reader,
            key,
            fields,
            class,
            bom,
            numbers,
        })
    }

    pub fn to_json(&self) -> Value {
        let r = &self.reader;
        let mut reader =
            json!({"type": match r.kind { ReaderKind::Csv => "csv", ReaderKind::Xlsx => "xlsx" }});
        if let Some(f) = &r.file {
            reader["file"] = json!(f);
        }
        if let Some(e) = r.encoding {
            reader["encoding"] = json!(e.as_str());
        }
        if let Some(d) = r.delimiter {
            reader["delimiter"] = json!(d.to_string());
        }
        if let Some(s) = &r.sheet {
            reader["sheet"] = json!(s);
        }
        if r.header_row != 1 {
            reader["header_row"] = json!(r.header_row);
        }
        let mut v = json!({"format": FORMAT, "kind": KIND, "reader": reader, "key": self.key});
        if !self.fields.is_empty() {
            let f: Map<String, Value> = self
                .fields
                .iter()
                .map(|(t, f)| {
                    let mut o = json!({"from": f.from});
                    if f.trim {
                        o["trim"] = json!(true);
                    }
                    if let Some(r) = &f.regex {
                        o["regex"] = json!(r.as_str());
                    }
                    if !f.values.is_empty() {
                        o["values"] = json!(f.values);
                    }
                    if let Some(c) = f.case {
                        o["case"] = json!(if c == Case::Upper { "upper" } else { "lower" });
                    }
                    if let Some(u) = &f.unit {
                        o["unit"] = json!(u);
                    }
                    (t.clone(), o)
                })
                .collect();
            v["fields"] = Value::Object(f);
        }
        match self.class.as_slice() {
            [] => {}
            [one] if one.when.is_empty() => v["class"] = json!(one.class),
            rules => {
                v["class"] = rules
                    .iter()
                    .map(|r| {
                        if r.when.is_empty() {
                            json!({"class": r.class})
                        } else {
                            json!({"when": r.when, "class": r.class})
                        }
                    })
                    .collect();
            }
        }
        if let Some(b) = &self.bom {
            let mut o = json!({"quantity": b.quantity});
            for (k, x) in [
                ("parent", &b.parent),
                ("level", &b.level),
                ("root", &b.root),
                ("find", &b.find),
                ("refdes", &b.refdes),
            ] {
                if let Some(x) = x {
                    o[k] = json!(x);
                }
            }
            v["bom"] = o;
        }
        match &self.numbers {
            Numbers::Keep => v["numbers"] = json!("keep"),
            Numbers::Assign(s) => {
                v["numbers"] = json!("assign");
                if let Some(s) = s {
                    v["scheme"] = json!(s);
                }
            }
        }
        v
    }

    pub fn to_text(&self) -> String {
        crate::canonical::to_string(&self.to_json())
    }

    /// The mapping file `bytes` for mapping `name`: errors are
    /// `import_mapping_invalid`, naming the file and the pointer.
    pub fn read(name: &str, bytes: &[u8]) -> Result<Mapping> {
        let path = super::mapping_path(name);
        let at = |e: Error| {
            let mut out = Error::new(Code::ImportMappingInvalid, format!("{path}: {}", e.message))
                .with("path", &path)
                .with("import", name);
            if let Some(p) = e.details.get("pointer") {
                out = out.with("pointer", p);
            }
            out
        };
        if !name_ok(name) {
            return Err(at(invalid(
                "",
                "a mapping's name is 1 to 63 of a-z, 0-9, _ and -",
            )));
        }
        let text = std::str::from_utf8(bytes)
            .map_err(|_| at(Error::new(Code::InvalidJson, "not UTF-8")))?;
        let v = crate::canonical::parse(text).map_err(at)?;
        Mapping::from_json(name, &v).map_err(at)
    }

    /// Every column the mapping reads: the key, each field's, each class
    /// rule's and the BOM's.
    pub fn columns(&self) -> BTreeSet<&str> {
        let mut c: BTreeSet<&str> = BTreeSet::from([self.key.as_str()]);
        c.extend(self.fields.values().map(|f| f.from.as_str()));
        c.extend(
            self.class
                .iter()
                .flat_map(|r| r.when.keys().map(String::as_str)),
        );
        if let Some(b) = &self.bom {
            c.insert(b.quantity.as_str());
            c.extend(
                [&b.parent, &b.level, &b.find, &b.refdes]
                    .into_iter()
                    .flatten()
                    .map(String::as_str),
            );
        }
        c
    }

    /// What the catalogue must declare for this mapping: its properties,
    /// classes and scheme. Each fault is `import_mapping_invalid`.
    pub fn validate(&self, config: &Config, manifest: &Manifest) -> Vec<Error> {
        let path = super::mapping_path(&self.name);
        let bad = |pointer: String, msg: String| {
            Error::new(Code::ImportMappingInvalid, format!("{path}: {msg}"))
                .with("path", &path)
                .with("import", &self.name)
                .with("pointer", pointer)
        };
        let mut out = Vec::new();
        for t in self.fields.keys() {
            if let Some(Target::Property(p)) = target(t)
                && config.decl(p).is_none()
            {
                out.push(bad(
                    format!("/fields/{t}"),
                    format!("property {p:?} isn't declared in ryupdm.json"),
                ));
            }
        }
        for (i, r) in self.class.iter().enumerate() {
            if !manifest.classes.contains_key(&r.class) {
                out.push(bad(
                    format!("/class/{i}/class"),
                    format!("class {:?} isn't declared in ryuplm.json", r.class),
                ));
            }
        }
        if let Numbers::Assign(Some(s)) = &self.numbers
            && !config.schemes.iter().any(|x| &x.name == s)
        {
            out.push(bad(
                "/scheme".into(),
                format!("scheme {s:?} isn't in ryupdm.json"),
            ));
        }
        out
    }
}

/// Whether file name `name` matches `pattern`'s `*` (any run) and `?` (one
/// character).
pub fn file_matches(pattern: &str, name: &str) -> bool {
    fn go(p: &[char], n: &[char]) -> bool {
        match (p.first(), n.first()) {
            (None, None) => true,
            (Some('*'), _) => go(&p[1..], n) || (!n.is_empty() && go(p, &n[1..])),
            (Some('?'), Some(_)) => go(&p[1..], &n[1..]),
            (Some(a), Some(b)) if a == b => go(&p[1..], &n[1..]),
            _ => false,
        }
    }
    let p: Vec<char> = pattern.chars().collect();
    let n: Vec<char> = name.chars().collect();
    go(&p, &n)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn erp() -> Value {
        json!({"format": 1, "kind": "ryuplm.import",
            "reader": {"type": "csv", "file": "items*.csv", "encoding": "windows-1252", "delimiter": ";", "header_row": 2},
            "key": "Item ID",
            "fields": {
                "number": {"from": "Part Number", "trim": true},
                "description": {"from": "Desc", "trim": true, "case": "upper"},
                "make_buy": {"from": "Source", "values": {"M": "make", "P": "buy"}},
                "properties.thread": {"from": "Desc", "regex": "\\b(M\\d+)\\b"},
                "aml.0.manufacturer": {"from": "Mfr"},
                "aml.0.mpn": {"from": "MPN"}},
            "class": [{"when": {"Category": "Screws"}, "class": "fastener"}, {"class": "machined"}],
            "numbers": "keep"})
    }

    #[test]
    fn a_mapping_reads_and_writes_canonically() {
        let m = Mapping::from_json("erp", &erp()).unwrap();
        assert_eq!(m.reader.encoding, Some(Encoding::Windows1252));
        assert_eq!(m.reader.delimiter, Some(';'));
        assert_eq!(m.class.len(), 2);
        assert_eq!(m.to_json(), erp());
        let text = m.to_text();
        assert!(crate::canonical::is_canonical(&text));
        assert_eq!(Mapping::read("erp", text.as_bytes()).unwrap(), m);
        assert!(m.columns().contains("Category"));
    }

    #[test]
    fn bad_mappings_say_where() {
        let cases: Vec<(Value, &str)> = vec![
            (
                json!({"format": 1, "kind": "ryuplm.import", "reader": {"type": "ods"}, "key": "k", "fields": {"number": {"from": "n"}}}),
                "/reader/type",
            ),
            (
                json!({"format": 1, "kind": "ryuplm.import", "reader": {"type": "xlsx", "delimiter": ";"}, "key": "k", "fields": {"number": {"from": "n"}}}),
                "/reader",
            ),
            (
                json!({"format": 1, "kind": "ryuplm.import", "reader": {"type": "csv", "delimiter": ";;"}, "key": "k", "fields": {"number": {"from": "n"}}}),
                "/reader/delimiter",
            ),
            (
                json!({"format": 1, "kind": "ryuplm.import", "reader": {"type": "csv"}, "key": "k", "fields": {"colour": {"from": "c"}}}),
                "/fields/colour",
            ),
            (
                json!({"format": 1, "kind": "ryuplm.import", "reader": {"type": "csv"}, "key": "k", "fields": {"number": {"from": "n", "regex": "("}}}),
                "/fields/number/regex",
            ),
            (
                json!({"format": 1, "kind": "ryuplm.import", "reader": {"type": "csv"}, "key": "k"}),
                "/fields",
            ),
            (
                json!({"format": 1, "kind": "ryuplm.import", "reader": {"type": "csv"}, "key": "k", "numbers": "assign", "fields": {"number": {"from": "n"}}}),
                "/fields/number",
            ),
            (
                json!({"format": 1, "kind": "ryuplm.import", "reader": {"type": "csv"}, "key": "k", "fields": {"number": {"from": "n"}}, "bom": {"quantity": "q"}}),
                "/bom",
            ),
            (
                json!({"format": 1, "kind": "ryuplm.import", "reader": {"type": "csv"}, "key": "k", "fields": {"number": {"from": "n"}}, "extra": 1}),
                "/extra",
            ),
        ];
        for (v, at) in cases {
            let e = Mapping::read("m", crate::canonical::to_string(&v).as_bytes()).unwrap_err();
            assert_eq!(
                (e.code, e.details["pointer"].clone()),
                (Code::ImportMappingInvalid, json!(at)),
                "{v}"
            );
            assert_eq!(e.details["path"], "imports/m.json");
        }
        assert_eq!(
            Mapping::read("Bad Name", b"{}").unwrap_err().code,
            Code::ImportMappingInvalid
        );
    }

    #[test]
    fn targets_and_patterns() {
        assert_eq!(target("aml.3.mpn"), Some(Target::Aml(3, "mpn")));
        assert_eq!(target("aml.10.mpn"), None);
        assert_eq!(target("properties.x"), Some(Target::Property("x")));
        assert_eq!(target("properties."), None);
        assert!(file_matches("items*.csv", "items-2026.csv"));
        assert!(file_matches("bom?.csv", "bom1.csv"));
        assert!(!file_matches("*.csv", "items.xlsx"));
    }
}
