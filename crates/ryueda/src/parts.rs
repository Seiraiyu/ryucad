//! `parts.search` (design 347): parts found by category and typed
//! parameters. This plan searches the project's own library and its
//! aliases' library components, from files alone, so an answer is the
//! same on every machine; 6.B and 6.C add outside sources behind the same
//! request (`in`).

use crate::error::{Code, Error, Result};
use crate::fault::{self, Fault, Severity};
use crate::libcomp::{Category, LibComp};
use crate::library::ObjKind;
use crate::project::{Project, SELF, ref_text};
use ryueda_parts::library::Library;
use ryueda_parts::query::{Condition, Hit, LIMIT, Query};
use ryueda_parts::record::Record;
use ryueda_parts::source::{Call, Source};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// The sources this ryueda searches: the library, each catalogue, then
/// each distributor.
pub fn sources_known() -> Vec<&'static str> {
    std::iter::once(Library::NAME)
        .chain(catalogues())
        .chain(crate::distributors::names())
        .collect()
}

/// `parts.max_age`'s default: 7 days.
pub const MAX_AGE: u64 = 7 * 24 * 3600;

type Shared = ryueda_parts::catalogue::Shared;

/// This machine's catalogues as a request reads them: where the parts
/// cache is, now, and `parts.max_age`. Cheap to clone (a loaded catalogue
/// is shared); `Default` reads no catalogue at all.
#[derive(Clone, Default)]
pub struct Catalogues {
    /// The parts cache; `None` for none.
    pub root: Option<std::path::PathBuf>,
    pub now: u64,
    pub max_age: u64,
    /// Each connector's catalogue once opened, by name (`None`: not synced).
    loaded: std::sync::Arc<std::sync::Mutex<BTreeMap<&'static str, Option<Shared>>>>,
    /// This machine's distributors (6.C): `Default` has no keys, no
    /// kept records and no network.
    pub live: crate::distributors::Distributors,
}

impl Catalogues {
    pub fn at(root: &std::path::Path, now: u64, max_age: u64) -> Catalogues {
        Catalogues {
            root: Some(root.to_path_buf()),
            now,
            max_age,
            loaded: Default::default(),
            live: Default::default(),
        }
    }

    /// This, with `live` its distributors.
    pub fn with_live(self, live: crate::distributors::Distributors) -> Catalogues {
        Catalogues { live, ..self }
    }

    /// The machine's: the parts cache, now, and `parts.max_age` from the
    /// preferences (or `RYUEDA_PARTS_MAX_AGE`).
    /// With no cache folder to be found (no home), no catalogue.
    pub fn here() -> Result<Catalogues> {
        let live = crate::distributors::Distributors::here();
        Ok(match cache_root() {
            Ok(root) => Catalogues::at(&root, now(), max_age()).with_live(live),
            Err(_) => Catalogues {
                now: now(),
                max_age: max_age(),
                live,
                ..Default::default()
            },
        })
    }

    /// This, as of `now`, letting go of each catalogue synced again since
    /// it was opened (reading only what says when).
    pub fn refreshed(&self, now: u64) -> Catalogues {
        let mut loaded = self.loaded.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(root) = &self.root {
            loaded.retain(|name, c| {
                let Some(conn) = ryueda_parts::catalogue::connector(name) else {
                    return false;
                };
                (conn.fetched_at)(&root.join(conn.name)) == c.as_ref().map(|c| c.fetched_at())
            });
        }
        drop(loaded);
        Catalogues {
            now,
            ..self.clone()
        }
    }

    /// Connector `name`'s catalogue, if this machine has synced it.
    pub fn get(&self, name: &str) -> Result<Option<Shared>> {
        let Some(conn) = ryueda_parts::catalogue::connector(name) else {
            return Ok(None);
        };
        let mut loaded = self.loaded.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(c) = loaded.get(conn.name) {
            return Ok(c.clone());
        }
        let c = match &self.root {
            Some(r) => (conn.open)(&r.join(conn.name))?,
            None => None,
        };
        loaded.insert(conn.name, c.clone());
        Ok(c)
    }

    /// The synced catalogues whose keys an offer of `supplier` names.
    pub fn for_supplier(&self, supplier: &str) -> Result<Vec<Shared>> {
        let mut out = Vec::new();
        for conn in ryueda_parts::catalogue::for_supplier(supplier) {
            out.extend(self.get(conn.name)?);
        }
        Ok(out)
    }

    /// A catalogue's state (`{name, kind, state, fetched_at, age_s, stale,
    /// parts, created, attribution, sync}`), and a `parts_cache_stale`
    /// warning where it is missing or past `max_age`.
    pub fn state(&self, name: &str) -> Result<(Value, Option<Value>)> {
        let sync = format!("ryueda parts sync {name}");
        let attribution = ryueda_parts::catalogue::connector(name).map_or("", |c| c.attribution);
        let Some(c) = self.get(name)? else {
            return Ok((
                json!({"name": name, "kind": "catalogue", "state": "missing", "sync": sync}),
                Some(json!({
                    "code": Code::PartsCacheStale, "source": name, "reason": "missing",
                    "message": format!("this machine has no {name} catalogue; run {sync}"),
                    "command": sync,
                })),
            ));
        };
        let age = self.now.saturating_sub(c.fetched_at());
        let stale = age > self.max_age;
        let v = json!({
            "name": name, "kind": "catalogue", "state": "synced",
            "fetched_at": c.fetched_at(), "age_s": age, "stale": stale, "parts": c.parts(),
            "created": c.created(), "attribution": attribution, "sync": sync,
        });
        let warn = stale.then(|| {
            json!({
                "code": Code::PartsCacheStale, "source": name, "reason": "old",
                "age_s": age, "max_age_s": self.max_age,
                "message": format!("the {name} catalogue is {} days old; run {sync}", age / 86_400),
                "command": sync,
            })
        });
        Ok((v, warn))
    }
}

/// `parts.max_age` (seconds): `RYUEDA_PARTS_MAX_AGE`, else the
/// preferences, else [`MAX_AGE`].
pub fn max_age() -> u64 {
    std::env::var("RYUEDA_PARTS_MAX_AGE")
        .ok()
        .and_then(|v| v.parse().ok())
        .or_else(|| {
            ryu_session::prefs::Preferences::<crate::Eda>::load()
                .ok()?
                .values
                .get("parts.max_age")?
                .as_u64()
        })
        .unwrap_or(MAX_AGE)
}

/// The most results an answer gives, however many are asked for.
pub const MAX_LIMIT: usize = 1000;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Search {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    /// `name=value`, `name>=value`, `name<=value`, `name~text`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conditions: Vec<String>,
    /// Which sources, in order (`library` by default).
    #[serde(default, rename = "in", skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stock: Option<u64>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub basic: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
    /// A library component (`<id>`, `lib/<alias>/libcomp/<id>`) or a placed
    /// one (`<block>/<component>`) whose category and typed parameters to
    /// search with; conditions given replace its own of the same name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub like: Option<String>,
    /// Stay off the network: the library never fetches, so it changes
    /// nothing yet.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub offline: bool,
}

/// A library component as a record of the `library` source: its
/// identifiers, category (inferred where left out), parameters and
/// offers, and no supply fields.
pub fn record(alias: &str, id: &crate::id::Id, c: &LibComp) -> Record {
    let at = ref_text(alias, ObjKind::Libcomp, id);
    let mut r = Record::new(Library::NAME, &at);
    r.component = Some(at);
    r.manufacturer = c.manufacturer.clone();
    r.mpn = c.mpn.clone();
    r.description = c.description.clone();
    r.datasheet = c.datasheet.clone();
    r.category = Some(c.category().0);
    r.parameters = c.parameters.clone();
    r.offers = c.offers.clone();
    r
}

/// Every library component of every library that loaded, and the faults
/// of the libraries and components that didn't.
fn library_records(p: &Project) -> (Vec<Record>, Vec<Fault>) {
    let (mut out, mut faults) = (Vec::new(), Vec::new());
    for (alias, lib) in &p.libraries {
        let lib = match lib {
            Ok(l) => l,
            Err(e) => {
                faults.push(Fault {
                    severity: Severity::Error,
                    error: e.clone().with("alias", alias),
                });
                continue;
            }
        };
        for (id, l) in &lib.components {
            match &l.item {
                Ok(c) => out.push(record(alias, id, c)),
                Err(e) => faults.push(Fault {
                    severity: Severity::Error,
                    error: e.clone(),
                }),
            }
        }
    }
    (out, faults)
}

fn category(text: &str) -> Result<Category> {
    Category::parse(text).ok_or_else(|| {
        Error::new(
            Code::InvalidParam,
            format!("{text} isn't a category: {}", Category::names().join(", ")),
        )
        .with("param", "category")
    })
}

/// What `like` searches with: the component's category, and each
/// vocabulary parameter it gives a value that reads (a placed one's own
/// over its library component's), as `name=value`.
fn like_of(p: &Project, text: &str) -> Result<(Category, Vec<Condition>)> {
    let not_found = |what: String| Error::new(Code::NotFound, what).with("reference", text);
    let (c, own): (&LibComp, Option<&BTreeMap<String, String>>) =
        if text.starts_with("lib/") || !text.contains('/') {
            let (alias, id) = crate::libcmd::target(text, ObjKind::Libcomp)?;
            let l = p
                .library(&alias)?
                .components
                .get(&id)
                .ok_or_else(|| not_found(format!("{text} isn't in the library")))?;
            (
                l.item
                    .as_ref()
                    .map_err(|e| e.clone().with("reference", text))?,
                None,
            )
        } else {
            let (b, id) = crate::write::in_block(p, text, "component")?;
            let placed = b
                .components
                .get(&id)
                .ok_or_else(|| not_found(format!("block {} has no component {id}", b.id)))?
                .item
                .as_ref()
                .map_err(|e| e.clone().with("reference", text))?;
            (p.libcomp(SELF, &placed.of)?, Some(&placed.parameters))
        };
    let mut params = c.parameters.clone();
    if let Some(own) = own {
        params.extend(own.iter().map(|(k, v)| (k.clone(), v.clone())));
    }
    let conditions = params
        .iter()
        .filter(|(k, v)| matches!(ryueda_parts::param::read(k, v), Some(Ok(_))))
        .map(|(k, v)| Condition::parse(&format!("{k}={v}")))
        .collect::<Result<_>>()?;
    Ok((c.category().0, conditions))
}

/// A search worked out under the lock: the query, the sources in order,
/// what the library and catalogues found, and which distributors are
/// still to ask (called by [`finish`], with the lock released).
pub struct Planned {
    pub q: Query,
    query: Value,
    sources: Vec<String>,
    found: BTreeMap<String, Vec<Hit>>,
    states: Vec<Value>,
    warnings: Vec<Value>,
    notes: Vec<Value>,
    faults: Value,
    offline: bool,
}

impl Planned {
    /// The distributors it is to ask (none when offline).
    pub fn live(&self) -> Vec<&'static ryueda_parts::distributor::Distributor> {
        if self.offline {
            return Vec::new();
        }
        self.sources
            .iter()
            .filter_map(|n| ryueda_parts::distributor::distributor(n))
            .collect()
    }
}

/// `parts.search`'s work under the lock: everything but the distributors.
pub fn plan(p: &Project, s: &Search, cats: &Catalogues) -> Result<Planned> {
    let known = sources_known();
    let default = default_sources(p);
    let asked: Vec<&str> = if s.sources.is_empty() {
        default.iter().map(String::as_str).collect()
    } else {
        s.sources.iter().map(String::as_str).collect()
    };
    let mut sources: Vec<String> = Vec::new();
    for name in asked {
        if !known.contains(&name) {
            return Err(Error::new(
                Code::InvalidParam,
                format!(
                    "{name} isn't a source this ryueda searches: {}",
                    known.join(", ")
                ),
            )
            .with("param", "in")
            .with("sources", &known));
        }
        if !sources.iter().any(|x| x == name) {
            sources.push(name.to_owned());
        }
    }
    // The library first: its answers are the project's own.
    sources.sort_by_key(|n| n != Library::NAME);
    let mut cat = s.category.as_deref().map(category).transpose()?;
    let mut conditions: Vec<Condition> = s
        .conditions
        .iter()
        .map(|t| Condition::parse(t))
        .collect::<Result<_>>()?;
    if let Some(like) = &s.like {
        let (c, from) = like_of(p, like)?;
        cat.get_or_insert(c);
        for x in from {
            if !conditions.iter().any(|y| y.name == x.name) {
                conditions.push(x);
            }
        }
    }
    let q = Query {
        category: cat,
        conditions,
        stock: s.stock,
        basic: s.basic,
        limit: s.limit.unwrap_or(LIMIT).clamp(1, MAX_LIMIT),
    };
    let (records, faults) = library_records(p);
    let library = Library::new(records);
    let mut found = BTreeMap::new();
    let (mut states, mut warnings) = (Vec::new(), Vec::new());
    for name in &sources {
        if name == Library::NAME {
            found.insert(name.clone(), library.search(&q, &Call::default())?);
            states.push(json!({"name": Library::NAME, "kind": "files"}));
        } else if let Some(d) = ryueda_parts::distributor::distributor(name) {
            states.push(cats.live.state(d, cats.now));
        } else {
            let (state, warn) = cats.state(name)?;
            states.push(state);
            warnings.extend(warn);
            if let Some(c) = cats.get(name)? {
                found.insert(name.clone(), c.search(&q, &Call::default())?);
            }
        }
    }
    let query = json!({
        "category": q.category,
        "conditions": q.conditions.iter().map(Condition::to_text).collect::<Vec<_>>(),
        "in": sources,
        "stock": q.stock,
        "basic": q.basic,
        "limit": q.limit,
        "like": s.like,
    });
    Ok(Planned {
        q,
        query,
        sources,
        found,
        states,
        warnings,
        notes: Vec::new(),
        faults: fault::list(&faults),
        offline: s.offline,
    })
}

/// A distributor's failure as a search's or show's warning: its code,
/// message and details (never a key). A cancel isn't one.
fn failed(name: &str, e: Error) -> Result<Value> {
    if e.code == Code::Cancelled {
        return Err(e);
    }
    Ok(json!({"code": e.code, "source": name, "message": e.message, "details": e.details}))
}

/// `parts.search`'s distributors, called under `job` (a source a step in
/// phase `fetch`), then the answer: each source's results in order, at
/// most `limit` in all. A distributor that fails, or has no keys, is a
/// warning; the others answer.
pub fn finish(mut pl: Planned, cats: &Catalogues, job: &ryu_session::job::Job) -> Result<Value> {
    let live = pl.live();
    if pl.offline {
        for name in &pl.sources {
            if ryueda_parts::distributor::distributor(name).is_some() {
                pl.notes.push(json!({"source": name, "reason": "offline",
                    "message": format!("{name} is searched live; --offline left it out")}));
            }
        }
    }
    let stage = (!live.is_empty())
        .then(|| job.begin(ryu_session::job::Phase::Fetch, "sources", live.len()));
    let call = Call {
        cancel: job.kernel_cancel(),
        deadline: None,
    };
    for d in live {
        if let Some(s) = &stage {
            s.step(|| d.name.to_owned())?;
        }
        match cats.live.open(d) {
            Ok(Some(src)) => match src.search(&pl.q, &call) {
                Ok(hits) => {
                    let records: Vec<Record> = hits.iter().map(|h| h.record.clone()).collect();
                    cats.live.keep(d, &records);
                    pl.found.insert(d.name.to_owned(), hits);
                }
                Err(e) => pl.warnings.push(failed(d.name, e)?),
            },
            Ok(None) => pl
                .warnings
                .push(failed(d.name, crate::distributors::not_configured(d))?),
            Err(e) => pl.warnings.push(failed(d.name, e)?),
        }
    }
    drop(stage);
    let mut results: Vec<Hit> = Vec::new();
    for name in &pl.sources {
        results.extend(pl.found.remove(name).unwrap_or_default());
    }
    results.truncate(pl.q.limit);
    let mut v = json!({
        "query": pl.query,
        "results": results,
        "sources": pl.states,
        "warnings": pl.warnings,
        "faults": pl.faults,
    });
    if !pl.notes.is_empty() {
        v["notes"] = json!(pl.notes);
    }
    Ok(v)
}

/// `parts.search` in one go: the asked sources' results in order, each
/// source's ranked by conditions met, stock and key, at most `limit` in
/// all, with the faults of what didn't load.
pub fn search(p: &Project, s: &Search, cats: &Catalogues) -> Result<Value> {
    finish(plan(p, s, cats)?, cats, &ryu_session::job::Job::default())
}

/// The catalogues the placed parts' offers draw on (a connector claims an
/// offer by its supplier), over every design that flattens, by name.
pub fn catalogues_placed(p: &Project) -> Vec<&'static str> {
    let mut used = std::collections::BTreeSet::new();
    for design in p.conn.designs.keys() {
        let Ok(n) = crate::netlist::flatten(p, design) else {
            continue;
        };
        for (_, c, _) in crate::erc::placed_by_libcomp(p, &n) {
            for o in c
                .offers
                .iter()
                .chain(c.alternates.iter().flat_map(|a| &a.offers))
            {
                used.extend(ryueda_parts::catalogue::for_supplier(&o.supplier).map(|k| k.name));
            }
        }
    }
    used.into_iter().collect()
}

/// `parts.sources`: every source this ryueda has, the library's count of
/// library components and each catalogue's state on this machine, with
/// the parts cache's folder (`null` where there is none).
pub fn sources(p: &Project, cats: &Catalogues) -> Result<Value> {
    let (records, faults) = library_records(p);
    let mut list = vec![json!({
        "name": Library::NAME, "kind": "files",
        "components": records.len(), "faults": faults.len(),
    })];
    for name in catalogues() {
        list.push(cats.state(name)?.0);
    }
    for d in ryueda_parts::distributor::DISTRIBUTORS {
        list.push(cats.live.state(d, cats.now));
    }
    Ok(json!({"sources": list, "max_age_s": cats.max_age, "cache": cats.root}))
}

/// The sources a key's `<source>:` names: `library`; a catalogue or a
/// distributor, by its name or a supplier it claims (so `lcsc:` names the
/// catalogue keyed by LCSC numbers); or `mpn` for every source.
fn named(source: &str) -> Option<Vec<&'static str>> {
    match source {
        "library" => Some(vec![Library::NAME]),
        "mpn" => Some(sources_known()),
        s => {
            let found: Vec<&'static str> = ryueda_parts::catalogue::for_supplier(s)
                .map(|c| c.name)
                .chain(ryueda_parts::distributor::for_supplier(s).map(|d| d.name))
                .collect();
            (!found.is_empty()).then_some(found)
        }
    }
}

/// Every record `key` names, in source order, the warnings of catalogues
/// missing or stale, and the sources searched.
pub fn records_of(
    p: &Project,
    key: &str,
    cats: &Catalogues,
) -> Result<(Vec<Record>, Vec<Value>, Vec<&'static str>)> {
    let k = ryueda_parts::source::Key::parse(key)?;
    let names = named(&k.source).ok_or_else(|| {
        Error::new(
            Code::InvalidParam,
            format!(
                "{} isn't a source to look parts up in: library, mpn, a catalogue or a distributor ({})",
                k.source,
                catalogues()
                    .into_iter()
                    .chain(crate::distributors::names())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        )
        .with("param", "key")
        .with("key", key)
    })?;
    let (mut out, mut warnings) = (Vec::new(), Vec::new());
    for name in &names {
        if *name == Library::NAME {
            let want = ryueda_parts::record::squash(&k.key);
            out.extend(library_records(p).0.into_iter().filter(|r| {
                if k.is_mpn() {
                    !r.mpn.is_empty() && ryueda_parts::record::squash(&r.mpn) == want
                } else {
                    r.key == k.key
                }
            }));
            continue;
        }
        // A distributor's records as kept on this machine: calling it is
        // `parts show`'s, with the lock released.
        if let Some(d) = ryueda_parts::distributor::distributor(name) {
            if !k.is_mpn() {
                out.extend(cats.live.cached(d, &k.key, cats.now));
            }
            continue;
        }
        let (_, warn) = cats.state(name)?;
        warnings.extend(warn);
        if let Some(c) = cats.get(name)? {
            if k.is_mpn() {
                out.extend(c.by_mpn(&k.key)?);
            } else {
                out.extend(c.get(k.key.trim(), &Call::default())?);
            }
        }
    }
    Ok((out, warnings, names))
}

/// `parts.show`: the records `key` names, from this machine's sources. A
/// catalogue refreshes by `parts sync`, so `refresh` only says so.
pub fn show(p: &Project, key: &str, refresh: bool, cats: &Catalogues) -> Result<Value> {
    let (records, warnings, searched) = records_of(p, key, cats)?;
    if records.is_empty() {
        return Err(
            Error::new(Code::PartsNotFound, format!("no source here has {key}"))
                .with("key", key)
                .with("searched", &searched)
                .with("warnings", &warnings),
        );
    }
    let notes: Vec<Value> = if refresh {
        searched
            .iter()
            .filter(|n| **n != Library::NAME)
            .map(|n| {
                json!({"source": n, "reason": "catalogue",
                       "message": format!("{n} is a catalogue: run ryueda parts sync {n} to refresh it")})
            })
            .collect()
    } else {
        Vec::new()
    };
    Ok(json!({"key": key, "records": records, "warnings": warnings, "notes": notes}))
}

/// The sources a search without `in` asks: `parts.json`'s, else the
/// library.
fn default_sources(p: &Project) -> Vec<String> {
    let c = crate::sourcing::config(p);
    if c.sources.is_empty() {
        vec![Library::NAME.to_owned()]
    } else {
        c.sources
    }
}

/// UTC seconds now; `RYUEDA_NOW` sets it, for tests.
pub fn now() -> u64 {
    if let Some(n) = std::env::var("RYUEDA_NOW")
        .ok()
        .and_then(|v| v.parse().ok())
    {
        return n;
    }
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// The parts cache's folder on this machine.
pub fn cache_root() -> Result<std::path::PathBuf> {
    Ok(ryueda_parts::cache::Cache::open()?.root().to_path_buf())
}

/// Every catalogue this ryueda can sync: each connector's name.
pub fn catalogues() -> Vec<&'static str> {
    ryueda_parts::catalogue::CONNECTORS
        .iter()
        .map(|c| c.name)
        .collect()
}

/// Where connector `name` downloads from (`""` for none).
pub fn base_of(name: &str) -> String {
    ryueda_parts::catalogue::connector(name).map_or_else(String::new, |c| (c.base)())
}

/// `parts.sync` of catalogue `source` from `base` through `fetch` into the
/// parts cache at `cache`, as of `now`, under `job` (phase `fetch`, a file
/// a step). A sync stopped or failed leaves the catalogue there was.
pub fn sync_with(
    fetch: &dyn ryueda_parts::fetch::Fetch,
    base: &str,
    cache: &std::path::Path,
    source: &str,
    now: u64,
    job: &ryu_session::job::Job,
) -> Result<Value> {
    let Some(conn) = ryueda_parts::catalogue::connector(source) else {
        return Err(Error::new(
            Code::InvalidParam,
            format!(
                "{source} isn't a catalogue to sync: {}",
                catalogues().join(", ")
            ),
        )
        .with("param", "source")
        .with("catalogues", catalogues()));
    };
    let call = Call {
        cancel: job.kernel_cancel(),
        deadline: None,
    };
    let mut stage: Option<ryu_session::job::Stage<'_>> = None;
    let out = (conn.sync)(
        fetch,
        base,
        &cache.join(conn.name),
        now,
        &call,
        &mut |_, total, item| {
            if stage.is_none() && total > 0 {
                stage = Some(job.begin(ryu_session::job::Phase::Fetch, "files", total));
            }
            match &stage {
                Some(s) => s.step(|| item.to_owned()),
                None => job.check(),
            }
        },
    )?;
    drop(stage);
    let mut v = serde_json::to_value(&out).expect("a sync's answer serializes");
    v["source"] = json!(conn.name);
    v["attribution"] = json!(conn.attribution);
    Ok(v)
}

/// `parts.sync` from the connector's own site over HTTPS, into this
/// machine's parts cache.
pub fn sync(source: &str, job: &ryu_session::job::Job) -> Result<Value> {
    sync_with(
        &crate::net::Https::new(),
        &base_of(source),
        &cache_root()?,
        source,
        now(),
        job,
    )
}

/// `parts.refresh` of catalogues `names`: each synced again (from `base`,
/// else its own site) when missing, past `max_age` or `force`d; the rest
/// listed as fresh.
#[allow(clippy::too_many_arguments)]
pub fn refresh_these(
    names: &[&str],
    fetch: &dyn ryueda_parts::fetch::Fetch,
    base: &str,
    cache: &std::path::Path,
    now: u64,
    max_age: u64,
    force: bool,
    job: &ryu_session::job::Job,
) -> Result<Value> {
    let cats = Catalogues::at(cache, now, max_age);
    let (mut synced, mut fresh) = (Vec::new(), Vec::new());
    for name in names {
        let (state, stale) = cats.state(name)?;
        if force || stale.is_some() {
            let from = if base.is_empty() {
                base_of(name)
            } else {
                base.to_owned()
            };
            synced.push(sync_with(fetch, &from, cache, name, now, job)?);
        } else {
            fresh.push(
                json!({"source": name, "fetched_at": state["fetched_at"], "age_s": state["age_s"]}),
            );
        }
    }
    Ok(json!({"synced": synced, "fresh": fresh, "force": force}))
}

/// `parts.refresh` of the catalogues the placed parts' offers draw on.
#[allow(clippy::too_many_arguments)]
pub fn refresh_with(
    p: &Project,
    fetch: &dyn ryueda_parts::fetch::Fetch,
    base: &str,
    cache: &std::path::Path,
    now: u64,
    max_age: u64,
    force: bool,
    job: &ryu_session::job::Job,
) -> Result<Value> {
    refresh_these(
        &catalogues_placed(p),
        fetch,
        base,
        cache,
        now,
        max_age,
        force,
        job,
    )
}

/// `parts.refresh` on this machine: each connector from its own site.
pub fn refresh(names: &[&str], force: bool, job: &ryu_session::job::Job) -> Result<Value> {
    refresh_these(
        names,
        &crate::net::Https::new(),
        "",
        &cache_root()?,
        now(),
        max_age(),
        force,
        job,
    )
}

/// The lock-free work of a long parts request, if `req` is one.
pub fn prepare(
    session: &mut crate::session::Session,
    req: &crate::proto::Request,
    job: &ryu_session::job::Job,
) -> Option<Box<dyn FnOnce() -> Result<Value> + Send>> {
    match req {
        crate::proto::Request::PartsSync { source } => {
            let (source, job) = (source.clone(), job.clone());
            Some(Box::new(move || sync(&source, &job)))
        }
        // What the placed parts draw on is found under the lock; the
        // syncs run without it.
        crate::proto::Request::PartsRefresh { force } => {
            let names = catalogues_placed(session.project());
            let (force, job) = (*force, job.clone());
            Some(Box::new(move || refresh(&names, force, &job)))
        }
        // The library and catalogues are searched under the lock; the
        // distributors are called without it.
        crate::proto::Request::PartsSearch { search } => {
            let cats = session.catalogues().ok()?;
            let planned = match plan(session.project(), search, &cats) {
                Ok(pl) if !pl.live().is_empty() => pl,
                // Nothing to call, or a refusal: answered under the lock.
                _ => return None,
            };
            let job = job.clone();
            Some(Box::new(move || finish(planned, &cats, &job)))
        }
        _ => None,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn a_library_component_is_a_record_of_the_library() {
        let c = crate::libcomp::tests::r_100k();
        let r = record(SELF, &crate::id::Id::parse("r_100k", "c").unwrap(), &c);
        assert_eq!(
            (r.source.as_str(), r.key.as_str(), r.component.as_deref()),
            (
                "library",
                "lib/self/libcomp/r_100k",
                Some("lib/self/libcomp/r_100k")
            )
        );
        assert_eq!(
            (r.category, r.mpn.as_str()),
            (Some(Category::Resistor), "RC0402FR-07100KL")
        );
        assert_eq!((r.stock, r.basic, r.fetched_at), (None, None, None));
        let v = serde_json::to_value(&r).unwrap();
        assert!(v.get("stock").is_none() && v.get("price_breaks").is_none());
    }

    #[test]
    fn searches_parse_their_request() {
        let r = crate::proto::Request::from_rpc(
            "parts.search",
            Some(json!({"category": "capacitor", "conditions": ["capacitance=100nF"], "in": ["library"], "limit": 5})),
        )
        .unwrap();
        let crate::proto::Request::PartsSearch { search } = r else {
            panic!("parts.search")
        };
        assert_eq!(search.sources, ["library"]);
        assert_eq!(search.limit, Some(5));
        let e = crate::proto::Request::from_rpc("parts.search", Some(json!({"colour": "red"})))
            .unwrap_err();
        assert_eq!(e.code, Code::InvalidDocument);
    }

    #[test]
    fn a_sync_fills_the_catalogue_and_reports_its_progress() {
        let dir = tempfile::tempdir().unwrap();
        let fake = ryueda_parts::jlc::testing::fake("2026-11-06T11:33:30+00:00");
        let base = ryueda_parts::jlc::testing::BASE;
        let (tx, rx) = std::sync::mpsc::channel();
        let tx = std::sync::Mutex::new(tx);
        let job = ryu_session::job::Job::new("t", "agent", "parts.sync", Default::default())
            .with_line(Box::new(move |v: &Value| {
                let _ = tx.lock().unwrap().send(v.clone());
            }))
            .unthrottled();
        let v = sync_with(&fake, base, dir.path(), "jlc", 5_000, &job).unwrap();
        assert_eq!(
            (
                v["source"].as_str(),
                v["parts"].as_u64(),
                v["files"].as_u64()
            ),
            (Some("jlc"), Some(5), Some(5))
        );
        assert_eq!(v["attribution"], ryueda_parts::jlc::ATTRIBUTION);
        assert_eq!(v["fetched_at"], 5_000);
        assert!(dir.path().join("jlc/current/meta.json").exists());
        let lines: Vec<Value> = rx.try_iter().collect();
        assert!(
            lines.iter().any(|l| {
                let t = l.to_string();
                t.contains("\"fetch\"") && t.contains("\"files\"")
            }),
            "{lines:?}"
        );
        let e = sync_with(&fake, base, dir.path(), "library", 5_000, &job).unwrap_err();
        assert_eq!(e.code, Code::InvalidParam);
        let stop = ryu_session::job::Job::cancelled_at(1);
        let e = sync_with(&fake, base, dir.path(), "jlc", 6_000, &stop).unwrap_err();
        assert_eq!(e.code, Code::Cancelled);
        let root = dir.path().join("jlc");
        assert_eq!(
            ryueda_parts::jlc::index::read(&root)
                .unwrap()
                .unwrap()
                .meta
                .fetched_at,
            5_000
        );
    }

    /// A catalogue synced from the fixture site into `dir`, at 5,000.
    pub(crate) fn synced(dir: &std::path::Path) {
        let fake = ryueda_parts::jlc::testing::fake("2026-11-06T11:33:30+00:00");
        sync_with(
            &fake,
            ryueda_parts::jlc::testing::BASE,
            dir,
            "jlc",
            5_000,
            &Default::default(),
        )
        .unwrap();
    }

    #[test]
    fn searches_take_the_catalogue_after_the_library_and_say_how_old_it_is() {
        let dir = tempfile::tempdir().unwrap();
        synced(dir.path());
        let (_p, p) = crate::netlist::tests::project(&[]);
        let fresh = Catalogues::at(dir.path(), 5_100, MAX_AGE);
        let s = Search {
            category: Some("capacitor".into()),
            conditions: vec!["capacitance=100nF".into()],
            sources: vec!["jlc".into(), "library".into()],
            stock: Some(10),
            ..Default::default()
        };
        let v = search(&p, &s, &fresh).unwrap();
        assert_eq!(
            v["query"]["in"],
            json!(["library", "jlc"]),
            "the library first"
        );
        assert_eq!(v["results"][0]["key"], "C1525");
        assert_eq!(
            v["results"][0]["attribution"],
            ryueda_parts::jlc::ATTRIBUTION
        );
        assert_eq!(v["sources"][0]["name"], "library");
        assert_eq!(v["sources"][1]["fetched_at"], 5_000);
        assert!(v["warnings"].as_array().unwrap().is_empty());
        let old = Catalogues::at(dir.path(), 5_000 + MAX_AGE + 1, MAX_AGE);
        let w = &search(&p, &s, &old).unwrap()["warnings"][0];
        assert_eq!(
            (w["code"].as_str(), w["reason"].as_str()),
            (Some("parts_cache_stale"), Some("old"))
        );
        let none = Catalogues::at(&dir.path().join("empty"), 5_000, MAX_AGE);
        let v = search(&p, &s, &none).unwrap();
        assert_eq!(v["warnings"][0]["reason"], "missing");
        assert_eq!(v["warnings"][0]["command"], "ryueda parts sync jlc");
        assert!(
            v["results"]
                .as_array()
                .unwrap()
                .iter()
                .all(|r| r["source"] == "library")
        );
        let e = search(
            &p,
            &Search {
                sources: vec!["acme".into()],
                ..Default::default()
            },
            &fresh,
        )
        .unwrap_err();
        assert_eq!(e.details["sources"], json!(["library", "jlc", "digikey"]));
        // The library alone, as 6.A searched: no catalogue read, no warning.
        let v = search(&p, &Search::default(), &Catalogues::default()).unwrap();
        assert!(v["warnings"].as_array().unwrap().is_empty());
        // A new sync is seen by a refreshed copy.
        let kept = fresh.clone();
        assert!(kept.get("jlc").unwrap().is_some());
        sync_with(
            &ryueda_parts::jlc::testing::fake("x"),
            ryueda_parts::jlc::testing::BASE,
            dir.path(),
            "jlc",
            7_000,
            &Default::default(),
        )
        .unwrap();
        let again = kept.refreshed(7_001);
        assert_eq!(again.state("jlc").unwrap().0["fetched_at"], 7_000);
    }

    #[test]
    fn sources_say_what_this_machine_has() {
        let dir = tempfile::tempdir().unwrap();
        let (_p, p) = crate::netlist::tests::project(&[]);
        let v = sources(&p, &Catalogues::at(dir.path(), 9, MAX_AGE)).unwrap();
        assert_eq!(v["sources"][0]["name"], "library");
        assert!(v["sources"][0]["components"].as_u64().unwrap() > 0);
        assert_eq!(
            (
                v["sources"][1]["name"].as_str(),
                v["sources"][1]["state"].as_str()
            ),
            (Some("jlc"), Some("missing"))
        );
        assert_eq!(v["max_age_s"], MAX_AGE);
        synced(dir.path());
        let v = sources(&p, &Catalogues::at(dir.path(), 5_009, MAX_AGE)).unwrap();
        assert_eq!(
            (
                v["sources"][1]["parts"].as_u64(),
                v["sources"][1]["age_s"].as_u64()
            ),
            (Some(5), Some(9))
        );
    }

    #[test]
    fn a_part_is_shown_by_key_or_part_number() {
        let dir = tempfile::tempdir().unwrap();
        synced(dir.path());
        let cats = Catalogues::at(dir.path(), 5_000, MAX_AGE);
        let (_p, p) = crate::netlist::tests::project(&[]);
        for key in ["lcsc:C1525", "jlc:C1525", "mpn:cl05b104ko5nnnc"] {
            let v = show(&p, key, false, &cats).unwrap();
            assert_eq!(v["records"][0]["mpn"], "CL05B104KO5NNNC", "{key}");
            assert_eq!(
                v["records"][0]["attribution"],
                ryueda_parts::jlc::ATTRIBUTION
            );
        }
        let e = show(&p, "lcsc:C9", false, &cats).unwrap_err();
        assert_eq!(
            (e.code, &e.details["searched"]),
            (Code::PartsNotFound, &json!(["jlc"]))
        );
        assert_eq!(
            show(&p, "acme:1-ND", false, &cats).unwrap_err().code,
            Code::InvalidParam
        );
        assert_eq!(
            show(&p, "C1525", false, &cats).unwrap_err().code,
            Code::InvalidParam
        );
        let v = show(&p, "lcsc:C1525", true, &cats).unwrap();
        assert_eq!(
            v["notes"][0]["reason"], "catalogue",
            "a catalogue refreshes by parts sync"
        );
        let none = Catalogues::at(&dir.path().join("x"), 5_000, MAX_AGE);
        let e = show(&p, "lcsc:C1525", false, &none).unwrap_err();
        assert_eq!(e.details["warnings"][0]["reason"], "missing");
        // The library by its key, and by a part number it names.
        let lib = library_records(&p).0;
        let first = lib
            .iter()
            .find(|r| !r.mpn.is_empty())
            .expect("a library component with a part number");
        let v = show(&p, &format!("library:{}", first.key), false, &cats).unwrap();
        assert_eq!(v["records"][0]["key"], first.key);
        let v = show(&p, &format!("mpn:{}", first.mpn), false, &cats).unwrap();
        assert_eq!(v["records"][0]["source"], "library");
    }

    #[test]
    fn refresh_syncs_only_what_the_placed_parts_need() {
        // The test library's r_100k, placed, has an LCSC offer.
        let (_d, p) = crate::netlist::tests::project(&[]);
        assert_eq!(catalogues_placed(&p), ["jlc"]);
        let cache = tempfile::tempdir().unwrap();
        let fake = ryueda_parts::jlc::testing::fake("2026-11-06T11:33:30+00:00");
        let base = ryueda_parts::jlc::testing::BASE;
        let job = ryu_session::job::Job::default();
        let v = refresh_with(&p, &fake, base, cache.path(), 5_000, MAX_AGE, false, &job).unwrap();
        assert_eq!(v["synced"][0]["source"], "jlc", "missing: synced");
        let v = refresh_with(&p, &fake, base, cache.path(), 5_100, MAX_AGE, false, &job).unwrap();
        assert_eq!(
            (
                v["synced"].as_array().unwrap().len(),
                v["fresh"][0]["source"].as_str()
            ),
            (0, Some("jlc"))
        );
        let v = refresh_with(
            &p,
            &fake,
            base,
            cache.path(),
            5_000 + MAX_AGE + 1,
            MAX_AGE,
            false,
            &job,
        )
        .unwrap();
        assert_eq!(
            v["synced"][0]["fetched_at"],
            5_000 + MAX_AGE + 1,
            "old: synced"
        );
        let v = refresh_with(
            &p,
            &fake,
            base,
            cache.path(),
            9_999_999,
            MAX_AGE,
            true,
            &job,
        )
        .unwrap();
        assert_eq!(v["synced"][0]["fetched_at"], 9_999_999, "--force");
        let bare_r = json!({"kind": "ryueda.libcomp", "format": 1, "symbol": {"id": "r"}, "prefix": "R",
                            "footprints": [{"footprint": {"id": "r_0402_1005metric"}}]});
        let (_e, bare) =
            crate::netlist::tests::project(&[("library/components/r_100k.json", bare_r)]);
        let v = refresh_with(&bare, &fake, base, cache.path(), 9_000, MAX_AGE, true, &job).unwrap();
        assert_eq!(
            v["synced"],
            json!([]),
            "nothing placed draws on a catalogue"
        );
    }

    /// A project, distributors over a fake DigiKey with keys, and the
    /// catalogues the search reads (none synced), at 1,000.
    fn live() -> (
        tempfile::TempDir,
        tempfile::TempDir,
        std::sync::Arc<ryueda_parts::fetch::Fake>,
        Catalogues,
    ) {
        let (cfg, cache) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        crate::distributors::tests::logged_in(cfg.path());
        let fake = std::sync::Arc::new(ryueda_parts::digikey::testing::fake());
        let ds = crate::distributors::tests::fake(cfg.path(), cache.path(), fake.clone(), 1_000);
        let cats = Catalogues::at(cache.path(), 1_000, MAX_AGE).with_live(ds);
        (cfg, cache, fake, cats)
    }

    #[test]
    fn a_distributor_is_searched_live_and_its_records_kept() {
        let (_c, _k, fake, cats) = live();
        let (_p, p) = crate::netlist::tests::project(&[]);
        let s = Search {
            category: Some("capacitor".into()),
            conditions: vec!["capacitance=100nF".into(), "voltage_rating>=10V".into()],
            sources: vec!["digikey".into(), "library".into()],
            ..Default::default()
        };
        let v = search(&p, &s, &cats).unwrap();
        assert_eq!(v["query"]["in"], json!(["library", "digikey"]));
        let dk: Vec<&Value> = v["results"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| r["source"] == "digikey")
            .collect();
        assert_eq!(dk.len(), 1);
        assert_eq!(
            (dk[0]["key"].as_str(), dk[0]["attribution"].as_str()),
            (Some("1276-1001-1-ND"), Some("Data from DigiKey"))
        );
        assert_eq!(v["sources"][1]["state"], "configured");
        // Kept: a show needs no call.
        let calls = fake.sent().len();
        let v = show(&p, "digikey:1276-1001-1-ND", false, &cats).unwrap();
        assert_eq!(v["records"][0]["mpn"], "CL05B104KO5NNNC");
        assert_eq!(fake.sent().len(), calls);
        // Offline: left out, with a note.
        let v = search(
            &p,
            &Search {
                offline: true,
                ..s.clone()
            },
            &cats,
        )
        .unwrap();
        assert!(
            v["results"]
                .as_array()
                .unwrap()
                .iter()
                .all(|r| r["source"] != "digikey")
        );
        assert_eq!(v["notes"][0]["reason"], "offline");
        assert_eq!(fake.sent().len(), calls);
        // No keys: a warning, and the library still answers.
        let none = Catalogues::at(_k.path(), 1_000, MAX_AGE);
        let v = search(&p, &s, &none).unwrap();
        assert_eq!(
            (
                v["warnings"][0]["code"].as_str(),
                v["warnings"][0]["details"]["reason"].as_str()
            ),
            (Some("parts_source_refused"), Some("not_configured"))
        );
        // DigiKey down: a warning naming it.
        fake.set(
            ryueda_parts::fetch::Method::Post,
            &ryueda_parts::digikey::testing::keyword_url(),
            ryueda_parts::fetch::Response::new(503, ""),
        );
        let v = search(
            &p,
            &Search {
                category: Some("resistor".into()),
                ..s
            },
            &cats,
        )
        .unwrap();
        assert_eq!(
            (
                v["warnings"][0]["code"].as_str(),
                v["warnings"][0]["source"].as_str()
            ),
            (Some("parts_source_unreachable"), Some("digikey"))
        );
    }
}
