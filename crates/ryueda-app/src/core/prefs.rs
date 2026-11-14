//! The user's preferences the window reads and writes (`ui.*`,
//! `app.window`), in ryueda's preferences file: never the project's.

use ryu_app::style::{Contrast, Look};
use ryu_session::prefs::{Preferences, Schema};
use serde_json::Value;

/// A preference: what is set, else its default, else null.
pub fn get(name: &str) -> Value {
    Preferences::<ryueda::Eda>::load()
        .ok()
        .and_then(|p| p.values.get(name).cloned())
        .or_else(|| ryueda::Eda::default_of(name))
        .unwrap_or(Value::Null)
}

/// Sets one; a failure is the window's toast, never a crash.
pub fn set(name: &str, value: Value) -> ryu_doc::error::Result<()> {
    ryu_session::prefs::locked::<ryueda::Eda, _>(|| Preferences::<ryueda::Eda>::load()?.set(name, value))
}

pub fn look() -> Look {
    Look::parse(get("ui.style").as_str().unwrap_or("drafting"))
}

pub fn contrast() -> Contrast {
    Contrast::parse(get("ui.contrast").as_str().unwrap_or("normal"))
}

/// `Some(dark)` for `light` or `dark`; `None` follows the system.
pub fn theme() -> Option<bool> {
    match get("ui.theme").as_str() {
        Some("dark") => Some(true),
        Some("light") => Some(false),
        _ => None,
    }
}

pub fn flag(name: &str) -> bool {
    get(name).as_bool().unwrap_or(true)
}
