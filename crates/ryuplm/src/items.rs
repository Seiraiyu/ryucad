//! The item commands: create, show, list, edit, renumber, delete.

use crate::error::{Code, Error, Result};
use crate::id::Id;
use crate::ids::item_path;
use crate::item::Item;
use crate::session::Session;
use crate::write::{Writes, commit};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// The fields `item create` and `item edit` set. `properties` merges into
/// the item's (a `null` value removes one).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fields {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub class: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub make_buy: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub properties: Option<BTreeMap<String, Value>>,
    /// The whole AML, replacing the item's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aml: Option<Vec<Value>>,
}

/// The fields `item edit --unset` may clear.
const UNSETTABLE: [&str; 6] = ["name", "description", "class", "unit", "make_buy", "aml"];

fn apply(item: &mut Item, f: Fields) {
    if f.name.is_some() {
        item.name = f.name;
    }
    if f.description.is_some() {
        item.description = f.description;
    }
    if f.class.is_some() {
        item.class = f.class;
    }
    if f.unit.is_some() {
        item.unit = f.unit;
    }
    if f.make_buy.is_some() {
        item.make_buy = f.make_buy;
    }
    if let Some(p) = f.properties {
        for (k, v) in p {
            if v.is_null() {
                item.properties.remove(&k);
            } else {
                item.properties.insert(k, v);
            }
        }
    }
    if let Some(a) = f.aml {
        item.aml = a;
    }
}

/// The item text, checked as the file reader checks it, so a bad field
/// is refused with its pointer before anything is judged.
pub(crate) fn text_of(id: &Id, item: &Item) -> Result<String> {
    let text = item.to_text();
    Item::read(id, text.as_bytes())?;
    Ok(text)
}

pub(crate) fn check_expected(what: &str, actual: &str, expected: Option<&str>) -> Result<()> {
    match expected {
        Some(e) if e != actual => Err(Error::new(
            Code::Conflict,
            format!("{what} changed since revision {e}; read it again and retry"),
        )
        .with("expected", e)
        .with("actual", actual)),
        _ => Ok(()),
    }
}

fn number_for(
    session: &Session,
    number: Option<String>,
    next: Option<String>,
    class: Option<&str>,
) -> Result<String> {
    match (number, next) {
        (Some(n), None) => Ok(n),
        (None, Some(scheme)) => {
            let config =
                ryupdm::config::Config::read_in(&session.snapshot(), crate::catalogue::NAME)?;
            let name = (!scheme.is_empty()).then_some(scheme.as_str());
            crate::numbers::next(
                &session.snapshot(),
                session.catalogue(),
                &config,
                name,
                class,
            )
        }
        _ => Err(Error::new(
            Code::Usage,
            "give a number or --next [<scheme>], not both",
        )),
    }
}

/// An item as answers show it: its ID, path, revision and file.
pub(crate) fn shown(session: &Session, id: &Id) -> Value {
    let mut item = session.catalogue().items[id].to_json();
    if let Some(o) = item.as_object_mut() {
        o.remove("format");
        o.remove("kind");
        o.insert("id".into(), json!(id));
    }
    json!({"schema": crate::OUTPUT_SCHEMA, "item": item, "path": item_path(id), "revision": session.item_revision(id)})
}

pub fn create(
    session: &mut Session,
    fields: Fields,
    number: Option<String>,
    next: Option<String>,
    expected: Option<&str>,
) -> Result<Value> {
    check_expected("the catalogue", &session.revision(), expected)?;
    let number = number_for(session, number, next, fields.class.as_deref())?;
    let mut item = Item::new(&number);
    apply(&mut item, fields);
    let id = crate::ids::random(|s| {
        Id::parse(s, "item").is_ok_and(|i| session.catalogue().items.contains_key(&i))
    })?;
    let text = text_of(&id, &item)?;
    commit(
        session,
        &format!("creating item {number}"),
        Writes(vec![(item_path(&id), Some(text))]),
    )?;
    Ok(shown(session, &id))
}

pub fn show(session: &mut Session, item: &str) -> Result<Value> {
    let id = session.catalogue().find(item)?;
    let mut v = shown(session, &id);
    let releases = crate::release::releases_or_none(session);
    v["state"] =
        ryupdm::state::derive_for::<crate::Plm>(&session.snapshot(), &id, &releases).to_json();
    let faults: Vec<Value> = crate::judge::all(
        &session.snapshot(),
        session.catalogue(),
        &Default::default(),
    )
    .into_iter()
    .filter(|f| f.error.details["item"] == json!(id))
    .map(|f| f.to_json())
    .collect();
    v["faults"] = json!(faults);
    let legacy = crate::import::history::legacy(&session.catalogue().items[&id]);
    if !legacy.is_empty() {
        v["legacy"] = json!(legacy);
    }
    Ok(v)
}

/// How `item.list` asks.
#[derive(Clone, Debug, Default)]
pub struct ListAsk {
    pub number: Option<String>,
    pub class: Option<String>,
    pub text: Option<String>,
    pub state: Option<String>,
    pub make_buy: Option<String>,
    pub faults: Option<String>,
    pub sort: Option<String>,
    pub offset: Option<usize>,
    pub limit: Option<usize>,
}

fn bad_param(param: &str, value: &str, allowed: &[&str]) -> Error {
    Error::new(
        Code::InvalidParam,
        format!("{param} is {}, not {value:?}", allowed.join(", ")),
    )
    .with("param", param)
    .with("allowed", allowed)
}

/// `item.list`: `{schema, total, offset, count, items}`. `total` counts
/// the items matching the filters; `items` are those from `offset`, at
/// most `limit` of them, in `sort`'s order (by ID when none is named; by
/// `number`'s own order, the current holder first, when only a number is).
/// Each row: `id`, `number`, `name`, `class`, `unit`, `make_buy`,
/// `state`, `label` when the state has one, and its own faults counted
/// (`errors`, `warnings`).
pub fn list(session: &mut Session, ask: &ListAsk) -> Result<Value> {
    use crate::listing::Sort;
    let sort = ask
        .sort
        .as_deref()
        .map(|s| Sort::parse(s).ok_or_else(|| bad_param("sort", s, &Sort::NAMES)))
        .transpose()?;
    let faults = match ask.faults.as_deref() {
        None => None,
        Some("errors") => Some(true),
        Some("warnings") => Some(false),
        Some(other) => return Err(bad_param("faults", other, &["errors", "warnings"])),
    };
    let picked: Option<Vec<Id>> = ask
        .number
        .as_deref()
        .map(|n| session.catalogue().by_number(n));
    let mut listing = crate::listing::of(session);
    // The order walked: a named sort's; else a number's own; else by ID.
    let order: std::sync::Arc<Vec<u32>> = match (&sort, &picked) {
        (None, Some(ids)) => std::sync::Arc::new(
            ids.iter()
                .filter_map(|id| listing.rows().binary_search_by(|r| r.id.cmp(id)).ok())
                .map(|k| k as u32)
                .collect(),
        ),
        _ => listing.order(sort.unwrap_or(Sort::Id)),
    };
    let needle = ask.text.as_deref().map(str::to_lowercase);
    let (offset, limit) = (ask.offset.unwrap_or(0), ask.limit.unwrap_or(usize::MAX));
    let mut total = 0usize;
    let mut rows = Vec::new();
    for &k in order.iter() {
        let r = &listing.rows()[k as usize];
        let keep = picked.as_ref().is_none_or(|ids| ids.contains(&r.id))
            && ask
                .class
                .as_ref()
                .is_none_or(|c| r.class.as_ref() == Some(c))
            && ask.state.as_ref().is_none_or(|s| &r.state == s)
            && ask
                .make_buy
                .as_ref()
                .is_none_or(|m| r.make_buy.as_ref() == Some(m))
            && faults.is_none_or(|errors| if errors { r.errors > 0 } else { r.warnings > 0 })
            && needle.as_ref().is_none_or(|t| r.hay.contains(t.as_str()));
        if !keep {
            continue;
        }
        if total >= offset && rows.len() < limit {
            let mut v = json!({
                "id": r.id, "number": r.number, "name": r.name, "class": r.class,
                "unit": r.unit, "make_buy": r.make_buy, "state": r.state,
                "errors": r.errors, "warnings": r.warnings,
            });
            if let Some(l) = &r.label {
                v["label"] = json!(l);
            }
            rows.push(v);
        }
        total += 1;
    }
    crate::listing::put(session, listing);
    Ok(json!({
        "schema": crate::OUTPUT_SCHEMA,
        "total": total,
        "offset": offset,
        "count": rows.len(),
        "items": rows,
    }))
}

/// The item `item` names, its revision checked against `expected`.
pub(crate) fn current(session: &Session, item: &str, expected: Option<&str>) -> Result<(Id, Item)> {
    let id = session.catalogue().find(item)?;
    let rev = session.item_revision(&id).unwrap_or_default();
    check_expected(&format!("item {id}"), &rev, expected)?;
    Ok((id.clone(), session.catalogue().items[&id].clone()))
}

pub fn edit(
    session: &mut Session,
    item: &str,
    fields: Fields,
    unset: Vec<String>,
    expected: Option<&str>,
) -> Result<Value> {
    let (id, mut it) = current(session, item, expected)?;
    for u in &unset {
        match u.as_str() {
            "name" => it.name = None,
            "description" => it.description = None,
            "class" => it.class = None,
            "unit" => it.unit = None,
            "make_buy" => it.make_buy = None,
            "aml" => it.aml.clear(),
            other => {
                return Err(Error::new(
                    Code::InvalidParam,
                    format!("can't unset {other:?}: {}", UNSETTABLE.join(", ")),
                )
                .with("field", other));
            }
        }
    }
    apply(&mut it, fields);
    let text = text_of(&id, &it)?;
    commit(
        session,
        &format!("editing item {id}"),
        Writes(vec![(item_path(&id), Some(text))]),
    )?;
    Ok(shown(session, &id))
}

pub fn renumber(
    session: &mut Session,
    item: &str,
    number: Option<String>,
    next: Option<String>,
    expected: Option<&str>,
) -> Result<Value> {
    let (id, mut it) = current(session, item, expected)?;
    let n = number_for(session, number, next, it.class.as_deref())?;
    it.renumber(&n)?;
    let text = text_of(&id, &it)?;
    commit(
        session,
        &format!("renumbering item {id} as {n}"),
        Writes(vec![(item_path(&id), Some(text))]),
    )?;
    Ok(shown(session, &id))
}

pub fn delete(session: &mut Session, item: &str, expected: Option<&str>) -> Result<Value> {
    let (id, it) = current(session, item, expected)?;
    commit(
        session,
        &format!("deleting item {id}"),
        Writes(vec![(item_path(&id), None)]),
    )?;
    Ok(json!({"deleted": id, "number": it.number}))
}

#[cfg(test)]
mod tests {
    use crate::proto::Request;
    use crate::server::tests::{ask, catalogue};
    use serde_json::json;

    fn fields(v: serde_json::Value) -> super::Fields {
        serde_json::from_value(v).unwrap()
    }

    fn list(root: &std::path::Path, ask_: serde_json::Value) -> serde_json::Value {
        ask(root, Request::from_rpc("item.list", Some(ask_)).unwrap()).unwrap()
    }

    #[test]
    fn a_list_is_a_window_of_a_sorted_filtered_whole() {
        let (_d, root) = catalogue();
        ask(
            &root,
            Request::SettingsEdit {
                change: crate::settings::Change {
                    class: Some("fastener".into()),
                    ..Default::default()
                },
                expected_revision: None,
            },
        )
        .unwrap();
        let mk = |n: &str, f: serde_json::Value| {
            ask(
                &root,
                Request::ItemCreate {
                    fields: serde_json::from_value(f).unwrap(),
                    number: Some(n.into()),
                    next: None,
                    expected_revision: None,
                },
            )
            .unwrap()["item"]["id"]
                .as_str()
                .unwrap()
                .to_owned()
        };
        mk(
            "S-3",
            json!({"name": "Screw", "class": "fastener", "make_buy": "buy"}),
        );
        mk(
            "B-1",
            json!({"name": "Bracket", "make_buy": "make", "unit": "kg"}),
        );
        let nut = mk(
            "N-2",
            json!({"name": "Nut", "class": "fastener", "make_buy": "buy"}),
        );
        mk("L-4", json!({"description": "a label for the box"}));
        let all = list(&root, json!({}));
        assert_eq!(
            (
                all["schema"].clone(),
                all["total"].clone(),
                all["count"].clone(),
                all["offset"].clone()
            ),
            (json!(1), json!(4), json!(4), json!(0))
        );
        // The default order is by ID, as it always was.
        let ids: Vec<&str> = all["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["id"].as_str().unwrap())
            .collect();
        let mut sorted = ids.clone();
        sorted.sort();
        assert_eq!(ids, sorted);
        let row = all["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["number"] == "B-1")
            .unwrap();
        assert_eq!(
            (
                row["state"].clone(),
                row["unit"].clone(),
                row["make_buy"].clone(),
                row["errors"].clone(),
                row["warnings"].clone()
            ),
            (
                json!("in_work"),
                json!("kg"),
                json!("make"),
                json!(0),
                json!(0)
            )
        );
        let numbers = |v: &serde_json::Value| -> Vec<String> {
            v["items"]
                .as_array()
                .unwrap()
                .iter()
                .map(|r| r["number"].as_str().unwrap().to_owned())
                .collect()
        };
        // A window: `total` is the matches, `count` the rows returned.
        let w = list(&root, json!({"sort": "number", "offset": 1, "limit": 2}));
        assert_eq!(numbers(&w), ["L-4", "N-2"]);
        assert_eq!(
            (w["total"].clone(), w["count"].clone(), w["offset"].clone()),
            (json!(4), json!(2), json!(1))
        );
        // Past the end: no rows, the same total.
        let past = list(&root, json!({"sort": "number", "offset": 9}));
        assert_eq!(
            (past["total"].clone(), past["count"].clone()),
            (json!(4), json!(0))
        );
        // Filters narrow the whole before the window is cut.
        let f = list(&root, json!({"class": "fastener", "sort": "number"}));
        assert_eq!(numbers(&f), ["N-2", "S-3"]);
        assert_eq!(numbers(&list(&root, json!({"make_buy": "make"}))), ["B-1"]);
        assert_eq!(list(&root, json!({"state": "released"}))["total"], 0);
        assert_eq!(list(&root, json!({"state": "in_work"}))["total"], 4);
        assert_eq!(list(&root, json!({"faults": "errors"}))["total"], 0);
        // Text: number, name, description, and a number the item had.
        assert_eq!(numbers(&list(&root, json!({"text": "LABEL"}))), ["L-4"]);
        ask(
            &root,
            Request::ItemRenumber {
                item: nut.clone(),
                number: Some("N-200".into()),
                next: None,
                expected_revision: None,
            },
        )
        .unwrap();
        assert_eq!(numbers(&list(&root, json!({"text": "n-2"}))), ["N-200"]);
        // By number: the current holder first, then one that had it.
        let by = list(&root, json!({"number": "N-2"}));
        assert_eq!(by["items"][0]["id"], json!(nut));
        // What it refuses.
        for bad in [json!({"sort": "colour"}), json!({"faults": "many"})] {
            let e = ask(&root, Request::from_rpc("item.list", Some(bad)).unwrap()).unwrap_err();
            assert_eq!(e.code, crate::error::Code::InvalidParam);
        }
    }

    #[test]
    fn a_shown_item_says_its_schema() {
        let (_d, root) = catalogue();
        ask(
            &root,
            Request::ItemCreate {
                fields: Default::default(),
                number: Some("A-1".into()),
                next: None,
                expected_revision: None,
            },
        )
        .unwrap();
        let v = ask(&root, Request::ItemShow { item: "A-1".into() }).unwrap();
        assert_eq!(v["schema"], 1);
        // A created or edited item's answer is the same shape.
        let e = ask(
            &root,
            Request::ItemEdit {
                item: "A-1".into(),
                fields: fields(json!({"name": "n"})),
                unset: vec![],
                expected_revision: None,
            },
        )
        .unwrap();
        assert_eq!(e["schema"], 1);
    }

    #[test]
    fn an_item_from_create_to_delete() {
        let (_d, root) = catalogue();
        let made = ask(
            &root,
            Request::ItemCreate {
                fields: fields(json!({"name": "Screw", "unit": "each", "make_buy": "buy",
                "aml": [{"manufacturer": "Acme", "mpn": "S-1", "status": "active"}]})),
                number: Some("100-0001".into()),
                next: None,
                expected_revision: None,
            },
        )
        .unwrap();
        let id = made["item"]["id"].as_str().unwrap().to_owned();
        assert_eq!(id.len(), 13);
        assert!(root.join(format!("items/{}/{id}.json", &id[..2])).is_file());
        let shown = ask(
            &root,
            Request::ItemShow {
                item: "100-0001".into(),
            },
        )
        .unwrap();
        assert_eq!(shown["item"]["name"], "Screw");
        assert_eq!(shown["state"]["state"], "in_work");
        let rev = shown["revision"].as_str().unwrap().to_owned();
        // A stale revision is refused; the right one edits.
        let stale = ask(
            &root,
            Request::ItemEdit {
                item: id.clone(),
                fields: fields(json!({"description": "x"})),
                unset: vec![],
                expected_revision: Some("0".repeat(32)),
            },
        )
        .unwrap_err();
        assert_eq!(stale.code, crate::error::Code::Conflict);
        let edited = ask(
            &root,
            Request::ItemEdit {
                item: id.clone(),
                fields: fields(json!({"description": "M3 x 8", "properties": {}})),
                unset: vec!["make_buy".into()],
                expected_revision: Some(rev),
            },
        )
        .unwrap();
        assert_eq!(edited["item"]["description"], "M3 x 8");
        assert!(edited["item"].get("make_buy").is_none());
        // A second item may not take the first one's number.
        let dup = ask(
            &root,
            Request::ItemCreate {
                fields: fields(json!({})),
                number: Some("100-0001".into()),
                next: None,
                expected_revision: None,
            },
        )
        .unwrap_err();
        assert_eq!(dup.code, crate::error::Code::PartNumberDuplicate);
        let renumbered = ask(
            &root,
            Request::ItemRenumber {
                item: id.clone(),
                number: Some("200-0001".into()),
                next: None,
                expected_revision: None,
            },
        )
        .unwrap();
        assert_eq!(renumbered["item"]["previous_numbers"], json!(["100-0001"]));
        let listed = ask(
            &root,
            Request::ItemList {
                number: Some("100-0001".into()),
                class: None,
                text: None,
                state: None,
                make_buy: None,
                faults: None,
                sort: None,
                offset: None,
                limit: None,
            },
        )
        .unwrap();
        assert_eq!(listed["items"][0]["id"], id);
        ask(
            &root,
            Request::ItemDelete {
                item: id.clone(),
                expected_revision: None,
            },
        )
        .unwrap();
        assert!(!root.join("items").exists(), "the empty shard is removed");
        let gone = ask(&root, Request::ItemShow { item: id }).unwrap_err();
        assert_eq!(gone.code, crate::error::Code::ItemNotFound);
    }

    #[test]
    fn next_numbers_come_from_the_scheme() {
        let (_d, root) = catalogue();
        ask(
            &root,
            Request::SettingsEdit {
                change: serde_json::from_value(json!({
            "scheme": "mech", "pattern": "MECH-\\d{5}", "next_template": "MECH-{seq:5}"}))
                .unwrap(),
                expected_revision: None,
            },
        )
        .unwrap();
        for want in ["MECH-00001", "MECH-00002"] {
            let v = ask(
                &root,
                Request::ItemCreate {
                    fields: fields(json!({})),
                    number: None,
                    next: Some(String::new()),
                    expected_revision: None,
                },
            )
            .unwrap();
            assert_eq!(v["item"]["number"], want);
        }
        let s = ask(&root, Request::SettingsShow {}).unwrap();
        assert_eq!(s["ryupdm"]["part_number"]["schemes"][0]["name"], "mech");
        assert_eq!(s["ryupdm"]["format"], 5);
    }

    #[test]
    fn number_local_says_the_scheme_its_floor_and_the_ledger() {
        let (_d, root) = catalogue();
        ask(
            &root,
            Request::SettingsEdit {
                change: serde_json::from_value(json!({
            "scheme": "mech", "pattern": "MECH-\\d{5}", "next_template": "MECH-{seq:5}"}))
                .unwrap(),
                expected_revision: None,
            },
        )
        .unwrap();
        ask(
            &root,
            Request::ItemCreate {
                fields: fields(json!({})),
                number: Some("MECH-00041".into()),
                next: None,
                expected_revision: None,
            },
        )
        .unwrap();
        let local = |next: Option<&str>, item: Option<&str>| {
            ask(
                &root,
                Request::NumberLocal {
                    next: next.map(str::to_owned),
                    class: None,
                    item: item.map(str::to_owned),
                },
            )
            .unwrap()
        };
        let v = local(Some(""), None);
        assert_eq!(
            (
                &v["scheme"],
                &v["template"],
                &v["floor"],
                &v["next"],
                &v["ledger"]
            ),
            (
                &json!("mech"),
                &json!("MECH-{seq:5}"),
                &json!(41),
                &json!("MECH-00042"),
                &json!(false)
            )
        );
        assert_eq!(v["catalogue"].as_str().unwrap().len(), 32);
        let plain = local(None, None);
        assert!(plain.get("template").is_none());
        assert_eq!(plain["revision"], v["revision"]);
        assert_eq!(local(Some(""), Some("MECH-00041"))["next"], "MECH-00042");
    }

    #[test]
    fn the_ledger_is_turned_on_and_off_in_ryupdm_json() {
        let (_d, root) = catalogue();
        let edit = |change: serde_json::Value| {
            ask(
                &root,
                Request::SettingsEdit {
                    change: serde_json::from_value(change).unwrap(),
                    expected_revision: None,
                },
            )
        };
        let on = edit(json!({"ledger": true})).unwrap();
        assert_eq!(
            (
                &on["ryupdm"]["part_number"]["ledger"],
                &on["ryupdm"]["format"]
            ),
            (&json!(true), &json!(6))
        );
        let local = ask(
            &root,
            Request::NumberLocal {
                next: None,
                class: None,
                item: None,
            },
        )
        .unwrap();
        assert_eq!(local["ledger"], true);
        let off = edit(json!({"ledger": false})).unwrap();
        assert_eq!(
            (off["ryupdm"].get("part_number"), &off["ryupdm"]["format"]),
            (None, &json!(1))
        );
        let both = edit(json!({"ledger": true, "class": "x"})).unwrap_err();
        assert_eq!(both.code, crate::error::Code::Usage);
    }
}
