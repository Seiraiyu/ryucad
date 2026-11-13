//! Reference paths (designs 518 and 519): what a selection, a highlight,
//! a status or `view.show` names, resolved against the project's blocks.
//! `<block>/<component>`, `<block>/<component>/<pin>`, `<block>/<net>`,
//! `<block>/rail:<id>`, `<block>/port:<id>`, `<block>/instance:<id>`,
//! `<block>/bus:<id>`, `<block>/region:<id>`, `<block>/note:<id>` and
//! `<block>/label:<net>@<component>/<pin>`: what `schem::ref_path`
//! writes from a layout key, and a pin besides.

use crate::design::BlockFiles;
use crate::error::{Code, Error, Result};
use crate::id::Id;
use crate::project::Project;
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// What a reference names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Component,
    Pin,
    Net,
    Rail,
    Port,
    Instance,
    Bus,
    Region,
    Note,
    Label,
}

/// A reference that names something: its block, what it names, and the
/// reference path of what draws it (a pin's component; else itself).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resolved {
    pub block: String,
    pub kind: Kind,
    pub owner: String,
}

fn has<T>(map: &BTreeMap<Id, T>, id: &str) -> bool {
    map.keys().any(|k| k.as_str() == id)
}

/// Whether `key` (a net's layout key: `<net>`, `rail:<id>`, `port:<id>`)
/// names a net, rail or port of block `b`.
fn net_in(p: &Project, b: &BlockFiles, key: &str) -> bool {
    if let Some(id) = key.strip_prefix("rail:") {
        return has(&p.conn.rails, id);
    }
    if let Some(id) = key.strip_prefix("port:") {
        return b.block.item.as_ref().is_ok_and(|x| has(&x.ports, id));
    }
    has(&b.nets, key)
}

/// What `rest` (a reference path after its block) names in block `b`.
fn kind_in(p: &Project, b: &BlockFiles, rest: &str) -> Option<Kind> {
    let block = b.block.item.as_ref().ok();
    let layout = b.layout.as_ref().and_then(|l| l.as_ref().ok());
    if let Some(l) = rest.strip_prefix("label:") {
        let (net, on) = l.rsplit_once('@')?;
        let (component, _) = on.rsplit_once('/')?;
        return (net_in(p, b, net) && has(&b.components, component)).then_some(Kind::Label);
    }
    if let Some((component, pin)) = rest.split_once('/') {
        let (_, c) = b.components.iter().find(|(k, _)| k.as_str() == component)?;
        let c = c.item.as_ref().ok()?;
        let drawn = c.pins.contains_key(pin)
            || crate::partcmd::placed(p, &c.of).is_ok_and(|(_, s)| s.pins.contains_key(pin));
        return drawn.then_some(Kind::Pin);
    }
    match rest.split_once(':') {
        Some(("rail", id)) => has(&p.conn.rails, id).then_some(Kind::Rail),
        Some(("port", id)) => block
            .is_some_and(|x| has(&x.ports, id))
            .then_some(Kind::Port),
        Some(("instance", id)) => has(&b.instances, id).then_some(Kind::Instance),
        Some(("bus", id)) => block
            .is_some_and(|x| has(&x.buses, id))
            .then_some(Kind::Bus),
        Some(("region", id)) => layout
            .is_some_and(|l| l.regions.contains_key(id))
            .then_some(Kind::Region),
        Some(("note", id)) => layout
            .is_some_and(|l| l.notes.contains_key(id))
            .then_some(Kind::Note),
        Some(_) => None,
        None if has(&b.components, rest) => Some(Kind::Component),
        None if has(&b.nets, rest) => Some(Kind::Net),
        None => None,
    }
}

/// What reference path `r` names, or `ref_unresolved` with the closest
/// names.
pub fn resolve(p: &Project, r: &str) -> Result<Resolved> {
    let unresolved = |why: String| {
        Error::new(Code::RefUnresolved, why)
            .with("ref", r)
            .with("closest", closest(p, r))
    };
    let Some((block, rest)) = r.split_once('/') else {
        return Err(unresolved(format!(
            "{r} isn't a reference path: <block>/<object>"
        )));
    };
    let Some((_, b)) = p.conn.blocks.iter().find(|(id, _)| id.as_str() == block) else {
        return Err(unresolved(format!("{r}: there is no block {block}")));
    };
    let Some(kind) = kind_in(p, b, rest) else {
        return Err(unresolved(format!("{r}: block {block} has no {rest}")));
    };
    let owner = match kind {
        Kind::Pin => format!("{block}/{}", rest.split_once('/').map_or(rest, |(c, _)| c)),
        _ => r.to_owned(),
    };
    Ok(Resolved {
        block: block.to_owned(),
        kind,
        owner,
    })
}

/// Every reference in `refs` resolved, or the first refusal.
pub fn resolve_all(p: &Project, refs: &[String]) -> Result<Vec<Resolved>> {
    refs.iter().map(|r| resolve(p, r)).collect()
}

/// What each of `refs` names, `{ref, kind, block}`, leaving out those
/// that name nothing (a selection pruned at the next write).
pub fn objects(p: &Project, refs: &[String]) -> Vec<Value> {
    refs.iter()
        .filter_map(|r| {
            let got = resolve(p, r).ok()?;
            Some(json!({"ref": r, "kind": got.kind, "block": got.block}))
        })
        .collect()
}

/// Every reference path block `b` has but pins and labels: what
/// [`closest`] chooses from.
fn names(p: &Project, b: &BlockFiles) -> Vec<String> {
    let id = &b.id;
    let mut out: Vec<String> = b.components.keys().map(|c| format!("{id}/{c}")).collect();
    out.extend(b.nets.keys().map(|n| format!("{id}/{n}")));
    out.extend(p.conn.rails.keys().map(|r| format!("{id}/rail:{r}")));
    out.extend(b.instances.keys().map(|i| format!("{id}/instance:{i}")));
    if let Ok(x) = &b.block.item {
        out.extend(x.ports.keys().map(|k| format!("{id}/port:{k}")));
        out.extend(x.buses.keys().map(|k| format!("{id}/bus:{k}")));
    }
    if let Some(Ok(l)) = &b.layout {
        out.extend(l.regions.keys().map(|k| format!("{id}/region:{k}")));
        out.extend(l.notes.keys().map(|k| format!("{id}/note:{k}")));
    }
    out
}

/// Up to five reference paths nearest `r` by edit distance, within a
/// third of its length (at least 2), nearest first: from its block, or
/// from every block when its block is unknown.
pub fn closest(p: &Project, r: &str) -> Vec<String> {
    let block = r.split_once('/').map_or(r, |(b, _)| b);
    let pool: Vec<String> = match p.conn.blocks.iter().find(|(id, _)| id.as_str() == block) {
        Some((_, b)) => names(p, b),
        None => p.conn.blocks.values().flat_map(|b| names(p, b)).collect(),
    };
    let limit = (r.chars().count() / 3).max(2);
    let mut scored: Vec<(usize, String)> = pool
        .into_iter()
        .map(|n| (distance(r, &n), n))
        .filter(|(d, _)| *d <= limit)
        .collect();
    scored.sort();
    scored.into_iter().take(5).map(|(_, n)| n).collect()
}

/// Levenshtein's distance over characters.
fn distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let above = row[j + 1];
            row[j + 1] = (above + 1)
                .min(row[j] + 1)
                .min(diagonal + usize::from(ca != *cb));
            diagonal = above;
        }
    }
    row[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::ProjectKind;
    use crate::project::tests::{design_in, library_in, manifest};

    #[test]
    fn references_resolve_or_name_the_closest() {
        let dir = tempfile::tempdir().unwrap();
        let board = dir.path().join("board");
        manifest(&board, ProjectKind::Design, &[]);
        library_in(&board);
        design_in(&board);
        let p = Project::load(&board).unwrap();
        // design_in: block `top` with component r1 (pin 1 on net sda, pin
        // 2 on rail v3v3), net sda, instance ch_a; rails gnd and v3v3.
        for (r, kind, owner) in [
            ("top/r1", Kind::Component, "top/r1"),
            ("top/r1/1", Kind::Pin, "top/r1"),
            ("top/sda", Kind::Net, "top/sda"),
            ("top/rail:gnd", Kind::Rail, "top/rail:gnd"),
            ("top/instance:ch_a", Kind::Instance, "top/instance:ch_a"),
            ("top/label:sda@r1/1", Kind::Label, "top/label:sda@r1/1"),
        ] {
            let got = resolve(&p, r).unwrap_or_else(|e| panic!("{r}: {e:?}"));
            assert_eq!(
                (got.block.as_str(), got.kind, got.owner.as_str()),
                ("top", kind, owner),
                "{r}"
            );
        }
        for (r, near) in [
            ("top/r2", "top/r1"),
            ("tpo/r1", "top/r1"),
            ("top/sdaa", "top/sda"),
            ("top/r1/99", "top/r1"),
        ] {
            let e = resolve(&p, r).unwrap_err();
            assert_eq!(e.code, Code::RefUnresolved, "{r}");
            assert_eq!(e.details["ref"], r);
            let closest = e.details["closest"].as_array().unwrap();
            assert!(closest.iter().any(|c| c == near), "{r}: {closest:?}");
        }
        assert_eq!(
            resolve(&p, "nothing").unwrap_err().code,
            Code::RefUnresolved
        );
        assert_eq!(
            resolve(&p, "top/rail:nope").unwrap_err().code,
            Code::RefUnresolved
        );
        // Every reference given, or the first that names nothing.
        assert_eq!(
            resolve_all(&p, &["top/r1".into(), "top/sda".into()])
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            resolve_all(&p, &["top/r1".into(), "top/r7".into()])
                .unwrap_err()
                .details["ref"],
            "top/r7"
        );
        // What a selection's references name, the unresolved left out.
        assert_eq!(
            objects(&p, &["top/r1/1".into(), "top/gone".into()]),
            vec![serde_json::json!({"ref": "top/r1/1", "kind": "pin", "block": "top"})]
        );
    }

    #[test]
    fn distances_count_edits() {
        assert_eq!(distance("top/r1", "top/r1"), 0);
        assert_eq!(distance("top/r2", "top/r1"), 1);
        assert_eq!(distance("tpo/r1", "top/r1"), 2);
        assert_eq!(distance("", "abc"), 3);
    }
}
