//! The item commands (until Task 17).

use crate::error::{Code, Error, Result};
use crate::session::Session;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// The fields `item create` and `item edit` set. `properties` merges into
/// the item's (a `null` value removes one).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fields {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub class: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub make_buy: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub properties: Option<BTreeMap<String, Value>>,
    /// The whole AML, replacing the item's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aml: Option<Vec<Value>>,
}

pub fn create(
    _: &mut Session,
    _: Fields,
    _: Option<String>,
    _: Option<String>,
    _: Option<&str>,
) -> Result<Value> {
    Err(Error::new(Code::Usage, "not yet"))
}
pub fn show(_: &mut Session, _: &str) -> Result<Value> {
    Err(Error::new(Code::Usage, "not yet"))
}
pub fn list(
    _: &Session,
    _: Option<String>,
    _: Option<String>,
    _: Option<String>,
    _: Option<usize>,
) -> Value {
    Value::Null
}
pub fn edit(_: &mut Session, _: &str, _: Fields, _: Vec<String>, _: Option<&str>) -> Result<Value> {
    Err(Error::new(Code::Usage, "not yet"))
}
pub fn renumber(
    _: &mut Session,
    _: &str,
    _: Option<String>,
    _: Option<String>,
    _: Option<&str>,
) -> Result<Value> {
    Err(Error::new(Code::Usage, "not yet"))
}
pub fn delete(_: &mut Session, _: &str, _: Option<&str>) -> Result<Value> {
    Err(Error::new(Code::Usage, "not yet"))
}
