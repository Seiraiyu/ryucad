//! `diff`: items and documents added, removed and changed between two revisions of the
//! catalogue (`HEAD` and the working tree by default), each change as
//! textconv lines.

use crate::Plm;
use crate::error::Result;
use crate::load::Catalogue;
use crate::session::Session;
use serde_json::{Value, json};
use std::collections::BTreeMap;

fn lines_of(text: &str) -> Vec<String> {
    let t = String::from_utf8(crate::textconv::textconv(text.as_bytes())).unwrap_or_default();
    t.lines().skip(1).map(str::to_owned).collect()
}

/// Added, removed and changed between two maps of records, each as `{id,
/// number}` and each change as its textconv lines removed and added.
fn compare<T: PartialEq>(
    a: &BTreeMap<crate::id::Id, T>,
    b: &BTreeMap<crate::id::Id, T>,
    number: impl Fn(&T) -> &str,
    text: impl Fn(&T) -> String,
) -> Value {
    let row = |id: &crate::id::Id, x: &T| json!({"id": id, "number": number(x)});
    let added: Vec<Value> = b
        .iter()
        .filter(|(id, _)| !a.contains_key(*id))
        .map(|(id, x)| row(id, x))
        .collect();
    let removed: Vec<Value> = a
        .iter()
        .filter(|(id, _)| !b.contains_key(*id))
        .map(|(id, x)| row(id, x))
        .collect();
    let mut changed = Vec::new();
    for (id, old) in a {
        let Some(new) = b.get(id) else { continue };
        if old == new {
            continue;
        }
        let (l0, l1) = (lines_of(&text(old)), lines_of(&text(new)));
        changed.push(json!({
            "id": id,
            "number": number(new),
            "removed": l0.iter().filter(|l| !l1.contains(l)).collect::<Vec<_>>(),
            "added": l1.iter().filter(|l| !l0.contains(l)).collect::<Vec<_>>(),
        }));
    }
    json!({"added": added, "removed": removed, "changed": changed})
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
    let items = compare(&a.items, &b.items, |i| i.number.as_str(), |i| i.to_text());
    let documents = compare(
        &a.documents,
        &b.documents,
        |d| d.number.as_str(),
        |d| d.to_text(),
    );
    Ok(json!({
        "schema": crate::OUTPUT_SCHEMA,
        "from": from.unwrap_or("HEAD"),
        "to": to.unwrap_or("working tree"),
        "added": items["added"],
        "removed": items["removed"],
        "changed": items["changed"],
        "documents": documents,
    }))
}
