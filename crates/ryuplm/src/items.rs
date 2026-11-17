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
fn text_of(id: &Id, item: &Item) -> Result<String> {
    let text = item.to_text();
    Item::read(id, text.as_bytes())?;
    Ok(text)
}

fn check_expected(what: &str, actual: &str, expected: Option<&str>) -> Result<()> {
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
            crate::numbers::next(session.catalogue(), &config, name, class)
        }
        _ => Err(Error::new(
            Code::Usage,
            "give a number or --next [<scheme>], not both",
        )),
    }
}

/// An item as answers show it: its ID, path, revision and file.
fn shown(session: &Session, id: &Id) -> Value {
    let mut item = session.catalogue().items[id].to_json();
    if let Some(o) = item.as_object_mut() {
        o.remove("format");
        o.remove("kind");
        o.insert("id".into(), json!(id));
    }
    json!({"item": item, "path": item_path(id), "revision": session.item_revision(id)})
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
    Ok(v)
}

pub fn list(
    session: &Session,
    number: Option<String>,
    class: Option<String>,
    text: Option<String>,
    limit: Option<usize>,
) -> Value {
    let c = session.catalogue();
    let ids: Vec<Id> = match &number {
        Some(n) => c.by_number(n),
        None => c.items.keys().cloned().collect(),
    };
    let needle = text.as_deref().map(str::to_lowercase);
    let rows: Vec<Value> = ids
        .into_iter()
        .filter(|id| {
            class
                .as_ref()
                .is_none_or(|k| c.items[id].class.as_ref() == Some(k))
        })
        .filter(|id| {
            needle.as_ref().is_none_or(|t| {
                let i = &c.items[id];
                [Some(&i.number), i.name.as_ref(), i.description.as_ref()]
                    .into_iter()
                    .flatten()
                    .any(|s| s.to_lowercase().contains(t))
            })
        })
        .take(limit.unwrap_or(usize::MAX))
        .map(|id| {
            let i = &c.items[&id];
            json!({"id": id, "number": i.number, "name": i.name, "class": i.class})
        })
        .collect();
    json!({"count": rows.len(), "items": rows})
}

/// The item `item` names, its revision checked against `expected`.
fn current(session: &Session, item: &str, expected: Option<&str>) -> Result<(Id, Item)> {
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
}
