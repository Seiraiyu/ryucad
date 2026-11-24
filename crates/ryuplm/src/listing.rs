//! The item list a session keeps (design 938): each item's row with its
//! lifecycle state and its own faults counted, and each sort's order,
//! built once per catalogue generation and release-branch refs. A window
//! of `item list` over 100,000 items is then one pass over rows in
//! memory, never a judge of the catalogue per request.

use crate::Plm;
use crate::id::Id;
use crate::session::Session;
use std::collections::BTreeMap;
use std::sync::Arc;

/// One item as the list shows it.
#[derive(Clone, Debug)]
pub struct Row {
    pub id: Id,
    pub number: String,
    pub name: Option<String>,
    pub class: Option<String>,
    /// `each` unless the item says otherwise.
    pub unit: String,
    pub make_buy: Option<String>,
    /// ryupdm's state name: `in_work`, `released`, `modified`,
    /// `obsolete`, `unknown`.
    pub state: String,
    /// The label that state is about.
    pub label: Option<String>,
    /// The catalogue's own files' faults naming this item.
    pub errors: u32,
    pub warnings: u32,
    /// The import mappings its provenance names (`item.source`'s keys).
    pub sources: Vec<String>,
    /// What `--text` searches, lower-cased: the number, previous numbers,
    /// name and description, each on its own line.
    pub hay: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Sort {
    Id,
    Number,
    Name,
    Class,
}

impl Sort {
    pub const NAMES: [&'static str; 4] = ["id", "number", "name", "class"];

    pub fn parse(s: &str) -> Option<Sort> {
        Some(match s {
            "id" => Sort::Id,
            "number" => Sort::Number,
            "name" => Sort::Name,
            "class" => Sort::Class,
            _ => return None,
        })
    }
}

/// What a listing was built for: the session's generation and the
/// release branch's refs key (empty outside a repository).
pub type Key = (u64, String);

pub struct Listing {
    key: Key,
    /// Counts builds, so a test can tell one listing from the next.
    built: u64,
    rows: Vec<Row>,
    orders: BTreeMap<Sort, Arc<Vec<u32>>>,
}

impl Listing {
    /// Every item's row, in ID order.
    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    pub fn built(&self) -> u64 {
        self.built
    }

    /// Row indices in `sort`'s order: by its key, then by ID. A key an
    /// item lacks sorts after every key. Text compares without case, then
    /// as written, so the order is total and the same everywhere.
    pub fn order(&mut self, sort: Sort) -> Arc<Vec<u32>> {
        if let Some(o) = self.orders.get(&sort) {
            return Arc::clone(o);
        }
        let mut ix: Vec<u32> = (0..self.rows.len() as u32).collect();
        let key = |r: &Row| -> (bool, String, String) {
            let k: Option<&str> = match sort {
                Sort::Id => Some(""),
                Sort::Number => Some(&r.number),
                Sort::Name => r.name.as_deref(),
                Sort::Class => r.class.as_deref(),
            };
            match k {
                Some(s) => (false, s.to_lowercase(), s.to_owned()),
                None => (true, String::new(), String::new()),
            }
        };
        if sort != Sort::Id {
            let keys: Vec<(bool, String, String)> = self.rows.iter().map(key).collect();
            // Stable: rows are in ID order, so equal keys stay in ID order.
            ix.sort_by(|a, b| keys[*a as usize].cmp(&keys[*b as usize]));
        }
        let o = Arc::new(ix);
        self.orders.insert(sort, Arc::clone(&o));
        o
    }
}

fn build(session: &mut Session, key: Key, built: u64) -> Listing {
    let releases = crate::release::releases_or_none(session);
    let files = session.files();
    let states = ryupdm::state::derive_all_for::<Plm>(&*files, &releases);
    let mut counts: BTreeMap<String, (u32, u32)> = BTreeMap::new();
    for f in crate::judge::all(&*files, session.catalogue(), &Default::default()) {
        if let Some(i) = f.error.details["item"].as_str() {
            let c = counts.entry(i.to_owned()).or_default();
            if f.is_error() {
                c.0 += 1;
            } else {
                c.1 += 1;
            }
        }
    }
    let rows = session
        .catalogue()
        .items
        .iter()
        .map(|(id, i)| {
            let state = states.get(id).map(|s| s.to_json());
            let (errors, warnings) = counts.get(id.as_str()).copied().unwrap_or((0, 0));
            let mut hay = i.number.to_lowercase();
            for s in i
                .previous_numbers
                .iter()
                .chain(i.name.iter())
                .chain(i.description.iter())
            {
                hay.push('\n');
                hay.push_str(&s.to_lowercase());
            }
            Row {
                id: id.clone(),
                number: i.number.clone(),
                name: i.name.clone(),
                class: i.class.clone(),
                unit: i.unit().to_owned(),
                make_buy: i.make_buy.clone(),
                state: state
                    .as_ref()
                    .and_then(|s| s["state"].as_str())
                    .unwrap_or("in_work")
                    .to_owned(),
                label: state
                    .as_ref()
                    .and_then(|s| s["label"].as_str())
                    .map(str::to_owned),
                errors,
                warnings,
                sources: i.source.keys().cloned().collect(),
                hay,
            }
        })
        .collect();
    Listing {
        key,
        built,
        rows,
        orders: BTreeMap::new(),
    }
}

/// The session's listing, taken out of it: the one it holds when the
/// catalogue's generation and the release refs haven't moved, else built
/// now. Give it back with [`put`] so the next request finds it.
pub fn of(session: &mut Session) -> Listing {
    // Reading the release branch fills `session.pdm` with its refs key.
    let _ = crate::release::releases_or_none(session);
    let refs = session
        .pdm
        .as_ref()
        .map(|(k, _)| k.clone())
        .unwrap_or_default();
    let key = (session.generation(), refs);
    let (held, builds) = session.take_listing();
    match held {
        Some(l) if l.key == key => l,
        _ => build(session, key, builds + 1),
    }
}

pub fn put(session: &mut Session, listing: Listing) {
    session.keep_listing(listing);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::Request;
    use crate::server::tests::{ask, catalogue};
    use crate::session::Session;
    use serde_json::json;

    fn make(root: &std::path::Path, number: &str, fields: serde_json::Value) -> String {
        ask(
            root,
            Request::ItemCreate {
                fields: serde_json::from_value(fields).unwrap(),
                number: Some(number.into()),
                next: None,
                expected_revision: None,
            },
        )
        .unwrap()["item"]["id"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    #[test]
    fn rows_are_built_once_per_generation() {
        let (_d, root) = catalogue();
        make(&root, "B-2", json!({"name": "Washer", "make_buy": "buy"}));
        make(&root, "A-1", json!({"name": "Bracket", "unit": "kg"}));
        let mut s = Session::open(&root).unwrap();
        let g = s.generation();
        let l = of(&mut s);
        assert_eq!(l.rows().len(), 2);
        // Rows are in ID order, whatever the numbers.
        let ids: Vec<&str> = l.rows().iter().map(|r| r.id.as_str()).collect();
        let mut sorted = ids.clone();
        sorted.sort();
        assert_eq!(ids, sorted);
        let a = l.rows().iter().find(|r| r.number == "A-1").unwrap();
        assert_eq!((a.unit.as_str(), a.state.as_str()), ("kg", "in_work"));
        assert_eq!((a.errors, a.warnings), (0, 0));
        let built = l.built();
        put(&mut s, l);
        // Asked again with nothing changed: the same listing.
        let again = of(&mut s);
        assert_eq!(again.built(), built);
        put(&mut s, again);
        // A file moves: the generation moves and the rows are built again.
        make(&root, "C-3", json!({}));
        s.reload().unwrap();
        assert!(s.generation() > g);
        let l = of(&mut s);
        assert_eq!(l.rows().len(), 3);
        assert_ne!(l.built(), built);
    }

    #[test]
    fn each_order_is_by_its_key_then_by_id() {
        let (_d, root) = catalogue();
        make(&root, "B-2", json!({"name": "alpha", "class": null}));
        make(&root, "A-1", json!({}));
        make(&root, "C-3", json!({"name": "Alpha"}));
        let mut s = Session::open(&root).unwrap();
        let mut l = of(&mut s);
        let numbers = |l: &mut Listing, sort| -> Vec<String> {
            let order = l.order(sort);
            order
                .iter()
                .map(|&k| l.rows()[k as usize].number.clone())
                .collect()
        };
        assert_eq!(numbers(&mut l, Sort::Number), ["A-1", "B-2", "C-3"]);
        // Names compare without case; an item with no name comes last.
        let by_name = numbers(&mut l, Sort::Name);
        assert_eq!(by_name[2], "A-1");
        assert_eq!(Sort::parse("number"), Some(Sort::Number));
        assert_eq!(Sort::parse("nope"), None);
    }
}
