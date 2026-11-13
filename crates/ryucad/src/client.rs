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

/// What a long request asks for (`progress`, `token`), and its job.
pub use ryu_session::job::{Ask, Job};

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

/// A connection to the project's daemon that has said hello as the
/// agent `name`: `Err` when no daemon could be reached (nothing was
/// sent), `Ok(Err(..))` when the hello was refused.
pub fn hello_connection(root: &Path, name: Option<&str>) -> Result<Result<Connection>> {
    ryu_session::client::hello_connection::<Cad>(root, name)
}

/// [`request`] with `ask`'s `progress` and `token`, handing each
/// `progress` notification's params to `note` before the answer.
pub fn request_noting(
    root: &Path,
    req: &Request,
    name: Option<&str>,
    ask: &ryu_session::job::Ask,
    note: &mut dyn FnMut(&Value),
) -> Result<Result<Answer>> {
    ryu_session::client::request_noting::<Cad>(root, req, name, ask, note)
}

/// Asks the project's running daemon to cancel the agent `name`'s long
/// request `token`, on a connection of its own; starts no daemon.
pub fn cancel(root: &Path, name: Option<&str>, token: &str) -> Result<Value> {
    ryu_session::client::cancel::<Cad>(root, name, token)
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

/// The params, by method, that name files or folders on the caller's
/// machine. The daemon runs in a folder of its own, so a client makes
/// them absolute against its own folder before sending
/// ([`absolute_params`]): the CLI's, or a script's (`ryucad session
/// --stdio`).
pub const PATH_PARAMS: [(&str, &[&str]); 6] = [
    ("import", &["path", "search"]),
    ("export", &["out"]),
    ("drawing.export", &["out"]),
    ("release.package", &["out", "into"]),
    ("release.package.verify", &["file"]),
    ("bom.where_used", &["in"]),
];

/// The params of any method that name revisions: any of them may name a
/// worktree by its path (`worktree:../arm`).
pub const REVISION_PARAMS: [&str; 6] = ["rev", "from", "to", "against", "compare", "frame"];

/// `params` of `method` with every relative path in them made absolute
/// against `here`, as the daemon takes them: [`PATH_PARAMS`]' values,
/// and the worktree paths of [`REVISION_PARAMS`]' values (each a string
/// or a list of them). A path keeps its trailing separator (a folder to
/// write into). Anything else, a value that isn't a string included, is
/// left for the request's own checks.
pub fn absolute_params(method: &str, params: &mut Value, here: &Path) {
    let Some(map) = params.as_object_mut() else {
        return;
    };
    let paths = PATH_PARAMS
        .iter()
        .find(|(m, _)| *m == method)
        .map_or(&[][..], |(_, keys)| *keys);
    for key in paths {
        if let Some(v) = map.get_mut(*key) {
            each_string(v, |s| {
                (!s.is_empty() && Path::new(s).is_relative()).then(|| absolute_text(s, here))
            });
        }
    }
    for key in REVISION_PARAMS {
        if let Some(v) = map.get_mut(key) {
            each_string(v, |s| {
                crate::session::worktree_path(s)
                    .filter(|p| p.is_relative())
                    .map(|p| {
                        let abs = absolute_text(&p.to_string_lossy(), here);
                        format!("{}{abs}", crate::session::WORKTREE)
                    })
            });
        }
    }
}

/// Replaces `v`, a string or a list of them, by what `fix` makes of each
/// string it changes.
fn each_string(v: &mut Value, fix: impl Fn(&str) -> Option<String>) {
    let one = |v: &mut Value| {
        if let Some(new) = v.as_str().and_then(&fix) {
            *v = Value::String(new);
        }
    };
    match v.as_array_mut() {
        Some(list) => list.iter_mut().for_each(one),
        None => one(v),
    }
}

/// `s`, a relative path, joined to `here` and made absolute, its
/// trailing separator kept.
fn absolute_text(s: &str, here: &Path) -> String {
    let joined = here.join(s);
    let abs = std::path::absolute(&joined).unwrap_or(joined);
    let mut t = abs.to_string_lossy().into_owned();
    if s.ends_with(['/', '\\']) && !t.ends_with(['/', '\\']) {
        t.push('/');
    }
    t
}

/// `req` with its paths made absolute against `here`
/// ([`absolute_params`]); given back as it was when none are relative.
pub fn absolute(req: Request, here: &Path) -> Result<Request> {
    let method = req.method();
    let mut params = req.params();
    let before = params.clone();
    absolute_params(&method, &mut params, here);
    if params == before {
        return Ok(req);
    }
    Request::from_rpc(&method, Some(params))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `here` made absolute, `p` joined to it, as text.
    fn abs(here: &Path, p: &str) -> String {
        std::path::absolute(here.join(p))
            .unwrap()
            .to_string_lossy()
            .into_owned()
    }

    #[test]
    fn paths_become_absolute_against_the_callers_folder() {
        let here = std::env::temp_dir().join("caller");
        let already = abs(&here, "already.step");
        let mut p = json!({"part": "p", "out": "o/p.stl"});
        absolute_params("export", &mut p, &here);
        assert_eq!(p, json!({"part": "p", "out": abs(&here, "o/p.stl")}));
        let mut p = json!({"path": "a.step", "search": ["lib", already]});
        absolute_params("import", &mut p, &here);
        assert_eq!(
            p,
            json!({"path": abs(&here, "a.step"), "search": [abs(&here, "lib"), already]})
        );
        let mut p = json!({"part": "p", "in": ["../other"]});
        absolute_params("bom.where_used", &mut p, &here);
        assert_eq!(p["in"], json!([abs(&here, "../other")]));
        let mut p = json!({"file": "pkg.zip"});
        absolute_params("release.package.verify", &mut p, &here);
        assert_eq!(p["file"], json!(abs(&here, "pkg.zip")));
        let mut p = json!({"part": "d", "drawing": "a", "out": "a.pdf"});
        absolute_params("drawing.export", &mut p, &here);
        assert_eq!(p["out"], json!(abs(&here, "a.pdf")));
        // A folder to write into keeps its trailing separator.
        let mut p = json!({"part": "p", "into": "pkgs/"});
        absolute_params("release.package", &mut p, &here);
        let into = p["into"].as_str().unwrap();
        assert!(into.ends_with(['/', '\\']), "{into}");
        assert!(into.starts_with(abs(&here, "pkgs").as_str()), "{into}");
        // Another method's `out` isn't a path the daemon reads.
        let mut p = json!({"part": "p", "out": "x"});
        absolute_params("mesh.get", &mut p, &here);
        assert_eq!(p, json!({"part": "p", "out": "x"}));
    }

    #[test]
    fn worktree_revisions_become_absolute_in_every_method() {
        let here = std::env::temp_dir().join("caller");
        let mut p =
            json!({"part": "p", "rev": "worktree:../arm", "compare": ["worktree:b", "HEAD"]});
        absolute_params("render", &mut p, &here);
        assert_eq!(
            p["rev"],
            json!(format!("worktree:{}", abs(&here, "../arm")))
        );
        assert_eq!(
            p["compare"],
            json!([format!("worktree:{}", abs(&here, "b")), "HEAD"])
        );
        // Not a revision of a worktree: left alone.
        let mut p = json!({"from": "a", "to": "b"});
        absolute_params("param.rename", &mut p, &here);
        assert_eq!(p, json!({"from": "a", "to": "b"}));
    }

    #[test]
    fn a_request_with_nothing_relative_is_given_back_as_it_was() {
        let here = std::env::temp_dir().join("caller");
        let req = Request::PartList {};
        assert_eq!(absolute(req.clone(), &here).unwrap(), req);
        let req = Request::from_rpc("export", Some(json!({"part": "p", "out": "p.stl"}))).unwrap();
        let back = absolute(req, &here).unwrap();
        assert_eq!(back.params()["out"], json!(abs(&here, "p.stl")));
    }

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
