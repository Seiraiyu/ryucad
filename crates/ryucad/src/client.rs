//! The clients' side of the daemon: `ryu-session`'s, for ryucad. Connect
//! (starting the daemon if needed), check that it is the same build, and
//! send requests. The CLI starts the daemon from its own binary; another
//! program (the desktop app) names the `ryucad` binary to start it from.

use crate::Cad;
use crate::error::{Code, Error, Result};
use crate::proto::{Answer, Request, VersionInfo};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

pub use ryu_session::client::{Call, Connection, Subscription, command, detach, find_program};

/// The `ryucad` binary that daemons are started from, and its version.
pub type Daemon = ryu_session::client::Daemon<Cad>;

impl VersionInfo {
    /// Whether `v` (a `version` object as JSON) is the same build: the same
    /// ryucad, ryukei, commit and protocol. The target may differ: on Linux
    /// the app is a glibc build and the CLI a static musl one.
    pub fn same_build(&self, v: &Value) -> bool {
        let me = json!(self);
        ["ryucad", "ryukei", "commit", "protocol"]
            .iter()
            .all(|k| v.get(k) == me.get(k))
    }
}

/// Names the desktop app `ryucad open` starts, instead of finding
/// `ryucad-app`: for tests and unusual installs.
pub const APP_VAR: &str = "RYUCAD_APP";

/// Opens the folder `dir` in the desktop app. When `root` is a project
/// whose daemon is running, the app windows attached to it come to the
/// front; otherwise (or if none is attached) the app starts on `dir`. A
/// daemon is never started just to ask.
pub fn open(dir: &Path, root: Option<&Path>) -> Result<Value> {
    open_app(dir, root, None)
}

/// [`open`], starting [`APP_VAR`]'s app when a new window is needed, else
/// `app` (the app opening another worktree starts itself), else
/// `ryucad-app` on `PATH`.
pub fn open_app(dir: &Path, root: Option<&Path>, app: Option<&Path>) -> Result<Value> {
    allow_foreground();
    if let Some(root) = root
        && let Ok(mut conn) = Connection::connect(&crate::endpoint::Endpoint::for_project(root)?)
        && let Ok(Ok(r)) = conn.call(&Request::AppOpen { participant: None })
        && r["asked"].as_array().is_some_and(|a| !a.is_empty())
    {
        return Ok(json!({"opened": "existing", "asked": r["asked"], "dir": dir}));
    }
    let app = match (std::env::var_os(APP_VAR), app) {
        (Some(p), _) => Some(PathBuf::from(p)).filter(|p| p.is_file()),
        (None, Some(a)) => Some(a.to_path_buf()),
        (None, None) => find_program("ryucad-app"),
    };
    let missing = || {
        Error::new(
            Code::AppNotInstalled,
            "the desktop app, ryucad-app, isn't installed; install it, or use `ryucad render` to see the model",
        )
    };
    let app = app.ok_or_else(missing)?;
    let mut cmd = Command::new(&app);
    cmd.arg(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    detach(&mut cmd);
    let child = cmd
        .spawn()
        .map_err(|e| missing().with("path", &app).with("why", e.to_string()))?;
    Ok(json!({"opened": "new", "app": app, "pid": child.id(), "dir": dir}))
}

/// Lets the window `open` raises or starts take the foreground. Windows
/// keeps it from background processes (the app, asked through the
/// daemon), and this process, run by the person at the keyboard, may hand
/// it on; elsewhere there is nothing to do.
fn allow_foreground() {
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::WindowsAndMessaging::{ASFW_ANY, AllowSetForegroundWindow};
        // SAFETY: a plain Win32 call; failing just leaves focus as it is.
        unsafe {
            AllowSetForegroundWindow(ASFW_ANY);
        }
    }
}

/// Sends `req` to the project's daemon, starting one if none is running,
/// as the agent participant `name` (`agent` when `None`).
///
/// `Err` means the request was never delivered, so running it in process
/// instead is safe. `Ok` holds the daemon's answer; if the connection broke
/// after sending, that answer is a `daemon` error, because the request may
/// have been applied and must not be repeated.
pub fn request(root: &Path, req: &Request, name: Option<&str>) -> Result<Result<Answer>> {
    ryu_session::client::request::<Cad>(root, req, name)
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
    ryu_session::client::watch::<Cad>(root, name, topics, count, timeout)
}

/// The daemon's status, without starting one.
pub fn status(root: &Path) -> Result<Value> {
    ryu_session::client::status::<Cad>(root)
}

/// Stops the project's daemon, if one is running.
pub fn stop(root: &Path) -> Result<Value> {
    ryu_session::client::stop::<Cad>(root)
}

/// Stops every daemon this user has, of any version: the installers run
/// this before replacing the binary.
pub fn stop_all() -> Result<Value> {
    ryu_session::client::stop_all::<Cad>()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_build_may_have_another_target() {
        let me = VersionInfo::current();
        let mut v = json!(me);
        assert!(me.same_build(&v));
        v["target"] = json!("x86_64-unknown-linux-musl");
        assert!(me.same_build(&v));
        v["commit"] = json!("0123456789ab");
        assert!(!me.same_build(&v));
        assert!(!me.same_build(&json!({"ryucad": "0.0.0"})));
    }

    #[test]
    fn a_version_names_ryucad_so_other_products_clients_refuse_its_daemon() {
        let v = json!(VersionInfo::current());
        assert!(v.get(<Cad as ryu_session::Product>::NAME).is_some());
        assert!(v.get("ryueda").is_none());
    }

    #[test]
    fn programs_are_found_beside_this_one() {
        let exe = std::env::current_exe().unwrap();
        let stem = exe.file_stem().unwrap().to_str().unwrap();
        assert_eq!(find_program(stem), Some(exe));
        assert_eq!(find_program("no-such-program-ryucad"), None);
    }

    #[test]
    fn a_program_that_isnt_ryucad_is_refused() {
        let e = Daemon::at(Path::new("/no/such/ryucad")).unwrap_err();
        assert_eq!(e.code, Code::Daemon);
    }

    /// Set in the child that `commands_get_no_console_window` starts.
    #[cfg(windows)]
    const CONSOLE_CHILD_VAR: &str = "RYUCAD_TEST_CONSOLE_CHILD";

    /// Runs this test binary again, through `command`, as the child below,
    /// which fails if it has a console window.
    #[cfg(windows)]
    #[test]
    fn commands_get_no_console_window() {
        let st = command(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "client::tests::console_child",
                "--test-threads=1",
            ])
            .env(CONSOLE_CHILD_VAR, "1")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert!(st.success(), "the child had a console window");
    }

    #[cfg(windows)]
    #[test]
    fn console_child() {
        use windows_sys::Win32::System::Console::GetConsoleWindow;
        if std::env::var_os(CONSOLE_CHILD_VAR).is_none() {
            return;
        }
        // SAFETY: a plain Win32 call with no arguments.
        assert!(unsafe { GetConsoleWindow() }.is_null());
    }
}
