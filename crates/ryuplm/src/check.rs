//! `check` (until Task 18).

use crate::error::{Code, Error, Result};
use crate::session::Session;
use serde_json::Value;

pub fn check(_: &mut Session, _: &[String]) -> Result<Value> {
    Err(Error::new(Code::Usage, "not yet"))
}
