//! `ryueda config` (#1077): this machine's preferences, read and written in
//! the user's preferences.json under its lock, never through a daemon or the
//! log.

use crate::Eda;
use crate::error::{Code, Error, Result};
use ryu_session::prefs::{self, Preferences, Schema};
use serde_json::{Value, json};

fn known(key: &str) -> Result<()> {
    if crate::PREFERENCE_KEYS.contains(&key) {
        return Ok(());
    }
    Err(
        Error::new(Code::UnknownSetting, format!("no preference {key:?}"))
            .with("key", key)
            .with("known", crate::PREFERENCE_KEYS),
    )
}

/// `{key, value, default}`: the value set, else the default, and whether
/// it is the default.
fn report(key: &str, p: &Preferences<Eda>) -> Value {
    let set = p.values.get(key);
    json!({
        "key": key,
        "value": set.cloned().or_else(|| Eda::default_of(key)).unwrap_or(Value::Null),
        "default": set.is_none(),
    })
}

/// One preference's value, or every one's: `{key, value, default, path}`
/// or `{preferences, path}`, with `problem` when the file couldn't be used.
pub fn get(key: Option<&str>) -> Result<Value> {
    let p = Preferences::<Eda>::load()?;
    let mut v = match key {
        Some(k) => {
            known(k)?;
            report(k, &p)
        }
        None => json!({
            "preferences": crate::PREFERENCE_KEYS
                .iter()
                .map(|k| report(k, &p))
                .collect::<Vec<_>>(),
        }),
    };
    v["path"] = json!(p.path);
    if let Some(why) = &p.problem {
        v["problem"] = json!(why);
    }
    Ok(v)
}

/// A preference set; set to its default, it is removed.
pub fn set(key: &str, value: Value) -> Result<Value> {
    known(key)?;
    if !Eda::accepts(key, &value) {
        return Err(Error::new(
            Code::InvalidSetting,
            format!("{value} is no value for {key}"),
        )
        .with("key", key)
        .with("value", &value));
    }
    prefs::locked::<Eda, _>(|| {
        let mut p = Preferences::<Eda>::load()?;
        p.set(key, value)?;
        let mut v = report(key, &p);
        v["path"] = json!(p.path);
        Ok(v)
    })
}
