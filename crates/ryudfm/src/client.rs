//! The clients' side of the daemon: `ryu-session`'s, for ryudfm.

use crate::Dfm;
use crate::error::Result;
use crate::proto::{Answer, Request};
use serde_json::Value;
use std::path::Path;
use std::time::Duration;

/// Sends `req` to the project's daemon, starting one if none is running,
/// as the agent participant `name` (`agent` when `None`). `Err` means it
/// was never delivered, so running it in process instead is safe.
pub fn request(root: &Path, req: &Request, name: Option<&str>) -> Result<Result<Answer>> {
    ryu_session::client::request::<Dfm>(root, req, name)
}

/// Subscribes to `topics` as the agent `name` and collects notifications
/// until there are `count` of them or `timeout` passes.
pub fn watch(
    root: &Path,
    name: Option<&str>,
    topics: &[String],
    count: usize,
    timeout: Duration,
) -> Result<Value> {
    ryu_session::client::watch::<Dfm>(root, name, topics, count, timeout)
}

/// The daemon's status, without starting one.
pub fn status(root: &Path) -> Result<Value> {
    ryu_session::client::status::<Dfm>(root)
}

/// Stops the project's daemon, if one is running.
pub fn stop(root: &Path) -> Result<Value> {
    ryu_session::client::stop::<Dfm>(root)
}

/// Stops every ryudfm daemon this user has, of any version.
pub fn stop_all() -> Result<Value> {
    ryu_session::client::stop_all::<Dfm>()
}
