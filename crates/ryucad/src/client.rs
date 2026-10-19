//! The clients' side of the daemon: connect (starting the daemon if
//! needed), check that it is the same build, and send requests. The CLI
//! starts the daemon from its own binary; another program (the desktop
//! app) names the `ryucad` binary to start it from.

use crate::daemon::IDLE_VAR;
use crate::endpoint::Endpoint;
use crate::error::{Code, Error, Result};
use crate::live::Kind;
use crate::proto::{Answer, Request, VersionInfo, parse_rpc_response, rpc_request};
use interprocess::local_socket::{RecvHalf, SendHalf, Stream, prelude::*};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

/// How long the CLI waits for a daemon it started.
const START_WAIT: Duration = Duration::from_secs(5);

/// A log larger than this is moved aside when a daemon starts.
const LOG_LIMIT: u64 = 10 * 1024 * 1024;

pub struct Connection {
    reader: BufReader<RecvHalf>,
    writer: SendHalf,
    next_id: u64,
}

impl Connection {
    pub fn connect(endpoint: &Endpoint) -> std::io::Result<Connection> {
        let (recv, send) = Stream::connect(endpoint.name()?)?.split();
        Ok(Connection {
            reader: BufReader::new(recv),
            writer: send,
            next_id: 1,
        })
    }

    /// Sends one request and reads its answer. The outer error means the
    /// connection failed; the inner result is the daemon's answer.
    pub fn call(&mut self, req: &Request) -> std::io::Result<Result<Value>> {
        Ok(self.call_answer(req)?.map(|a| a.result))
    }

    /// [`Connection::call`], with the bytes that follow the answer if it
    /// announces any.
    pub fn call_answer(&mut self, req: &Request) -> std::io::Result<Result<Answer>> {
        let id = self.next_id;
        self.next_id += 1;
        let mut line = rpc_request(id, req);
        line.push('\n');
        self.writer.write_all(line.as_bytes())?;
        self.writer.flush()?;
        let mut answer = String::new();
        if self.reader.read_line(&mut answer)? == 0 {
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }
        let result = match parse_rpc_response(&answer) {
            Ok(v) => v,
            Err(e) => return Ok(Err(e)),
        };
        let binary = match Answer::announced(&result) {
            Some(n) => {
                let mut bytes = vec![0; n];
                std::io::Read::read_exact(&mut self.reader, &mut bytes)?;
                Some(bytes)
            }
            None => None,
        };
        Ok(Ok(Answer { result, binary }))
    }
}

/// A subscribed connection. Dropping it ends the subscription and closes
/// the connection.
pub struct Subscription {
    writer: Option<SendHalf>,
    closing: Arc<AtomicBool>,
}

impl Connection {
    /// Subscribes the connection to `topics` (all when empty) and hands
    /// every notification to `note`, on a thread of the subscription's
    /// own; `note(None)` means the daemon closed the connection. Returns
    /// `subscribe`'s answer.
    pub fn subscribe(
        mut self,
        topics: &[String],
        mut note: impl FnMut(Option<Value>) + Send + 'static,
    ) -> Result<(Value, Subscription)> {
        let answer = self
            .call(&Request::Subscribe {
                topics: topics.to_vec(),
            })
            .map_err(unavailable)??;
        let closing = Arc::new(AtomicBool::new(false));
        let done = Arc::clone(&closing);
        let mut reader = self.reader;
        std::thread::spawn(move || {
            let mut line = String::new();
            while reader.read_line(&mut line).is_ok_and(|n| n > 0) {
                let Ok(v) = serde_json::from_str::<Value>(&line) else {
                    break;
                };
                line.clear();
                // Only the request a closing subscription sends is answered.
                if v.get("id").is_some() {
                    if done.load(Ordering::SeqCst) {
                        return;
                    }
                    continue;
                }
                note(Some(v));
            }
            if !done.load(Ordering::SeqCst) {
                note(None);
            }
        });
        let sub = Subscription {
            writer: Some(self.writer),
            closing,
        };
        Ok((answer, sub))
    }
}

impl Drop for Subscription {
    /// The reader thread holds the connection open while it waits for a
    /// line, and local sockets can't be shut down from another thread on
    /// every platform. So the subscription asks the daemon something: the
    /// answer wakes the reader, which sees it is closing and lets go.
    fn drop(&mut self) {
        self.closing.store(true, Ordering::SeqCst);
        if let Some(mut w) = self.writer.take() {
            let line = format!("{}\n", rpc_request(0, &Request::DaemonStatus {}));
            let _ = w.write_all(line.as_bytes()).and_then(|()| w.flush());
        }
    }
}

fn unavailable(why: impl std::fmt::Display) -> Error {
    Error::new(Code::Daemon, format!("daemon unavailable: {why}"))
}

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

/// A program next to this executable, or else on `PATH`: how the app finds
/// `ryucad`, and `ryucad open` finds the app.
pub fn find_program(name: &str) -> Option<PathBuf> {
    let file = format!("{name}{}", std::env::consts::EXE_SUFFIX);
    let beside = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|d| d.join(&file)));
    let on_path = std::env::var_os("PATH")
        .into_iter()
        .flat_map(|p| std::env::split_paths(&p).collect::<Vec<_>>())
        .map(|d| d.join(&file));
    beside.into_iter().chain(on_path).find(|p| p.is_file())
}

/// Names the desktop app `ryucad open` starts, instead of finding
/// `ryucad-app`: for tests and unusual installs.
pub const APP_VAR: &str = "RYUCAD_APP";

/// Opens the folder `dir` in the desktop app. When `root` is a project
/// whose daemon is running, the app windows attached to it come to the
/// front; otherwise (or if none is attached) the app starts on `dir`. A
/// daemon is never started just to ask.
pub fn open(dir: &Path, root: Option<&Path>) -> Result<Value> {
    allow_foreground();
    if let Some(root) = root
        && let Ok(mut conn) = Connection::connect(&Endpoint::for_project(root)?)
        && let Ok(Ok(r)) = conn.call(&Request::AppOpen { participant: None })
        && r["asked"].as_array().is_some_and(|a| !a.is_empty())
    {
        return Ok(json!({"opened": "existing", "asked": r["asked"], "dir": dir}));
    }
    let app = match std::env::var_os(APP_VAR) {
        Some(p) => Some(PathBuf::from(p)).filter(|p| p.is_file()),
        None => find_program("ryucad-app"),
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

/// The `ryucad` binary that daemons are started from, and its version.
#[derive(Clone, Debug, PartialEq)]
pub struct Daemon {
    pub exe: PathBuf,
    pub version: VersionInfo,
}

impl Daemon {
    /// This binary: what the CLI uses.
    pub fn this() -> Result<Daemon> {
        Ok(Daemon {
            exe: std::env::current_exe().map_err(unavailable)?,
            version: VersionInfo::current(),
        })
    }

    /// The `ryucad` binary at `exe`, asked for its version.
    pub fn at(exe: &Path) -> Result<Daemon> {
        let failed = |why: String| {
            Error::new(Code::Daemon, format!("can't run {}: {why}", exe.display()))
                .with("path", exe)
        };
        let out = command(exe)
            .arg("version")
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .map_err(|e| failed(e.to_string()))?;
        let doc: Value = serde_json::from_slice(&out.stdout)
            .map_err(|_| failed("it didn't answer `version` with JSON".into()))?;
        let version = serde_json::from_value(doc["result"].clone())
            .map_err(|_| failed("it isn't a ryucad of this protocol".into()))?;
        Ok(Daemon {
            exe: exe.to_path_buf(),
            version,
        })
    }

    /// A connection to the daemon of the project at `root` (canonical),
    /// started from this binary if none is running. A daemon of another
    /// build is replaced.
    pub fn connect(&self, root: &Path) -> Result<Connection> {
        ready(root, &Endpoint::for_project(root)?, self)
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
    let mut conn = Daemon::this()?.connect(root)?;
    let hello = Request::SessionHello {
        kind: Kind::Agent,
        name: name.map(str::to_owned),
        participant: None,
    };
    // A refused hello (a bad name) is the agent's to fix; nothing else ran.
    if let Err(e) = conn.call(&hello).map_err(unavailable)? {
        return Ok(Err(e));
    }
    Ok(conn.call_answer(req).unwrap_or_else(|e| {
        Err(Error::new(
            Code::Daemon,
            format!("the daemon stopped answering ({e}); the request may have been applied"),
        ))
    }))
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
    let mut conn = Daemon::this()?.connect(root)?;
    let hello = Request::SessionHello {
        kind: Kind::Agent,
        name: name.map(str::to_owned),
        participant: None,
    };
    conn.call(&hello).map_err(unavailable)??;
    let (tx, rx) = mpsc::channel();
    let (_, _sub) = conn.subscribe(topics, move |v| {
        if let Some(v) = v {
            let _ = tx.send(v);
        }
    })?;
    let deadline = Instant::now() + timeout;
    let mut got = Vec::new();
    while got.len() < count {
        let left = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left) {
            Ok(v) => got.push(json!({"method": v["method"], "params": v["params"]})),
            Err(mpsc::RecvTimeoutError::Timeout) => break,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(unavailable("the daemon closed the subscription"));
            }
        }
    }
    let timed_out = got.len() < count;
    Ok(json!({"notifications": got, "timed_out": timed_out}))
}

/// A connection to a daemon of `daemon`'s build.
fn ready(root: &Path, endpoint: &Endpoint, daemon: &Daemon) -> Result<Connection> {
    let mut restarted = false;
    loop {
        let mut conn = match Connection::connect(endpoint) {
            Ok(c) => c,
            Err(_) => {
                spawn(root, endpoint, &daemon.exe)?;
                wait_for(endpoint)?
            }
        };
        let status = conn
            .call(&Request::DaemonStatus {})
            .map_err(unavailable)?
            .map_err(unavailable)?;
        if daemon.version.same_build(&status["version"]) {
            return Ok(conn);
        }
        // Another ryucad's daemon (an upgrade, say): replace it, once.
        if restarted {
            return Err(unavailable("a daemon of another version keeps answering"));
        }
        let _ = conn.call(&Request::DaemonShutdown {});
        drop(conn);
        wait_gone(endpoint);
        restarted = true;
    }
}

fn wait_for(endpoint: &Endpoint) -> Result<Connection> {
    let start = Instant::now();
    let mut pause = Duration::from_millis(10);
    loop {
        match Connection::connect(endpoint) {
            Ok(c) => return Ok(c),
            Err(e) if start.elapsed() >= START_WAIT => return Err(unavailable(e)),
            Err(_) => {
                std::thread::sleep(pause);
                pause = (pause * 2).min(Duration::from_millis(200));
            }
        }
    }
}

fn wait_gone(endpoint: &Endpoint) {
    let start = Instant::now();
    while Connection::connect(endpoint).is_ok() && start.elapsed() < START_WAIT {
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Starts `<exe> daemon run` for `root`, detached, logging to the
/// endpoint's log file.
fn spawn(root: &Path, endpoint: &Endpoint, exe: &Path) -> Result<()> {
    if std::fs::metadata(&endpoint.log).is_ok_and(|m| m.len() > LOG_LIMIT) {
        let _ = std::fs::rename(&endpoint.log, endpoint.log.with_extension("log.1"));
    }
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&endpoint.log)
        .map_err(|e| Error::io(&endpoint.log, &e))?;
    let mut cmd = Command::new(exe);
    cmd.arg("-C").arg(root).args(["daemon", "run"]);
    if let Some(secs) = std::env::var(IDLE_VAR)
        .ok()
        .filter(|s| s.parse::<u64>().is_ok())
    {
        cmd.args(["--idle-timeout", &secs]);
    }
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(log);
    detach(&mut cmd);
    cmd.spawn().map_err(unavailable)?;
    Ok(())
}

/// A command that runs `exe` and waits for it: what a program without a
/// console of its own (the app, a GUI on Windows) uses to run `ryucad`.
/// On Windows it gets no console window, where it would otherwise get a
/// new one that flashes up for as long as it runs; elsewhere it is a plain
/// `Command`. Daemons are started detached instead (`detach`).
pub fn command(exe: impl AsRef<std::ffi::OsStr>) -> Command {
    #[allow(unused_mut)]
    let mut cmd = Command::new(exe);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// Out of the terminal's process group, so Ctrl-C on the CLI spares it.
#[cfg(unix)]
fn detach(cmd: &mut Command) {
    use std::os::unix::process::CommandExt;
    cmd.process_group(0);
}

/// No console, its own process group, and none of this process's handles:
/// an inherited stdout pipe would keep whoever reads the CLI's output
/// waiting until the daemon exits.
#[cfg(windows)]
fn detach(cmd: &mut Command) {
    use std::os::windows::process::CommandExt;
    use windows_sys::Win32::Foundation::{HANDLE_FLAG_INHERIT, SetHandleInformation};
    use windows_sys::Win32::System::Console::{
        GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
    };
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    for which in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
        // SAFETY: plain Win32 calls on this process's own standard handles.
        unsafe {
            let h = GetStdHandle(which);
            if !h.is_null() {
                SetHandleInformation(h, HANDLE_FLAG_INHERIT, 0);
            }
        }
    }
    cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
}

/// The daemon's status, without starting one.
pub fn status(root: &Path) -> Result<Value> {
    let endpoint = Endpoint::for_project(root)?;
    match Connection::connect(&endpoint) {
        Err(_) => Ok(json!({"running": false})),
        Ok(mut conn) => {
            let mut s = conn
                .call(&Request::DaemonStatus {})
                .map_err(unavailable)??;
            s["running"] = json!(true);
            Ok(s)
        }
    }
}

/// Asks one endpoint's daemon to stop. Returns whether one was running.
fn stop_endpoint(endpoint: &Endpoint) -> bool {
    match Connection::connect(endpoint) {
        Ok(mut conn) => {
            let _ = conn.call(&Request::DaemonShutdown {});
            drop(conn);
            wait_gone(endpoint);
            true
        }
        Err(_) => {
            // A socket file nobody answers on is a crashed daemon's.
            #[cfg(unix)]
            let _ = std::fs::remove_file(&endpoint.address);
            false
        }
    }
}

/// Stops the project's daemon, if one is running.
pub fn stop(root: &Path) -> Result<Value> {
    Ok(json!({"stopped": stop_endpoint(&Endpoint::for_project(root)?)}))
}

/// Stops every daemon this user has, of any version: the installers run
/// this before replacing the binary.
pub fn stop_all() -> Result<Value> {
    let stopped = Endpoint::all()?.iter().filter(|e| stop_endpoint(e)).count();
    Ok(json!({ "stopped": stopped }))
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
