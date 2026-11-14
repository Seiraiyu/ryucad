//! `diff`'s `pdm` block: the settings, and each part's properties,
//! records and state, from one side to the other (design 629), as
//! ryucad's `diff` gives them.

use super::{NAME, PDM_CONFIG, PROPERTIES, RELEASES};
use ryu_session::fresh::Files;
use ryupdm::config::Config;
use ryupdm::history::Releases;
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};

fn json_at(files: &Files, path: &str) -> Option<Value> {
    crate::canonical::parse(std::str::from_utf8(files.get(path)?).ok()?).ok()
}

/// What changed between `from`'s and `to`'s ryupdm, `None` when nothing
/// did: `changes` (each settings key that differs, `{key, from, to}`, the
/// defaults filled in) and `parts` (each part's properties changed, with
/// whether they are revising, records added, removed or changed, and its
/// state on each side when they differ, read against `releases`).
pub fn pdm(from: &Files, to: &Files, releases: &Releases) -> Option<Value> {
    let settings = |f: &Files| match Config::read_in(f, NAME) {
        Ok(c) => Some(c.to_json()),
        Err(_) => json_at(f, PDM_CONFIG),
    };
    let (sa, sb) = (
        settings(from).unwrap_or_default(),
        settings(to).unwrap_or_default(),
    );
    let keys: BTreeSet<&String> = sa
        .as_object()
        .into_iter()
        .flat_map(|o| o.keys())
        .chain(sb.as_object().into_iter().flat_map(|o| o.keys()))
        .collect();
    let changes: Vec<Value> = keys
        .into_iter()
        .filter(|k| sa.get(k.as_str()) != sb.get(k.as_str()))
        .map(|k| json!({"key": k, "from": sa.get(k.as_str()), "to": sb.get(k.as_str())}))
        .collect();
    let config = Config::read_in(to, NAME)
        .or_else(|_| Config::read_in(from, NAME))
        .unwrap_or_default();
    let properties = format!("{PROPERTIES}/");
    let records = format!("{RELEASES}/");
    let parts: BTreeSet<String> = [from, to]
        .iter()
        .flat_map(|f| f.keys())
        .filter_map(|p| {
            let part = match p.strip_prefix(&properties) {
                Some(r) => r.strip_suffix(".json")?,
                None => p.strip_prefix(&records)?.split('/').next()?,
            };
            Some(part.to_owned())
        })
        .collect();
    let mut out = Map::new();
    for part in parts {
        if let Some(v) = part_pdm(from, to, &part, &config, releases) {
            out.insert(part, v);
        }
    }
    if changes.is_empty() && out.is_empty() {
        return None;
    }
    Some(json!({"changes": changes, "parts": out}))
}

/// What changed in `part`'s PDM from `from` to `to`, `None` when nothing
/// did: each property changed (`from` and `to` as written, absent sides
/// left out, and whether the settings make it revising), each record
/// `added`, `removed` or `changed` (`{label, kind}`), and the part's
/// release state on each side when they differ.
fn part_pdm(
    from: &Files,
    to: &Files,
    part: &str,
    config: &Config,
    releases: &Releases,
) -> Option<Value> {
    let path = format!("{PROPERTIES}/{part}.json");
    let props = |files: &Files| -> Map<String, Value> {
        let mut o = json_at(files, &path)
            .and_then(|v| v.as_object().cloned())
            .unwrap_or_default();
        o.remove("format");
        o.remove("kind");
        o
    };
    let (pa, pb) = (props(from), props(to));
    let mut properties = Map::new();
    for k in pa.keys().chain(pb.keys()).collect::<BTreeSet<_>>() {
        let (a, b) = (pa.get(k), pb.get(k));
        if a == b {
            continue;
        }
        let mut c = json!({"revising": config.decl(k).is_some_and(|d| d.revising)});
        if let Some(a) = a {
            c["from"] = a.clone();
        }
        if let Some(b) = b {
            c["to"] = b.clone();
        }
        properties.insert(k.clone(), c);
    }
    let records = |files: &Files| -> BTreeMap<String, (Value, Vec<u8>)> {
        ryupdm::record::Records::read_in(files, NAME)
            .unwrap_or_default()
            .entries
            .into_iter()
            .filter(|e| e.part.as_str() == part)
            .map(|e| {
                let what = json!({"label": e.label, "kind": e.kind.as_str()});
                (e.path, (what, e.bytes))
            })
            .collect()
    };
    let (ra, rb) = (records(from), records(to));
    let (mut added, mut removed, mut changed) = (Vec::new(), Vec::new(), Vec::new());
    for path in ra.keys().chain(rb.keys()).collect::<BTreeSet<_>>() {
        match (ra.get(path), rb.get(path)) {
            (None, Some((w, _))) => added.push(w.clone()),
            (Some((w, _)), None) => removed.push(w.clone()),
            (Some((w, a)), Some((_, b))) if a != b => changed.push(w.clone()),
            _ => {}
        }
    }
    let state = crate::id::Id::parse(part, "part").ok().and_then(|id| {
        let a = ryupdm::state::derive_for::<crate::Eda>(from, &id, releases).text();
        let b = ryupdm::state::derive_for::<crate::Eda>(to, &id, releases).text();
        (a != b).then(|| json!({"from": a, "to": b}))
    });
    if properties.is_empty()
        && added.is_empty()
        && removed.is_empty()
        && changed.is_empty()
        && state.is_none()
    {
        return None;
    }
    let mut v = json!({
        "properties": properties,
        "records": {"added": added, "removed": removed, "changed": changed},
    });
    if let Some(s) = state {
        v["state"] = s;
    }
    Some(v)
}

/// Each pin that moved between `from`'s and `to`'s manifests (`working`
/// for none), `None` when none did.
pub fn pins(from: &Files, to: &Files) -> Option<Value> {
    let read = |f: &Files| -> BTreeMap<(String, String), String> {
        f.get(crate::project::MANIFEST)
            .and_then(|b| crate::project::Manifest::read(b).ok())
            .map(|m| {
                m.libraries
                    .iter()
                    .flat_map(|(a, x)| {
                        x.labels
                            .iter()
                            .map(move |(c, l)| ((a.to_string(), c.to_string()), l.clone()))
                    })
                    .collect()
            })
            .unwrap_or_default()
    };
    let (a, b) = (read(from), read(to));
    let keys: BTreeSet<&(String, String)> = a.keys().chain(b.keys()).collect();
    let moved: Vec<Value> = keys
        .into_iter()
        .filter(|k| a.get(*k) != b.get(*k))
        .map(|k| {
            json!({
                "alias": k.0,
                "component": k.1,
                "from": a.get(k).map_or("working", String::as_str),
                "to": b.get(k).map_or("working", String::as_str),
            })
        })
        .collect();
    (!moved.is_empty()).then(|| json!(moved))
}

/// The lock's labels `added`, `removed` and `changed` (`commit: {from,
/// to}`), each `{project, component, label, commit}`; `None` when the
/// lock didn't move. As ryucad's `deps` block.
pub fn lock(from: &Files, to: &Files) -> Option<Value> {
    type Key = (String, String, String);
    let read = |f: &Files| -> BTreeMap<Key, String> {
        f.get(crate::project::LOCK)
            .and_then(|b| crate::libdeps::read_lock(b).ok())
            .map(|l| {
                l.projects
                    .iter()
                    .flat_map(|(p, x)| {
                        x.labels.iter().map(move |((c, label), v)| {
                            (
                                (p.to_string(), c.to_string(), label.clone()),
                                v.commit.clone(),
                            )
                        })
                    })
                    .collect()
            })
            .unwrap_or_default()
    };
    let (a, b) = (read(from), read(to));
    let row = |(p, c, l): &Key, commit: Value| json!({"project": p, "component": c, "label": l, "commit": commit});
    let added: Vec<Value> = b
        .iter()
        .filter(|(k, _)| !a.contains_key(*k))
        .map(|(k, c)| row(k, json!(c)))
        .collect();
    let removed: Vec<Value> = a
        .iter()
        .filter(|(k, _)| !b.contains_key(*k))
        .map(|(k, c)| row(k, json!(c)))
        .collect();
    let changed: Vec<Value> = a
        .iter()
        .filter_map(|(k, c)| {
            let n = b.get(k)?;
            (n != c).then(|| row(k, json!({"from": c, "to": n})))
        })
        .collect();
    (!(added.is_empty() && removed.is_empty() && changed.is_empty()))
        .then(|| json!({"added": added, "removed": removed, "changed": changed}))
}

#[cfg(test)]
mod tests {

    #[test]
    fn moved_pins_and_locked_labels_diff_at_the_top() {
        let manifest = |label: &str| {
            let mut m = crate::project::Manifest::new(
                crate::project::tests::pid(1),
                crate::units::Unit::Mm,
                crate::project::ProjectKind::Design,
            );
            let mut a = crate::project::Alias::local("../lib");
            a.project = Some(crate::project::tests::pid(2));
            a.labels
                .insert(crate::id::Id::parse("r", "c").unwrap(), label.into());
            m.libraries
                .insert(crate::id::Id::parse("passives", "a").unwrap(), a);
            m.to_text()
        };
        let lock_of = |commit: char| {
            let mut l = ryu_doc::lock::Lock::default();
            l.projects
                .entry(crate::project::tests::pid(2))
                .or_default()
                .labels
                .insert(
                    (crate::id::Id::parse("r", "c").unwrap(), "A".into()),
                    ryu_doc::lock::LockedLabel {
                        commit: commit.to_string().repeat(40),
                        content: "c".repeat(64),
                    },
                );
            crate::libdeps::lock_text(&l).unwrap()
        };
        let files = |label: &str, commit: Option<char>| -> Files {
            let mut f: Files = [(
                "ryueda.json".to_owned(),
                manifest(label).into_bytes().into(),
            )]
            .into_iter()
            .collect();
            if let Some(c) = commit {
                f.insert("ryueda.lock".into(), lock_of(c).into_bytes().into());
            }
            f
        };
        let p = pins(&files("A", None), &files("B", None)).unwrap();
        assert_eq!(
            p,
            json!([{"alias": "passives", "component": "r", "from": "A", "to": "B"}])
        );
        assert_eq!(pins(&files("A", None), &files("A", None)), None);
        assert_eq!(lock(&files("A", Some('a')), &files("A", Some('a'))), None);
        let d = lock(&files("A", None), &files("A", Some('a'))).unwrap();
        assert_eq!(d["added"][0]["label"], "A");
        let d = lock(&files("A", Some('a')), &files("A", Some('b'))).unwrap();
        assert_eq!(d["changed"][0]["commit"]["to"], "b".repeat(40));
    }
    use super::*;
    use crate::pdm::tests::board;
    use crate::project::ProjectKind;

    fn files(f: BTreeMap<String, Vec<u8>>) -> Files {
        f.into_iter().map(|(p, b)| (p, b.into())).collect()
    }

    #[test]
    fn the_block_says_what_moved_in_settings_properties_and_records() {
        let a = board(ProjectKind::Design);
        assert_eq!(
            pdm(&files(a.clone()), &files(a.clone()), &Default::default()),
            None
        );
        let mut b = a.clone();
        b.insert(
            "ryupdm.json".into(),
            b"{\"format\": 4, \"kind\": \"ryupdm.config\", \"revisions\": \"numeric\"}\n".to_vec(),
        );
        b.insert(
            "properties/d.json".into(),
            b"{\"format\": 1, \"kind\": \"ryupdm.properties\", \"number\": \"1\"}\n".to_vec(),
        );
        let v = pdm(&files(a), &files(b), &Default::default()).unwrap();
        assert_eq!(v["changes"][0]["key"], "revisions", "{v}");
        assert_eq!(v["parts"]["d"]["properties"]["number"]["to"], "1", "{v}");
        assert_eq!(v["parts"]["d"]["properties"]["number"]["revising"], true);
    }
}
