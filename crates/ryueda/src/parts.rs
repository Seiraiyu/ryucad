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

/// The sources this ryueda searches, in the order a search without `in`
/// asks them.
pub const SOURCES: [&str; 1] = [Library::NAME];

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

/// `parts.search`: the asked sources' results in order, each source's
/// ranked by conditions met, stock and key, at most `limit` in all, with
/// the faults of what didn't load.
pub fn search(p: &Project, s: &Search) -> Result<Value> {
    let mut sources: Vec<String> = Vec::new();
    for name in s
        .sources
        .iter()
        .map(String::as_str)
        .chain(s.sources.is_empty().then_some(Library::NAME))
    {
        if !SOURCES.contains(&name) {
            return Err(Error::new(
                Code::InvalidParam,
                format!(
                    "{name} isn't a source this ryueda searches: {}",
                    SOURCES.join(", ")
                ),
            )
            .with("param", "in")
            .with("sources", SOURCES));
        }
        if !sources.iter().any(|x| x == name) {
            sources.push(name.to_owned());
        }
    }
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
    let mut results: Vec<Hit> = Vec::new();
    for name in &sources {
        if name == Library::NAME {
            results.extend(library.search(&q, &Call::default())?);
        }
    }
    results.truncate(q.limit);
    Ok(json!({
        "query": {
            "category": q.category,
            "conditions": q.conditions.iter().map(Condition::to_text).collect::<Vec<_>>(),
            "in": sources,
            "stock": q.stock,
            "basic": q.basic,
            "limit": q.limit,
            "like": s.like,
        },
        "results": results,
        "faults": fault::list(&faults),
    }))
}

#[cfg(test)]
mod tests {
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
}
