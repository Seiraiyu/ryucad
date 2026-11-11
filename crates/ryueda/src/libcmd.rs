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
use std::collections::BTreeMap;

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
/// A class as a request names it: one of the classes, or (`none_ok`)
/// `none` for no class.
fn class_param(text: &str, none_ok: bool) -> Result<Option<crate::standards::Class>> {
    if none_ok && text == "none" {
        return Ok(None);
    }
    crate::standards::Class::parse(text)
        .map(Some)
        .ok_or_else(|| {
            Error::new(
                Code::InvalidParam,
                format!(
                    "{text} isn't a class of part{}",
                    if none_ok { " (or none)" } else { "" }
                ),
            )
            .with("param", "class")
            .with("classes", crate::standards::Class::names())
        })
}

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

/// The design components that place library component `id` of this
/// project's library, as `<block>/<component>`.
fn placed_by(p: &Project, id: &Id) -> Vec<String> {
    let mut out = Vec::new();
    for (bid, b) in &p.conn.blocks {
        for (cid, c) in &b.components {
            let places = c.item.as_ref().is_ok_and(|c| {
                c.of.id == *id && c.of.library.as_ref().is_none_or(|l| l.as_str() == SELF)
            });
            if places {
                out.push(format!("{bid}/{cid}"));
            }
        }
    }
    out
}

/// The design pins (`<block>/<component>/<pin>`) among `pins` of the
/// components that place `libcomps` (references to this library's).
fn pins_connected(p: &Project, libcomps: &[String], pins: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for r in libcomps {
        let Some(Ok(id)) = r.rsplit('/').next().map(|i| Id::parse(i, "libcomp")) else {
            continue;
        };
        for c in placed_by(p, &id) {
            let (bid, cid) = c.split_once('/').expect("block/component");
            let comp = Id::parse(bid, "block")
                .ok()
                .and_then(|b| p.conn.blocks.get(&b))
                .and_then(|b| b.components.get(&Id::parse(cid, "component").ok()?))
                .and_then(|l| l.item.as_ref().ok());
            if let Some(comp) = comp {
                for pin in pins.iter().filter(|n| comp.pins.contains_key(*n)) {
                    out.push(format!("{c}/{pin}"));
                }
            }
        }
    }
    out
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
#[allow(clippy::too_many_arguments)]
pub fn symbol_create(
    session: &mut Session,
    name: &str,
    id: Option<&str>,
    units: Option<u32>,
    class: Option<&str>,
    template: Option<&str>,
    pins: &[crate::template::PinSpec],
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
    if template.is_none() && !pins.is_empty() {
        return Err(Error::new(
            Code::InvalidParam,
            "pins are placed by a template: give --template",
        )
        .with("param", "pins"));
    }
    let class = class.map(|c| class_param(c, false)).transpose()?.flatten();
    let mut s = match template {
        Some(t) => {
            let Some(tc) = class_param(t, false)? else {
                unreachable!("none is refused")
            };
            if class.is_some_and(|c| c != tc) || units.is_some_and(|u| u != 1) {
                return Err(Error::new(
                    Code::InvalidParam,
                    format!("a {t} template draws one unit of class {t}"),
                )
                .with("param", "template"));
            }
            let (st, _) = p.standards_for(SELF)?;
            crate::template::template(name, tc, pins, &st)?
        }
        None => Symbol {
            name: name.to_owned(),
            units: units.unwrap_or(1),
            class,
            pins: Default::default(),
            graphics: Default::default(),
        },
    };
    let applied = edit::apply(&mut s, ops, p.manifest.default_unit)?;
    let errors = added_errors(&[], &s.check());
    if !errors.is_empty() {
        return Err(refused(&format!("symbol {id}"), &errors));
    }
    let path = lib.path(ObjKind::Symbol, &id);
    save(session, &path, &s, symbol::KIND, symbol::format_of(&s))?;
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
    class: Option<&str>,
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
    if let Some(c) = class {
        s.class = class_param(c, true)?;
    }
    let applied = edit::apply(&mut s, ops, p.manifest.default_unit)?;
    let users = symbol_users(lib, &id);
    if !applied.pins_gone.is_empty() && !users.is_empty() {
        // The library components that map the pins, then the design pins
        // connected through them.
        let mut dependents = users.clone();
        dependents.extend(pins_connected(p, &users, &applied.pins_gone));
        return Err(Error::new(
            Code::HasDependents,
            format!(
                "pin {} is mapped by {}; map the component's pins first, or remove it",
                applied.pins_gone.join(", "),
                users.join(", ")
            ),
        )
        .with("pins", &applied.pins_gone)
        .with("dependents", &dependents));
    }
    let errors = added_errors(&loaded.faults, &s.check());
    if !errors.is_empty() {
        return Err(refused(&format!("symbol {id}"), &errors));
    }
    let path = lib.path(ObjKind::Symbol, &id);
    save(session, &path, &s, symbol::KIND, symbol::format_of(&s))?;
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

fn footprint_json(lib: &Library, id: &Id) -> Result<Value> {
    let at = ref_text(&lib.alias, ObjKind::Footprint, id);
    let l = lib.footprints.get(id).ok_or_else(|| {
        Error::new(Code::NotFound, format!("{at} isn't in the library")).with("reference", &at)
    })?;
    let f = l
        .item
        .as_ref()
        .map_err(|e| e.clone().with("reference", &at))?;
    Ok(json!({
        "reference": at,
        "path": lib.path(ObjKind::Footprint, id),
        "footprint": f,
        "faults": fault::list(&l.faults),
        "revision": lib.revision,
    }))
}

/// `footprint.show`: a footprint of any library, with its faults.
pub fn footprint_show(p: &Project, target: &str) -> Result<Value> {
    let (alias, id) = self::target(target, ObjKind::Footprint)?;
    footprint_json(p.library(&alias)?, &id)
}

/// `footprint.list`: every footprint of a library (this project's by
/// default).
pub fn footprint_list(p: &Project, library: Option<&str>) -> Result<Value> {
    let lib = p.library(library.unwrap_or(SELF))?;
    let list: Vec<Value> = lib
        .footprints
        .iter()
        .map(|(id, l)| {
            let mut v = json!({
                "id": id,
                "reference": ref_text(&lib.alias, ObjKind::Footprint, id),
                "faults": l.faults.len(),
            });
            match &l.item {
                Ok(f) => {
                    v["name"] = json!(f.name);
                    v["pads"] = json!(f.pads.len());
                    v["models"] = json!(f.models.len());
                }
                Err(e) => v["error"] = json!(e),
            }
            v
        })
        .collect();
    Ok(json!({"library": lib.alias, "revision": lib.revision, "footprints": list}))
}

/// What a request gives of a library component: every field may be
/// left out (an edit keeps what it doesn't give).
#[derive(Clone, Debug, Default, PartialEq, Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompFields {
    /// A part record (`lcsc:C1525`, `mpn:…`) whose identifiers to take:
    /// maker, part number, description, datasheet, category and its
    /// supplier offers, never its parameters (design 347). Fields given
    /// win.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefix: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub datasheet: Option<String>,
    /// Merged into the parameters; `null` removes one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<serde_json::Map<String, Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manufacturer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mpn: Option<String>,
    /// The whole list: `[{supplier, sku}]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offers: Option<Vec<crate::libcomp::Offer>>,
    /// One of the categories (`resistor`, `capacitor`, …); `""` puts it
    /// back to the one the prefix gives.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    /// The whole list of alternates, replacing it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alternates: Option<Vec<crate::libcomp::Alternate>>,
    /// Alternates to take off, by part number.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remove_alternates: Option<Vec<String>>,
    /// Alternates to add; one with a part number already listed replaces it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub add_alternates: Option<Vec<crate::libcomp::Alternate>>,
    /// Offers for alternates, each naming its alternate's part number.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alternate_offers: Option<Vec<AlternateOffer>>,
    /// The whole list, the default first: `[{footprint, pins?}]`, each
    /// footprint `<id>` or `lib/<alias>/footprint/<id>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub footprints: Option<Vec<FootprintArg>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FootprintArg {
    pub footprint: String,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub pins: std::collections::BTreeMap<String, Vec<String>>,
}

/// An offer for one of a component's alternates.
#[derive(Clone, Debug, PartialEq, Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AlternateOffer {
    pub mpn: String,
    pub supplier: String,
    pub sku: String,
}

/// A request's object reference as a file's: no `library` for this
/// project's own.
fn lib_ref(text: &str, kind: ObjKind) -> Result<crate::libcomp::LibRef> {
    let (alias, id) = target(text, kind)?;
    let library = (alias != SELF)
        .then(|| Id::parse(&alias, "library alias"))
        .transpose()?;
    Ok(crate::libcomp::LibRef { library, id })
}

/// `c` with `f`'s fields written over it.
fn fill(c: &mut crate::libcomp::LibComp, f: &CompFields) -> Result<()> {
    if let Some(s) = &f.symbol {
        c.symbol = lib_ref(s, ObjKind::Symbol).map_err(|e| e.with("param", "symbol"))?;
    }
    for (field, into) in [
        (&f.prefix, &mut c.prefix),
        (&f.value, &mut c.value),
        (&f.description, &mut c.description),
        (&f.datasheet, &mut c.datasheet),
        (&f.manufacturer, &mut c.manufacturer),
        (&f.mpn, &mut c.mpn),
    ] {
        if let Some(v) = field {
            *into = v.clone();
        }
    }
    if let Some(params) = &f.parameters {
        for (k, v) in params {
            match v {
                Value::Null => {
                    c.parameters.remove(k);
                }
                Value::String(s) => {
                    c.parameters.insert(k.clone(), s.clone());
                }
                v => {
                    return Err(Error::new(
                        Code::InvalidParam,
                        format!("parameter {k} is text (\"50 V\", \"X7R\"), not {v}"),
                    )
                    .with("param", format!("parameters/{k}")));
                }
            }
        }
    }
    if let Some(o) = &f.offers {
        c.offers = o.clone();
    }
    if let Some(list) = &f.footprints {
        c.footprints = list
            .iter()
            .enumerate()
            .map(|(i, u)| {
                Ok(crate::libcomp::Use {
                    footprint: lib_ref(&u.footprint, ObjKind::Footprint)
                        .map_err(|e| e.with("param", format!("footprints/{i}/footprint")))?,
                    pins: u.pins.clone(),
                })
            })
            .collect::<Result<_>>()?;
    }
    if let Some(cat) = &f.category {
        c.category = match cat.as_str() {
            "" => None,
            s => Some(crate::libcomp::Category::parse(s).ok_or_else(|| {
                Error::new(
                    Code::InvalidParam,
                    format!(
                        "{s} isn't a category: {}",
                        crate::libcomp::Category::names().join(", ")
                    ),
                )
                .with("param", "category")
            })?),
        };
    }
    if let Some(all) = &f.alternates {
        c.alternates = all.clone();
    }
    for mpn in f.remove_alternates.iter().flatten() {
        let before = c.alternates.len();
        c.alternates.retain(|a| &a.mpn != mpn);
        if c.alternates.len() == before {
            return Err(Error::new(
                Code::NotFound,
                format!("no alternate has part number {mpn}"),
            )
            .with("param", "remove_alternates")
            .with("mpn", mpn));
        }
    }
    for a in f.add_alternates.iter().flatten() {
        match c.alternates.iter_mut().find(|x| x.mpn == a.mpn) {
            Some(x) => *x = a.clone(),
            None => c.alternates.push(a.clone()),
        }
    }
    for o in f.alternate_offers.iter().flatten() {
        let a = c
            .alternates
            .iter_mut()
            .find(|x| x.mpn == o.mpn)
            .ok_or_else(|| {
                Error::new(
                    Code::NotFound,
                    format!(
                        "no alternate has part number {}; add the alternate first",
                        o.mpn
                    ),
                )
                .with("param", "alternate_offers")
                .with("mpn", &o.mpn)
            })?;
        let offer = crate::libcomp::Offer {
            supplier: o.supplier.clone(),
            sku: o.sku.clone(),
        };
        if !a.offers.contains(&offer) {
            a.offers.push(offer);
        }
    }
    if c.prefix.is_empty() || !c.prefix.chars().all(|ch| ch.is_ascii_alphabetic()) {
        return Err(Error::new(
            Code::InvalidParam,
            "a prefix is the reference designator's letters: R, C, U, SW",
        )
        .with("param", "prefix"));
    }
    Ok(())
}

/// What `c`'s checks find, as this project's library would hold it.
fn comp_faults(p: &Project, c: &crate::libcomp::LibComp) -> Vec<Fault> {
    let footprints: Vec<Result<&crate::footprint::Footprint>> = c
        .footprints
        .iter()
        .map(|u| p.footprint(SELF, &u.footprint))
        .collect();
    let mut out = c.check(p.symbol(SELF, &c.symbol), &footprints);
    out.extend(c.check_data());
    out
}

fn comp_json(lib: &Library, id: &Id) -> Result<Value> {
    let at = ref_text(&lib.alias, ObjKind::Libcomp, id);
    let l = lib.components.get(id).ok_or_else(|| {
        Error::new(Code::NotFound, format!("{at} isn't in the library")).with("reference", &at)
    })?;
    let c = l
        .item
        .as_ref()
        .map_err(|e| e.clone().with("reference", &at))?;
    let (category, inferred) = c.category();
    Ok(json!({
        "reference": at,
        "path": lib.path(ObjKind::Libcomp, id),
        "libcomp": c,
        "category": category,
        "category_inferred": inferred,
        "faults": fault::list(&l.faults),
        "revision": lib.revision,
    }))
}

/// The one record `from` names, from this machine's sources but the
/// library: refused where none, or several, match.
pub fn record_from(
    p: &Project,
    from: &str,
    cats: &crate::parts::Catalogues,
) -> Result<ryueda_parts::record::Record> {
    let (mut recs, warnings, mut searched) = crate::parts::records_of(p, from, cats)?;
    let library = ryueda_parts::library::Library::NAME;
    recs.retain(|r| r.source != library);
    searched.retain(|n| *n != library);
    // A distributor's records are fetched by `parts show` (with the
    // daemon's lock released) and kept a day; a write reads only those.
    let live: Vec<&str> = searched
        .iter()
        .copied()
        .filter(|n| ryueda_parts::distributor::distributor(n).is_some())
        .collect();
    match recs.len() {
        0 if !live.is_empty() => Err(Error::new(
            Code::PartsNotFound,
            format!("this machine keeps no record of {from}; run ryueda parts show {from} first"),
        )
        .with("key", from)
        .with("searched", searched)
        .with("warnings", warnings)
        .with("command", format!("ryueda parts show {from}"))),
        0 => Err(
            Error::new(Code::PartsNotFound, format!("no source here has {from}"))
                .with("key", from)
                .with("searched", searched)
                .with("warnings", warnings),
        ),
        1 => Ok(recs.remove(0)),
        n => Err(Error::new(
            Code::InvalidParam,
            format!("{from} names {n} parts; name one by its key"),
        )
        .with("param", "from")
        .with(
            "keys",
            recs.iter()
                .map(|r| format!("{}:{}", r.source, r.key))
                .collect::<Vec<_>>(),
        )),
    }
}

/// `f` with the record's identifiers where `f` doesn't give them.
fn from_record(f: &CompFields, r: &ryueda_parts::record::Record) -> CompFields {
    let mut out = f.clone();
    let take = |given: &mut Option<String>, have: &str| {
        if given.is_none() && !have.is_empty() {
            *given = Some(have.to_owned());
        }
    };
    take(&mut out.manufacturer, &r.manufacturer);
    take(&mut out.mpn, &r.mpn);
    take(&mut out.description, &r.description);
    take(&mut out.datasheet, &r.datasheet);
    if out.category.is_none() {
        out.category = r.category.map(|c| c.as_str().to_owned());
    }
    if out.offers.is_none() && !r.offers.is_empty() {
        out.offers = Some(r.offers.clone());
    }
    out
}

/// `have` with the offers of `record`'s suppliers (matched without regard
/// to case, as `catalogue::for_supplier` matches them) replaced by the
/// record's, where the first of them was, else after the rest; every
/// other supplier's offers kept as they are.
fn merged_offers(
    have: &[ryueda_parts::record::Offer],
    record: &[ryueda_parts::record::Offer],
) -> Vec<ryueda_parts::record::Offer> {
    let norm = |s: &str| s.trim().to_ascii_lowercase();
    let theirs: Vec<String> = record.iter().map(|o| norm(&o.supplier)).collect();
    let mut out = Vec::with_capacity(have.len() + record.len());
    let mut placed = false;
    for o in have {
        if theirs.contains(&norm(&o.supplier)) {
            if !placed {
                out.extend(record.iter().cloned());
                placed = true;
            }
        } else {
            out.push(o.clone());
        }
    }
    if !placed {
        out.extend(record.iter().cloned());
    }
    out
}

/// `r`'s text read in library `alias`: `lib/<alias>/<kind>/<id>`.
fn text_from(r: &crate::libcomp::LibRef, alias: &str, kind: ObjKind) -> String {
    let alias = r.library.as_ref().map_or(alias, |a| a.as_str());
    ref_text(alias, kind, &r.id)
}

/// What library components of `category` and `package` in every usable
/// library use most: the symbol, the first footprint and the prefix
/// (ties: the least), with how many use each.
struct Proposed {
    symbol: Option<(String, usize)>,
    footprint: Option<(String, usize)>,
    prefix: Option<(String, usize)>,
}

fn proposed(
    p: &Project,
    category: Option<crate::libcomp::Category>,
    package: Option<&str>,
) -> Proposed {
    let mut symbols: BTreeMap<String, usize> = BTreeMap::new();
    let mut footprints: BTreeMap<String, usize> = BTreeMap::new();
    let mut prefixes: BTreeMap<String, usize> = BTreeMap::new();
    for (alias, lib) in &p.libraries {
        let Ok(lib) = lib else { continue };
        for l in lib.components.values() {
            let Ok(c) = &l.item else { continue };
            if Some(c.category().0) != category {
                continue;
            }
            let pkg = c.parameters.get("package").map(|x| x.trim().to_lowercase());
            if package
                .is_some_and(|want| pkg.as_deref() != Some(want.trim().to_lowercase().as_str()))
            {
                continue;
            }
            *symbols
                .entry(text_from(&c.symbol, alias, ObjKind::Symbol))
                .or_default() += 1;
            if let Some(u) = c.footprints.first() {
                *footprints
                    .entry(text_from(&u.footprint, alias, ObjKind::Footprint))
                    .or_default() += 1;
            }
            *prefixes.entry(c.prefix.clone()).or_default() += 1;
        }
    }
    let best = |m: BTreeMap<String, usize>| {
        m.into_iter()
            .max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0)))
    };
    Proposed {
        symbol: best(symbols),
        footprint: best(footprints),
        prefix: best(prefixes),
    }
}

/// What `libcomp create --from` and `edit --from` answer of the record.
fn from_json(r: &ryueda_parts::record::Record) -> Value {
    json!({
        "key": format!("{}:{}", r.source, r.key),
        "source": r.source,
        "parameters": r.parameters,
        "attribution": r.attribution,
        "fetched_at": r.fetched_at,
    })
}

/// `libcomp.create`: a library component, its ID made from `name` unless
/// given, tying a symbol to its footprints. With `from`, a record's
/// identifiers fill what isn't given, and a symbol, footprint and prefix
/// left out are those the library's components of its category and
/// package use most.
pub fn libcomp_create(
    session: &mut Session,
    name: &str,
    id: Option<&str>,
    fields: &CompFields,
    expected: Option<&str>,
    cats: &crate::parts::Catalogues,
) -> Result<Value> {
    let p = session.project();
    let mut chose = serde_json::Map::new();
    let mut from = None;
    let filled;
    let fields = match &fields.from {
        None => fields,
        Some(key) => {
            let r = record_from(p, key, cats)?;
            let mut f = from_record(fields, &r);
            let pr = proposed(
                p,
                r.category,
                r.parameters.get("package").map(String::as_str),
            );
            if f.symbol.is_none() {
                let (sym, uses) = pr.symbol.clone().ok_or_else(|| {
                    Error::new(
                        Code::InvalidParam,
                        "no library component of this category and package to take a symbol from; give --symbol",
                    )
                    .with("param", "symbol")
                })?;
                chose.insert("symbol".into(), json!({"reference": sym, "uses": uses}));
                f.symbol = Some(sym);
            }
            if f.footprints.is_none()
                && let Some((fp, uses)) = pr.footprint.clone()
            {
                chose.insert("footprint".into(), json!({"reference": fp, "uses": uses}));
                f.footprints = Some(vec![FootprintArg {
                    footprint: fp,
                    pins: Default::default(),
                }]);
            }
            if f.prefix.is_none()
                && let Some((px, uses)) = pr.prefix.clone()
            {
                chose.insert("prefix".into(), json!({"prefix": px, "uses": uses}));
                f.prefix = Some(px);
            }
            from = Some(from_json(&r));
            filled = f;
            &filled
        }
    };
    let lib = writable(p, SELF)?;
    check_revision("library self", &lib.revision, expected)?;
    let id = match id {
        Some(t) => {
            let id = Id::parse(t, "libcomp")?;
            if lib.has(ObjKind::Libcomp, id.as_str()) {
                return Err(Error::new(
                    Code::DuplicateId,
                    format!("the library already has libcomp {id}"),
                )
                .with("reference", ref_text(SELF, ObjKind::Libcomp, &id)));
            }
            id
        }
        None => name::free(name, |s| lib.has(ObjKind::Libcomp, s)),
    };
    let symbol = fields.symbol.as_deref().ok_or_else(|| {
        Error::new(Code::InvalidParam, "a library component names its symbol")
            .with("param", "symbol")
    })?;
    let mut c = crate::libcomp::LibComp {
        symbol: lib_ref(symbol, ObjKind::Symbol)?,
        prefix: String::new(),
        value: String::new(),
        description: String::new(),
        datasheet: String::new(),
        parameters: Default::default(),
        manufacturer: String::new(),
        mpn: String::new(),
        offers: Vec::new(),
        category: None,
        alternates: Vec::new(),
        footprints: Vec::new(),
    };
    fill(&mut c, fields)?;
    let errors = added_errors(&[], &comp_faults(p, &c));
    if !errors.is_empty() {
        return Err(refused(&format!("libcomp {id}"), &errors));
    }
    let path = lib.path(ObjKind::Libcomp, &id);
    let bare = c.parameters.is_empty();
    save(
        session,
        &path,
        &c,
        crate::libcomp::KIND,
        crate::libcomp::format_of(&c),
    )?;
    let mut v = comp_json(session.project().library(SELF)?, &id)?;
    if let Some(from) = from {
        v["from"] = from;
        v["chose"] = Value::Object(chose);
        v["warnings"] = json!(if bare {
            vec![json!({
                "code": "parameter_missing_from",
                "message": format!("{id} has no parameters: type them with --param (the record's are under from.parameters)"),
            })]
        } else {
            Vec::new()
        });
    }
    Ok(v)
}

/// `libcomp.edit`: the fields given replace the component's, its
/// parameters merged. With `from`, each of a record's identifiers the
/// fields don't give is taken where the component's differs (`changed`
/// names them); with nothing to change, nothing is written.
pub fn libcomp_edit(
    session: &mut Session,
    target: &str,
    fields: &CompFields,
    expected: Option<&str>,
    cats: &crate::parts::Catalogues,
) -> Result<Value> {
    let p = session.project();
    let (alias, id) = self::target(target, ObjKind::Libcomp)?;
    let lib = writable(p, &alias)?;
    check_revision("library self", &lib.revision, expected)?;
    let at = ref_text(SELF, ObjKind::Libcomp, &id);
    let loaded = lib.components.get(&id).ok_or_else(|| {
        Error::new(Code::NotFound, format!("{at} isn't in the library")).with("reference", &at)
    })?;
    let mut c = loaded.item.clone().map_err(|e| e.with("reference", &at))?;
    let (mut changed, mut from) = (Vec::new(), None);
    let filled;
    let fields = match &fields.from {
        None => fields,
        Some(key) => {
            let r = record_from(p, key, cats)?;
            let mut f = fields.clone();
            let mut take =
                |name: &'static str, given: &mut Option<String>, have: &str, now: &str| {
                    if given.is_none() && !have.is_empty() && have != now {
                        *given = Some(have.to_owned());
                        changed.push(name);
                    }
                };
            take(
                "manufacturer",
                &mut f.manufacturer,
                &r.manufacturer,
                &c.manufacturer,
            );
            take("mpn", &mut f.mpn, &r.mpn, &c.mpn);
            take(
                "description",
                &mut f.description,
                &r.description,
                &c.description,
            );
            take("datasheet", &mut f.datasheet, &r.datasheet, &c.datasheet);
            // Against the category the component has, inferred or not.
            if f.category.is_none()
                && let Some(rc) = r.category
                && rc != c.category().0
            {
                f.category = Some(rc.as_str().to_owned());
                changed.push("category");
            }
            if f.offers.is_none() && !r.offers.is_empty() {
                let merged = merged_offers(&c.offers, &r.offers);
                if merged != c.offers {
                    f.offers = Some(merged);
                    changed.push("offers");
                }
            }
            changed.sort_unstable();
            from = Some(from_json(&r));
            let only_from = CompFields {
                from: None,
                ..fields.clone()
            } == CompFields::default();
            if changed.is_empty() && only_from {
                let mut v = comp_json(lib, &id)?;
                v["changed"] = json!(changed);
                v["from"] = from.take().unwrap_or_default();
                return Ok(v);
            }
            filled = f;
            &filled
        }
    };
    fill(&mut c, fields)?;
    let errors = added_errors(&loaded.faults, &comp_faults(p, &c));
    if !errors.is_empty() {
        return Err(refused(&format!("libcomp {id}"), &errors));
    }
    let path = lib.path(ObjKind::Libcomp, &id);
    save(
        session,
        &path,
        &c,
        crate::libcomp::KIND,
        crate::libcomp::format_of(&c),
    )?;
    let mut v = comp_json(session.project().library(SELF)?, &id)?;
    if let Some(from) = from {
        v["changed"] = json!(changed);
        v["from"] = from;
    }
    Ok(v)
}

/// `libcomp.show`: a library component of any library, with its faults.
pub fn libcomp_show(p: &Project, target: &str) -> Result<Value> {
    let (alias, id) = self::target(target, ObjKind::Libcomp)?;
    comp_json(p.library(&alias)?, &id)
}

/// `libcomp.list`: every library component of a library (this project's
/// by default).
pub fn libcomp_list(p: &Project, library: Option<&str>) -> Result<Value> {
    let lib = p.library(library.unwrap_or(SELF))?;
    let list: Vec<Value> = lib
        .components
        .iter()
        .map(|(id, l)| {
            let mut v = json!({
                "id": id,
                "reference": ref_text(&lib.alias, ObjKind::Libcomp, id),
                "faults": l.faults.len(),
            });
            match &l.item {
                Ok(c) => {
                    v["prefix"] = json!(c.prefix);
                    v["value"] = json!(c.value);
                    v["mpn"] = json!(c.mpn);
                    v["footprints"] = json!(c.footprints.len());
                    v["category"] = json!(c.category().0);
                }
                Err(e) => v["error"] = json!(e),
            }
            v
        })
        .collect();
    Ok(json!({"library": lib.alias, "revision": lib.revision, "libcomps": list}))
}

/// `libcomp.delete`: refused while a design places the component.
pub fn libcomp_delete(
    session: &mut Session,
    target: &str,
    expected: Option<&str>,
) -> Result<Value> {
    let p = session.project();
    let (alias, id) = self::target(target, ObjKind::Libcomp)?;
    let lib = writable(p, &alias)?;
    check_revision("library self", &lib.revision, expected)?;
    let at = ref_text(SELF, ObjKind::Libcomp, &id);
    if !lib.components.contains_key(&id) {
        return Err(
            Error::new(Code::NotFound, format!("{at} isn't in the library")).with("reference", &at),
        );
    }
    let placed = placed_by(p, &id);
    if !placed.is_empty() {
        return Err(Error::new(
            Code::HasDependents,
            format!(
                "{} place {at}; delete them or place another first",
                placed.join(", ")
            ),
        )
        .with("dependents", &placed));
    }
    let path = path_of(session.root(), &lib.path(ObjKind::Libcomp, &id));
    std::fs::remove_file(&path).map_err(|e| Error::io(&path, &e))?;
    session.reload()?;
    Ok(json!({
        "deleted": at,
        "revision": session.project().library(SELF)?.revision,
    }))
}

#[cfg(test)]
mod part_data_tests {
    use super::*;
    use crate::libcomp::Category;

    fn fields(v: Value) -> CompFields {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn fields_set_categories_and_alternates() {
        let mut c = crate::libcomp::tests::r_100k();
        let f = fields(json!({
            "category": "resistor",
            "add_alternates": [{"manufacturer": "Vishay", "mpn": "CRCW0402100KFKED"}],
            "alternate_offers": [{"mpn": "CRCW0402100KFKED", "supplier": "Mouser", "sku": "71-CRCW0402100KFKED"}]
        }));
        fill(&mut c, &f).unwrap();
        assert_eq!(c.category, Some(Category::Resistor));
        assert_eq!(c.alternates[0].offers.len(), 1);
        // The same again changes nothing.
        fill(&mut c, &f).unwrap();
        assert_eq!((c.alternates.len(), c.alternates[0].offers.len()), (1, 1));
        fill(
            &mut c,
            &fields(json!({"category": "", "remove_alternates": ["CRCW0402100KFKED"]})),
        )
        .unwrap();
        assert_eq!((c.category, c.alternates.len()), (None, 0));
        for (bad, code) in [
            (json!({"category": "valve"}), Code::InvalidParam),
            (json!({"remove_alternates": ["X"]}), Code::NotFound),
            (
                json!({"alternate_offers": [{"mpn": "X", "supplier": "LCSC", "sku": "C1"}]}),
                Code::NotFound,
            ),
        ] {
            assert_eq!(
                fill(&mut c, &fields(bad.clone())).unwrap_err().code,
                code,
                "{bad}"
            );
        }
    }

    /// A design project whose library holds the test library and `c_1u`,
    /// a 0402 capacitor drawn with symbol `r` on footprint
    /// `r_0402_1005metric`, open in a session.
    pub(crate) fn session_with_capacitor() -> (tempfile::TempDir, Session) {
        use crate::project::tests::{library_in, manifest};
        let dir = tempfile::tempdir().unwrap();
        let board = dir.path().join("board");
        manifest(&board, crate::project::ProjectKind::Design, &[]);
        library_in(&board);
        let c = json!({
            "kind": crate::libcomp::KIND, "format": 2,
            "symbol": {"id": "r"}, "prefix": "C", "value": "1u", "category": "capacitor",
            "parameters": {"package": "0402"},
            "footprints": [{"footprint": {"id": "r_0402_1005metric"}}]
        });
        crate::source::write_atomic(
            &board.join("library/components/c_1u.json"),
            crate::canonical::to_string(&c),
        )
        .unwrap();
        let s = Session::open(&board).unwrap();
        (dir, s)
    }

    /// The fixture catalogue synced into a cache of its own.
    pub(crate) fn synced_catalogue() -> (tempfile::TempDir, crate::parts::Catalogues) {
        let dir = tempfile::tempdir().unwrap();
        crate::parts::tests::synced(dir.path());
        let cats = crate::parts::Catalogues::at(dir.path(), 5_000, crate::parts::MAX_AGE);
        (dir, cats)
    }

    #[test]
    fn a_component_is_made_from_a_record_with_its_identifiers_only() {
        let (_c, cats) = synced_catalogue();
        let (_d, mut s) = session_with_capacitor();
        let from = fields(json!({"from": "lcsc:C1525"}));
        let v = libcomp_create(&mut s, "100n", None, &from, None, &cats).unwrap();
        let c = &v["libcomp"];
        assert_eq!(
            (c["mpn"].as_str(), c["manufacturer"].as_str()),
            (Some("CL05B104KO5NNNC"), Some("Samsung Electro-Mechanics"))
        );
        assert_eq!(c["offers"], json!([{"supplier": "LCSC", "sku": "C1525"}]));
        assert_eq!(
            (c["category"].as_str(), c["prefix"].as_str()),
            (Some("capacitor"), Some("C"))
        );
        assert!(
            c.get("parameters").is_none(),
            "no source's parameters are written"
        );
        assert_eq!(
            v["from"]["parameters"]["capacitance"], "100 nF",
            "they are listed for the agent"
        );
        assert_eq!(v["from"]["attribution"], ryueda_parts::jlc::ATTRIBUTION);
        assert_eq!(v["from"]["key"], "jlc:C1525");
        assert_eq!(v["chose"]["symbol"]["reference"], "lib/self/symbol/r");
        assert_eq!(v["chose"]["symbol"]["uses"], 1);
        assert_eq!(
            v["chose"]["footprint"]["reference"],
            "lib/self/footprint/r_0402_1005metric"
        );
        assert_eq!(v["warnings"][0]["code"], "parameter_missing_from");
        // Given fields win, and typed parameters are written.
        let f2 = fields(json!({"from": "lcsc:C1525", "description": "Mine",
                               "parameters": {"capacitance": "100 nF"}}));
        let v = libcomp_create(&mut s, "100n b", None, &f2, None, &cats).unwrap();
        assert_eq!(v["libcomp"]["description"], "Mine");
        assert_eq!(v["libcomp"]["parameters"]["capacitance"], "100 nF");
        assert!(v["warnings"].as_array().unwrap().is_empty());
        let e = libcomp_create(
            &mut s,
            "x",
            None,
            &fields(json!({"from": "lcsc:C9"})),
            None,
            &cats,
        )
        .unwrap_err();
        assert_eq!(e.code, Code::PartsNotFound);
        // Nothing of the regulator's category and package to take a symbol from.
        let e = libcomp_create(
            &mut s,
            "ldo",
            None,
            &fields(json!({"from": "lcsc:C5446"})),
            None,
            &cats,
        )
        .unwrap_err();
        assert_eq!(
            (e.code, &e.details["param"]),
            (Code::InvalidParam, &json!("symbol"))
        );
        // Without --from a symbol is still needed.
        let e = libcomp_create(&mut s, "y", None, &CompFields::default(), None, &cats).unwrap_err();
        assert_eq!(e.details["param"], "symbol");
    }

    #[test]
    fn edit_from_reapplies_only_identifiers_that_differ() {
        let (_c, cats) = synced_catalogue();
        let (_d, mut s) = session_with_capacitor();
        let f = fields(json!({"symbol": "r", "prefix": "C", "mpn": "OLD",
                              "parameters": {"capacitance": "100 nF"}}));
        libcomp_create(&mut s, "cap", Some("cap"), &f, None, &cats).unwrap();
        let from = fields(json!({"from": "lcsc:C1525"}));
        let v = libcomp_edit(&mut s, "cap", &from, None, &cats).unwrap();
        assert_eq!(v["libcomp"]["mpn"], "CL05B104KO5NNNC");
        assert_eq!(
            v["libcomp"]["parameters"]["capacitance"], "100 nF",
            "parameters stay the agent's"
        );
        assert_eq!(
            v["changed"],
            json!(["datasheet", "description", "manufacturer", "mpn", "offers"])
        );
        assert_eq!(v["from"]["attribution"], ryueda_parts::jlc::ATTRIBUTION);
        let revision = s.project().library(SELF).unwrap().revision.clone();
        let again = libcomp_edit(&mut s, "cap", &from, None, &cats).unwrap();
        assert_eq!(again["changed"], json!([]));
        assert_eq!(
            s.project().library(SELF).unwrap().revision,
            revision,
            "nothing written"
        );
        // A field given wins over the record's.
        let mine = fields(json!({"from": "lcsc:C1525", "description": "Mine"}));
        let v = libcomp_edit(&mut s, "cap", &mine, None, &cats).unwrap();
        assert_eq!(
            (v["libcomp"]["description"].as_str(), &v["changed"]),
            (Some("Mine"), &json!([]))
        );
    }

    #[test]
    fn edit_from_keeps_other_suppliers_offers() {
        let (_c, cats) = synced_catalogue();
        let (_d, mut s) = session_with_capacitor();
        let f = fields(json!({"symbol": "r", "prefix": "C",
                              "offers": [{"supplier": "DigiKey", "sku": "1276-1000-1-ND"},
                                         {"supplier": "LCSC", "sku": "C1"}]}));
        libcomp_create(&mut s, "cap", Some("cap"), &f, None, &cats).unwrap();
        let v = libcomp_edit(
            &mut s,
            "cap",
            &fields(json!({"from": "lcsc:C1525"})),
            None,
            &cats,
        )
        .unwrap();
        let offers = &v["libcomp"]["offers"];
        assert_eq!(offers[0]["supplier"], "DigiKey", "{offers}");
        assert_eq!(offers[0]["sku"], "1276-1000-1-ND");
        let lcsc: Vec<_> = offers
            .as_array()
            .unwrap()
            .iter()
            .filter(|o| o["supplier"].as_str().unwrap().eq_ignore_ascii_case("lcsc"))
            .collect();
        assert_eq!(lcsc.len(), 1, "{offers}");
        assert_eq!(lcsc[0]["sku"], "C1525");
        assert!(
            !v["changed"]
                .as_array()
                .unwrap()
                .contains(&json!("category")),
            "the prefix already makes it a capacitor"
        );
    }

    #[test]
    fn merged_offers_replace_only_the_records_suppliers() {
        use ryueda_parts::record::Offer;
        let o = |s: &str, k: &str| Offer {
            supplier: s.into(),
            sku: k.into(),
        };
        let have = vec![
            o("Mouser", "m"),
            o("lcsc", "C1"),
            o("LCSC ", "C2"),
            o("DigiKey", "d"),
        ];
        assert_eq!(
            merged_offers(&have, &[o("LCSC", "C9")]),
            vec![o("Mouser", "m"), o("LCSC", "C9"), o("DigiKey", "d")]
        );
        assert_eq!(
            merged_offers(&[o("Mouser", "m")], &[o("LCSC", "C9")]),
            vec![o("Mouser", "m"), o("LCSC", "C9")]
        );
    }

    #[test]
    fn a_distributors_part_comes_from_its_kept_record() {
        let (cfg, cache) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let live = crate::distributors::tests::fake(
            cfg.path(),
            cache.path(),
            std::sync::Arc::new(crate::distributors::NoNetwork),
            1_000,
        );
        let cats = crate::parts::Catalogues::at(cache.path(), 1_000, crate::parts::MAX_AGE)
            .with_live(live.clone());
        let (_p, p) = crate::netlist::tests::project(&[]);
        let e = record_from(&p, "digikey:1276-1001-1-ND", &cats).unwrap_err();
        assert_eq!(
            (e.code, e.details["command"].as_str()),
            (
                Code::PartsNotFound,
                Some("ryueda parts show digikey:1276-1001-1-ND")
            )
        );
        let r = ryueda_parts::digikey::record::record(
            &ryueda_parts::digikey::testing::product(ryueda_parts::digikey::testing::CAP),
            None,
            900,
        )
        .unwrap();
        live.keep(
            ryueda_parts::distributor::distributor("digikey").unwrap(),
            &[r],
        );
        let r = record_from(&p, "digikey:1276-1001-1-ND", &cats).unwrap();
        assert_eq!(
            (r.mpn.as_str(), r.offers[0].supplier.as_str()),
            ("CL05B104KO5NNNC", "DigiKey")
        );
    }
}
