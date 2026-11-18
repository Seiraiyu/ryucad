//! Legacy history (design 935): a legacy system's revisions of an item,
//! read one row per (item, revision) through a mapping's `history`
//! block, kept in the item's provenance as a record of what that system
//! said. They aren't releases: nothing derives a state or a content hash
//! from them.

use crate::error::{Code, Error, Result};
use crate::id::Id;
use crate::item::Item;
use crate::load::Catalogue;
use crate::source::Source;
use ryupdm::config::Config;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

/// One legacy revision as a history mapping read it.
#[derive(Clone, Debug, PartialEq)]
pub struct Revision {
    /// As written, trimmed.
    pub label: String,
    pub date: Option<String>,
    pub state: Option<String>,
    pub change: Option<String>,
    pub author: Option<String>,
    /// The mapping's fields read on this revision's row, by target.
    pub values: BTreeMap<String, Value>,
    /// The row's cells nothing maps.
    pub extra: BTreeMap<String, String>,
    /// The row it was read from.
    pub at: u64,
    /// Whether the mapping's `released` rule matched its row.
    pub released: bool,
}

impl Revision {
    /// `{label, date?, state?, change?, author?, values, extra?}`.
    pub fn to_json(&self) -> Value {
        let mut v = json!({"label": self.label, "values": self.values});
        for (k, x) in [
            ("date", &self.date),
            ("state", &self.state),
            ("change", &self.change),
            ("author", &self.author),
        ] {
            if let Some(x) = x {
                v[k] = json!(x);
            }
        }
        if !self.extra.is_empty() {
            v["extra"] = json!(self.extra);
        }
        v
    }
}

/// A key's revisions in order, and the carried one's label: the last
/// released in order, or none when the mapping's rule released none.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Legacy {
    pub revisions: Vec<Revision>,
    pub carried: Option<String>,
}

impl Legacy {
    pub fn json(&self) -> Vec<Value> {
        self.revisions.iter().map(Revision::to_json).collect()
    }
}

/// The items with release records: their folders under `releases/`
/// (design 935: an import never moves their carried label).
pub fn released(src: &dyn Source) -> Result<BTreeSet<Id>> {
    Ok(src
        .list(ryupdm::RELEASES)?
        .into_iter()
        .filter(|(_, k)| *k == crate::source::Kind::Dir)
        .filter_map(|(d, _)| Id::parse(&d, "item").ok())
        .collect())
}

/// The source's revisions in its order, each stored one it no longer has
/// kept after the stored revision before it: a revision gone from the
/// source stays (design 935).
pub fn merged(stored: &[Value], new: &[Value]) -> Vec<Value> {
    let label = |v: &Value| v["label"].as_str().unwrap_or_default().to_owned();
    let mut out: Vec<Value> = new.to_vec();
    let mut before: Option<String> = None;
    for s in stored {
        let l = label(s);
        if !out.iter().any(|x| label(x) == l) {
            let at = before
                .as_ref()
                .and_then(|b| out.iter().position(|x| &label(x) == b))
                .map_or(0, |i| i + 1);
            out.insert(at, s.clone());
        }
        before = Some(l);
    }
    out
}

/// The item's legacy revisions as `item.show` and `release.list` answer
/// them: each stored revision with its import's name and whether it is
/// the carried one.
pub fn legacy(item: &Item) -> Vec<Value> {
    let mut out = Vec::new();
    for (name, p) in &item.source {
        let carried = p.get("carried").and_then(Value::as_str);
        for r in p
            .get("revisions")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let mut v = r.clone();
            if let Some(o) = v.as_object_mut() {
                o.insert("import".into(), json!(name));
                o.insert(
                    "carried".into(),
                    json!(carried.is_some() && r["label"].as_str() == carried),
                );
            }
            out.push(v);
        }
    }
    out
}

/// The carried label of the history import `item` holds, if any.
pub fn carried(item: &Item) -> Option<&str> {
    item.source.values().find_map(|p| {
        p.get("revisions")?;
        p.get("carried")?.as_str()
    })
}

/// `item_history_label_foreign` for each item with no release yet whose
/// carried legacy label isn't one of the revision scheme's (design 935).
pub fn foreign(src: &dyn Source, cat: &Catalogue, config: &Config) -> Vec<Error> {
    let released = released(src).unwrap_or_default();
    let scheme = config.revisions;
    cat.items
        .iter()
        .filter(|(id, _)| !released.contains(*id))
        .filter_map(|(id, item)| {
            let c = carried(item)?;
            ryupdm::label::index(scheme, c).is_none().then(|| {
                Error::new(
                    Code::ItemHistoryLabelForeign,
                    format!(
                        "item {id}: its legacy label {c:?} isn't a {} label; its first release takes {}",
                        scheme.as_str(),
                        ryupdm::label::first(scheme)
                    ),
                )
                .with("item", id)
                .with("label", c)
                .with("scheme", scheme.as_str())
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::Request;
    use crate::server::tests::{ask, catalogue};

    #[test]
    fn item_show_and_release_list_answer_legacy_revisions() {
        let (_d, root) = catalogue();
        let made = ask(
            &root,
            Request::ItemCreate {
                fields: Default::default(),
                number: Some("A-1".into()),
                next: None,
                expected_revision: None,
            },
        )
        .unwrap();
        let id = Id::parse(made["item"]["id"].as_str().unwrap(), "item").unwrap();
        let path = root.join(crate::ids::item_path(&id));
        let mut v: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        v["source"] = json!({"hist": {"key": "A-1", "digest": "ff", "values": {}, "carried": "B",
            "revisions": [{"label": "A", "values": {}}, {"label": "B", "date": "2020-11-02", "values": {}}]}});
        std::fs::write(&path, crate::canonical::to_string(&v)).unwrap();
        let shown = ask(&root, Request::ItemShow { item: "A-1".into() }).unwrap();
        assert_eq!(
            shown["legacy"],
            json!([
                {"label": "A", "values": {}, "import": "hist", "carried": false},
                {"label": "B", "date": "2020-11-02", "values": {}, "import": "hist", "carried": true}
            ])
        );
        let listed = ask(
            &root,
            Request::ReleaseList {
                item: Some("A-1".into()),
            },
        )
        .unwrap();
        assert_eq!(listed["legacy"], shown["legacy"]);
        let all = ask(&root, Request::ReleaseList { item: None }).unwrap();
        assert!(all.get("legacy").is_none());
        let item = crate::item::Item::read(&id, &std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(carried(&item), Some("B"));
    }

    #[test]
    fn a_carried_label_is_continued_or_warned_of() {
        use ryupdm::config::Scheme;
        use ryupdm::product::Product;
        let item = |number: &str, carried: &str| {
            let mut i = Item::new(number);
            i.source.insert(
                "hist".into(),
                json!({"key": number, "digest": "ff", "values": {}, "carried": carried,
                       "revisions": [{"label": carried, "values": {}}]}),
            );
            i
        };
        let (a, b) = ("k3v9q2m8x4c1p", "k3v9q2m8x4c1q");
        let mut f =
            crate::judge::tests::files(&[], None, &[(a, item("A-1", "C")), (b, item("B-1", "X3"))]);
        let (ia, ib) = (Id::parse(a, "item").unwrap(), Id::parse(b, "item").unwrap());
        assert_eq!(
            <crate::Plm as Product>::first_label(&f, &ia, Scheme::Letters),
            "D"
        );
        assert_eq!(
            <crate::Plm as Product>::first_label(&f, &ib, Scheme::Letters),
            "A"
        );
        assert_eq!(
            <crate::Plm as Product>::first_label(&f, &ia, Scheme::Numeric),
            "1"
        );
        let warned = |f: &BTreeMap<String, Vec<u8>>| -> Vec<Value> {
            let cat = crate::load::Catalogue::load(f).unwrap();
            foreign(f, &cat, &ryupdm::config::Config::default())
                .into_iter()
                .map(|e| e.details["label"].clone())
                .collect()
        };
        assert_eq!(warned(&f), [json!("X3")]);
        assert!(
            crate::judge::tests::codes(&f)
                .iter()
                .any(|(code, _, _)| code == "item_history_label_foreign"),
            "{:?}",
            crate::judge::tests::codes(&f)
        );
        f.insert(format!("releases/{b}/A.json"), b"{}".to_vec());
        assert_eq!(
            warned(&f),
            Vec::<Value>::new(),
            "released: it no longer matters"
        );
    }
}
