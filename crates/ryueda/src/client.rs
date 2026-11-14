//! The clients' side of the daemon: `ryu-session`'s, for ryueda.

use crate::Eda;
use crate::error::Result;
use crate::proto::{Answer, Request};
use serde_json::Value;
use std::path::Path;
use std::time::Duration;

/// Sends `req` to the project's daemon, starting one if none is running,
/// as the agent participant `name` (`agent` when `None`). `Err` means it
/// was never delivered, so running it in process instead is safe.
pub fn request(root: &Path, req: &Request, name: Option<&str>) -> Result<Result<Answer>> {
    ryu_session::client::request::<Eda>(root, req, name)
}

/// [`request`] of a long request, with `ask`'s `progress` and `token`,
/// handing each progress notification's params to `note`.
pub fn request_noting(
    root: &Path,
    req: &Request,
    name: Option<&str>,
    ask: &ryu_session::job::Ask,
    note: &mut dyn FnMut(&serde_json::Value),
) -> Result<Result<Answer>> {
    ryu_session::client::request_noting::<Eda>(root, req, name, ask, note)
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
    ryu_session::client::watch::<Eda>(root, name, topics, count, timeout)
}

/// The daemon's status, without starting one.
pub fn status(root: &Path) -> Result<Value> {
    ryu_session::client::status::<Eda>(root)
}

/// Stops the project's daemon, if one is running.
pub fn stop(root: &Path) -> Result<Value> {
    ryu_session::client::stop::<Eda>(root)
}

/// Stops every ryueda daemon this user has, of any version.
pub fn stop_all() -> Result<Value> {
    ryu_session::client::stop_all::<Eda>()
}

/// Names the desktop app `ryueda open` starts, instead of finding
/// `ryueda-app`: for tests and unusual installs.
pub const APP_VAR: &str = "RYUEDA_APP";

/// Starts the desktop app on folder `dir`: [`APP_VAR`]'s if it names a
/// file, else `ryueda-app` beside this binary or on `PATH`; detached,
/// its output discarded.
pub fn open(dir: &Path) -> Result<Value> {
    use crate::error::{Code, Error};
    use std::process::{Command, Stdio};
    let app = match std::env::var_os(APP_VAR) {
        Some(p) => Some(std::path::PathBuf::from(p)).filter(|p| p.is_file()),
        None => ryu_session::client::find_program("ryueda-app"),
    };
    let missing = || {
        Error::new(
            Code::AppNotInstalled,
            "the desktop app, ryueda-app, isn't installed; install it, or use `ryueda render` to see the sheets",
        )
    };
    let app = app.ok_or_else(missing)?;
    let mut cmd = Command::new(&app);
    cmd.arg(dir).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    ryu_session::client::detach(&mut cmd);
    let child = cmd.spawn().map_err(|e| missing().with("path", &app).with("why", e.to_string()))?;
    Ok(serde_json::json!({"opened": "new", "app": app, "pid": child.id(), "dir": dir}))
}
