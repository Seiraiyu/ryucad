//! The library's requests: symbols authored by batch edits, footprints
//! read, library components made and edited. Writes go to this project's
//! own library (`self`), each one logged, undoable change guarded by the
//! library's revision; a library project is edited by running ryueda in
//! it. Reads reach every library.

use crate::doc;
use crate::edit;
use crate::error::{Code, Error, Result};
use crate::fault::{self, Fault};
use crate::id::Id;
use crate::library::{Library, ObjKind};
use crate::name;
use crate::project::{Project, SELF, ref_text};
use crate::server::check_revision;
use crate::session::{Session, path_of};
use crate::source::write_atomic;
use crate::symbol::{self, Symbol};
use serde::Serialize;
use serde_json::{Value, json};

/// The library and ID a request names an object of `kind` by: `<id>` in
/// this project's library, or `lib/<alias>/<kind>/<id>`.
pub fn target(text: &str, kind: ObjKind) -> Result<(String, Id)> {
    let bad = || {
        Error::new(
            Code::InvalidParam,
            format!(
                "name a {} by its ID, or as lib/<alias>/{}/<id>",
                kind.as_str(),
                kind.as_str()
            ),
        )
        .with("reference", text)
    };
    let (alias, id) = match text.split('/').collect::<Vec<_>>().as_slice() {
        [id] => (SELF.to_owned(), *id),
        ["lib", alias, k, id] if *k == kind.as_str() => ((*alias).to_owned(), *id),
        _ => return Err(bad()),
    };
    Ok((alias, Id::parse(id, kind.as_str())?))
}

/// The library a write may change: this project's own.
fn writable<'a>(p: &'a Project, alias: &str) -> Result<&'a Library> {
    if alias != SELF {
        return Err(Error::new(
            Code::InvalidParam,
            format!(
                "writes go to this project's own library; edit {alias} by running ryueda in its library project"
            ),
        )
        .with("library", alias));
    }
    p.library(SELF)
}

/// Writes `t` as the file at `path` and takes it in.
fn save<T: Serialize>(
    session: &mut Session,
    path: &str,
    t: &T,
    kind: &str,
    format: u64,
) -> Result<()> {
    write_atomic(
        &path_of(session.root(), path),
        doc::to_string(t, kind, format),
    )?;
    session.reload()
}

/// The errors in `after` that `before` doesn't have, by code and place.
fn added_errors(before: &[Fault], after: &[Fault]) -> Vec<Fault> {
    let key = |f: &Fault| (f.error.code, f.error.details.get("pointer").cloned());
    after
        .iter()
        .filter(|f| f.is_error() && !before.iter().any(|b| key(b) == key(f)))
        .cloned()
        .collect()
}

/// The refusal of a write that would add `errors`: the first's code, with
/// every one in the details.
fn refused(what: &str, errors: &[Fault]) -> Error {
    let first = &errors[0].error;
    Error::new(
        first.code,
        format!(
            "{what} would have {} fault(s): {}",
            errors.len(),
            first.message
        ),
    )
    .with("faults", fault::list(errors))
}

/// The components of this project's library that place symbol `id` of
/// it, by reference.
fn symbol_users(lib: &Library, id: &Id) -> Vec<String> {
    lib.components
        .iter()
        .filter(|(_, c)| {
            c.item.as_ref().is_ok_and(|c| {
                c.symbol.id == *id && c.symbol.library.as_ref().is_none_or(|l| l.as_str() == SELF)
            })
        })
        .map(|(cid, _)| ref_text(SELF, ObjKind::Libcomp, cid))
        .collect()
}

/// Each of `refs`' faults as they are now: what a write did to the
/// components that use what it changed.
fn faults_of(p: &Project, refs: &[String]) -> Value {
    let lib = p.library(SELF).ok();
    let list: Vec<Value> = refs
        .iter()
        .filter_map(|r| {
            let id = r.rsplit('/').next()?;
            let c = lib?.components.get(&Id::parse(id, "libcomp").ok()?)?;
            Some(json!({"reference": r, "faults": fault::list(&c.faults)}))
        })
        .collect();
    Value::Array(list)
}

fn symbol_json(lib: &Library, id: &Id) -> Result<Value> {
    let at = ref_text(&lib.alias, ObjKind::Symbol, id);
    let l = lib.symbols.get(id).ok_or_else(|| {
        Error::new(Code::NotFound, format!("{at} isn't in the library")).with("reference", &at)
    })?;
    let s = l
        .item
        .as_ref()
        .map_err(|e| e.clone().with("reference", &at))?;
    Ok(json!({
        "reference": at,
        "path": lib.path(ObjKind::Symbol, id),
        "symbol": s,
        "faults": fault::list(&l.faults),
        "revision": lib.revision,
    }))
}

/// `symbol.create`: a symbol called `name`, with `units` (1 by default)
/// and the batch `ops` applied to it.
pub fn symbol_create(
    session: &mut Session,
    name: &str,
    id: Option<&str>,
    units: Option<u32>,
    ops: &[Value],
    expected: Option<&str>,
) -> Result<Value> {
    let p = session.project();
    let lib = writable(p, SELF)?;
    check_revision("library self", &lib.revision, expected)?;
    let id = match id {
        Some(t) => {
            let id = Id::parse(t, "symbol")?;
            if lib.has(ObjKind::Symbol, id.as_str()) {
                return Err(Error::new(
                    Code::DuplicateId,
                    format!("the library already has symbol {id}"),
                )
                .with("reference", ref_text(SELF, ObjKind::Symbol, &id)));
            }
            id
        }
        None => name::free(name, |s| lib.has(ObjKind::Symbol, s)),
    };
    let mut s = Symbol {
        name: name.to_owned(),
        units: units.unwrap_or(1),
        pins: Default::default(),
        graphics: Default::default(),
    };
    let applied = edit::apply(&mut s, ops, p.manifest.default_unit)?;
    let errors = added_errors(&[], &s.check());
    if !errors.is_empty() {
        return Err(refused(&format!("symbol {id}"), &errors));
    }
    let path = lib.path(ObjKind::Symbol, &id);
    save(session, &path, &s, symbol::KIND, symbol::FORMAT)?;
    let lib = session.project().library(SELF)?;
    let mut v = symbol_json(lib, &id)?;
    v["added"] = json!(applied.added);
    Ok(v)
}

/// `symbol.edit`: a new name or unit count, and a batch of ops, applied
/// all or nothing.
pub fn symbol_edit(
    session: &mut Session,
    target: &str,
    name: Option<&str>,
    units: Option<u32>,
    ops: &[Value],
    expected: Option<&str>,
) -> Result<Value> {
    let p = session.project();
    let (alias, id) = self::target(target, ObjKind::Symbol)?;
    let lib = writable(p, &alias)?;
    check_revision("library self", &lib.revision, expected)?;
    let at = ref_text(SELF, ObjKind::Symbol, &id);
    let loaded = lib.symbols.get(&id).ok_or_else(|| {
        Error::new(Code::NotFound, format!("{at} isn't in the library")).with("reference", &at)
    })?;
    let mut s = loaded.item.clone().map_err(|e| e.with("reference", &at))?;
    if let Some(n) = name {
        s.name = n.to_owned();
    }
    if let Some(u) = units {
        s.units = u;
    }
    let applied = edit::apply(&mut s, ops, p.manifest.default_unit)?;
    let users = symbol_users(lib, &id);
    if !applied.pins_gone.is_empty() && !users.is_empty() {
        return Err(Error::new(
            Code::HasDependents,
            format!(
                "pin {} is mapped by {}; map the component's pins first, or remove it",
                applied.pins_gone.join(", "),
                users.join(", ")
            ),
        )
        .with("pins", &applied.pins_gone)
        .with("dependents", &users));
    }
    let errors = added_errors(&loaded.faults, &s.check());
    if !errors.is_empty() {
        return Err(refused(&format!("symbol {id}"), &errors));
    }
    let path = lib.path(ObjKind::Symbol, &id);
    save(session, &path, &s, symbol::KIND, symbol::FORMAT)?;
    let p = session.project();
    let mut v = symbol_json(p.library(SELF)?, &id)?;
    v["added"] = json!(applied.added);
    v["components"] = faults_of(p, &users);
    Ok(v)
}

/// `symbol.show`: a symbol of any library, with its faults.
pub fn symbol_show(p: &Project, target: &str) -> Result<Value> {
    let (alias, id) = self::target(target, ObjKind::Symbol)?;
    symbol_json(p.library(&alias)?, &id)
}

/// `symbol.list`: every symbol of a library (this project's by default).
pub fn symbol_list(p: &Project, library: Option<&str>) -> Result<Value> {
    let lib = p.library(library.unwrap_or(SELF))?;
    let list: Vec<Value> = lib
        .symbols
        .iter()
        .map(|(id, l)| {
            let mut v = json!({
                "id": id,
                "reference": ref_text(&lib.alias, ObjKind::Symbol, id),
                "faults": l.faults.len(),
            });
            match &l.item {
                Ok(s) => {
                    v["name"] = json!(s.name);
                    v["units"] = json!(s.units);
                    v["pins"] = json!(s.pins.len());
                }
                Err(e) => v["error"] = json!(e),
            }
            v
        })
        .collect();
    Ok(json!({"library": lib.alias, "revision": lib.revision, "symbols": list}))
}

/// `symbol.delete`: refused while a library component places it.
pub fn symbol_delete(session: &mut Session, target: &str, expected: Option<&str>) -> Result<Value> {
    let p = session.project();
    let (alias, id) = self::target(target, ObjKind::Symbol)?;
    let lib = writable(p, &alias)?;
    check_revision("library self", &lib.revision, expected)?;
    let at = ref_text(SELF, ObjKind::Symbol, &id);
    if !lib.symbols.contains_key(&id) {
        return Err(
            Error::new(Code::NotFound, format!("{at} isn't in the library")).with("reference", &at),
        );
    }
    let users = symbol_users(lib, &id);
    if !users.is_empty() {
        return Err(Error::new(
            Code::HasDependents,
            format!(
                "{} place {at}; delete them or point them elsewhere first",
                users.join(", ")
            ),
        )
        .with("dependents", &users));
    }
    let path = path_of(session.root(), &lib.path(ObjKind::Symbol, &id));
    std::fs::remove_file(&path).map_err(|e| Error::io(&path, &e))?;
    session.reload()?;
    Ok(json!({
        "deleted": at,
        "revision": session.project().library(SELF)?.revision,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targets_are_ids_or_references() {
        assert_eq!(
            target("r", ObjKind::Symbol).unwrap(),
            ("self".into(), Id::parse("r", "s").unwrap())
        );
        assert_eq!(
            target("lib/passives/symbol/r", ObjKind::Symbol).unwrap().0,
            "passives"
        );
        for bad in ["lib/passives/footprint/r", "a/b", "lib/x/symbol", ""] {
            assert!(target(bad, ObjKind::Symbol).is_err(), "{bad}");
        }
        assert_eq!(
            target("R", ObjKind::Symbol).unwrap_err().code,
            Code::InvalidId
        );
    }
}
