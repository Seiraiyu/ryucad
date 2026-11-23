//! An item, `items/<ab>/<id>.json` (kind `ryuplm.item`, design 734):
//! something the company makes, buys or ships. Its number is a property,
//! never its identity; its lifecycle is ryupdm's, never stored.

use crate::error::{Code, Error, Result};
use crate::id::{Id, ProjectId};
use ryu_doc::files::{Obj, header_in, invalid};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

pub const KIND: &str = "ryuplm.item";
pub const FORMAT: u64 = 1;
/// Units of measure: ryupdm's, so an item's `unit` is its `uom`.
pub const UNITS: [&str; 6] = ryupdm::config::UOMS;

/// A representation (design 734's Links; judged by 0.B).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    pub product: String,
    pub project: ProjectId,
    pub part: Id,
}

/// A child item: one of this catalogue's, or `<project>:<id>` another's.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ItemRef {
    pub catalogue: Option<ProjectId>,
    pub item: Id,
}

impl ItemRef {
    pub fn parse(s: &str) -> Option<ItemRef> {
        match s.split_once(':') {
            Some((p, i)) => Some(ItemRef {
                catalogue: Some(ProjectId::parse(p).ok()?),
                item: Id::parse(i, "item").ok()?,
            }),
            None => Some(ItemRef {
                catalogue: None,
                item: Id::parse(s, "item").ok()?,
            }),
        }
    }

    pub fn text(&self) -> String {
        match &self.catalogue {
            Some(p) => format!("{p}:{}", self.item),
            None => self.item.to_string(),
        }
    }
}

/// An own BOM line (design 734's BOMs; judged by 0.B).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    pub line: Id,
    pub item: ItemRef,
    /// As written: a number in the child's unit, or a quantity.
    pub quantity: String,
    pub unit: Option<String>,
    pub find: Option<String>,
    pub refdes: Option<String>,
    pub notes: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Item {
    pub number: String,
    pub previous_numbers: Vec<String>,
    pub name: Option<String>,
    pub description: Option<String>,
    pub class: Option<String>,
    /// `None` is `each`.
    pub unit: Option<String>,
    pub make_buy: Option<String>,
    pub properties: BTreeMap<String, Value>,
    /// Entries as written; [`crate::aml`] judges them.
    pub aml: Vec<Value>,
    pub links: Vec<Link>,
    pub bom: Vec<Line>,
    /// Documents named, sorted and without repeats (design 937).
    pub documents: Vec<crate::document::DocRef>,
    /// Provenance by import name, opaque here (0.C).
    pub source: Map<String, Value>,
    /// Unmapped fields by import name, kept as read (0.C).
    pub extra: Map<String, Value>,
}

/// Gives a numbered thing `number`, keeping the one it had first in
/// `previous` (design 734): what names it names its ID, so nothing else
/// changes. Items and documents both renumber this way.
pub fn renumber_in(current: &mut String, previous: &mut Vec<String>, number: &str) -> Result<()> {
    if !number_ok(number) {
        return Err(
            Error::new(Code::InvalidParam, format!("bad part number {number:?}"))
                .with("number", number),
        );
    }
    if number == current {
        return Err(Error::new(
            Code::InvalidParam,
            format!("the number is already {number:?}"),
        )
        .with("number", number));
    }
    let old = std::mem::replace(current, number.to_owned());
    previous.retain(|p| p != number && *p != old);
    previous.insert(0, old);
    Ok(())
}

/// Whether `n` may be a part number: non-empty, at most 200 characters, no
/// control characters, no space at either end. Anything else, any case
/// and punctuation, is kept exactly as written.
pub fn number_ok(n: &str) -> bool {
    !n.is_empty() && n.chars().count() <= 200 && n.trim() == n && !n.chars().any(char::is_control)
}

fn text_at(v: Value, at: &str) -> Result<String> {
    v.as_str()
        .map(str::to_owned)
        .ok_or_else(|| invalid(at, "expected a string"))
}

impl Item {
    pub fn new(number: &str) -> Item {
        Item {
            number: number.to_owned(),
            ..Item::default()
        }
    }

    /// Gives the item `number`, keeping the one it had first in
    /// `previous_numbers` (design 734): links, lines and documents name
    /// the ID, so nothing else changes.
    pub fn renumber(&mut self, number: &str) -> Result<()> {
        renumber_in(&mut self.number, &mut self.previous_numbers, number)
    }

    /// The unit of measure, `each` unless set.
    pub fn unit(&self) -> &str {
        self.unit.as_deref().unwrap_or("each")
    }

    pub fn to_json(&self) -> Value {
        let mut v = json!({"format": FORMAT, "kind": KIND, "number": self.number});
        let o = v.as_object_mut().expect("an object");
        let mut put = |k: &str, x: Value| {
            o.insert(k.to_owned(), x);
        };
        if !self.previous_numbers.is_empty() {
            put("previous_numbers", json!(self.previous_numbers));
        }
        for (k, x) in [
            ("name", &self.name),
            ("description", &self.description),
            ("class", &self.class),
            ("unit", &self.unit),
            ("make_buy", &self.make_buy),
        ] {
            if let Some(x) = x {
                put(k, json!(x));
            }
        }
        if !self.properties.is_empty() {
            put("properties", json!(self.properties));
        }
        if !self.aml.is_empty() {
            put("aml", Value::Array(self.aml.clone()));
        }
        if !self.links.is_empty() {
            put(
                "links",
                self.links
                    .iter()
                    .map(|l| json!({"product": l.product, "project": l.project.as_str(), "part": l.part.as_str()}))
                    .collect(),
            );
        }
        if !self.bom.is_empty() {
            put(
                "bom",
                self.bom
                    .iter()
                    .map(|l| {
                        let mut x = json!({"line": l.line.as_str(), "item": l.item.text(), "quantity": l.quantity});
                        for (k, y) in [("unit", &l.unit), ("find", &l.find), ("refdes", &l.refdes), ("notes", &l.notes)] {
                            if let Some(y) = y {
                                x[k] = json!(y);
                            }
                        }
                        x
                    })
                    .collect(),
            );
        }
        if !self.documents.is_empty() {
            put(
                "documents",
                self.documents.iter().map(|d| json!(d.text())).collect(),
            );
        }
        if !self.source.is_empty() {
            put("source", Value::Object(self.source.clone()));
        }
        if !self.extra.is_empty() {
            put("extra", Value::Object(self.extra.clone()));
        }
        v
    }

    pub fn to_text(&self) -> String {
        crate::canonical::to_string(&self.to_json())
    }

    pub fn from_json(v: &Value) -> Result<Item> {
        let mut o = header_in(v, KIND, FORMAT, crate::catalogue::NAME)?;
        let number = text_at(o.take("number")?, "/number")?;
        if !number_ok(&number) {
            return Err(invalid(
                "/number",
                "a part number is 1 to 200 characters, no control characters, no space at either end",
            ));
        }
        let mut item = Item::new(&number);
        if let Some(p) = o.take_opt("previous_numbers") {
            let list = p
                .as_array()
                .ok_or_else(|| invalid("/previous_numbers", "a list of numbers"))?;
            for (i, n) in list.iter().enumerate() {
                let at = format!("/previous_numbers/{i}");
                let n = n
                    .as_str()
                    .filter(|n| number_ok(n))
                    .ok_or_else(|| invalid(&at, "a part number"))?;
                item.previous_numbers.push(n.to_owned());
            }
        }
        for (k, slot) in [
            ("name", &mut item.name),
            ("description", &mut item.description),
            ("class", &mut item.class),
        ] {
            if let Some(x) = o.take_opt(k) {
                *slot = Some(text_at(x, &format!("/{k}"))?);
            }
        }
        if let Some(u) = o.take_opt("unit") {
            let u = text_at(u, "/unit")?;
            if !UNITS.contains(&u.as_str()) {
                return Err(invalid("/unit", "unit is each, m, m2, m3, kg or l"));
            }
            item.unit = Some(u);
        }
        if let Some(m) = o.take_opt("make_buy") {
            let m = text_at(m, "/make_buy")?;
            if m != "make" && m != "buy" {
                return Err(invalid("/make_buy", "make_buy is make or buy"));
            }
            item.make_buy = Some(m);
        }
        if let Some(p) = o.take_opt("properties") {
            let map = p
                .as_object()
                .ok_or_else(|| invalid("/properties", "properties must be an object"))?;
            item.properties = map.iter().map(|(k, x)| (k.clone(), x.clone())).collect();
        }
        if let Some(a) = o.take_opt("aml") {
            item.aml = a
                .as_array()
                .cloned()
                .ok_or_else(|| invalid("/aml", "aml must be a list"))?;
        }
        if let Some(l) = o.take_opt("links") {
            let list = l
                .as_array()
                .ok_or_else(|| invalid("/links", "links must be a list"))?;
            for (i, x) in list.iter().enumerate() {
                let at = format!("/links/{i}");
                let mut lo = Obj::new(x, &at)?;
                let product = text_at(lo.take("product")?, &format!("{at}/product"))?;
                if product != "ryucad" && product != "ryueda" {
                    return Err(invalid(
                        &format!("{at}/product"),
                        "product is ryucad or ryueda",
                    ));
                }
                let project =
                    ProjectId::parse(&text_at(lo.take("project")?, &format!("{at}/project"))?)
                        .map_err(|e| invalid(&format!("{at}/project"), &e.message))?;
                let part = Id::parse(&text_at(lo.take("part")?, &format!("{at}/part"))?, "part")
                    .map_err(|e| invalid(&format!("{at}/part"), &e.message))?;
                lo.finish()?;
                item.links.push(Link {
                    product,
                    project,
                    part,
                });
            }
        }
        if let Some(b) = o.take_opt("bom") {
            let list = b
                .as_array()
                .ok_or_else(|| invalid("/bom", "bom must be a list"))?;
            for (i, x) in list.iter().enumerate() {
                let at = format!("/bom/{i}");
                let mut lo = Obj::new(x, &at)?;
                let line = text_at(lo.take("line")?, &format!("{at}/line"))?;
                if !crate::ids::generated(&line) || item.bom.iter().any(|l| l.line.as_str() == line)
                {
                    return Err(invalid(
                        &format!("{at}/line"),
                        "a line's ID is 13 base32 characters, once per item",
                    ));
                }
                let child = text_at(lo.take("item")?, &format!("{at}/item"))?;
                let child = ItemRef::parse(&child).ok_or_else(|| {
                    invalid(
                        &format!("{at}/item"),
                        "an item ID, or <catalogue project ID>:<item ID>",
                    )
                })?;
                let quantity = text_at(lo.take("quantity")?, &format!("{at}/quantity"))?;
                let mut opt = |k: &str| -> Result<Option<String>> {
                    lo.take_opt(k)
                        .map(|x| text_at(x, &format!("{at}/{k}")))
                        .transpose()
                };
                let (unit, find, refdes, notes) =
                    (opt("unit")?, opt("find")?, opt("refdes")?, opt("notes")?);
                lo.finish()?;
                item.bom.push(Line {
                    line: Id::parse(&line, "line").expect("checked"),
                    item: child,
                    quantity,
                    unit,
                    find,
                    refdes,
                    notes,
                });
            }
        }
        if let Some(d) = o.take_opt("documents") {
            let list = d
                .as_array()
                .ok_or_else(|| invalid("/documents", "documents must be a list"))?;
            for (i, x) in list.iter().enumerate() {
                let at = format!("/documents/{i}");
                let r = x
                    .as_str()
                    .and_then(crate::document::DocRef::parse)
                    .ok_or_else(|| {
                        invalid(
                            &at,
                            "a document ID, or <catalogue project ID>:<document ID>",
                        )
                    })?;
                item.documents.push(r);
            }
            item.documents.sort();
            item.documents.dedup();
        }
        for (k, slot) in [("source", &mut item.source), ("extra", &mut item.extra)] {
            if let Some(x) = o.take_opt(k) {
                *slot = x
                    .as_object()
                    .cloned()
                    .ok_or_else(|| invalid(&format!("/{k}"), "an object keyed by import name"))?;
            }
        }
        o.finish()?;
        Ok(item)
    }

    /// The item file `bytes` for item `id`: errors name the item.
    pub fn read(id: &Id, bytes: &[u8]) -> Result<Item> {
        let path = crate::ids::item_path(id);
        let at = |e: Error| e.with("item", id).with("path", &path);
        let text = std::str::from_utf8(bytes).map_err(|_| {
            at(Error::new(
                Code::InvalidJson,
                format!("{path} is not UTF-8"),
            ))
        })?;
        let v = crate::canonical::parse(text).map_err(at)?;
        if let Some(e) = ryu_doc::files::conflicted_in(&v, &path, crate::catalogue::NAME) {
            return Err(at(e));
        }
        Item::from_json(&v).map_err(at)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use serde_json::json;

    pub(crate) fn full() -> Value {
        json!({
            "format": 1, "kind": "ryuplm.item",
            "number": "MECH-00042", "previous_numbers": ["100-0042"],
            "name": "Bracket", "description": "Motor bracket, 6061",
            "class": "machined", "unit": "each", "make_buy": "make",
            "properties": {"finish": "anodized"},
            "aml": [{"manufacturer": "Acme", "mpn": "BR-42", "status": "active", "preferred": true}],
            "links": [{"product": "ryucad", "project": "0123456789abcdef0123456789abcdef", "part": "bracket"}],
            "bom": [{"line": "0000000000000", "item": "k3v9q2m8x4c1p", "quantity": "4", "find": "10"}],
            "documents": ["d0000000000000"],
            "source": {"erp": {"key": "42", "digest": "ab"}},
            "extra": {"erp": {"Old Column": "x"}}
        })
    }

    #[test]
    fn an_item_reads_and_writes_canonically() {
        let id = Id::parse("k3v9q2m8x4c1p", "item").unwrap();
        let item = Item::from_json(&full()).unwrap();
        assert_eq!(item.number, "MECH-00042");
        assert_eq!(item.unit(), "each");
        assert_eq!(item.bom[0].quantity, "4");
        assert_eq!(item.to_json(), full());
        let text = item.to_text();
        assert!(crate::canonical::is_canonical(&text));
        assert_eq!(Item::read(&id, text.as_bytes()).unwrap(), item);
        let bare = Item::new("100-1");
        assert_eq!(
            bare.to_json(),
            json!({"format": 1, "kind": "ryuplm.item", "number": "100-1"})
        );
    }

    #[test]
    fn bad_items_say_where() {
        let id = Id::parse("k3v9q2m8x4c1p", "item").unwrap();
        let cases: Vec<(Value, &str)> = vec![
            (json!({"format": 1, "kind": "ryuplm.item"}), "/number"),
            (
                json!({"format": 1, "kind": "ryuplm.item", "number": ""}),
                "/number",
            ),
            (
                json!({"format": 1, "kind": "ryuplm.item", "number": " x"}),
                "/number",
            ),
            (
                json!({"format": 1, "kind": "ryuplm.item", "number": "x", "unit": "pcs"}),
                "/unit",
            ),
            (
                json!({"format": 1, "kind": "ryuplm.item", "number": "x", "make_buy": "both"}),
                "/make_buy",
            ),
            (
                json!({"format": 1, "kind": "ryuplm.item", "number": "x", "properties": []}),
                "/properties",
            ),
            (
                json!({"format": 1, "kind": "ryuplm.item", "number": "x", "status": "released"}),
                "/status",
            ),
            (
                json!({"format": 1, "kind": "ryuplm.item", "number": "x",
                "links": [{"product": "ryudfm", "project": "0123456789abcdef0123456789abcdef", "part": "p"}]}),
                "/links/0/product",
            ),
            (
                json!({"format": 1, "kind": "ryuplm.item", "number": "x",
                "bom": [{"line": "L", "item": "k3v9q2m8x4c1p", "quantity": "1"}]}),
                "/bom/0/line",
            ),
            (
                json!({"format": 1, "kind": "ryuplm.item", "number": "x",
                "bom": [{"line": "0000000000000", "item": "k3v9q2m8x4c1p", "quantity": 1}]}),
                "/bom/0/quantity",
            ),
            (
                json!({"format": 1, "kind": "ryuplm.item", "number": "x",
                "bom": [{"line": "0000000000000", "item": "0123456789abcdef0123456789abcdef:k3v9q2m8x4c1p", "quantity": "1"},
                        {"line": "0000000000000", "item": "k3v9q2m8x4c1p", "quantity": "1"}]}),
                "/bom/1/line",
            ),
        ];
        for (v, at) in cases {
            let e = Item::read(&id, crate::canonical::to_string(&v).as_bytes()).unwrap_err();
            assert_eq!(
                (e.code, e.details["pointer"].clone()),
                (Code::InvalidDocument, json!(at)),
                "{v}"
            );
            assert_eq!(e.details["item"], "k3v9q2m8x4c1p");
        }
    }
    #[test]
    fn documents_are_a_sorted_set() {
        let mut v = full();
        v["documents"] = json!([
            "0123456789abcdef0123456789abcdef:k3v9q2m8x4c1p",
            "k3v9q2m8x4c1p",
            "0000000000001",
            "k3v9q2m8x4c1p"
        ]);
        let item = Item::from_json(&v).unwrap();
        let texts: Vec<String> = item
            .documents
            .iter()
            .map(crate::document::DocRef::text)
            .collect();
        assert_eq!(
            texts,
            [
                "0000000000001",
                "k3v9q2m8x4c1p",
                "0123456789abcdef0123456789abcdef:k3v9q2m8x4c1p"
            ]
        );
        let id = Id::parse("k3v9q2m8x4c1p", "item").unwrap();
        assert_eq!(Item::read(&id, item.to_text().as_bytes()).unwrap(), item);
        v["documents"] = json!(["x:y"]);
        assert_eq!(
            Item::from_json(&v).unwrap_err().details["pointer"],
            "/documents/0"
        );
    }

    #[test]
    fn renumbering_keeps_the_old_number() {
        let mut i = Item::new("A-1");
        i.renumber("B-1").unwrap();
        assert_eq!(
            (i.number.as_str(), i.previous_numbers.clone()),
            ("B-1", vec!["A-1".to_owned()])
        );
        i.renumber("C-1").unwrap();
        assert_eq!(i.previous_numbers, ["B-1", "A-1"]);
        // Back to an old number: it leaves the list, the current one joins.
        i.renumber("A-1").unwrap();
        assert_eq!(
            (i.number.as_str(), i.previous_numbers.clone()),
            ("A-1", vec!["C-1".to_owned(), "B-1".to_owned()])
        );
        assert_eq!(i.renumber("A-1").unwrap_err().code, Code::InvalidParam);
        assert_eq!(i.renumber(" bad").unwrap_err().code, Code::InvalidParam);
    }
}
