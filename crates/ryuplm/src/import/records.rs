//! Rows through a mapping (design 734): each row's key, its mapped values
//! by target, its class, the cells nothing maps (kept in `extra`), and for
//! a BOM its parent, quantity and find number.

use super::mapping::{Mapping, Target, target};
use super::rows::{Record, Refusal, Table};
use crate::error::{Code, Error, Result};
use crate::units::Unit;
use ryupdm::config::Config;
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// A row read through a mapping.
#[derive(Clone, Debug, PartialEq)]
pub struct Mapped {
    pub record: Record,
    /// By target: `number`, `name`, `description`, `unit`, `make_buy`,
    /// `class`, `properties.<name>`, and `aml` (the whole imported list).
    pub values: BTreeMap<String, Value>,
    pub extra: BTreeMap<String, String>,
}

pub const AML_STATUSES: [&str; 4] = ["active", "nrnd", "obsolete", "unknown"];

/// The depth of a level cell: an integer, or a dotted outline's number of
/// parts (`1.2.3` is 3).
pub fn depth(s: &str) -> Option<usize> {
    let s = s.trim();
    if let Ok(n) = s.parse::<usize>() {
        return Some(n);
    }
    let parts: Vec<&str> = s.split('.').collect();
    (parts.len() > 1
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit())))
    .then_some(parts.len())
}

pub fn map(
    table: &Table,
    m: &Mapping,
    config: &Config,
    unit: Unit,
) -> Result<(Vec<Mapped>, Vec<Refusal>)> {
    let missing: Vec<&str> = m
        .columns()
        .into_iter()
        .filter(|c| table.column(c).is_none())
        .collect();
    if !missing.is_empty() {
        return Err(Error::new(
            Code::ImportMappingInvalid,
            format!(
                "the source has no column {}",
                missing
                    .iter()
                    .map(|c| format!("{c:?}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        )
        .with("import", &m.name)
        .with("missing", &missing)
        .with("columns", &table.columns));
    }
    let col = |c: &str| table.column(c).expect("checked above");
    let used = m.columns();
    let mut out: Vec<Mapped> = Vec::with_capacity(table.rows.len());
    let mut refusals = Vec::new();
    let mut first: BTreeMap<String, usize> = BTreeMap::new();
    // Indented BOMs: the open ancestors, (depth, key).
    let mut stack: Vec<(usize, String)> = Vec::new();
    let mut top: Option<usize> = None;
    for row in &table.rows {
        let at = row.at;
        let cell = |c: &str| row.cells[col(c)].as_str();
        let mut refuse = |c: Option<&str>, msg: String| refusals.push(Refusal::at(at, c, msg));
        let key = cell(&m.key).trim().to_owned();
        if key.is_empty() {
            refuse(Some(m.key.as_str()), "the key is empty".into());
            continue;
        }
        let mut values: BTreeMap<String, Value> = BTreeMap::new();
        let mut aml: BTreeMap<usize, BTreeMap<&str, String>> = BTreeMap::new();
        let mut ok = true;
        for (t, f) in &m.fields {
            let Some(v) = super::transform::apply(f, cell(&f.from)) else {
                continue;
            };
            match target(t) {
                Some(Target::Plain("number")) if !crate::item::number_ok(&v) => {
                    refuse(
                        Some(f.from.as_str()),
                        format!(
                            "{v:?} can't be a part number: 1 to 200 characters, no control characters, no space at either end"
                        ),
                    );
                    ok = false;
                }
                Some(Target::Plain("unit")) if !crate::item::UNITS.contains(&v.as_str()) => {
                    refuse(
                        Some(f.from.as_str()),
                        format!("unit {v:?} is none of each, m, m2, m3, kg, l"),
                    );
                    ok = false;
                }
                Some(Target::Plain("make_buy")) if v != "make" && v != "buy" => {
                    refuse(
                        Some(f.from.as_str()),
                        format!("make_buy {v:?} is neither make nor buy; map it with values"),
                    );
                    ok = false;
                }
                Some(Target::Plain(p)) => {
                    values.insert(p.to_owned(), json!(v));
                }
                Some(Target::Property(p)) => {
                    let Some(decl) = config.decl(p) else {
                        return Err(Error::new(
                            Code::ImportMappingInvalid,
                            format!("property {p:?} isn't declared in ryupdm.json"),
                        )
                        .with("import", &m.name));
                    };
                    let value = ryupdm::props::coerce(&decl, json!(v));
                    if let Some(why) = ryupdm::props::fits(&decl, &value, unit) {
                        refuse(Some(f.from.as_str()), format!("{p}: {why}"));
                        ok = false;
                    } else {
                        values.insert(t.clone(), value);
                    }
                }
                Some(Target::Aml(n, part)) => {
                    aml.entry(n).or_default().insert(part, v);
                }
                None => unreachable!("mappings read only known targets"),
            }
        }
        let mut list = Vec::new();
        for (n, e) in &aml {
            let (Some(mf), Some(mpn)) = (e.get("manufacturer"), e.get("mpn")) else {
                refuse(
                    None,
                    format!("aml.{n} needs both a manufacturer and an mpn"),
                );
                ok = false;
                continue;
            };
            let status = e.get("status").cloned().unwrap_or_else(|| "unknown".into());
            if !AML_STATUSES.contains(&status.as_str()) {
                refuse(
                    None,
                    format!("aml.{n}.status {status:?} is none of active, nrnd, obsolete, unknown"),
                );
                ok = false;
                continue;
            }
            list.push(json!({"manufacturer": mf, "mpn": mpn, "status": status}));
        }
        if !list.is_empty() {
            values.insert("aml".into(), Value::Array(list));
        }
        if let Some(r) = m
            .class
            .iter()
            .find(|r| r.when.iter().all(|(c, v)| cell(c).trim() == v))
        {
            values.insert("class".into(), json!(r.class));
        }
        let extra: BTreeMap<String, String> = table
            .columns
            .iter()
            .zip(&row.cells)
            .filter(|(c, v)| !v.is_empty() && !used.contains(c.as_str()))
            .map(|(c, v)| (c.clone(), v.clone()))
            .collect();
        let mut record = Record {
            at,
            key: key.clone(),
            cells: table.cells(row),
            parent: None,
            quantity: None,
            find: None,
            refdes: None,
        };
        if let Some(b) = &m.bom {
            let opt = |c: &Option<String>| {
                c.as_deref()
                    .map(|c| cell(c).trim().to_owned())
                    .filter(|s| !s.is_empty())
            };
            record.quantity = Some(cell(&b.quantity).trim().to_owned()).filter(|s| !s.is_empty());
            record.find = opt(&b.find);
            record.refdes = opt(&b.refdes);
            if b.parent.is_some() {
                record.parent = opt(&b.parent);
            } else if let Some(l) = &b.level {
                match depth(cell(l)) {
                    None => {
                        refuse(
                            Some(l.as_str()),
                            format!(
                                "level {:?} is neither a number nor an outline like 1.2.3",
                                cell(l)
                            ),
                        );
                        ok = false;
                    }
                    Some(d) => {
                        let t = *top.get_or_insert(d);
                        while stack.last().is_some_and(|(sd, _)| *sd >= d) {
                            stack.pop();
                        }
                        if d < t {
                            refuse(
                                Some(l.as_str()),
                                format!("level {d} is above the first row's level {t}"),
                            );
                            ok = false;
                        } else if d > t {
                            match stack.last() {
                                Some((sd, k)) if *sd + 1 == d => record.parent = Some(k.clone()),
                                _ => {
                                    refuse(
                                        Some(l.as_str()),
                                        format!("level {d} has no row at level {} above it", d - 1),
                                    );
                                    ok = false;
                                }
                            }
                        }
                        stack.push((d, key.clone()));
                    }
                }
            }
            let has_parent = record.parent.is_some() || b.root.is_some();
            if has_parent && record.quantity.is_none() {
                refuse(
                    Some(b.quantity.as_str()),
                    "a BOM line needs its quantity".into(),
                );
                ok = false;
            }
        }
        if !ok {
            continue;
        }
        match first.get(&key) {
            Some(&i) if m.bom.is_none() => {
                refuse(
                    Some(m.key.as_str()),
                    format!("key {key:?} is on row {} too", out[i].record.at),
                );
                continue;
            }
            Some(&i) if out[i].values != values => {
                refuse(
                    Some(m.key.as_str()),
                    format!("key {key:?} has other values on row {}", out[i].record.at),
                );
                continue;
            }
            Some(_) => {}
            None => {
                first.insert(key.clone(), out.len());
            }
        }
        out.push(Mapped {
            record,
            values,
            extra,
        });
    }
    Ok((out, refusals))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::import::mapping::Mapping;
    use crate::import::rows::Table;

    fn table(rows: &[&[&str]]) -> Table {
        let raw = rows
            .iter()
            .enumerate()
            .map(|(i, r)| ((i + 1) as u64, r.iter().map(|s| (*s).to_owned()).collect()))
            .collect();
        Table::from_raw(raw, 1).unwrap()
    }

    fn config() -> Config {
        Config::from_json_in(
            &json!({"format": 5, "kind": "ryupdm.config", "properties": {"thread": {"type": "string"}, "length": {"type": "quantity", "dim": "length"}}}),
            "ryuplm",
        )
        .unwrap()
    }

    fn mapping(v: Value) -> Mapping {
        Mapping::from_json("m", &v).unwrap()
    }

    #[test]
    fn an_item_master_maps_row_by_row() {
        let m = mapping(
            json!({"format": 1, "kind": "ryuplm.import", "reader": {"type": "csv"}, "key": "ID",
            "fields": {"number": {"from": "PN", "trim": true}, "properties.thread": {"from": "Desc", "regex": "(M\\d+)"},
                "properties.length": {"from": "Len", "unit": "mm"}, "make_buy": {"from": "Src", "values": {"P": "buy"}},
                "aml.0.manufacturer": {"from": "Mfr"}, "aml.0.mpn": {"from": "MPN"}},
            "class": [{"when": {"Cat": "Screws"}, "class": "fastener"}]}),
        );
        let t = table(&[
            &["ID", "PN", "Desc", "Len", "Src", "Mfr", "MPN", "Cat", "Old"],
            &[
                "1",
                " 100-0001 ",
                "Screw M3",
                "8",
                "P",
                "Acme",
                "S-3",
                "Screws",
                "x",
            ],
            &["2", "100-0002", "Plate", "", "Q", "", "", "", ""],
            &["", "100-0003", "", "", "", "", "", "", ""],
            &["1", "100-0009", "", "", "", "", "", "", ""],
            &["4", "100-0004", "", "", "", "Acme", "", "", ""],
        ]);
        let (rows, refusals) = map(&t, &m, &config(), Unit::Mm).unwrap();
        assert_eq!(rows.len(), 1);
        let r = &rows[0];
        assert_eq!(r.record.key, "1");
        assert_eq!(r.values["number"], "100-0001");
        assert_eq!(r.values["properties.thread"], "M3");
        assert_eq!(r.values["properties.length"], "8 mm");
        assert_eq!(r.values["make_buy"], "buy");
        assert_eq!(r.values["class"], "fastener");
        assert_eq!(
            r.values["aml"],
            json!([{"manufacturer": "Acme", "mpn": "S-3", "status": "unknown"}])
        );
        assert_eq!(
            r.extra,
            BTreeMap::from([("Old".to_owned(), "x".to_owned())])
        );
        let at: Vec<(u64, Option<&str>)> = refusals
            .iter()
            .map(|x| (x.row, x.column.as_deref()))
            .collect();
        assert_eq!(
            at,
            [
                (3, Some("Src")),
                (4, Some("ID")),
                (5, Some("ID")),
                (6, None)
            ]
        );
    }

    #[test]
    fn a_missing_column_is_the_mappings_fault() {
        let m = mapping(
            json!({"format": 1, "kind": "ryuplm.import", "reader": {"type": "csv"}, "key": "ID", "fields": {"number": {"from": "Part"}}}),
        );
        let e = map(&table(&[&["ID", "PN"]]), &m, &config(), Unit::Mm).unwrap_err();
        assert_eq!(e.code, Code::ImportMappingInvalid);
        assert_eq!(e.details["missing"], json!(["Part"]));
    }

    #[test]
    fn indented_boms_find_their_parents() {
        let m = mapping(
            json!({"format": 1, "kind": "ryuplm.import", "reader": {"type": "csv"}, "key": "PN",
            "fields": {"number": {"from": "PN"}}, "bom": {"level": "Lvl", "quantity": "Qty", "find": "Find"}}),
        );
        let t = table(&[
            &["Lvl", "PN", "Qty", "Find"],
            &["0", "TOP", "", ""],
            &["1", "A", "2", "10"],
            &["2", "A1", "4", "10"],
            &["1", "B", "1", "20"],
            &["1", "A", "2", "30"],
            &["3", "X", "1", ""],
        ]);
        let (rows, refusals) = map(&t, &m, &config(), Unit::Mm).unwrap();
        let parents: Vec<(&str, Option<&str>)> = rows
            .iter()
            .map(|r| (r.record.key.as_str(), r.record.parent.as_deref()))
            .collect();
        assert_eq!(
            parents,
            [
                ("TOP", None),
                ("A", Some("TOP")),
                ("A1", Some("A")),
                ("B", Some("TOP")),
                ("A", Some("TOP"))
            ]
        );
        assert_eq!(refusals.len(), 1);
        assert_eq!(
            (refusals[0].row, refusals[0].column.as_deref()),
            (7, Some("Lvl"))
        );
        assert_eq!(depth("1.2.3"), Some(3));
        assert_eq!(depth("1..2"), None);
    }
}
