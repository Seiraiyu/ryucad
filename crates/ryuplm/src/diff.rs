//! `diff`: items added, removed and changed between two revisions of the
//! catalogue (`HEAD` and the working tree by default), each change as
//! textconv lines.

use crate::Plm;
use crate::error::Result;
use crate::load::Catalogue;
use crate::session::Session;
use serde_json::{Value, json};
use std::collections::BTreeMap;

fn lines(item: &crate::item::Item) -> Vec<String> {
    let text =
        String::from_utf8(crate::textconv::textconv(item.to_text().as_bytes())).unwrap_or_default();
    text.lines().skip(1).map(str::to_owned).collect()
}

fn side(session: &mut Session, rev: Option<&str>) -> Result<Catalogue> {
    match rev {
        None => Ok(session.catalogue().clone()),
        Some(r) => {
            let (repo, _) = crate::release::history(session)?;
            let files: BTreeMap<String, Vec<u8>> =
                ryupdm::history::files_for::<Plm>(repo.gix(), r, repo.prefix())?;
            Catalogue::load(&files)
        }
    }
}

pub fn diff(session: &mut Session, from: Option<&str>, to: Option<&str>) -> Result<Value> {
    let a = side(session, Some(from.unwrap_or("HEAD")))?;
    let b = side(session, to)?;
    let row = |id: &crate::id::Id, i: &crate::item::Item| json!({"id": id, "number": i.number});
    let added: Vec<Value> = b
        .items
        .iter()
        .filter(|(id, _)| !a.items.contains_key(*id))
        .map(|(id, i)| row(id, i))
        .collect();
    let removed: Vec<Value> = a
        .items
        .iter()
        .filter(|(id, _)| !b.items.contains_key(*id))
        .map(|(id, i)| row(id, i))
        .collect();
    let mut changed = Vec::new();
    for (id, old) in &a.items {
        let Some(new) = b.items.get(id) else { continue };
        if old == new {
            continue;
        }
        let (l0, l1) = (lines(old), lines(new));
        changed.push(json!({
            "id": id,
            "number": new.number,
            "removed": l0.iter().filter(|l| !l1.contains(l)).collect::<Vec<_>>(),
            "added": l1.iter().filter(|l| !l0.contains(l)).collect::<Vec<_>>(),
        }));
    }
    Ok(json!({
        "schema": crate::OUTPUT_SCHEMA,
        "from": from.unwrap_or("HEAD"),
        "to": to.unwrap_or("working tree"),
        "added": added,
        "removed": removed,
        "changed": changed,
    }))
}
