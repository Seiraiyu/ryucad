//! BOMs as lines (design 629): what a product's BOM is once the product
//! has grouped what a part places into purchasable lines (ryueda's, from
//! a design's netlist). Their columns, CSV and diff are ryupdm's, so
//! every product's line BOM reads alike. ryucad's assembly BOM
//! (`crate::bom`, feature `cad`) is a tree of rows, and shares only the
//! CSV's quoting and its declared properties with this.

use crate::config::Config;
use ryu_doc::id::ProjectId;
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// The version of the shape line BOMs, their diffs and ryueda's
/// where-used answer with. Adding a field is not a change.
pub const SCHEMA: u64 = 1;

/// One line of a BOM.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    /// What tells it apart from the BOM's other lines, its revision left
    /// out: the product's key (ryueda's: part, value, footprint).
    pub key: String,
    /// The part it is, as the product names it.
    pub part: String,
    /// The other project it is a part of; `None` for the reading
    /// project's own.
    pub project: Option<ProjectId>,
    /// The label it is placed at, or `working`.
    pub revision: String,
    /// How many it counts.
    pub quantity: u64,
    /// Its properties: the part's file over the product's defaults.
    pub properties: BTreeMap<String, Value>,
    /// The names in `properties` the product's files gave.
    pub defaulted: Vec<String>,
    /// The product's own columns, by name, as CSV writes them.
    pub columns: BTreeMap<String, String>,
}

/// ryupdm's columns, before a product's own.
pub const CORE_COLUMNS: [&str; 8] = [
    "number",
    "name",
    "description",
    "part",
    "revision",
    "quantity",
    "uom",
    "make_buy",
];

/// One field as RFC 4180 writes it: quoted when it holds a comma, a
/// quote or a line break, quotes doubled.
pub(crate) fn field(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_owned()
    }
}

/// The declared properties' names that aren't core ones, in the order
/// the settings give them.
pub(crate) fn custom(config: &Config) -> Vec<String> {
    config
        .decls()
        .into_iter()
        .map(|(n, _)| n)
        .filter(|n| !crate::config::CORE.contains(&n.as_str()))
        .collect()
}

/// A BOM's columns, in order: [`CORE_COLUMNS`], the product's
/// (`product`), then each declared custom property.
pub fn columns(config: &Config, product: &[&str]) -> Vec<String> {
    let mut out: Vec<String> = CORE_COLUMNS.iter().map(|c| (*c).to_owned()).collect();
    out.extend(product.iter().map(|c| (*c).to_owned()));
    out.extend(custom(config));
    out
}

/// A property's value as text: a string as it is, anything else as JSON.
fn text(line: &Line, name: &str) -> String {
    match line.properties.get(name) {
        None => String::new(),
        Some(Value::String(s)) => s.clone(),
        Some(v) => v.to_string(),
    }
}

/// What `line` holds in `column`.
pub fn cell(line: &Line, column: &str) -> String {
    match column {
        "part" => line.part.clone(),
        "revision" => line.revision.clone(),
        "quantity" => line.quantity.to_string(),
        "uom" => match text(line, "uom") {
            s if s.is_empty() => "each".to_owned(),
            s => s,
        },
        c => match line.columns.get(c) {
            Some(v) => v.clone(),
            None => text(line, c),
        },
    }
}

/// A line as answers give it: its part, project, revision, quantity,
/// unit and `make_buy`, its properties and where each came from
/// (`properties` or `product`), and the product's columns.
pub fn line_json(line: &Line) -> Value {
    let from: BTreeMap<&String, &str> = line
        .properties
        .keys()
        .map(|k| {
            let by = if line.defaulted.contains(k) {
                "product"
            } else {
                "properties"
            };
            (k, by)
        })
        .collect();
    let mut v = json!({
        "part": line.part,
        "revision": line.revision,
        "quantity": line.quantity,
        "uom": cell(line, "uom"),
        "make_buy": cell(line, "make_buy"),
        "properties": line.properties,
        "from": from,
    });
    if let Some(p) = &line.project {
        v["project"] = json!(p);
    }
    for (k, c) in &line.columns {
        v[k] = json!(c);
    }
    v
}

/// One CSV record of `fields`, quoted where needed, ending in a newline.
fn record(fields: impl IntoIterator<Item = String>) -> String {
    let mut out = fields
        .into_iter()
        .map(|f| field(&f))
        .collect::<Vec<_>>()
        .join(",");
    out.push('\n');
    out
}

/// `lines` as CSV: a header of [`columns`], then a row per line.
pub fn csv(lines: &[Line], config: &Config, product: &[&str]) -> String {
    let cols = columns(config, product);
    let mut out = record(cols.iter().cloned());
    for l in lines {
        out.push_str(&record(cols.iter().map(|c| cell(l, c))));
    }
    out
}

/// Groups of lines as CSV, each group's name in a first column called
/// `first` (ryueda's `block`), groups in the order given.
pub fn csv_grouped(
    groups: &[(String, Vec<Line>)],
    first: &str,
    config: &Config,
    product: &[&str],
) -> String {
    let cols = columns(config, product);
    let mut out = record(std::iter::once(first.to_owned()).chain(cols.iter().cloned()));
    for (name, lines) in groups {
        for l in lines {
            out.push_str(&record(
                std::iter::once(name.clone()).chain(cols.iter().map(|c| cell(l, c))),
            ));
        }
    }
    out
}

/// What changed between two lines of one key, `None` when nothing did:
/// the revision, the quantity, each of the product's `compared` columns
/// and each property (marked `revising` as the settings declare it).
fn change(a: &Line, b: &Line, config: &Config, compared: &[&str]) -> Option<Value> {
    let mut v = json!({"part": b.part});
    if let Some(p) = &b.project {
        v["project"] = json!(p);
    }
    let mut changed = false;
    if a.revision != b.revision {
        v["revision"] = json!({"from": a.revision, "to": b.revision});
        changed = true;
    } else {
        v["revision"] = json!(b.revision);
    }
    if a.quantity != b.quantity {
        v["quantity"] = json!({"from": a.quantity, "to": b.quantity});
        changed = true;
    }
    for c in compared {
        let (x, y) = (cell(a, c), cell(b, c));
        if x != y {
            v[*c] = json!({"from": x, "to": y});
            changed = true;
        }
    }
    let mut props = serde_json::Map::new();
    for k in a.properties.keys().chain(b.properties.keys()) {
        let (x, y) = (a.properties.get(k), b.properties.get(k));
        if x != y && !props.contains_key(k) {
            let revising = config.decl(k).is_none_or(|d| d.revising);
            props.insert(k.clone(), json!({"from": x, "to": y, "revising": revising}));
        }
    }
    if !props.is_empty() {
        v["properties"] = Value::Object(props);
        changed = true;
    }
    changed.then_some(v)
}

/// What changed from the lines `from` to the lines `to`: lines paired by
/// key (in order, where a key has several), the rest added or removed;
/// `{summary, added, removed, changed}`, each list in key order.
pub fn diff(from: &[Line], to: &[Line], config: &Config, compared: &[&str]) -> Value {
    diff_pairing(from, to, config, compared, &[])
}

/// [`diff`], then each removed line paired with the first added line of the
/// same part and project whose `same` columns agree (ryueda: refdes, dnp)
/// into one changed row, so a line whose key moved (a revalue) reads as
/// changed (#1078). With `same` empty, [`diff`]. Lists in key order.
pub fn diff_pairing(
    from: &[Line],
    to: &[Line],
    config: &Config,
    compared: &[&str],
    same: &[&str],
) -> Value {
    let mut keys: Vec<&str> = from.iter().chain(to).map(|l| l.key.as_str()).collect();
    keys.sort_unstable();
    keys.dedup();
    let (mut added, mut removed, mut changed): (Vec<&Line>, Vec<&Line>, Vec<(&str, Value)>) =
        Default::default();
    for k in keys {
        let a: Vec<&Line> = from.iter().filter(|l| l.key == k).collect();
        let b: Vec<&Line> = to.iter().filter(|l| l.key == k).collect();
        for (x, y) in a.iter().zip(&b) {
            if let Some(c) = change(x, y, config, compared) {
                changed.push((k, c));
            }
        }
        removed.extend(a.iter().skip(b.len()).copied());
        added.extend(b.iter().skip(a.len()).copied());
    }
    if !same.is_empty() {
        let mut kept = Vec::new();
        for r in removed {
            let found = added.iter().position(|a| {
                a.part == r.part
                    && a.project == r.project
                    && same.iter().all(|c| cell(a, c) == cell(r, c))
            });
            match found {
                Some(i) => {
                    let a = added.remove(i);
                    if let Some(c) = change(r, a, config, compared) {
                        changed.push((r.key.as_str(), c));
                    }
                }
                None => kept.push(r),
            }
        }
        removed = kept;
        // Stable: rows of one key keep their order.
        changed.sort_by(|x, y| x.0.cmp(y.0));
    }
    json!({
        "summary": {"added": added.len(), "removed": removed.len(), "changed": changed.len()},
        "added": added.iter().map(|l| line_json(l)).collect::<Vec<_>>(),
        "removed": removed.iter().map(|l| line_json(l)).collect::<Vec<_>>(),
        "changed": changed.into_iter().map(|(_, c)| c).collect::<Vec<_>>(),
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::config::Config;

    pub(crate) fn line(part: &str, revision: &str, quantity: u64) -> Line {
        Line {
            key: format!("{part}\0"),
            part: part.to_owned(),
            project: None,
            revision: revision.to_owned(),
            quantity,
            properties: BTreeMap::from([
                ("number".to_owned(), json!("100-0001")),
                ("name".to_owned(), json!("10k")),
                ("make_buy".to_owned(), json!("buy")),
            ]),
            defaulted: vec!["name".to_owned()],
            columns: BTreeMap::from([("refdes".to_owned(), "R1,R2".to_owned())]),
        }
    }

    /// #1078: a line whose key moved (a revalue) pairs by part and the
    /// `same` columns into one changed row; with another designator it doesn't.
    #[test]
    fn a_line_whose_key_moved_pairs_by_part_and_the_same_columns() {
        let config = Config::default();
        let mut a = line("self/r", "working", 1);
        a.key = "self/r\u{0}100k\u{0}".into();
        a.columns.insert("refdes".into(), "R9".into());
        a.columns.insert("value".into(), "100k".into());
        let mut b = a.clone();
        b.key = "self/r\u{0}330k\u{0}".into();
        b.columns.insert("value".into(), "330k".into());
        let cols = ["refdes", "value"];
        assert_eq!(
            diff(&[a.clone()], &[b.clone()], &config, &cols)["summary"],
            json!({"added": 1, "removed": 1, "changed": 0})
        );
        let p = diff_pairing(&[a.clone()], &[b.clone()], &config, &cols, &["refdes"]);
        assert_eq!(
            p["summary"],
            json!({"added": 0, "removed": 0, "changed": 1}),
            "{p}"
        );
        assert_eq!(
            p["changed"][0]["value"],
            json!({"from": "100k", "to": "330k"})
        );
        let mut c = b;
        c.columns.insert("refdes".into(), "R10".into());
        assert_eq!(
            diff_pairing(&[a], &[c], &config, &cols, &["refdes"])["summary"],
            json!({"added": 1, "removed": 1, "changed": 0})
        );
    }

    #[test]
    fn columns_are_the_cores_then_the_products_then_custom() {
        let config = Config::default();
        assert_eq!(
            columns(&config, &["refdes", "mpn"]),
            [
                "number",
                "name",
                "description",
                "part",
                "revision",
                "quantity",
                "uom",
                "make_buy",
                "refdes",
                "mpn"
            ]
        );
        let l = line("self/r", "working", 2);
        assert_eq!(cell(&l, "uom"), "each");
        assert_eq!(cell(&l, "quantity"), "2");
        assert_eq!(cell(&l, "refdes"), "R1,R2");
        assert_eq!(cell(&l, "description"), "");
        let v = line_json(&l);
        assert_eq!(v["from"]["name"], "product");
        assert_eq!(v["from"]["number"], "properties");
        assert_eq!(v["refdes"], "R1,R2");
        assert!(v.get("project").is_none());
        assert_eq!(field("a,\"b\""), "\"a,\"\"b\"\"\"");
    }

    #[test]
    fn csv_has_a_header_and_a_row_per_line_quoted_where_needed() {
        let config = Config::default();
        let lines = [line("self/r", "working", 2), line("p:c", "A", 1)];
        let csv = csv(&lines, &config, &["refdes"]);
        assert_eq!(
            csv,
            "number,name,description,part,revision,quantity,uom,make_buy,refdes\n\
             100-0001,10k,,self/r,working,2,each,buy,\"R1,R2\"\n\
             100-0001,10k,,p:c,A,1,each,buy,\"R1,R2\"\n"
        );
        let grouped = csv_grouped(
            &[("d:".to_owned(), lines.to_vec())],
            "block",
            &config,
            &["refdes"],
        );
        assert!(grouped.starts_with("block,number,"), "{grouped}");
        assert!(grouped.contains("\nd:,100-0001,10k,,self/r,"), "{grouped}");
    }

    #[test]
    fn a_diff_pairs_lines_by_key_and_says_what_changed() {
        let config = Config::default();
        let a = vec![line("p:c", "A", 2), line("self/r", "working", 1)];
        let mut b = vec![line("p:c", "B", 3), line("self/u", "working", 1)];
        b[0].columns.insert("refdes".into(), "R1,R2,R3".into());
        let d = diff(&a, &b, &config, &["refdes"]);
        assert_eq!(
            d["summary"],
            json!({"added": 1, "removed": 1, "changed": 1})
        );
        let c = &d["changed"][0];
        assert_eq!(c["revision"], json!({"from": "A", "to": "B"}));
        assert_eq!(c["quantity"], json!({"from": 2, "to": 3}));
        assert_eq!(c["refdes"], json!({"from": "R1,R2", "to": "R1,R2,R3"}));
        assert_eq!(d["added"][0]["part"], "self/u");
        assert_eq!(d["removed"][0]["part"], "self/r");
        let same = diff(&a, &a, &config, &["refdes"]);
        assert_eq!(
            same["summary"],
            json!({"added": 0, "removed": 0, "changed": 0})
        );
    }
}
