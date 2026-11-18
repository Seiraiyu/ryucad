//! The merge driver: canonical JSON merged field by field (ryu-git's
//! `value::Walk`), an item's lists entry by entry, records whole, and
//! anything unreadable merged as text with markers.

pub use ryu_git::value::Outcome;
use ryu_git::value::{self as value, Merged, Rules, Walk};
use serde_json::{Map, Value};

/// `previous_numbers` is one value: two sides renumbering is a conflict
/// on `number`, not a merged list.
fn atom(at: &str) -> bool {
    at == "/previous_numbers"
}

/// An entry's key in `list`, as `keyed` makes it.
fn key_of(list: &str, e: &Value) -> String {
    match list {
        "aml" => format!(
            "{}/{}",
            ryu_git::value::escape(e["manufacturer"].as_str().unwrap_or("")),
            ryu_git::value::escape(e["mpn"].as_str().unwrap_or(""))
        ),
        "links" => e["product"].as_str().unwrap_or("").to_owned(),
        _ => e["line"].as_str().unwrap_or("").to_owned(),
    }
}

/// The keys of `list` in the side's own array order.
fn order(side: &Value, list: &str) -> Vec<String> {
    side.get(list)
        .and_then(Value::as_array)
        .map(|a| a.iter().map(|e| key_of(list, e)).collect())
        .unwrap_or_default()
}

/// The lists `keyed` made, back as arrays: ours' order, then entries only
/// theirs has in theirs' order (a keyed object's keys come back sorted).
fn unkeyed(v: &mut Value, ours: &Value, theirs: &Value) {
    let Some(o) = v.as_object_mut() else { return };
    for (list, key) in [
        ("aml", None),
        ("links", Some("product")),
        ("bom", Some("line")),
    ] {
        let Some(Value::Object(m)) = o.remove(list) else {
            continue;
        };
        let mut keys = order(ours, list);
        for k in order(theirs, list).into_iter().chain(m.keys().cloned()) {
            if !keys.contains(&k) {
                keys.push(k);
            }
        }
        let mut out = Vec::new();
        for k in keys {
            let Some(mut e) = m.get(&k).cloned() else {
                continue;
            };
            if let Some(x) = e.as_object_mut() {
                match key {
                    Some(name) => {
                        x.insert(name.to_owned(), Value::from(k.clone()));
                    }
                    None => {
                        if let Some((mf, mpn)) = k.split_once('/') {
                            x.insert(
                                "manufacturer".into(),
                                Value::from(ryu_git::value::unescape(mf)),
                            );
                            x.insert("mpn".into(), Value::from(ryu_git::value::unescape(mpn)));
                        }
                    }
                }
            }
            out.push(e);
        }
        if !out.is_empty() {
            o.insert(list.to_owned(), Value::Array(out));
        }
    }
    // Revisions back as arrays (design 935): ours' order, then theirs'
    // new ones, each its label again.
    let labels = |side: &Value, name: &str| -> Vec<String> {
        side.get("source")
            .and_then(|s| s.get(name))
            .and_then(|p| p.get("revisions"))
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|e| e["label"].as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default()
    };
    let Some(Value::Object(src)) = o.get_mut("source") else {
        return;
    };
    for (name, p) in src.iter_mut() {
        let Some(po) = p.as_object_mut() else {
            continue;
        };
        let Some(Value::Object(m)) = po.remove("revisions") else {
            continue;
        };
        let mut keys = labels(ours, name);
        for k in labels(theirs, name).into_iter().chain(m.keys().cloned()) {
            if !keys.contains(&k) {
                keys.push(k);
            }
        }
        let mut out = Vec::new();
        for k in keys {
            let Some(mut e) = m.get(&k).cloned() else {
                continue;
            };
            if let Some(x) = e.as_object_mut() {
                x.insert("label".into(), Value::from(k.clone()));
            }
            out.push(e);
        }
        if !out.is_empty() {
            po.insert("revisions".into(), Value::Array(out));
        }
    }
}

pub fn merge(base: Option<&Value>, ours: &Value, theirs: &Value) -> Merged {
    let kind = ours["kind"].as_str().unwrap_or("").to_owned();
    let mut walk = Walk::new(Rules {
        atom,
        ..Rules::default()
    });
    let prep = |v: &Value| {
        let mut v = v.clone();
        if kind == crate::item::KIND {
            crate::textconv::keyed(&mut v);
        }
        v
    };
    let (b, o, t) = (base.map(prep), prep(ours), prep(theirs));
    let mut value = match &b {
        None => walk.whole(None, &o, &t),
        Some(b) if ryupdm::git::whole(&kind) => walk.whole(Some(b), &o, &t),
        Some(b) => walk
            .value("", Some(b), Some(&o), Some(&t))
            .unwrap_or(Value::Object(Map::new())),
    };
    if kind == crate::item::KIND {
        unkeyed(&mut value, ours, theirs);
    }
    Merged {
        value,
        conflicts: walk.conflicts,
    }
}

/// The newest format this build reads of each kind it merges by value.
fn newest(kind: &str) -> Option<u64> {
    Some(match kind {
        crate::item::KIND => crate::item::FORMAT,
        crate::catalogue::KIND => crate::catalogue::FORMAT,
        ryupdm::config::KIND => ryupdm::config::FORMAT,
        _ => return None,
    })
}

/// A file's JSON if ryuplm can merge it by value: UTF-8 JSON, an object
/// with a `kind` and a `format`, no conflict record, no format newer than
/// this build reads. It needn't be canonical: the merge compares values
/// and writes the result canonical.
fn readable(bytes: &[u8]) -> Option<Value> {
    let v = crate::canonical::parse(std::str::from_utf8(bytes).ok()?).ok()?;
    if v.get(value::CONFLICTS).is_some() {
        return None;
    }
    let kind = v.get("kind")?.as_str()?;
    let format = v.get("format")?.as_u64()?;
    if newest(kind).is_some_and(|n| format > n) {
        return None;
    }
    Some(v)
}

/// Merges three versions of a file as Git hands them to the driver; an
/// empty base means both sides added it. Files ryuplm reads merge by
/// value and come out canonical: by [`merge`] when all are of one kind,
/// as one value when a side changed the kind,  If any version can't be read they merge as lines, as `git
/// merge-file` does.
pub fn bytes(base: &[u8], ours: &[u8], theirs: &[u8]) -> (Vec<u8>, Outcome) {
    let b = if base.is_empty() {
        Some(None)
    } else {
        readable(base).map(Some)
    };
    if let (Some(b), Some(o), Some(t)) = (b, readable(ours), readable(theirs)) {
        let kinds = [Some(&o), Some(&t), b.as_ref()].map(|v| v.map(|v| v["kind"].clone()));
        let one_kind = kinds.iter().flatten().all(|k| *k == o["kind"]);
        let m = if !one_kind {
            let mut walk = Walk::default();
            let value = walk.whole(b.as_ref(), &o, &t);
            Merged {
                value,
                conflicts: walk.conflicts,
            }
        } else {
            merge(b.as_ref(), &o, &t)
        };
        let outcome = match m.conflicts.len() {
            0 => Outcome::Clean,
            n => Outcome::Conflicts(n),
        };
        return (
            crate::canonical::to_string(&m.to_json()).into_bytes(),
            outcome,
        );
    }
    let (out, n) = value::text(base, ours, theirs);
    (out, Outcome::Text(n))
}

/// The catalogue's files a merge left unresolved, in path order: a
/// conflict record (`conflicts`) the driver wrote, or Git's markers where
/// it merged lines.
pub fn unresolved(root: &std::path::Path) -> Vec<String> {
    crate::session::tracked(root)
        .into_iter()
        .filter(|(rel, _)| {
            let Ok(bytes) = std::fs::read(crate::session::path_of(root, rel)) else {
                return false;
            };
            match std::str::from_utf8(&bytes)
                .ok()
                .and_then(|t| crate::canonical::parse(t).ok())
            {
                Some(v) => v.get(value::CONFLICTS).is_some(),
                None => bytes
                    .split(|b| *b == b'\n')
                    .any(|l| l.starts_with(b"<<<<<<< ") || l == b"<<<<<<<"),
            }
        })
        .map(|(rel, _)| rel)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn text(v: serde_json::Value) -> Vec<u8> {
        crate::canonical::to_string(&v).into_bytes()
    }

    fn base() -> serde_json::Value {
        json!({"format": 1, "kind": "ryuplm.item", "number": "A-1", "name": "Screw",
            "aml": [{"manufacturer": "Acme", "mpn": "S-1", "status": "active"}],
            "bom": [{"line": "0000000000001", "item": "k3v9q2m8x4c1p", "quantity": "1"}]})
    }

    #[test]
    fn items_merge_field_by_field_and_lists_by_key() {
        let mut ours = base();
        ours["name"] = json!("Screw, M3");
        ours["aml"]
            .as_array_mut()
            .unwrap()
            .push(json!({"manufacturer": "Bolt", "mpn": "B-3", "status": "nrnd"}));
        let mut theirs = base();
        theirs["description"] = json!("pan head");
        theirs["bom"][0]["quantity"] = json!("2");
        theirs["bom"]
            .as_array_mut()
            .unwrap()
            .push(json!({"line": "0000000000002", "item": "0000000000009", "quantity": "4"}));
        let (out, outcome) = bytes(&text(base()), &text(ours), &text(theirs));
        assert_eq!(outcome, Outcome::Clean);
        let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(v["name"], "Screw, M3");
        assert_eq!(v["description"], "pan head");
        assert_eq!(v["aml"].as_array().unwrap().len(), 2);
        assert_eq!(v["bom"][0]["quantity"], "2");
        assert_eq!(v["bom"][1]["line"], "0000000000002");
        assert!(crate::canonical::is_canonical(
            std::str::from_utf8(&out).unwrap()
        ));
    }

    #[test]
    fn conflicted_files_are_unresolved() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        crate::server::init(root, None).unwrap();
        std::fs::create_dir_all(root.join("items/00")).unwrap();
        let mut v = base();
        v["conflicts"] = json!({"/number": {"ours": "A-2", "theirs": "A-3"}});
        std::fs::write(root.join("items/00/0000000000001.json"), text(v)).unwrap();
        std::fs::write(
            root.join("items/00/0000000000002.json"),
            "<<<<<<< ours\n{}\n=======\n{}\n>>>>>>> theirs\n",
        )
        .unwrap();
        std::fs::write(root.join("items/00/0000000000003.json"), text(base())).unwrap();
        assert_eq!(
            unresolved(root),
            ["items/00/0000000000001.json", "items/00/0000000000002.json"]
        );
    }

    #[test]
    fn one_field_two_ways_is_a_conflict() {
        let mut ours = base();
        ours["number"] = json!("A-2");
        let mut theirs = base();
        theirs["number"] = json!("A-3");
        let (out, outcome) = bytes(&text(base()), &text(ours), &text(theirs));
        assert_eq!(outcome, Outcome::Conflicts(1));
        let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(v["conflicts"]["/number"]["ours"], "A-2");
        assert_eq!(v["conflicts"]["/number"]["theirs"], "A-3");
    }

    #[test]
    fn revisions_merge_by_label() {
        let item = |revs: serde_json::Value| {
            json!({"format": 1, "kind": "ryuplm.item", "number": "A-1",
                "source": {"hist": {"key": "A-1", "digest": "ff", "values": {}, "revisions": revs}}})
        };
        let base = item(json!([{"label": "A", "values": {}}]));
        let ours = item(json!([{"label": "A", "state": "Released", "values": {}}]));
        let theirs = item(json!([{"label": "A", "values": {}}, {"label": "B", "values": {}}]));
        let (out, outcome) = bytes(&text(base), &text(ours), &text(theirs));
        assert_eq!(outcome, Outcome::Clean);
        let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(
            v["source"]["hist"]["revisions"],
            json!([{"label": "A", "state": "Released", "values": {}}, {"label": "B", "values": {}}])
        );
    }
}
