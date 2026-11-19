//! Rows through a mapping (design 734): each row's key, its mapped values
//! by target, its class, the cells nothing maps (kept in `extra`), and for
//! a BOM its parent, quantity and find number.

use super::history::{Legacy, Revision};
use super::mapping::{Mapping, Numbers, Order, Target, target};
use super::rows::{Record, Refusal, Table};
use super::transform::Dates;
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
    /// A history mapping's revisions of this key (design 935); `None`
    /// for every other mapping.
    pub history: Option<Legacy>,
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
    let dates = if table.date1904 {
        Dates::S1904
    } else {
        Dates::S1900
    };
    // A history mapping's rows by key, in order of first appearance: the
    // key's first row's record, and its revisions so far.
    let mut groups: Vec<(Record, Vec<Revision>)> = Vec::new();
    let mut group_of: BTreeMap<String, usize> = BTreeMap::new();
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
            let v = match super::transform::apply_in(f, cell(&f.from), dates) {
                Ok(Some(v)) => v,
                Ok(None) => continue,
                Err(why) => {
                    refuse(Some(f.from.as_str()), format!("{t}: {why}"));
                    ok = false;
                    continue;
                }
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
        // `numbers: keep` writes the number as mapped, so a row without one
        // is refused here, where `import check` sees it too.
        if ok
            && m.numbers == Numbers::Keep
            && !values.contains_key("number")
            && let Some(f) = m.fields.get("number")
        {
            refuse(Some(f.from.as_str()), "the part number is empty".into());
            ok = false;
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
        if let Some(h) = &m.history {
            let label = cell(&h.revision).trim().to_owned();
            if label.is_empty() {
                refuse(Some(h.revision.as_str()), "the revision is empty".into());
                continue;
            }
            let mut parts: [Option<String>; 4] = Default::default();
            for (i, f) in [&h.date, &h.state, &h.change, &h.author]
                .into_iter()
                .enumerate()
            {
                let Some(f) = f else { continue };
                match super::transform::apply_in(f, cell(&f.from), dates) {
                    Ok(v) => parts[i] = v,
                    Err(why) => {
                        refuse(Some(f.from.as_str()), format!("history: {why}"));
                        ok = false;
                    }
                }
            }
            if ok && h.order == Order::Date && parts[0].is_none() {
                let from = h
                    .date
                    .as_ref()
                    .expect("order: date has a date")
                    .from
                    .as_str();
                refuse(Some(from), "order: date needs every revision's date".into());
                ok = false;
            }
            if !ok {
                continue;
            }
            let g = match group_of.get(&key) {
                Some(&g) => g,
                None => {
                    group_of.insert(key.clone(), groups.len());
                    groups.push((record, Vec::new()));
                    groups.len() - 1
                }
            };
            if let Some(prev) = groups[g].1.iter().find(|r| r.label == label) {
                refuse(
                    Some(h.revision.as_str()),
                    format!("key {key:?} has revision {label:?} on row {} too", prev.at),
                );
                continue;
            }
            let released = h.released.iter().all(|(c, v)| cell(c).trim() == v);
            let [date, state, change, author] = parts;
            groups[g].1.push(Revision {
                label,
                date,
                state,
                change,
                author,
                values,
                extra,
                at,
                released,
            });
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
            history: None,
        });
    }
    for (record, mut revisions) in groups {
        if m.history.as_ref().is_some_and(|h| h.order == Order::Date) {
            // Stable: revisions of one date keep the file's order.
            revisions.sort_by(|a, b| a.date.cmp(&b.date));
        }
        let carried = revisions.iter().rposition(|r| r.released);
        let take = carried.unwrap_or(revisions.len() - 1);
        out.push(Mapped {
            record,
            values: revisions[take].values.clone(),
            extra: revisions[take].extra.clone(),
            history: Some(Legacy {
                carried: carried.map(|i| revisions[i].label.clone()),
                revisions,
            }),
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

    #[test]
    fn an_empty_number_is_refused_when_numbers_are_kept() {
        let m = mapping(
            json!({"format": 1, "kind": "ryuplm.import", "reader": {"type": "csv"}, "key": "ID",
            "fields": {"number": {"from": "PN", "trim": true}, "description": {"from": "Desc"}}}),
        );
        let t = table(&[
            &["ID", "PN", "Desc"],
            &["1", "  ", "Screw"],
            &["2", "A-2", "Plate"],
        ]);
        let (rows, refusals) = map(&t, &m, &config(), Unit::Mm).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(
            refusals,
            vec![Refusal::at(2, Some("PN"), "the part number is empty")]
        );
        // Assigned numbers need no column.
        let a = mapping(
            json!({"format": 1, "kind": "ryuplm.import", "reader": {"type": "csv"}, "key": "ID",
            "fields": {"description": {"from": "Desc"}}, "numbers": "assign"}),
        );
        let (rows, refusals) = map(&t, &a, &config(), Unit::Mm).unwrap();
        assert_eq!((rows.len(), refusals.len()), (2, 0));
    }

    fn hist() -> Mapping {
        mapping(crate::import::mapping::tests::hist())
    }

    const HEAD: &[&str] = &["PN", "Rev", "Released", "Lifecycle", "ECO", "By", "Note"];

    #[test]
    fn a_date_that_doesnt_read_is_refused_at_its_cell() {
        let m = mapping(
            json!({"format": 2, "kind": "ryuplm.import", "reader": {"type": "csv"}, "key": "ID",
            "fields": {"number": {"from": "PN"}, "description": {"from": "Made", "date": ["DD/MM/YYYY", "serial"]}}}),
        );
        let mut t = table(&[
            &["ID", "PN", "Made"],
            &["1", "A", "07/03/2019"],
            &["2", "B", "43531"],
            &["3", "C", "someday"],
        ]);
        let (rows, refused) = map(&t, &m, &config(), Unit::Mm).unwrap();
        assert_eq!(rows[0].values["description"], "2019-03-07");
        assert_eq!(rows[1].values["description"], "2019-03-07");
        assert_eq!(
            (refused[0].row, refused[0].column.as_deref()),
            (4, Some("Made"))
        );
        t.date1904 = true;
        let (rows, _) = map(&t, &m, &config(), Unit::Mm).unwrap();
        assert_eq!(rows[1].values["description"], "2023-03-08");
    }

    #[test]
    fn history_rows_group_by_key_and_carry_the_last_released() {
        let t = table(&[
            HEAD,
            &["A-1", "A", "07/03/2019", "Released", "E1", " Jo ", "first"],
            &["B-1", "01", "2018-01-15", "Released", "", "Kim", ""],
            &["A-1", "B", "2020-11-02", "Released", "E2", "Jo", ""],
            &["A-1", "C", "", "In Work", "E3", "Al", "next"],
        ]);
        let (rows, refused) = map(&t, &hist(), &config(), Unit::Mm).unwrap();
        assert_eq!(refused, vec![]);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].record.key, "A-1");
        let a = rows[0].history.as_ref().unwrap();
        let labels: Vec<&str> = a.revisions.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(labels, ["A", "B", "C"]);
        assert_eq!(a.carried.as_deref(), Some("B"));
        assert_eq!(a.revisions[0].date.as_deref(), Some("2019-03-07"));
        assert_eq!(a.revisions[0].author.as_deref(), Some("Jo"));
        assert_eq!(a.revisions[2].date, None);
        assert_eq!(a.revisions[2].state.as_deref(), Some("In Work"));
        assert_eq!(a.revisions[0].extra["Note"], "first");
        assert_eq!(rows[0].values["number"], "A-1");
        assert!(rows[0].extra.is_empty(), "B's unmapped cells: none");
        assert_eq!(
            a.revisions[0].to_json(),
            json!({"label": "A", "date": "2019-03-07", "state": "Released", "change": "E1",
                   "author": "Jo", "values": {"number": "A-1"}, "extra": {"Note": "first"}})
        );
        assert_eq!(
            rows[1].history.as_ref().unwrap().carried.as_deref(),
            Some("01")
        );
    }

    #[test]
    fn a_revision_twice_and_an_undated_one_by_date_are_refused() {
        let twice = table(&[
            HEAD,
            &["A-1", "A", "", "Released", "", "", ""],
            &["A-1", "A", "", "Released", "", "", ""],
        ]);
        let (_, refused) = map(&twice, &hist(), &config(), Unit::Mm).unwrap();
        assert_eq!(
            (refused[0].row, refused[0].column.as_deref()),
            (3, Some("Rev"))
        );
        let mut j = crate::import::mapping::tests::hist();
        j["history"]["order"] = json!("date");
        let by_date = mapping(j);
        let t = table(&[
            HEAD,
            &["A-1", "B", "2020-11-02", "Released", "", "", ""],
            &["A-1", "A", "07/03/2019", "Released", "", "", ""],
            &["A-1", "C", "", "In Work", "", "", ""],
        ]);
        let (rows, refused) = map(&t, &by_date, &config(), Unit::Mm).unwrap();
        assert_eq!(
            (refused[0].row, refused[0].column.as_deref()),
            (4, Some("Released"))
        );
        let labels: Vec<&str> = rows[0]
            .history
            .as_ref()
            .unwrap()
            .revisions
            .iter()
            .map(|r| r.label.as_str())
            .collect();
        assert_eq!(labels, ["A", "B"]);
    }

    #[test]
    fn with_none_released_nothing_is_carried_and_the_last_gives_the_values() {
        let t = table(&[
            HEAD,
            &["A-1", "A", "", "In Work", "", "", "a"],
            &["A-1", "B", "", "In Work", "", "", "b"],
        ]);
        let (rows, _) = map(&t, &hist(), &config(), Unit::Mm).unwrap();
        assert_eq!(rows[0].history.as_ref().unwrap().carried, None);
        assert_eq!(rows[0].extra["Note"], "b");
    }
}
