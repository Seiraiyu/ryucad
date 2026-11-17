//! The catalogue's settings (until Task 17).

use crate::error::{Code, Error, Result};
use crate::session::Session;
use serde::{Deserialize, Serialize};
use serde_json::Value;

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub property: Option<String>,
    /// With `property`: its declaration, as `ryupdm.json` writes one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub declaration: Option<Value>,
    /// Remove the named class, scheme or property instead.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub remove: bool,
}

pub fn show(_: &Session) -> Result<Value> {
    Err(Error::new(Code::Usage, "not yet"))
}
pub fn edit(_: &mut Session, _: Change, _: Option<&str>) -> Result<Value> {
    Err(Error::new(Code::Usage, "not yet"))
}
