//! The session daemon: one long-running process per project that keeps the
//! loaded project and its rebuilds in memory. It serves newline-delimited
//! JSON-RPC 2.0 on the project's [`Endpoint`], one request at a time.

use crate::doc::project::MANIFEST;
use crate::endpoint::Endpoint;
use crate::error::{Code, Error, Result};
use crate::live::Live;
use crate::proto::{Request, VersionInfo, parse_rpc_request, rpc_response};
use crate::server::handle_safely;
use crate::session::Session;
use interprocess::local_socket::{ListenerOptions, Stream, prelude::*};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// How long a daemon waits for a request before it exits.
pub const DEFAULT_IDLE: Duration = Duration::from_secs(30 * 60);

/// Seconds of idle time for daemons the CLI starts; tests shorten it.
pub const IDLE_VAR: &str = "RYUCAD_IDLE_TIMEOUT";

struct State {
    root: PathBuf,
    endpoint: Endpoint,
    idle: Duration,
    started: Instant,
    last: Mutex<Instant>,
    core: Mutex<Core>,
}

/// What requests work on, under one lock: the project's session (opened on
/// first use) and the live session of who is attached.
struct Core {
    session: Option<Session>,
    live: Live,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    // Handlers run behind a panic boundary, so a poisoned lock holds no
    // broken state.
    m.lock().unwrap_or_else(|p| p.into_inner())
}

fn log(msg: &str) {
    eprintln!(
        "{} {msg}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs())
    );
}

/// Runs the daemon for the project at `root` until it is idle for `idle`,
/// asked to shut down, or the project disappears. Never returns on success.
pub fn run(root: &Path, idle: Duration) -> Result<()> {
    let endpoint = Endpoint::for_project(root)?;
    let listener = bind(&endpoint)?;
    log(&format!(
        "serving {} on {}",
        root.display(),
        endpoint.address
    ));
    let state = Arc::new(State {
        root: root.to_path_buf(),
        endpoint,
        idle,
        started: Instant::now(),
        last: Mutex::new(Instant::now()),
        core: Mutex::new(Core {
            session: None,
            live: Live::new(),
        }),
    });
    let watcher = Arc::clone(&state);
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(Duration::from_millis(250).min(watcher.idle));
            if !watcher.root.join(MANIFEST).is_file() {
                stop(&watcher, "the project is gone");
            }
            // Hold the lock so no request is mid-flight.
            let mut core = lock(&watcher.core);
            core.live.expire(Instant::now());
            if lock(&watcher.last).elapsed() >= watcher.idle {
                stop(&watcher, "idle");
            }
        }
    });
    for conn in listener.incoming() {
        match conn {
            Ok(conn) => {
                let state = Arc::clone(&state);
                std::thread::spawn(move || {
                    let mut me = None;
                    serve(&conn, &state, &mut me);
                    if let Some(id) = me {
                        lock(&state.core).live.goodbye(&id);
                    }
                });
            }
            Err(e) => log(&format!("accept failed: {e}")),
        }
    }
    Ok(())
}

/// Binds the endpoint. Binding is the lock: a second daemon for the same
/// project fails here. A socket file left by a crashed daemon is removed.
fn bind(endpoint: &Endpoint) -> Result<interprocess::local_socket::Listener> {
    let daemon_err = |e: std::io::Error| {
        Error::new(
            Code::Daemon,
            format!("can't listen on {}: {e}", endpoint.address),
        )
    };
    let create = || {
        let opts = ListenerOptions::new().name(endpoint.name()?);
        #[cfg(windows)]
        let opts = {
            use interprocess::os::windows::local_socket::ListenerOptionsExt;
            opts.security_descriptor(owner_only()?)
        };
        opts.create_sync()
    };
    match create() {
        Ok(l) => Ok(l),
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
            if Stream::connect(endpoint.name().map_err(daemon_err)?).is_ok() {
                return Err(Error::new(
                    Code::Daemon,
                    "a daemon is already running for this project",
                ));
            }
            // Nobody answers: a crashed daemon's socket file (Unix only;
            // a dead daemon's pipe vanishes with it on Windows).
            if cfg!(unix) {
                log("removing a stale socket");
                let _ = std::fs::remove_file(&endpoint.address);
                return create().map_err(daemon_err);
            }
            Err(daemon_err(e))
        }
        Err(e) => Err(daemon_err(e)),
    }
}

/// A descriptor that lets only the pipe's owner (and the system) open it.
#[cfg(windows)]
fn owner_only()
-> std::io::Result<interprocess::os::windows::security_descriptor::SecurityDescriptor> {
    let sddl = widestring::U16CString::from_str("D:P(A;;GA;;;OW)(A;;GA;;;SY)")
        .map_err(std::io::Error::other)?;
    interprocess::os::windows::security_descriptor::SecurityDescriptor::deserialize(&sddl)
}

fn stop(state: &State, why: &str) -> ! {
    log(&format!("stopping: {why}"));
    #[cfg(unix)]
    let _ = std::fs::remove_file(&state.endpoint.address);
    #[cfg(windows)]
    let _ = &state.endpoint;
    std::process::exit(0)
}

fn status(state: &State) -> Value {
    json!({
        "pid": std::process::id(),
        "root": state.root,
        "uptime_s": state.started.elapsed().as_secs(),
        "idle_timeout_s": state.idle.as_secs(),
        "version": VersionInfo::current(),
    })
}

/// Answers requests on one connection until the client hangs up. `me` is
/// the participant the connection said hello as.
fn serve(conn: &Stream, state: &State, me: &mut Option<String>) {
    let mut reader = BufReader::new(conn);
    let mut writer = conn;
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        *lock(&state.last) = Instant::now();
        let (id, req) = parse_rpc_request(line.trim_end());
        let mut shutdown = false;
        let result = match req {
            Ok(Request::DaemonStatus {}) => Ok(status(state)),
            Ok(Request::DaemonShutdown {}) => {
                shutdown = true;
                Ok(json!({"stopping": true, "pid": std::process::id()}))
            }
            Ok(req) => {
                let hello = matches!(req, Request::SessionHello { .. });
                let mut core = lock(&state.core);
                let Core { session, live } = &mut *core;
                let r = handle_safely(session, Some(live), me.as_deref(), &state.root, req);
                if hello && let Ok(v) = &r {
                    let id = v["participant"].as_str().map(str::to_owned);
                    // Saying hello again moves the connection.
                    if let Some(old) = std::mem::replace(me, id) {
                        live.goodbye(&old);
                    }
                }
                *lock(&state.last) = Instant::now();
                r
            }
            Err(e) => Err(e),
        };
        let mut out = rpc_response(id, &result);
        out.push('\n');
        if writer
            .write_all(out.as_bytes())
            .and_then(|()| writer.flush())
            .is_err()
        {
            return;
        }
        if shutdown {
            // Wait for any request in progress, then go.
            let _busy = lock(&state.core);
            stop(state, "asked to shut down");
        }
    }
}
