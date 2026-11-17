//! `diff` (until Task 22).

use crate::error::{Code, Error, Result};
use crate::session::Session;
use serde_json::Value;

pub fn diff(_: &mut Session, _: Option<&str>, _: Option<&str>) -> Result<Value> {
    Err(Error::new(Code::Usage, "not yet"))
}
