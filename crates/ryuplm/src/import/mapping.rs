//! A mapping file, `imports/<name>.json` (kind `ryuplm.import`, design
//! 734): how one outside source's rows become items. Supporting a new
//! layout means writing one of these, never code.

use super::text::Encoding;
use super::transform::DatePattern;
use crate::catalogue::Manifest;
use crate::error::{Code, Error, Result};
use ryu_doc::files::{Obj, header_in, invalid};
use ryupdm::config::Config;
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};

pub const KIND: &str = "ryuplm.import";
/// The newest mapping format: 2 since design 935's tree readers. A
/// mapping is written as format 2 only when its reader is json or xml.
pub const FORMAT: u64 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReaderKind {
    Csv,
    Xlsx,
    Json,
    Xml,
}

impl ReaderKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ReaderKind::Csv => "csv",
            ReaderKind::Xlsx => "xlsx",
            ReaderKind::Json => "json",
            ReaderKind::Xml => "xml",
        }
    }

    /// A tree reader: its columns are paths in a record.
    pub fn tree(self) -> bool {
        matches!(self, ReaderKind::Json | ReaderKind::Xml)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reader {
    pub kind: ReaderKind,
    pub file: Option<String>,
    pub encoding: Option<Encoding>,
    pub delimiter: Option<char>,
    pub sheet: Option<String>,
    pub header_row: u64,
    /// Tree readers: where the records are (a JSON Pointer, or an element
    /// path from the root), and where a record's children are, relative
    /// to it.
    pub records: Option<String>,
    pub children: Option<String>,
    /// JSON Lines: each line is a record.
    pub lines: bool,
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
    pub date: Option<Vec<DatePattern>>,
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
            && self.date == o.date
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
            date: None,
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

/// How a history orders an item's revisions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Order {
    /// As the file lists them.
    Source,
    /// By date, then as the file lists them.
    Date,
}

/// A history block (design 935): the mapping reads one row per (item,
/// revision), and the item takes the carried revision's values.
#[derive(Clone, Debug, PartialEq)]
pub struct History {
    /// The column holding the revision's label, kept as written.
    pub revision: String,
    pub date: Option<Field>,
    pub state: Option<Field>,
    pub change: Option<Field>,
    pub author: Option<Field>,
    pub order: Order,
    /// Which revisions the legacy system released, as a class rule's
    /// `when`; empty, every one is.
    pub released: BTreeMap<String, String>,
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
    pub history: Option<History>,
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

/// The date patterns at `at`: a list of one or more.
fn dates(v: Value, at: &str) -> Result<Vec<DatePattern>> {
    let Value::Array(list) = v else {
        return Err(invalid(at, "a list of date patterns"));
    };
    if list.is_empty() {
        return Err(invalid(at, "a list of date patterns"));
    }
    list.into_iter()
        .enumerate()
        .map(|(i, p)| {
            let here = format!("{at}/{i}");
            let s = text(p, &here)?;
            DatePattern::parse(&s).map_err(|why| invalid(&here, &why))
        })
        .collect()
}

/// A field object at `at`: its column and transforms.
fn field(body: &Value, at: &str) -> Result<Field> {
    let mut fo = Obj::new(body, at)?;
    let mut field = Field::from(&column(fo.take("from")?, &format!("{at}/from"))?);
    if let Some(x) = fo.take_opt("trim") {
        field.trim = x
            .as_bool()
            .ok_or_else(|| invalid(&format!("{at}/trim"), "true or false"))?;
    }
    if let Some(x) = fo.take_opt("regex") {
        let s = text(x, &format!("{at}/regex"))?;
        field.regex = Some(
            regex_lite::Regex::new(&s)
                .map_err(|e| invalid(&format!("{at}/regex"), &format!("not a regex: {e}")))?,
        );
    }
    if let Some(x) = fo.take_opt("values") {
        let m = x
            .as_object()
            .ok_or_else(|| invalid(&format!("{at}/values"), "an object of value to value"))?;
        for (k, v) in m {
            field.values.insert(
                k.clone(),
                v.as_str()
                    .ok_or_else(|| invalid(&format!("{at}/values/{k}"), "expected a string"))?
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
    if let Some(x) = fo.take_opt("date") {
        field.date = Some(dates(x, &format!("{at}/date"))?);
    }
    fo.finish()?;
    if field.date.is_some() && field.unit.is_some() {
        return Err(invalid(at, "a field takes date or unit, not both"));
    }
    Ok(field)
}

/// A field as a mapping writes it.
fn field_json(f: &Field) -> Value {
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
    if let Some(d) = &f.date {
        o["date"] = d.iter().map(|p| json!(p.text)).collect();
    }
    o
}

impl Mapping {
    pub fn from_json(name: &str, v: &Value) -> Result<Mapping> {
        let mut o = header_in(v, KIND, FORMAT, crate::catalogue::NAME)?;
        let format = v.get("format").and_then(Value::as_u64).unwrap_or(1);
        let r = o.take("reader")?;
        let mut ro = Obj::new(&r, "/reader")?;
        let kind = match text(ro.take("type")?, "/reader/type")?.as_str() {
            "csv" => ReaderKind::Csv,
            "xlsx" => ReaderKind::Xlsx,
            "json" => ReaderKind::Json,
            "xml" => ReaderKind::Xml,
            _ => return Err(invalid("/reader/type", "type is csv, xlsx, json or xml")),
        };
        if kind.tree() && format < 2 {
            return Err(invalid("/format", "a json or xml reader needs format 2"));
        }
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
        let header = ro.take_opt("header_row");
        let header_given = header.is_some();
        let header_row = match header {
            None => 1,
            Some(h) => h
                .as_u64()
                .filter(|h| (1..=1_000_000).contains(h))
                .ok_or_else(|| invalid("/reader/header_row", "a row number from 1"))?,
        };
        let records = ro
            .take_opt("records")
            .map(|x| text(x, "/reader/records"))
            .transpose()?;
        let children = ro
            .take_opt("children")
            .map(|x| text(x, "/reader/children"))
            .transpose()?;
        let lines = match ro.take_opt("lines") {
            None => false,
            Some(x) => x
                .as_bool()
                .ok_or_else(|| invalid("/reader/lines", "true or false"))?,
        };
        ro.finish()?;
        let given = [
            ("encoding", encoding.is_some()),
            ("delimiter", delimiter.is_some()),
            ("sheet", sheet.is_some()),
            ("header_row", header_given),
            ("records", records.is_some()),
            ("children", children.is_some()),
            ("lines", lines),
        ];
        let allowed: &[&str] = match kind {
            ReaderKind::Csv => &["encoding", "delimiter", "header_row"],
            ReaderKind::Xlsx => &["sheet", "header_row"],
            ReaderKind::Json => &["records", "children", "lines"],
            ReaderKind::Xml => &["records", "children"],
        };
        if let Some((n, _)) = given.iter().find(|(n, g)| *g && !allowed.contains(n)) {
            return Err(invalid(
                "/reader",
                &format!(
                    "a {} reader takes {}, not {n}",
                    kind.as_str(),
                    allowed.join(", ")
                ),
            ));
        }
        match kind {
            ReaderKind::Json => {
                match (&records, lines) {
                    (Some(_), true) => {
                        return Err(invalid(
                            "/reader/records",
                            "JSON Lines' records are its lines: give records or lines, not both",
                        ));
                    }
                    (None, false) => {
                        return Err(invalid(
                            "/reader/records",
                            "a json reader needs records, a JSON Pointer to the array of records (\"\" for the document), or lines: true",
                        ));
                    }
                    (Some(r), false) if super::json::tokens(r).is_none() => {
                        return Err(invalid(
                            "/reader/records",
                            "records is a JSON Pointer: empty, or /name/... from the document",
                        ));
                    }
                    _ => {}
                }
                if let Some(c) = &children
                    && (c.is_empty() || super::json::tokens(c).is_none())
                {
                    return Err(invalid(
                        "/reader/children",
                        "children is a JSON Pointer from a record: /name/...",
                    ));
                }
            }
            ReaderKind::Xml => {
                if !records
                    .as_deref()
                    .is_some_and(|r| super::xml::path_ok(r, true))
                {
                    return Err(invalid(
                        "/reader/records",
                        "an xml reader needs records, the elements' path from the root by local name: /Export/Items/Item",
                    ));
                }
                if let Some(c) = &children
                    && !super::xml::path_ok(c, false)
                {
                    return Err(invalid(
                        "/reader/children",
                        "children is an element path from a record by local name: Components/Component",
                    ));
                }
            }
            ReaderKind::Csv | ReaderKind::Xlsx => {}
        }
        let reader = Reader {
            kind,
            file,
            encoding,
            delimiter,
            sheet,
            header_row,
            records,
            children,
            lines,
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
                fields.insert(t.clone(), field(body, &at)?);
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
        let history = match o.take_opt("history") {
            None => None,
            Some(h) => {
                let mut ho = Obj::new(&h, "/history")?;
                let revision = column(ho.take("revision")?, "/history/revision")?;
                let mut part = |k: &str| -> Result<Option<Field>> {
                    let at = format!("/history/{k}");
                    ho.take_opt(k)
                        .map(|x| match x {
                            Value::String(c) if !c.trim().is_empty() => Ok(Field::from(&c)),
                            Value::String(_) => Err(invalid(&at, "a column's name")),
                            Value::Object(_) => field(&x, &at),
                            _ => Err(invalid(&at, "a column's name, or a field")),
                        })
                        .transpose()
                };
                let (date, state, change, author) = (
                    part("date")?,
                    part("state")?,
                    part("change")?,
                    part("author")?,
                );
                let order = match ho.take_opt("order").as_ref().map(Value::as_str) {
                    None | Some(Some("source")) => Order::Source,
                    Some(Some("date")) => Order::Date,
                    _ => return Err(invalid("/history/order", "order is source or date")),
                };
                let mut released = BTreeMap::new();
                if let Some(w) = ho.take_opt("released") {
                    let m = w.as_object().ok_or_else(|| {
                        invalid("/history/released", "an object of column to value")
                    })?;
                    for (k, v) in m {
                        released.insert(
                            k.clone(),
                            v.as_str()
                                .ok_or_else(|| {
                                    invalid(&format!("/history/released/{k}"), "expected a string")
                                })?
                                .to_owned(),
                        );
                    }
                }
                ho.finish()?;
                if order == Order::Date && date.is_none() {
                    return Err(invalid("/history/order", "order: date needs history.date"));
                }
                Some(History {
                    revision,
                    date,
                    state,
                    change,
                    author,
                    order,
                    released,
                })
            }
        };
        if history.is_some() && bom.is_some() {
            return Err(invalid(
                "/history",
                "a history mapping has no bom: revisions of structure aren't imported",
            ));
        }
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
        let dated = fields.values().any(|f| f.date.is_some())
            || history.as_ref().is_some_and(|h| {
                [&h.date, &h.state, &h.change, &h.author]
                    .into_iter()
                    .flatten()
                    .any(|f| f.date.is_some())
            });
        if format < 2 && (history.is_some() || dated) {
            return Err(invalid(
                "/format",
                "a history or a date transform needs format 2",
            ));
        }
        o.finish()?;
        Ok(Mapping {
            name: name.to_owned(),
            reader,
            key,
            fields,
            class,
            bom,
            history,
            numbers,
        })
    }

    pub fn to_json(&self) -> Value {
        let r = &self.reader;
        let mut reader = json!({"type": r.kind.as_str()});
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
        if let Some(x) = &r.records {
            reader["records"] = json!(x);
        }
        if let Some(x) = &r.children {
            reader["children"] = json!(x);
        }
        if r.lines {
            reader["lines"] = json!(true);
        }
        let format = if self.needs_format_2() { FORMAT } else { 1 };
        let mut v = json!({"format": format, "kind": KIND, "reader": reader, "key": self.key});
        if !self.fields.is_empty() {
            let f: Map<String, Value> = self
                .fields
                .iter()
                .map(|(t, f)| (t.clone(), field_json(f)))
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
        if let Some(h) = &self.history {
            let part = |f: &Field| {
                if *f == Field::from(&f.from) {
                    json!(f.from)
                } else {
                    field_json(f)
                }
            };
            let mut o = json!({"revision": h.revision});
            for (k, x) in [
                ("date", &h.date),
                ("state", &h.state),
                ("change", &h.change),
                ("author", &h.author),
            ] {
                if let Some(f) = x {
                    o[k] = part(f);
                }
            }
            if h.order == Order::Date {
                o["order"] = json!("date");
            }
            if !h.released.is_empty() {
                o["released"] = json!(h.released);
            }
            v["history"] = o;
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

    /// Whether this mapping is written as format 2 (design 935): a tree
    /// reader, a history, or a date transform, so an older ryuplm reads
    /// it as `format_too_new`.
    pub fn needs_format_2(&self) -> bool {
        let dated = |f: &Field| f.date.is_some();
        self.reader.kind.tree() || self.history.is_some() || self.fields.values().any(dated)
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
        if let Some(h) = &self.history {
            c.insert(h.revision.as_str());
            c.extend(
                [&h.date, &h.state, &h.change, &h.author]
                    .into_iter()
                    .flatten()
                    .map(|f| f.from.as_str()),
            );
            c.extend(h.released.keys().map(String::as_str));
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

    pub(crate) fn hist() -> Value {
        json!({"format": 2, "kind": "ryuplm.import",
            "reader": {"type": "csv", "file": "history*.csv"},
            "key": "PN", "fields": {"number": {"from": "PN", "trim": true}},
            "numbers": "keep",
            "history": {"revision": "Rev",
                "date": {"from": "Released", "date": ["DD/MM/YYYY", "YYYY-MM-DD"]},
                "state": "Lifecycle", "change": "ECO", "author": {"from": "By", "trim": true},
                "released": {"Lifecycle": "Released"}}})
    }

    #[test]
    fn a_history_mapping_reads_and_writes_as_format_2() {
        let m = Mapping::from_json("h", &hist()).unwrap();
        let h = m.history.as_ref().unwrap();
        assert_eq!((h.revision.as_str(), h.order), ("Rev", Order::Source));
        assert_eq!(h.date.as_ref().unwrap().date.as_ref().unwrap().len(), 2);
        assert_eq!(h.state, Some(Field::from("Lifecycle")));
        assert_eq!(m.to_json(), hist());
        assert_eq!(Mapping::read("h", m.to_text().as_bytes()).unwrap(), m);
        for c in ["PN", "Rev", "Released", "Lifecycle", "ECO", "By"] {
            assert!(m.columns().contains(c), "{c}");
        }
        let mut d = erp();
        d["format"] = json!(2);
        d["fields"]["description"] = json!({"from": "Desc", "date": ["YYYY-MM-DD", "serial"]});
        assert_eq!(Mapping::from_json("erp", &d).unwrap().to_json(), d);
        assert_eq!(
            Mapping::from_json("erp", &erp()).unwrap().to_json(),
            erp(),
            "still format 1"
        );
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
            (
                json!({"format": 1, "kind": "ryuplm.import", "reader": {"type": "json", "records": "/items"}, "key": "k", "fields": {"number": {"from": "n"}}}),
                "/format",
            ),
            (
                json!({"format": 2, "kind": "ryuplm.import", "reader": {"type": "csv", "records": "/items"}, "key": "k", "fields": {"number": {"from": "n"}}}),
                "/reader",
            ),
            (
                json!({"format": 2, "kind": "ryuplm.import", "reader": {"type": "xml", "records": "/A/B", "lines": true}, "key": "k", "fields": {"number": {"from": "n"}}}),
                "/reader",
            ),
            (
                json!({"format": 2, "kind": "ryuplm.import", "reader": {"type": "json"}, "key": "k", "fields": {"number": {"from": "n"}}}),
                "/reader/records",
            ),
            (
                json!({"format": 2, "kind": "ryuplm.import", "reader": {"type": "json", "records": "/items", "lines": true}, "key": "k", "fields": {"number": {"from": "n"}}}),
                "/reader/records",
            ),
            (
                json!({"format": 2, "kind": "ryuplm.import", "reader": {"type": "json", "records": "items"}, "key": "k", "fields": {"number": {"from": "n"}}}),
                "/reader/records",
            ),
            (
                json!({"format": 2, "kind": "ryuplm.import", "reader": {"type": "json", "records": "/items", "children": ""}, "key": "k", "fields": {"number": {"from": "n"}}}),
                "/reader/children",
            ),
            (
                json!({"format": 2, "kind": "ryuplm.import", "reader": {"type": "xml", "records": "Export/Item"}, "key": "k", "fields": {"number": {"from": "n"}}}),
                "/reader/records",
            ),
            (
                json!({"format": 2, "kind": "ryuplm.import", "reader": {"type": "xml", "records": "/Export/Item", "children": "/Components"}, "key": "k", "fields": {"number": {"from": "n"}}}),
                "/reader/children",
            ),
            (
                json!({"format": 2, "kind": "ryuplm.import", "reader": {"type": "xml", "records": "/e:Export/Item"}, "key": "k", "fields": {"number": {"from": "n"}}}),
                "/reader/records",
            ),
            (
                json!({"format": 1, "kind": "ryuplm.import", "reader": {"type": "csv"}, "key": "k", "fields": {"number": {"from": "n"}}, "history": {"revision": "r"}}),
                "/format",
            ),
            (
                json!({"format": 2, "kind": "ryuplm.import", "reader": {"type": "csv"}, "key": "k", "fields": {"number": {"from": "n"}}, "history": {"revision": "r"}, "bom": {"quantity": "q", "root": "R"}}),
                "/history",
            ),
            (
                json!({"format": 2, "kind": "ryuplm.import", "reader": {"type": "csv"}, "key": "k", "fields": {"number": {"from": "n"}}, "history": {"revision": "r", "order": "date"}}),
                "/history/order",
            ),
            (
                json!({"format": 2, "kind": "ryuplm.import", "reader": {"type": "csv"}, "key": "k", "fields": {"number": {"from": "n"}}, "history": {"date": "d"}}),
                "/history/revision",
            ),
            (
                json!({"format": 2, "kind": "ryuplm.import", "reader": {"type": "csv"}, "key": "k", "fields": {"number": {"from": "n"}}, "history": {"revision": "r", "state": 3}}),
                "/history/state",
            ),
            (
                json!({"format": 2, "kind": "ryuplm.import", "reader": {"type": "csv"}, "key": "k", "fields": {"number": {"from": "n", "date": ["MM/YYYY"]}}}),
                "/fields/number/date/0",
            ),
            (
                json!({"format": 2, "kind": "ryuplm.import", "reader": {"type": "csv"}, "key": "k", "fields": {"number": {"from": "n", "date": ["serial"], "unit": "mm"}}}),
                "/fields/number",
            ),
            (
                json!({"format": 1, "kind": "ryuplm.import", "reader": {"type": "csv"}, "key": "k", "fields": {"number": {"from": "n", "date": ["serial"]}}}),
                "/format",
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
    fn tree_mappings_are_format_2_and_csv_ones_stay_1() {
        let j = json!({"format": 2, "kind": "ryuplm.import",
            "reader": {"type": "json", "file": "*.json", "records": "/data/items", "children": "/parts"},
            "key": "/id", "fields": {"number": {"from": "/pn"}},
            "bom": {"level": "#level", "quantity": "/qty"}, "numbers": "keep"});
        let m = Mapping::from_json("t", &j).unwrap();
        assert_eq!(m.reader.kind, ReaderKind::Json);
        assert_eq!(m.reader.records.as_deref(), Some("/data/items"));
        assert_eq!(m.to_json(), j);
        assert_eq!(Mapping::read("t", m.to_text().as_bytes()).unwrap(), m);
        let l = json!({"format": 2, "kind": "ryuplm.import", "reader": {"type": "json", "lines": true},
            "key": "/id", "fields": {"number": {"from": "/pn"}}, "numbers": "keep"});
        assert_eq!(Mapping::from_json("l", &l).unwrap().to_json(), l);
        let x = json!({"format": 2, "kind": "ryuplm.import",
            "reader": {"type": "xml", "records": "/Export/Item", "children": "Components/Component"},
            "key": "@number", "fields": {"number": {"from": "@number"}}, "numbers": "keep"});
        assert_eq!(Mapping::from_json("x", &x).unwrap().to_json(), x);
        let mut c = erp();
        c["format"] = json!(2);
        assert_eq!(
            Mapping::from_json("erp", &c).unwrap().to_json(),
            erp(),
            "csv stays format 1"
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
