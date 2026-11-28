//! The clients' side of a product's daemon: connect (starting the daemon
//! if needed), check that it is the same build, and send requests. The
//! CLI starts the daemon from its own binary; another program (ryucad's
//! desktop app) names the binary to start it from.

use crate::Serve;
use crate::endpoint::Endpoint;
use crate::job::Ask;
use crate::live::Kind;
use crate::rpc::{Answer, parse_rpc_response, request_line};
use interprocess::local_socket::{RecvHalf, SendHalf, Stream, prelude::*};
use ryu_doc::error::{Code, Error, Result};
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

/// A request as it travels: its JSON-RPC method and params (null for
/// none). A product's request type implements it.
pub trait Call {
    fn method(&self) -> String;
    fn params(&self) -> Value;
}

/// One of the requests every product's daemon answers the same way.
pub(crate) struct Basic {
    pub(crate) method: &'static str,
    pub(crate) params: Value,
}

impl Call for Basic {
    fn method(&self) -> String {
        self.method.to_owned()
    }

    fn params(&self) -> Value {
        self.params.clone()
    }
}

fn daemon_status(timings: bool) -> Basic {
    Basic {
        method: "daemon.status",
        params: if timings {
            json!({"timings": true})
        } else {
            json!({})
        },
    }
}

fn daemon_shutdown() -> Basic {
    Basic {
        method: "daemon.shutdown",
        params: json!({}),
    }
}

/// `session.hello` as the agent `name` (`agent` when `None`).
pub(crate) fn hello(name: Option<&str>) -> Basic {
    let mut params = json!({ "kind": Kind::Agent });
    if let Some(n) = name {
        params["name"] = json!(n);
    }
    Basic {
        method: "session.hello",
        params,
    }
}

pub struct Connection {
    reader: BufReader<RecvHalf>,
    writer: SendHalf,
    next_id: u64,
}

impl Connection {
    /// Says `session.hello` as the agent `name` (`agent` when `None`).
    /// The outer error means the connection failed; the inner result is
    /// the daemon's answer (a refused name is the caller's to fix).
    pub fn hello(&mut self, name: Option<&str>) -> std::io::Result<Result<Value>> {
        self.call(&hello(name))
    }

    /// The connection's two halves: what it reads (answers, and the
    /// bytes they announce) and what it writes. For a client that reads
    /// on a thread of its own while it writes (`ryucad session --stdio`).
    pub fn into_halves(self) -> (BufReader<RecvHalf>, SendHalf) {
        (self.reader, self.writer)
    }

    pub fn connect<P>(endpoint: &Endpoint<P>) -> std::io::Result<Connection> {
        let (recv, send) = Stream::connect(endpoint.name()?)?.split();
        Ok(Connection {
            reader: BufReader::new(recv),
            writer: send,
            next_id: 1,
        })
    }

    /// Sends one request and reads its answer. The outer error means the
    /// connection failed; the inner result is the daemon's answer.
    pub fn call(&mut self, req: &impl Call) -> std::io::Result<Result<Value>> {
        Ok(self.call_answer(req)?.map(|a| a.result))
    }

    /// [`Connection::call`], with the bytes that follow the answer if it
    /// announces any.
    pub fn call_answer(&mut self, req: &impl Call) -> std::io::Result<Result<Answer>> {
        self.call_noting(req, &Ask::default(), &mut |_| {})
    }

    /// Sends one request with `ask`'s `progress` and `token` in its
    /// params, handing each notification that comes before its answer
    /// (`progress`) to `note`, then reads the answer and its bytes.
    pub fn call_noting(
        &mut self,
        req: &impl Call,
        ask: &Ask,
        note: &mut dyn FnMut(&Value),
    ) -> std::io::Result<Result<Answer>> {
        let id = self.next_id;
        self.next_id += 1;
        let mut params = req.params();
        if *ask != Ask::default() {
            ask.put(&mut params);
        }
        let mut line = request_line(id, &req.method(), params);
        line.push('\n');
        self.writer.write_all(line.as_bytes())?;
        self.writer.flush()?;
        let mut answer = String::new();
        loop {
            answer.clear();
            if self.reader.read_line(&mut answer)? == 0 {
                return Err(std::io::ErrorKind::UnexpectedEof.into());
            }
            // A notification has a method and no id; the answer an id.
            match serde_json::from_str::<Value>(&answer) {
                Ok(v) if v.get("id").is_none() && v.get("method").is_some() => {
                    note(&v["params"]);
                }
                _ => break,
            }
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
        Ok(Ok(Answer {
            result,
            binary,
            raw: None,
        }))
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
        let params = if topics.is_empty() {
            json!({})
        } else {
            json!({ "topics": topics })
        };
        let answer = self
            .call(&Basic {
                method: "subscribe",
                params,
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

impl Subscription {
    /// Sends a request on the subscribed connection without waiting for
    /// its answer (the reader passes answers by): `request.cancel`, which
    /// the daemon answers without its lock, from an app whose request
    /// connection is busy with the request it cancels.
    pub fn send(&mut self, req: &impl Call) -> std::io::Result<()> {
        let Some(w) = self.writer.as_mut() else {
            return Err(std::io::ErrorKind::NotConnected.into());
        };
        let line = format!("{}\n", request_line(u64::MAX, &req.method(), req.params()));
        w.write_all(line.as_bytes())?;
        w.flush()
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
            let status = daemon_status(false);
            let line = format!("{}\n", request_line(0, &status.method(), status.params()));
            let _ = w.write_all(line.as_bytes()).and_then(|()| w.flush());
        }
    }
}

fn unavailable(why: impl std::fmt::Display) -> Error {
    Error::new(Code::Daemon, format!("daemon unavailable: {why}"))
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

/// The product's binary that daemons are started from (`ryucad`), and its
/// version.
pub struct Daemon<P: Serve> {
    pub exe: PathBuf,
    pub version: P::Version,
}

impl<P: Serve> Clone for Daemon<P> {
    fn clone(&self) -> Self {
        Daemon {
            exe: self.exe.clone(),
            version: self.version.clone(),
        }
    }
}

impl<P: Serve> std::fmt::Debug for Daemon<P> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Daemon")
            .field("exe", &self.exe)
            .field("version", &self.version)
            .finish()
    }
}

impl<P: Serve> PartialEq for Daemon<P> {
    fn eq(&self, o: &Self) -> bool {
        self.exe == o.exe && self.version == o.version
    }
}

impl<P: Serve> Daemon<P> {
    /// This binary: what the CLI uses.
    pub fn this() -> Result<Daemon<P>> {
        Ok(Daemon {
            exe: std::env::current_exe().map_err(unavailable)?,
            version: P::version(),
        })
    }

    /// The product's binary at `exe`, asked for its version.
    pub fn at(exe: &Path) -> Result<Daemon<P>> {
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
            .map_err(|_| failed(format!("it isn't a {} of this protocol", P::NAME)))?;
        Ok(Daemon {
            exe: exe.to_path_buf(),
            version,
        })
    }

    /// A connection to the daemon of the project at `root` (canonical),
    /// started from this binary if none is running. A daemon of another
    /// build is replaced.
    pub fn connect(&self, root: &Path) -> Result<Connection> {
        ready(root, &Endpoint::<P>::for_project(root)?, self)
    }
}

/// Sends `req` to the project's daemon, starting one if none is running,
/// as the agent participant `name` (`agent` when `None`).
///
/// `Err` means the request was never delivered, so running it in process
/// instead is safe. `Ok` holds the daemon's answer; if the connection broke
/// after sending, that answer is a `daemon` error, because the request may
/// have been applied and must not be repeated.
pub fn request<P: Serve>(
    root: &Path,
    req: &impl Call,
    name: Option<&str>,
) -> Result<Result<Answer>> {
    request_noting::<P>(root, req, name, &Ask::default(), &mut |_| {})
}

/// A connection to the project's daemon (started if none is running,
/// replaced if it is of another build) that has said `session.hello` as
/// the agent `name`. `Err` means no daemon could be reached and nothing
/// was sent, so running in process instead is safe. `Ok(Err(..))` means
/// the daemon refused the hello (a bad name), which is the caller's to fix.
pub fn hello_connection<P: Serve>(root: &Path, name: Option<&str>) -> Result<Result<Connection>> {
    let mut conn = Daemon::<P>::this()?.connect(root)?;
    Ok(conn.hello(name).map_err(unavailable)?.map(|_| conn))
}

/// [`request`] with `ask`'s `progress` and `token`, handing each
/// `progress` notification's params to `note` before the answer.
pub fn request_noting<P: Serve>(
    root: &Path,
    req: &impl Call,
    name: Option<&str>,
    ask: &Ask,
    note: &mut dyn FnMut(&Value),
) -> Result<Result<Answer>> {
    let mut conn = match hello_connection::<P>(root, name)? {
        Ok(c) => c,
        // A refused hello (a bad name) is the agent's to fix; nothing else ran.
        Err(e) => return Ok(Err(e)),
    };
    Ok(conn.call_noting(req, ask, note).unwrap_or_else(|e| {
        Err(Error::new(
            Code::Daemon,
            format!("the daemon stopped answering ({e}); the request may have been applied"),
        ))
    }))
}

/// Asks the project's running daemon to cancel the agent `name`'s
/// request `token` (`agent` when `None`), on a connection of its own that
/// says no hello, so it never waits for the request's lock. Starts no
/// daemon: with none running, nothing is running to cancel.
pub fn cancel<P: Serve>(root: &Path, name: Option<&str>, token: &str) -> Result<Value> {
    let endpoint = Endpoint::<P>::for_project(root)?;
    let Ok(conn) = Connection::connect(&endpoint) else {
        return Ok(json!({"cancelled": false, "token": token}));
    };
    cancel_on(conn, name, token)
}

/// [`cancel`] on a connection already open.
pub fn cancel_on(mut conn: Connection, name: Option<&str>, token: &str) -> Result<Value> {
    let mut params = json!({ "token": token });
    if let Some(n) = name {
        params["as"] = json!(n);
    }
    conn.call(&Basic {
        method: "request.cancel",
        params,
    })
    .map_err(unavailable)?
}

/// Subscribes to `topics` as the agent `name` and collects notifications
/// until there are `count` of them or `timeout` passes.
pub fn watch<P: Serve>(
    root: &Path,
    name: Option<&str>,
    topics: &[String],
    count: usize,
    timeout: Duration,
) -> Result<Value> {
    let mut conn = Daemon::<P>::this()?.connect(root)?;
    conn.call(&hello(name)).map_err(unavailable)??;
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
fn ready<P: Serve>(root: &Path, endpoint: &Endpoint<P>, daemon: &Daemon<P>) -> Result<Connection> {
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
            .call(&daemon_status(false))
            .map_err(unavailable)?
            .map_err(unavailable)?;
        let version = &status["version"];
        // Another product's daemon is never replaced: it isn't this
        // one's to stop.
        if version.get(P::NAME).is_none() {
            return Err(Error::new(
                Code::Daemon,
                format!(
                    "daemon unavailable: the daemon at {} isn't {}'s",
                    endpoint.address,
                    P::NAME
                ),
            )
            .with("version", version));
        }
        if P::same_build(&daemon.version, version) {
            return Ok(conn);
        }
        // A daemon of the build now at `exe`, asked for by a client of an
        // earlier one (an app window left open across an upgrade): it is
        // the one to keep. Replacing it would start the same build again,
        // and every retry would cut its subscriptions (issue #274).
        if !restarted
            && let Ok(now) = Daemon::<P>::at(&daemon.exe)
            && now.version != daemon.version
            && P::same_build(&now.version, version)
        {
            return Err(Error::new(
                Code::Daemon,
                format!(
                    "daemon unavailable: {} was upgraded since this program started; \
                     restart it to use the new version",
                    daemon.exe.display()
                ),
            )
            .with("upgraded", true)
            .with("version", version));
        }
        // Another build's daemon (an upgrade, say): replace it, once.
        if restarted {
            return Err(unavailable("a daemon of another version keeps answering"));
        }
        let _ = conn.call(&daemon_shutdown());
        drop(conn);
        wait_gone(endpoint, START_WAIT);
        restarted = true;
    }
}

fn wait_for<P>(endpoint: &Endpoint<P>) -> Result<Connection> {
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

/// Waits up to `wait` for nothing to answer on `endpoint`: whether it went.
fn wait_gone<P>(endpoint: &Endpoint<P>, wait: Duration) -> bool {
    let start = Instant::now();
    loop {
        if Connection::connect(endpoint).is_err() {
            return true;
        }
        if start.elapsed() >= wait {
            return false;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// How long `daemon stop` waits for a daemon to go: a shutdown cancels
/// what is running, which stops at its next boundary, but a request that
/// has begun writing finishes first (#391).
const STOP_WAIT: Duration = Duration::from_secs(30);

/// Starts `<exe> daemon run` for `root`, detached, logging to the
/// endpoint's log file.
fn spawn<P: Serve>(root: &Path, endpoint: &Endpoint<P>, exe: &Path) -> Result<()> {
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
    if let Some(secs) = std::env::var(P::IDLE_VAR)
        .ok()
        .filter(|s| s.parse::<u64>().is_ok())
    {
        cmd.args(["--idle-timeout", &secs]);
    }
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(log);
    spawn_detached(&mut cmd).map_err(unavailable)?;
    Ok(())
}

/// A command that runs `exe` and waits for it: what a program without a
/// console of its own (the app, a GUI on Windows) uses to run a product's
/// binary.
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

/// Starts `cmd` detached ([`detach`]): how daemons and the apps, from
/// `open`, are started. On Windows it breaks away from its starter's job,
/// so a shell whose job is killed on close (Windows Terminal's tabs, some
/// agents' shells) takes neither with it (#799); a job that refuses
/// breakaway fails `CreateProcess` with access denied, and the process is
/// then started inside it, as before.
pub fn spawn_detached(cmd: &mut Command) -> std::io::Result<std::process::Child> {
    detach(cmd);
    match cmd.spawn() {
        Err(e) if cfg!(windows) && e.raw_os_error() == Some(5) => {
            detach_without_breakaway(cmd);
            cmd.spawn()
        }
        other => other,
    }
}

/// Out of the terminal's process group, so Ctrl-C on the CLI spares it,
/// and with none of this process's descriptors but the three it is given
/// (#1133, as Windows' handles below): every one from 3 up is made
/// close-on-exec in the child. macOS has no `pipe2`, so a pipe another
/// thread is making can reach a child before it is marked close-on-exec;
/// a daemon that kept it would leave whoever reads its starter's output
/// waiting until the daemon exits. Marked rather than closed, so the pipe
/// that reports a failed exec still does.
#[cfg(unix)]
pub fn detach(cmd: &mut Command) {
    use std::os::unix::process::CommandExt;
    cmd.process_group(0);
    // SAFETY: sysconf is a plain query, made before the fork.
    let limit = match unsafe { libc::sysconf(libc::_SC_OPEN_MAX) } {
        n if n > 3 => n.min(65_536) as libc::c_int,
        _ => 1024,
    };
    // SAFETY: between fork and exec only close_range(2) and fcntl(2) run,
    // both async-signal-safe, and nothing is allocated.
    unsafe {
        cmd.pre_exec(move || {
            #[cfg(target_os = "linux")]
            {
                const CLOSE_RANGE_CLOEXEC: libc::c_uint = 4;
                if libc::syscall(
                    libc::SYS_close_range,
                    3,
                    libc::c_uint::MAX,
                    CLOSE_RANGE_CLOEXEC,
                ) == 0
                {
                    return Ok(());
                }
            }
            for fd in 3..limit {
                let flags = libc::fcntl(fd, libc::F_GETFD);
                if flags >= 0 && flags & libc::FD_CLOEXEC == 0 {
                    libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC);
                }
            }
            Ok(())
        });
    }
}

/// [`detach`]: there is no job to break away from.
#[cfg(unix)]
pub fn detach_without_breakaway(cmd: &mut Command) {
    detach(cmd);
}

/// [`detach_without_breakaway`], and out of its starter's job (#799).
#[cfg(windows)]
pub fn detach(cmd: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
    detach_without_breakaway(cmd);
    cmd.creation_flags(DETACHED | CREATE_BREAKAWAY_FROM_JOB);
}

#[cfg(windows)]
const DETACHED: u32 = 0x0000_0008 | 0x0000_0200; // DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP

/// No console, its own process group, and none of this process's handles:
/// an inherited stdout pipe would keep whoever reads the CLI's output
/// waiting until the daemon exits.
#[cfg(windows)]
pub fn detach_without_breakaway(cmd: &mut Command) {
    use std::os::windows::process::CommandExt;
    use windows_sys::Win32::Foundation::{HANDLE_FLAG_INHERIT, SetHandleInformation};
    use windows_sys::Win32::System::Console::{
        GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
    };
    for which in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
        // SAFETY: plain Win32 calls on this process's own standard handles.
        unsafe {
            let h = GetStdHandle(which);
            if !h.is_null() {
                SetHandleInformation(h, HANDLE_FLAG_INHERIT, 0);
            }
        }
    }
    cmd.creation_flags(DETACHED);
}

/// The daemon's status, without starting one.
pub fn status<P: Serve>(root: &Path) -> Result<Value> {
    status_with::<P>(root, false)
}

/// [`status`], with what the daemon spent by method when `timings`
/// (#849).
pub fn status_with<P: Serve>(root: &Path, timings: bool) -> Result<Value> {
    let endpoint = Endpoint::<P>::for_project(root)?;
    match Connection::connect(&endpoint) {
        Err(_) => Ok(json!({"running": false})),
        Ok(mut conn) => {
            let mut s = conn.call(&daemon_status(timings)).map_err(unavailable)??;
            s["running"] = json!(true);
            Ok(s)
        }
    }
}

/// Asks one endpoint's daemon to stop: whether one was running, and
/// whether it went within [`STOP_WAIT`].
fn stop_endpoint<P>(endpoint: &Endpoint<P>) -> (bool, bool) {
    match Connection::connect(endpoint) {
        Ok(mut conn) => {
            let _ = conn.call(&daemon_shutdown());
            drop(conn);
            (true, wait_gone(endpoint, STOP_WAIT))
        }
        Err(_) => {
            // A socket file nobody answers on is a crashed daemon's.
            #[cfg(unix)]
            let _ = std::fs::remove_file(&endpoint.address);
            (false, true)
        }
    }
}

/// Stops the project's daemon, if one is running. `still_running` says
/// it was asked to and hasn't gone yet (a request it is finishing).
pub fn stop<P: Serve>(root: &Path) -> Result<Value> {
    let (stopped, gone) = stop_endpoint(&Endpoint::<P>::for_project(root)?);
    let mut v = json!({ "stopped": stopped });
    if !gone {
        v["still_running"] = json!(true);
    }
    Ok(v)
}

/// Stops every daemon this user has, of any version: the installers run
/// this before replacing the binary. `still_running` names the endpoints
/// of those asked that hadn't gone after [`STOP_WAIT`], with their logs,
/// so an installer can say so rather than report a clean stop (#1023).
pub fn stop_all<P: Serve>() -> Result<Value> {
    stop_these(&Endpoint::<P>::all()?, stop_endpoint)
}

/// [`stop_all`] over `endpoints`, each stopped by `stop`.
fn stop_these<P>(
    endpoints: &[Endpoint<P>],
    stop: impl Fn(&Endpoint<P>) -> (bool, bool),
) -> Result<Value> {
    let mut stopped = 0;
    let mut still = Vec::new();
    for e in endpoints {
        let (asked, gone) = stop(e);
        if asked {
            stopped += 1;
            if !gone {
                still.push(json!({"address": e.address, "log": e.log}));
            }
        }
    }
    let mut v = json!({ "stopped": stopped });
    if !still.is_empty() {
        v["still_running"] = Value::Array(still);
    }
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Test;
    use interprocess::local_socket::ListenerOptions;
    use std::sync::Mutex;

    /// A daemon on `endpoint` that answers `daemon.status` with `version`
    /// and notes every method it is asked.
    fn fake_daemon(endpoint: &Endpoint<Test>, version: Value) -> Arc<Mutex<Vec<String>>> {
        let listener = ListenerOptions::new()
            .name(endpoint.name().unwrap())
            .create_sync()
            .unwrap();
        let asked = Arc::new(Mutex::new(Vec::new()));
        let noted = Arc::clone(&asked);
        std::thread::spawn(move || {
            for conn in listener.incoming().flatten() {
                let mut reader = BufReader::new(&conn);
                let mut line = String::new();
                while reader.read_line(&mut line).is_ok_and(|n| n > 0) {
                    let v: Value = serde_json::from_str(&line).unwrap();
                    noted
                        .lock()
                        .unwrap()
                        .push(v["method"].as_str().unwrap().to_owned());
                    let answer =
                        json!({"jsonrpc": "2.0", "id": v["id"], "result": {"version": version}});
                    let mut w = &conn;
                    w.write_all(format!("{answer}\n").as_bytes()).unwrap();
                    line.clear();
                }
            }
        });
        asked
    }

    /// An endpoint in `dir` of its own, without the runtime folder (whose
    /// variable another test of this binary sets).
    fn endpoint_in(dir: &Path, name: &str) -> Endpoint<Test> {
        #[cfg(unix)]
        let address = dir
            .join(format!("{name}.sock"))
            .to_string_lossy()
            .into_owned();
        #[cfg(windows)]
        let address = format!("ryutest-client-{name}-{}", std::process::id());
        Endpoint {
            address,
            log: dir.join(format!("{name}.log")),
            product: std::marker::PhantomData,
        }
    }

    fn this_build(dir: &Path) -> Daemon<Test> {
        Daemon {
            exe: dir.join("no-such-binary"),
            version: Test::version(),
        }
    }

    /// #1023: a daemon asked to stop that outlives the wait is named,
    /// with its log, not counted as a clean stop and forgotten.
    #[test]
    fn stop_all_names_a_daemon_that_didnt_go() {
        let dir = tempfile::tempdir().unwrap();
        let es: Vec<_> = ["gone", "stuck", "none"]
            .iter()
            .map(|n| endpoint_in(dir.path(), n))
            .collect();
        let v = stop_these(&es, |e| {
            if e.log.ends_with("gone.log") {
                (true, true)
            } else if e.log.ends_with("stuck.log") {
                (true, false)
            } else {
                (false, true)
            }
        })
        .unwrap();
        assert_eq!(v["stopped"], 2, "{v}");
        let still = v["still_running"].as_array().unwrap();
        assert_eq!(still.len(), 1, "{v}");
        assert_eq!(still[0]["address"], es[1].address.as_str());
        assert_eq!(still[0]["log"], json!(es[1].log));
        let v = stop_these(&es[..1], |_| (true, true)).unwrap();
        assert_eq!(v, json!({"stopped": 1}));
    }

    #[test]
    fn another_products_daemon_is_refused_never_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let endpoint = endpoint_in(dir.path(), "other");
        let asked = fake_daemon(&endpoint, json!({"ryuother": "0.0.0", "protocol": 1}));
        let e = match ready(dir.path(), &endpoint, &this_build(dir.path())) {
            Ok(_) => panic!("connected to another product's daemon"),
            Err(e) => e,
        };
        assert_eq!(e.code, Code::Daemon);
        assert!(e.message.contains("isn't ryutest's"), "{}", e.message);
        // Asked who it is, and nothing else: it was never told to stop.
        assert_eq!(*asked.lock().unwrap(), ["daemon.status"]);
    }

    #[test]
    fn the_same_products_daemon_is_used() {
        let dir = tempfile::tempdir().unwrap();
        let endpoint = endpoint_in(dir.path(), "same");
        let asked = fake_daemon(&endpoint, Test::version());
        assert!(ready(dir.path(), &endpoint, &this_build(dir.path())).is_ok());
        assert_eq!(*asked.lock().unwrap(), ["daemon.status"]);
    }

    #[test]
    fn basic_requests_travel_as_products_have_always_sent_them() {
        let line = |c: &Basic| {
            let v: Value = serde_json::from_str(&request_line(0, &c.method(), c.params())).unwrap();
            v
        };
        assert_eq!(
            line(&daemon_status(false)),
            json!({"jsonrpc": "2.0", "id": 0, "method": "daemon.status", "params": {}})
        );
        assert_eq!(line(&daemon_shutdown())["params"], json!({}));
        assert_eq!(line(&hello(None))["params"], json!({"kind": "agent"}));
        assert_eq!(
            line(&hello(Some("reviewer")))["params"],
            json!({"kind": "agent", "name": "reviewer"})
        );
    }
}
