//! The catalogue's settings: classes (`ryuplm.json`), numbering schemes
//! and declared properties (`ryupdm.json`).

use crate::catalogue::{Class, MANIFEST};
use crate::error::{Code, Error, Result};
use crate::session::Session;
use crate::write::{Writes, commit};
use ryupdm::config::{Config, NumberScheme, Pattern, Template};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// One change: set or remove a class, a scheme or a declared property.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Change {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub class: Option<String>,
    /// With `class`: the declared properties it allows (`None`: all).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub class_properties: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheme: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheme_class: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_template: Option<String>,
    /// With `scheme`: `item` (the default) or `document` (design 937).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheme_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub property: Option<String>,
    /// With `property`: its declaration, as `ryupdm.json` writes one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub declaration: Option<Value>,
    /// Turn the organization's ledger on or off (`part_number.ledger`,
    /// design 936).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ledger: Option<bool>,
    /// Remove the named class, scheme or property instead.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub remove: bool,
}

pub fn show(session: &Session) -> Result<Value> {
    let config = Config::read_in(&session.snapshot(), crate::catalogue::NAME)?;
    Ok(
        json!({"catalogue": session.catalogue().manifest.to_json(), "ryupdm": config.to_json(), "revision": session.revision()}),
    )
}

fn bad(msg: &str) -> Error {
    Error::new(Code::InvalidParam, msg)
}

pub fn edit(session: &mut Session, change: Change, expected: Option<&str>) -> Result<Value> {
    if let Some(e) = expected
        && e != session.revision()
    {
        return Err(Error::new(
            Code::Conflict,
            "the catalogue changed; read it again and retry",
        )
        .with("expected", e)
        .with("actual", session.revision()));
    }
    let picked = [
        change.class.is_some(),
        change.scheme.is_some(),
        change.property.is_some(),
        change.ledger.is_some(),
    ]
    .iter()
    .filter(|b| **b)
    .count();
    if picked != 1 {
        return Err(Error::new(
            Code::Usage,
            "change one of --class, --scheme, --property or --ledger at a time",
        ));
    }
    let mut manifest = session.catalogue().manifest.clone();
    let mut config = Config::read_in(&session.snapshot(), crate::catalogue::NAME)?;
    if let Some(name) = &change.class {
        if change.remove {
            manifest
                .classes
                .remove(name)
                .ok_or_else(|| bad(&format!("no class {name:?}")))?;
        } else {
            if !ryupdm::config::scheme_name(name) {
                return Err(bad("a class's name is 1 to 63 of a-z, 0-9, _ and -"));
            }
            manifest.classes.insert(
                name.clone(),
                Class {
                    properties: change.class_properties.clone(),
                },
            );
        }
    }
    if let Some(name) = &change.scheme {
        config.schemes.retain(|s| &s.name != name || !change.remove);
        if !change.remove {
            if config.pattern.is_some() {
                return Err(bad(
                    "ryupdm.json has a single part_number pattern: remove it before adding schemes",
                ));
            }
            let pattern = Pattern::new(
                change
                    .pattern
                    .as_deref()
                    .ok_or_else(|| bad("a scheme needs --pattern"))?,
            )?;
            let next = change
                .next_template
                .as_deref()
                .map(Template::new)
                .transpose()
                .map_err(|w| bad(&w))?;
            let s = NumberScheme {
                name: name.clone(),
                class: change.scheme_class.clone(),
                kind: match change.scheme_kind.as_deref() {
                    None | Some("item") => ryupdm::config::SchemeKind::Item,
                    Some("document") => ryupdm::config::SchemeKind::Document,
                    Some(o) => {
                        return Err(bad(&format!(
                            "a scheme's kind is item or document, not {o:?}"
                        )));
                    }
                },
                pattern,
                next,
            };
            match config.schemes.iter_mut().find(|x| &x.name == name) {
                Some(x) => *x = s,
                None => config.schemes.push(s),
            }
        }
    }
    if let Some(name) = &change.property {
        if change.remove {
            config
                .properties
                .remove(name)
                .ok_or_else(|| bad(&format!("no declared property {name:?}")))?;
        } else {
            let at = format!("/properties/{name}");
            ryupdm::config::property_name(name, &at)?;
            let d = change
                .declaration
                .as_ref()
                .ok_or_else(|| bad("a property needs --declaration"))?;
            config
                .properties
                .insert(name.clone(), ryupdm::config::Decl::from_json(d, &at)?);
        }
    }
    if let Some(on) = change.ledger {
        config.ledger = on;
    }
    // The settings must read back as written.
    let config_text = crate::canonical::to_string(&config.to_json());
    Config::from_json_in(&config.to_json(), crate::catalogue::NAME)?;
    commit(
        session,
        "editing the settings",
        Writes(vec![
            (MANIFEST.to_owned(), Some(manifest.to_text())),
            ("ryupdm.json".to_owned(), Some(config_text)),
        ]),
    )?;
    show(session)
}
