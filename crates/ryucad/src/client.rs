//! The CLI's side of the daemon: connect (starting the daemon if needed),
//! check that it is this exact binary, and send requests.

use crate::daemon::IDLE_VAR;
use crate::endpoint::Endpoint;
use crate::error::{Code, Error, Result};
use crate::live::Kind;
use crate::proto::{Request, VersionInfo, parse_rpc_response, rpc_request};
use interprocess::local_socket::{RecvHalf, SendHalf, Stream, prelude::*};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
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
        Ok(parse_rpc_response(&answer))
    }
}

impl Connection {
    /// Hands the connection's incoming lines to a thread, which parses
    /// them onto the returned channel; the send half is kept so the
    /// connection stays open.
    fn into_lines(self) -> (mpsc::Receiver<Value>, SendHalf) {
        let (tx, rx) = mpsc::channel();
        let mut reader = self.reader;
        std::thread::spawn(move || {
            let mut line = String::new();
            while reader.read_line(&mut line).is_ok_and(|n| n > 0) {
                let Ok(v) = serde_json::from_str(&line) else {
                    return;
                };
                if tx.send(v).is_err() {
                    return;
                }
                line.clear();
            }
        });
        (rx, self.writer)
    }
}

fn unavailable(why: impl std::fmt::Display) -> Error {
    Error::new(Code::Daemon, format!("daemon unavailable: {why}"))
}

/// Sends `req` to the project's daemon, starting one if none is running,
/// as the agent participant `name` (`agent` when `None`).
///
/// `Err` means the request was never delivered, so running it in process
/// instead is safe. `Ok` holds the daemon's answer; if the connection broke
/// after sending, that answer is a `daemon` error, because the request may
/// have been applied and must not be repeated.
pub fn request(root: &Path, req: &Request, name: Option<&str>) -> Result<Result<Value>> {
    let endpoint = Endpoint::for_project(root)?;
    let mut conn = ready(root, &endpoint)?;
    let hello = Request::SessionHello {
        kind: Kind::Agent,
        name: name.map(str::to_owned),
        participant: None,
    };
    // A refused hello (a bad name) is the agent's to fix; nothing else ran.
    if let Err(e) = conn.call(&hello).map_err(unavailable)? {
        return Ok(Err(e));
    }
    Ok(conn.call(req).unwrap_or_else(|e| {
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
    let endpoint = Endpoint::for_project(root)?;
    let mut conn = ready(root, &endpoint)?;
    let hello = Request::SessionHello {
        kind: Kind::Agent,
        name: name.map(str::to_owned),
        participant: None,
    };
    conn.call(&hello).map_err(unavailable)??;
    let sub = Request::Subscribe {
        topics: topics.to_vec(),
    };
    conn.call(&sub).map_err(unavailable)??;
    let (rx, _keep) = conn.into_lines();
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

/// A connection to a daemon running this exact binary.
fn ready(root: &Path, endpoint: &Endpoint) -> Result<Connection> {
    let mut restarted = false;
    loop {
        let mut conn = match Connection::connect(endpoint) {
            Ok(c) => c,
            Err(_) => {
                spawn(root, endpoint)?;
                wait_for(endpoint)?
            }
        };
        let status = conn
            .call(&Request::DaemonStatus {})
            .map_err(unavailable)?
            .map_err(unavailable)?;
        if status["version"] == json!(VersionInfo::current()) {
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

/// Starts `ryucad daemon run` for `root`, detached, logging to the
/// endpoint's log file.
fn spawn(root: &Path, endpoint: &Endpoint) -> Result<()> {
    if std::fs::metadata(&endpoint.log).is_ok_and(|m| m.len() > LOG_LIMIT) {
        let _ = std::fs::rename(&endpoint.log, endpoint.log.with_extension("log.1"));
    }
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&endpoint.log)
        .map_err(|e| Error::io(&endpoint.log, &e))?;
    let exe = std::env::current_exe().map_err(unavailable)?;
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
