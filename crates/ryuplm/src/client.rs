//! The clients' side of the daemon: `ryu-session`'s, for ryuplm.

use crate::Plm;
use crate::error::Result;
use crate::proto::{Answer, Request};
use serde_json::Value;
use std::path::Path;
use std::time::Duration;

/// Sends `req` to the project's daemon, starting one if none is running,
/// as the agent participant `name` (`agent` when `None`). `Err` means it
/// was never delivered, so running it in process instead is safe.
pub fn request(root: &Path, req: &Request, name: Option<&str>) -> Result<Result<Answer>> {
    ryu_session::client::request::<Plm>(root, req, name)
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
    ryu_session::client::watch::<Plm>(root, name, topics, count, timeout)
}

/// The daemon's status, without starting one.
pub fn status(root: &Path) -> Result<Value> {
    ryu_session::client::status::<Plm>(root)
}

/// Stops the project's daemon, if one is running.
pub fn stop(root: &Path) -> Result<Value> {
    ryu_session::client::stop::<Plm>(root)
}

/// Stops every ryuplm daemon this user has, of any version.
pub fn stop_all() -> Result<Value> {
    ryu_session::client::stop_all::<Plm>()
}

/// Names the desktop app `ryuplm open` starts, instead of finding
/// `ryuplm-app`: for tests and unusual installs.
pub const APP_VAR: &str = "RYUPLM_APP";

/// Starts the desktop app on folder `dir`: [`APP_VAR`]'s if it names a
/// file, else `ryuplm-app` beside this binary or on `PATH`; detached,
/// its output discarded.
pub fn open(dir: &Path) -> Result<Value> {
    use crate::error::{Code, Error};
    use std::process::{Command, Stdio};
    let app = match std::env::var_os(APP_VAR) {
        Some(p) => Some(std::path::PathBuf::from(p)).filter(|p| p.is_file()),
        None => ryu_session::client::find_program("ryuplm-app"),
    };
    let missing = || {
        Error::new(
            Code::AppNotInstalled,
            "the desktop app, ryuplm-app, isn't installed: install it with install-ryuplm.sh (or .ps1) without RYUPLM_NO_APP, or use `ryuplm item list` and `ryuplm item show`",
        )
    };
    let app = app.ok_or_else(missing)?;
    let mut cmd = Command::new(&app);
    cmd.arg(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let child = ryu_session::client::spawn_detached(&mut cmd)
        .map_err(|e| missing().with("path", &app).with("why", e.to_string()))?;
    Ok(serde_json::json!({"opened": "new", "app": app, "pid": child.id(), "dir": dir}))
}
