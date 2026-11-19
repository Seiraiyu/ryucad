//! Every finding of a catalogue's own files (design 734), in a fixed
//! order: files that don't read, then per item in ID order its AML,
//! class and properties, then numbers. Errors refuse a write that adds
//! them and fail `check`; warnings only report.

use crate::error::{Code, Error};
use crate::fault::{Fault, Severity};
use crate::id::Id;
use crate::load::Catalogue;
use crate::source::Source;
use ryupdm::config::Config;
use std::path::PathBuf;

/// What judging reads besides the catalogue's files: other catalogues to
/// compare numbers with (Task 14).
#[derive(Clone, Debug, Default)]
pub struct Context {
    /// Folders searched for other catalogues (`--in`, `pdm.dependents`).
    pub elsewhere: Vec<PathBuf>,
    /// The catalogue's folder on disk, `None` for files in memory: other
    /// catalogues in its work tree are compared too.
    pub here: Option<PathBuf>,
}

fn err(e: Error) -> Fault {
    Fault::of(Severity::Error, e)
}

fn warn(e: Error) -> Fault {
    Fault::of(Severity::Warning, e)
}

/// The settings, or the fault that they don't read.
pub fn config(src: &dyn Source) -> Result<Config, Fault> {
    Config::read_in(src, crate::catalogue::NAME).map_err(err)
}

pub fn all(src: &dyn Source, cat: &Catalogue, cx: &Context) -> Vec<Fault> {
    let mut out: Vec<Fault> = cat.faults.iter().cloned().map(err).collect();
    let config = match config(src) {
        Ok(c) => Some(c),
        Err(f) => {
            out.push(f);
            None
        }
    };
    out.extend(
        crate::import::judge(src, cat, config.as_ref())
            .into_iter()
            .map(err),
    );
    for (id, item) in &cat.items {
        out.extend(crate::aml::judge(id, &item.aml).into_iter().map(err));
        out.extend(class(cat, id).into_iter().map(err));
    }
    out.extend(crate::links::judge(cat).into_iter().map(err));
    out.extend(crate::bom::judge(cat).into_iter().map(err));
    if let Some(c) = &config {
        out.extend(properties(src, cat, c).into_iter().map(err));
        out.extend(numbers(src, cat, c, cx));
        out.extend(
            crate::import::history::foreign(src, cat, c)
                .into_iter()
                .map(warn),
        );
    }
    out
}

/// `item_class_unknown` for an item whose class the manifest doesn't
/// declare.
fn class(cat: &Catalogue, id: &Id) -> Option<Error> {
    let c = cat.items[id].class.as_ref()?;
    (!cat.manifest.classes.contains_key(c)).then(|| {
        Error::new(
            Code::ItemClassUnknown,
            format!("item {id}: class {c:?} isn't declared in ryuplm.json"),
        )
        .with("item", id)
        .with("pointer", "/class")
        .with("class", c)
    })
}

/// ryupdm's property faults (unknown, invalid, off format 1's pattern),
/// then each declared property an item's class doesn't list, every one
/// naming its item. Duplicate numbers are [`numbers`]'.
fn properties(src: &dyn Source, cat: &Catalogue, config: &Config) -> Vec<Error> {
    let unit = cat.manifest.default_unit;
    let mut out = Vec::new();
    for (id, item) in &cat.items {
        let props = crate::pdm::properties(item);
        for mut e in ryupdm::props::check(config, unit, id, &props) {
            // ryupdm names the property by its pointer in a properties
            // file; an item keeps declared ones under /properties.
            if let Some(name) = e.details["property"].as_str().map(str::to_owned)
                && ryupdm::config::core(&name).is_none()
            {
                e = e.with("pointer", format!("/properties/{name}"));
            }
            out.push(e.with("item", id));
        }
        let allowed = item
            .class
            .as_ref()
            .and_then(|c| cat.manifest.classes.get(c))
            .and_then(|c| c.properties.as_ref());
        if let Some(allowed) = allowed {
            for name in item.properties.keys() {
                if config.properties.contains_key(name) && !allowed.contains(name) {
                    out.push(
                        Error::new(
                            Code::PropertyUnknown,
                            format!(
                                "item {id}: class {} doesn't take {name}",
                                item.class.as_deref().unwrap_or("")
                            ),
                        )
                        .with("item", id)
                        .with("property", name)
                        .with("pointer", format!("/properties/{name}")),
                    );
                }
            }
        }
    }
    let _ = src;
    out
}

/// Numbers: duplicates, schemes and other catalogues.
fn numbers(src: &dyn Source, cat: &Catalogue, config: &Config, cx: &Context) -> Vec<Fault> {
    let mut out: Vec<Fault> = crate::numbers::duplicates(src, cat)
        .into_iter()
        .map(err)
        .collect();
    out.extend(crate::numbers::unschemed(cat, config).into_iter().map(warn));
    let others = match &cx.here {
        Some(h) => crate::numbers::others(h, &cx.elsewhere),
        None => cx
            .elsewhere
            .iter()
            .flat_map(|f| {
                ryupdm::dependents::projects_under(f, crate::catalogue::MANIFEST)
                    .unwrap_or_default()
            })
            .collect(),
    };
    out.extend(
        crate::numbers::elsewhere(cat, &others)
            .into_iter()
            .map(warn),
    );
    out
}

/// The declared properties a release of `id` needs that it lacks: those
/// required, as its class allows them, then `amount` for an item not
/// counted in `each`.
pub fn missing(cat: &Catalogue, config: &Config, id: &Id) -> Vec<String> {
    let item = &cat.items[id];
    let allowed = item
        .class
        .as_ref()
        .and_then(|c| cat.manifest.classes.get(c))
        .and_then(|c| c.properties.clone());
    ryupdm::props::missing(config, &crate::pdm::properties(item))
        .into_iter()
        .filter(|n| {
            ryupdm::config::core(n).is_some() || allowed.as_ref().is_none_or(|a| a.contains(n))
        })
        .collect()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::catalogue::{Class, MANIFEST};
    use crate::item::Item;
    use serde_json::json;
    use std::collections::BTreeMap;

    /// A catalogue's files: `classes` in its manifest, `config` as its
    /// `ryupdm.json` (when given), and `items`.
    pub(crate) fn files(
        classes: &[(&str, Option<&[&str]>)],
        config: Option<serde_json::Value>,
        items: &[(&str, Item)],
    ) -> BTreeMap<String, Vec<u8>> {
        let mut m = crate::catalogue::Manifest::new(
            crate::catalogue::tests::pid(1),
            crate::units::Unit::Mm,
        );
        for (n, p) in classes {
            m.classes.insert(
                (*n).into(),
                Class {
                    properties: p.map(|l| l.iter().map(|s| (*s).to_owned()).collect()),
                },
            );
        }
        let mut f = BTreeMap::new();
        f.insert(MANIFEST.to_owned(), m.to_text().into_bytes());
        if let Some(c) = config {
            f.insert(
                "ryupdm.json".to_owned(),
                crate::canonical::to_string(&c).into_bytes(),
            );
        }
        for (id, item) in items {
            let id = Id::parse(id, "item").unwrap();
            f.insert(crate::ids::item_path(&id), item.to_text().into_bytes());
        }
        f
    }

    pub(crate) fn codes(f: &BTreeMap<String, Vec<u8>>) -> Vec<(String, &'static str, String)> {
        let c = crate::load::Catalogue::load(f).unwrap();
        all(f, &c, &Context::default())
            .iter()
            .map(|x| {
                (
                    serde_json::to_value(x.error.code)
                        .unwrap()
                        .as_str()
                        .unwrap()
                        .to_owned(),
                    if x.is_error() { "error" } else { "warning" },
                    x.error.details["item"].as_str().unwrap_or("").to_owned(),
                )
            })
            .collect()
    }

    #[test]
    fn classes_choose_the_declared_properties() {
        let config = json!({"format": 1, "kind": "ryupdm.config", "properties": {
            "thread": {"type": "string"}, "finish": {"type": "string"}}});
        let mut a = Item::new("A-1");
        a.class = Some("fastener".into());
        a.properties.insert("thread".into(), json!("M3"));
        let mut b = a.clone();
        b.number = "A-2".into();
        b.properties.insert("finish".into(), json!("zinc"));
        let mut c = Item::new("A-3");
        c.class = Some("gizmo".into());
        let mut d = Item::new("A-4");
        d.properties.insert("finish".into(), json!("zinc"));
        d.properties.insert("colour".into(), json!("red"));
        let f = files(
            &[("fastener", Some(&["thread"])), ("label", None)],
            Some(config),
            &[
                ("0000000000001", a),
                ("0000000000002", b),
                ("0000000000003", c),
                ("0000000000004", d),
            ],
        );
        assert_eq!(
            codes(&f),
            [
                // Per item first (AML, class), then properties over all.
                ("item_class_unknown".into(), "error", "0000000000003".into()),
                ("property_unknown".into(), "error", "0000000000002".into()),
                ("property_unknown".into(), "error", "0000000000004".into()),
            ]
        );
    }
}
