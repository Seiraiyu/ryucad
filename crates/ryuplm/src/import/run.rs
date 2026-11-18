//! `import run` (design 734): rows matched to items by `(import, key)`,
//! new keys made items, unchanged rows skipped by their digest, changed
//! fields settled three ways, BOM rows made own lines, rows gone from the
//! source reported and never deleted. Planned as a pure function over the
//! catalogue, then written through `write::commit`.

use super::mapping::{Mapping, Numbers};
use super::records::Mapped;
use crate::error::{Code, Error, Result};
use crate::fault::Fault;
use crate::id::Id;
use crate::item::Item;
use crate::load::Catalogue;
use crate::session::Session;
use crate::source::Source;
use ryupdm::config::Config;
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

pub const SCHEMA: u64 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Prefer {
    Source,
    Catalogue,
}

impl Prefer {
    pub fn parse(s: &str) -> Result<Prefer> {
        match s {
            "source" => Ok(Prefer::Source),
            "catalogue" => Ok(Prefer::Catalogue),
            _ => Err(Error::new(Code::Usage, "--prefer is source or catalogue")),
        }
    }
}

/// One BOM line an import made: the child's key, the find number, the
/// line's ID on the parent, and its quantity at the last import.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProvLine {
    pub child: String,
    pub find: Option<String>,
    pub line: Id,
    pub quantity: String,
}

/// `source.<import>` on an item (Decision 6).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Provenance {
    pub key: String,
    pub digest: String,
    pub values: BTreeMap<String, Value>,
    pub lines: Vec<ProvLine>,
}

impl Provenance {
    pub fn to_json(&self) -> Value {
        let mut v = json!({"key": self.key, "digest": self.digest, "values": self.values});
        if !self.lines.is_empty() {
            v["lines"] = self
                .lines
                .iter()
                .map(|l| {
                    let mut x =
                        json!({"child": l.child, "line": l.line.as_str(), "quantity": l.quantity});
                    if let Some(f) = &l.find {
                        x["find"] = json!(f);
                    }
                    x
                })
                .collect();
        }
        v
    }

    /// `source.<name>` of item `id`, or `invalid_document` at its pointer.
    pub fn of(id: &Id, item: &Item, name: &str) -> Result<Option<Provenance>> {
        let Some(v) = item.source.get(name) else {
            return Ok(None);
        };
        let bad = || {
            ryu_doc::files::invalid(
                &format!("/source/{name}"),
                "provenance is {key, digest, values, lines?}",
            )
            .with("item", id)
        };
        let o = v.as_object().ok_or_else(bad)?;
        let text = |k: &str| o.get(k).and_then(Value::as_str).map(str::to_owned);
        let values = o.get("values").and_then(Value::as_object).ok_or_else(bad)?;
        let mut lines = Vec::new();
        for l in o
            .get("lines")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let lt = |k: &str| l.get(k).and_then(Value::as_str).map(str::to_owned);
            lines.push(ProvLine {
                child: lt("child").ok_or_else(bad)?,
                find: lt("find"),
                line: lt("line")
                    .and_then(|s| Id::parse(&s, "line").ok())
                    .ok_or_else(bad)?,
                quantity: lt("quantity").ok_or_else(bad)?,
            });
        }
        Ok(Some(Provenance {
            key: text("key").ok_or_else(bad)?,
            digest: text("digest").ok_or_else(bad)?,
            values: values.iter().map(|(k, x)| (k.clone(), x.clone())).collect(),
            lines,
        }))
    }
}

/// BLAKE3 over the mapped values and the unmapped ones, 64 hex digits.
pub fn digest(values: &BTreeMap<String, Value>, extra: &BTreeMap<String, String>) -> String {
    let mut h = blake3::Hasher::new();
    h.update(b"ryuplm.import.1\0");
    h.update(crate::canonical::to_string(&json!({"extra": extra, "values": values})).as_bytes());
    h.finalize().to_hex().to_string()
}

/// The item's value for target `t`, as a mapping's values hold it.
pub fn current(item: &Item, t: &str) -> Option<Value> {
    let s = |x: &Option<String>| x.clone().map(Value::from);
    match t {
        "number" => Some(json!(item.number)),
        "name" => s(&item.name),
        "description" => s(&item.description),
        "unit" => s(&item.unit),
        "make_buy" => s(&item.make_buy),
        "class" => s(&item.class),
        "aml" => (!item.aml.is_empty()).then(|| Value::Array(item.aml.clone())),
        _ => t
            .strip_prefix("properties.")
            .and_then(|p| item.properties.get(p).cloned()),
    }
}

/// Sets target `t` of `item` to `v` (`None` clears it; a number is never
/// cleared, and changes through `renumber`, keeping the old one).
pub fn set(item: &mut Item, t: &str, v: Option<&Value>) -> Result<()> {
    let s = || v.and_then(Value::as_str).map(str::to_owned);
    match t {
        "number" => {
            if let Some(n) = s()
                && n != item.number
            {
                item.renumber(&n)?;
            }
        }
        "name" => item.name = s(),
        "description" => item.description = s(),
        "unit" => item.unit = s(),
        "make_buy" => item.make_buy = s(),
        "class" => item.class = s(),
        "aml" => item.aml = v.and_then(Value::as_array).cloned().unwrap_or_default(),
        _ => {
            let p = t.strip_prefix("properties.").expect("a mapped target");
            match v {
                Some(x) => {
                    item.properties.insert(p.to_owned(), x.clone());
                }
                None => {
                    item.properties.remove(p);
                }
            }
        }
    }
    Ok(())
}

/// What a run decided, to report.
#[derive(Default)]
struct Tally {
    created: Vec<Value>,
    adopted: Vec<Value>,
    updated: Vec<Value>,
    unchanged: usize,
    conflicts: Vec<Value>,
    missing: Vec<Value>,
    lines_added: Vec<Value>,
    lines_changed: Vec<Value>,
    lines_unchanged: usize,
}

/// What the three-way rule (Decision 7) makes of one value: `Take` sets
/// the source's, `Keep` leaves the catalogue's (either way the base
/// becomes the source's), `Conflict` leaves the value and the base.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Settled {
    Take,
    Keep,
    Conflict,
}

/// The rule for base `b`, the catalogue's `c` and the source's `n`.
pub fn settle(
    b: Option<&Value>,
    c: Option<&Value>,
    n: Option<&Value>,
    prefer: Option<Prefer>,
) -> Settled {
    if n == b {
        Settled::Keep
    } else if c == b {
        Settled::Take
    } else if c == n {
        Settled::Keep
    } else {
        match prefer {
            Some(Prefer::Source) => Settled::Take,
            Some(Prefer::Catalogue) => Settled::Keep,
            None => Settled::Conflict,
        }
    }
}

/// An `import_conflict` warning, with all three values.
#[allow(clippy::too_many_arguments)]
fn conflict(
    id: &Id,
    key: &str,
    row: u64,
    field: &str,
    b: Option<&Value>,
    c: Option<&Value>,
    n: Option<&Value>,
) -> Value {
    Fault::warning(Code::ImportConflict, format!("item {id}: {field} changed in the source and in the catalogue; --prefer source|catalogue, or edit it"))
        .with("item", id)
        .with("key", key)
        .with("row", row)
        .with("field", field)
        .with("base", b)
        .with("catalogue", c)
        .with("source", n)
        .to_json()
}

/// The rule over every target `base` or `new` names: changes `item`,
/// returns the values to store as the new base and the fields changed,
/// and adds each conflict to `conflicts`.
#[allow(clippy::too_many_arguments)]
fn three_way(
    id: &Id,
    item: &mut Item,
    key: &str,
    row: u64,
    base: &BTreeMap<String, Value>,
    new: &BTreeMap<String, Value>,
    prefer: Option<Prefer>,
    conflicts: &mut Vec<Value>,
) -> Result<(BTreeMap<String, Value>, Vec<String>)> {
    let targets: BTreeSet<&String> = base.keys().chain(new.keys()).collect();
    let mut stored = BTreeMap::new();
    let mut changed = Vec::new();
    for t in targets {
        let (b, n) = (base.get(t), new.get(t));
        let c = current(item, t);
        let keep = match settle(b, c.as_ref(), n, prefer) {
            Settled::Take => {
                if c.as_ref() != n {
                    set(item, t, n)?;
                    changed.push(t.clone());
                }
                n
            }
            Settled::Keep => n,
            Settled::Conflict => {
                conflicts.push(conflict(id, key, row, t, b, c.as_ref(), n));
                b
            }
        };
        if let Some(x) = keep {
            stored.insert(t.clone(), x.clone());
        }
    }
    Ok((stored, changed))
}

/// What a run plans: every item it changes or makes, and the report.
pub struct Outcome {
    pub items: BTreeMap<Id, Item>,
    pub report: Value,
}

fn row_json(id: &Id, item: &Item, r: &Mapped) -> Value {
    json!({"item": id, "number": item.number, "key": r.record.key, "row": r.record.at})
}

pub fn plan(
    src: &dyn Source,
    cat: &Catalogue,
    config: &Config,
    m: &Mapping,
    rows: &[Mapped],
    prefer: Option<Prefer>,
    limit: usize,
) -> Result<Outcome> {
    let name = m.name.as_str();
    // The items this import made before, by key.
    let mut by_key: BTreeMap<String, Id> = BTreeMap::new();
    let mut prov: BTreeMap<Id, Provenance> = BTreeMap::new();
    for (id, item) in &cat.items {
        if let Some(p) = Provenance::of(id, item, name)? {
            by_key.insert(p.key.clone(), id.clone());
            prov.insert(id.clone(), p);
        }
    }
    let mut work: BTreeMap<Id, Item> = BTreeMap::new();
    let mut t = Tally::default();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut taken: BTreeSet<String> = cat.items.keys().map(|i| i.as_str().to_owned()).collect();
    let mut seqs: BTreeMap<String, (ryupdm::config::Template, u64)> = BTreeMap::new();
    for r in rows {
        let key = &r.record.key;
        if !seen.insert(key.clone()) {
            continue; // a BOM's later row for the same item: its line only
        }
        let new_digest = digest(&r.values, &r.extra);
        if let Some(id) = by_key.get(key).cloned() {
            let p = prov.get_mut(&id).expect("indexed with its provenance");
            if p.digest == new_digest {
                t.unchanged += 1;
                continue;
            }
            let mut item = work.remove(&id).unwrap_or_else(|| cat.items[&id].clone());
            let (stored, changed) = three_way(
                &id,
                &mut item,
                key,
                r.record.at,
                &p.values,
                &r.values,
                prefer,
                &mut t.conflicts,
            )?;
            p.values = stored;
            p.digest = digest(&p.values, &r.extra);
            set_extra(&mut item, name, &r.extra);
            item.source.insert(name.to_owned(), p.to_json());
            if item != cat.items[&id] {
                let mut v = row_json(&id, &item, r);
                v["fields"] = json!(changed);
                t.updated.push(v);
            } else {
                t.unchanged += 1;
            }
            work.insert(id, item);
            continue;
        }
        // A key this import hasn't seen: adopt an untouched item holding
        // its number (Decision 9), or make one.
        let number = r.values.get("number").and_then(Value::as_str);
        let adopt = match (&m.numbers, number) {
            (Numbers::Keep, Some(n)) => cat
                .items
                .iter()
                .find(|(i, it)| {
                    it.number == n && !it.source.contains_key(name) && !work.contains_key(*i)
                })
                .map(|(i, _)| i.clone()),
            _ => None,
        };
        let (id, mut item, stored) = match adopt {
            Some(id) => {
                let mut item = cat.items[&id].clone();
                let (stored, _) = three_way(
                    &id,
                    &mut item,
                    key,
                    r.record.at,
                    &BTreeMap::new(),
                    &r.values,
                    prefer,
                    &mut t.conflicts,
                )?;
                t.adopted.push(row_json(&id, &item, r));
                (id, item, stored)
            }
            None => {
                let n = match &m.numbers {
                    Numbers::Keep => number
                        .expect("keep maps number; an empty one is refused when read")
                        .to_owned(),
                    Numbers::Assign(scheme) => {
                        let class = r.values.get("class").and_then(Value::as_str);
                        let s = crate::numbers::scheme(config, scheme.as_deref(), class)?;
                        let entry = match seqs.get_mut(&s.name) {
                            Some(e) => e,
                            None => {
                                let first =
                                    crate::numbers::next(src, cat, config, Some(&s.name), class)?;
                                let tpl = s
                                    .next
                                    .clone()
                                    .expect("next() refuses a scheme without a template");
                                let seq = tpl.read(&first).expect("next() fills its own template");
                                seqs.entry(s.name.clone()).or_insert((tpl, seq))
                            }
                        };
                        let n = entry.0.fill(entry.1);
                        entry.1 += 1;
                        n
                    }
                };
                let mut item = Item::new(&n);
                for (k, v) in &r.values {
                    set(&mut item, k, Some(v))?;
                }
                let id = crate::ids::random(|s| taken.contains(s))?;
                taken.insert(id.as_str().to_owned());
                t.created.push(row_json(&id, &item, r));
                (id, item, r.values.clone())
            }
        };
        let p = Provenance {
            key: key.clone(),
            digest: digest(&stored, &r.extra),
            values: stored,
            lines: Vec::new(),
        };
        set_extra(&mut item, name, &r.extra);
        item.source.insert(name.to_owned(), p.to_json());
        by_key.insert(key.clone(), id.clone());
        prov.insert(id.clone(), p);
        work.insert(id, item);
    }
    lines(cat, m, rows, &by_key, &mut prov, &mut work, prefer, &mut t)?;
    missing(cat, name, &seen, &by_key, &mut t);
    let cap = |v: &[Value]| -> Vec<Value> { v.iter().take(limit).cloned().collect() };
    let report = json!({
        "schema": SCHEMA,
        "import": name,
        "rows": rows.len(),
        "created": {"count": t.created.len(), "first": cap(&t.created)},
        "adopted": {"count": t.adopted.len(), "first": cap(&t.adopted)},
        "updated": {"count": t.updated.len(), "first": cap(&t.updated)},
        "unchanged": t.unchanged,
        "lines": {"added": {"count": t.lines_added.len(), "first": cap(&t.lines_added)},
                  "changed": {"count": t.lines_changed.len(), "first": cap(&t.lines_changed)},
                  "unchanged": t.lines_unchanged},
        "conflicts": t.conflicts,
        "missing": t.missing,
    });
    // Only what changed is written.
    work.retain(|id, item| cat.items.get(id) != Some(item));
    Ok(Outcome {
        items: work,
        report,
    })
}

fn set_extra(item: &mut Item, name: &str, extra: &BTreeMap<String, String>) {
    if extra.is_empty() {
        item.extra.remove(name);
    } else {
        item.extra.insert(
            name.to_owned(),
            Value::Object(
                extra
                    .iter()
                    .map(|(k, v)| (k.clone(), json!(v)))
                    .collect::<Map<_, _>>(),
            ),
        );
    }
}

/// BOM rows as own lines (Task 15).
#[allow(clippy::too_many_arguments)]
fn lines(
    _cat: &Catalogue,
    _m: &Mapping,
    _rows: &[Mapped],
    _by_key: &BTreeMap<String, Id>,
    _prov: &mut BTreeMap<Id, Provenance>,
    _work: &mut BTreeMap<Id, Item>,
    _prefer: Option<Prefer>,
    _t: &mut Tally,
) -> Result<()> {
    Ok(())
}

/// Items and lines this import made that its source no longer has (Task 14).
fn missing(
    _cat: &Catalogue,
    _name: &str,
    _seen: &BTreeSet<String>,
    _by_key: &BTreeMap<String, Id>,
    _t: &mut Tally,
) {
}

/// `import.run`.
#[allow(clippy::too_many_arguments)]
pub fn run(
    session: &mut Session,
    name: &str,
    file: &Path,
    dry_run: bool,
    prefer: Option<Prefer>,
    limit: Option<usize>,
    expected: Option<&str>,
) -> Result<Value> {
    crate::items::check_expected("the catalogue", &session.revision(), expected)?;
    let p = super::prepare(session, name, file)?;
    if !p.refusals.is_empty() {
        return Err(super::refused(name, file, p.refusals));
    }
    let out = plan(
        &session.snapshot(),
        session.catalogue(),
        &p.config,
        &p.mapping,
        &p.rows,
        prefer,
        limit.unwrap_or(100),
    )?;
    let mut report = out.report;
    report["file"] = json!(file);
    report["dry_run"] = json!(dry_run);
    let writes = crate::write::Writes(
        out.items
            .iter()
            .map(|(id, it)| {
                Ok((
                    crate::ids::item_path(id),
                    Some(crate::items::text_of(id, it)?),
                ))
            })
            .collect::<Result<Vec<_>>>()?,
    );
    report["written"] = json!(if dry_run { 0 } else { writes.0.len() });
    if dry_run {
        let refused = crate::write::would_add(session, &writes)?;
        report["would_refuse"] = crate::fault::list(&refused);
        return Ok(report);
    }
    if !writes.0.is_empty() {
        crate::write::commit(session, &format!("importing {name}"), writes)?;
    }
    Ok(report)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::import::mapping::Mapping;
    use crate::import::rows::Table;

    /// A catalogue's files with the classes, properties and the `mech`
    /// scheme the tests use, and `items`.
    pub(crate) fn files(items: &[(&str, Item)]) -> BTreeMap<String, Vec<u8>> {
        let config = json!({"format": 5, "kind": "ryupdm.config",
            "part_number": {"schemes": [{"name": "mech", "pattern": "MECH-\\d{5}", "next": "MECH-{seq:5}"}, {"name": "legacy", "pattern": ".*"}]},
            "properties": {"thread": {"type": "string"}}});
        crate::judge::tests::files(
            &[("fastener", None), ("machined", None)],
            Some(config),
            items,
        )
    }

    pub(crate) fn rows(m: &Mapping, csv: &str) -> Vec<Mapped> {
        let table = Table::from_raw(crate::import::csv::parse(csv, ',').unwrap(), 1).unwrap();
        let config = Config::from_json_in(&json!({"format": 5, "kind": "ryupdm.config", "properties": {"thread": {"type": "string"}}}), "ryuplm").unwrap();
        let (rows, refusals) =
            crate::import::records::map(&table, m, &config, crate::units::Unit::Mm).unwrap();
        assert_eq!(refusals, vec![]);
        rows
    }

    pub(crate) fn master() -> Mapping {
        Mapping::from_json("erp", &json!({"format": 1, "kind": "ryuplm.import", "reader": {"type": "csv"}, "key": "ID",
            "fields": {"number": {"from": "PN"}, "description": {"from": "Desc"}, "properties.thread": {"from": "Thread"}},
            "class": [{"when": {"Kind": "S"}, "class": "fastener"}, {"class": "machined"}]})).unwrap()
    }

    pub(crate) fn planned(
        f: &BTreeMap<String, Vec<u8>>,
        m: &Mapping,
        csv: &str,
        prefer: Option<Prefer>,
    ) -> (Catalogue, Outcome) {
        // SAFETY: tests that read the seed set the same value.
        unsafe { std::env::set_var(crate::ids::SEED_VAR, "test") };
        let cat = Catalogue::load(f).unwrap();
        let config = Config::read_in(f, "ryuplm").unwrap();
        let out = plan(f, &cat, &config, m, &rows(m, csv), prefer, 100).unwrap();
        (cat, out)
    }

    /// `f` with `out`'s items written.
    pub(crate) fn apply(f: &mut BTreeMap<String, Vec<u8>>, out: &Outcome) {
        for (id, it) in &out.items {
            f.insert(crate::ids::item_path(id), it.to_text().into_bytes());
        }
    }

    const CSV: &str = "ID,PN,Desc,Thread,Kind,Old\n1,100-0001,Screw,M3,S,a\n2,100-0002,Plate,,P,\n";

    #[test]
    fn a_first_import_creates_items_with_provenance() {
        let (_, out) = planned(&files(&[]), &master(), CSV, None);
        assert_eq!(out.report["created"]["count"], 2);
        assert_eq!(out.items.len(), 2);
        let screw = out.items.values().find(|i| i.number == "100-0001").unwrap();
        assert_eq!(screw.class.as_deref(), Some("fastener"));
        assert_eq!(screw.properties["thread"], "M3");
        assert_eq!(screw.extra["erp"], json!({"Old": "a"}));
        let p = &screw.source["erp"];
        assert_eq!(p["key"], "1");
        assert_eq!(p["values"]["description"], "Screw");
        assert_eq!(p["digest"].as_str().unwrap().len(), 64);
        let plate = out.items.values().find(|i| i.number == "100-0002").unwrap();
        assert!(plate.extra.is_empty());
        assert_eq!(plate.class.as_deref(), Some("machined"));
    }

    #[test]
    fn numbers_are_assigned_by_scheme() {
        let m = Mapping::from_json(
            "erp",
            &json!({"format": 1, "kind": "ryuplm.import", "reader": {"type": "csv"}, "key": "ID",
            "fields": {"description": {"from": "Desc"}}, "numbers": "assign", "scheme": "mech"}),
        )
        .unwrap();
        let mut existing = Item::new("MECH-00007");
        existing.description = Some("already here".into());
        let (_, out) = planned(
            &files(&[("0000000000001", existing)]),
            &m,
            "ID,Desc\na,One\nb,Two\n",
            None,
        );
        let mut numbers: Vec<&str> = out.items.values().map(|i| i.number.as_str()).collect();
        numbers.sort();
        assert_eq!(numbers, ["MECH-00008", "MECH-00009"]);
    }

    #[test]
    fn an_existing_number_is_adopted() {
        let mut mine = Item::new("100-0001");
        mine.description = Some("Screw".into());
        mine.properties.insert("thread".into(), json!("M4"));
        let (_, out) = planned(&files(&[("0000000000001", mine)]), &master(), CSV, None);
        assert_eq!(out.report["adopted"]["count"], 1);
        assert_eq!(out.report["created"]["count"], 1);
        let adopted = &out.items[&Id::parse("0000000000001", "item").unwrap()];
        assert_eq!(adopted.description.as_deref(), Some("Screw"), "equal: fine");
        assert_eq!(
            adopted.class.as_deref(),
            Some("fastener"),
            "empty: takes the source's"
        );
        assert_eq!(adopted.properties["thread"], "M4", "different: left alone");
        let c = &out.report["conflicts"][0];
        assert_eq!(
            (c["code"].as_str(), c["details"]["field"].as_str()),
            (Some("import_conflict"), Some("properties.thread"))
        );
        assert_eq!(c["details"]["base"], Value::Null);
        assert!(
            adopted.source["erp"]["values"]
                .get("properties.thread")
                .is_none(),
            "the conflict keeps no base"
        );
    }

    #[test]
    fn a_dry_run_writes_nothing_and_a_bad_run_is_refused() {
        let (_d, root) = crate::server::tests::catalogue();
        std::fs::create_dir_all(root.join("imports")).unwrap();
        std::fs::write(root.join("imports/erp.json"), master().to_text()).unwrap();
        for c in ["fastener", "machined"] {
            crate::server::tests::ask(
                &root,
                crate::proto::Request::SettingsEdit {
                    change: serde_json::from_value(json!({"class": c})).unwrap(),
                    expected_revision: None,
                },
            )
            .unwrap();
        }
        crate::server::tests::ask(
            &root,
            crate::proto::Request::SettingsEdit {
                change: serde_json::from_value(
                    json!({"property": "thread", "declaration": {"type": "string"}}),
                )
                .unwrap(),
                expected_revision: None,
            },
        )
        .unwrap();
        let src = root.join("items.csv");
        std::fs::write(&src, CSV).unwrap();
        let ask = |dry: bool| {
            crate::server::tests::ask(
                &root,
                crate::proto::Request::ImportRun {
                    name: "erp".into(),
                    file: src.display().to_string(),
                    dry_run: dry,
                    prefer: None,
                    limit: None,
                    expected_revision: None,
                },
            )
        };
        let dry = ask(true).unwrap();
        assert_eq!(
            (dry["created"]["count"].clone(), dry["written"].clone()),
            (json!(2), json!(0))
        );
        assert_eq!(dry["would_refuse"], json!([]));
        assert!(!root.join("items").exists());
        let run = ask(false).unwrap();
        assert_eq!(run["written"], 2);
        assert!(root.join("items").is_dir());
        // Two rows with one number: the write would add part_number_duplicate.
        std::fs::write(
            &src,
            "ID,PN,Desc,Thread,Kind,Old\n3,100-0003,A,,P,\n4,100-0003,B,,P,\n",
        )
        .unwrap();
        let e = ask(false).unwrap_err();
        assert_eq!(e.code, Code::PartNumberDuplicate);
    }
}
