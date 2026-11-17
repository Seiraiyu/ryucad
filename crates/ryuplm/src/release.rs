//! Releases (until Task 19).

use crate::error::{Code, Error, Result};
use crate::session::Session;
use serde_json::Value;

pub fn propose(
    _: &mut Session,
    _: &[String],
    _: bool,
    _: bool,
    _: Option<String>,
    _: Option<&str>,
    _: Option<&str>,
) -> Result<Value> {
    Err(Error::new(Code::Usage, "not yet"))
}
pub fn obsolete(
    _: &mut Session,
    _: &str,
    _: &str,
    _: Option<String>,
    _: Option<&str>,
    _: Option<&str>,
) -> Result<Value> {
    Err(Error::new(Code::Usage, "not yet"))
}
pub fn list(_: &mut Session, _: Option<&str>) -> Result<Value> {
    Err(Error::new(Code::Usage, "not yet"))
}
pub fn check(_: &mut Session) -> Result<Value> {
    Err(Error::new(Code::Usage, "not yet"))
}
pub fn sync(_: &mut Session) -> Result<Value> {
    Err(Error::new(Code::Usage, "not yet"))
}
pub fn findings(_: &mut Session) -> Result<Vec<Value>> {
    Ok(vec![])
}
