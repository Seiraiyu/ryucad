//! The session daemon: one long-running process per project that keeps the
//! loaded project and its rebuilds in memory. It serves newline-delimited
//! JSON-RPC 2.0 on the project's [`Endpoint`], one request at a time.

use crate::doc::project::MANIFEST;
use crate::endpoint::Endpoint;
use crate::error::{Code, Error, Result};
use crate::live::Live;
use crate::notify::{self, Subscriber};
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
            let before = core.live.snapshot();
            core.live.expire(Instant::now());
            core.live.announce(&before);
            // Someone watching keeps the daemon alive.
            if core.live.hub.is_watched() {
                *lock(&watcher.last) = Instant::now();
            }
            if lock(&watcher.last).elapsed() >= watcher.idle {
                stop(&watcher, "idle");
            }
        }
    });
    for conn in listener.incoming() {
        match conn {
            Ok(stream) => {
                let state = Arc::clone(&state);
                let conn = Arc::new(Conn {
                    stream,
                    write: Mutex::new(()),
                });
                std::thread::spawn(move || {
                    let (mut me, mut sub) = (None, None);
                    serve(&conn, &state, &mut me, &mut sub);
                    if let Some(s) = sub {
                        s.close();
                    }
                    if let Some(id) = me {
                        let mut core = lock(&state.core);
                        let before = core.live.snapshot();
                        core.live.goodbye(&id);
                        core.live.announce(&before);
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

/// A client connection. Answers and notifications are written whole under
/// `write`, so lines from the two never interleave.
struct Conn {
    stream: Stream,
    write: Mutex<()>,
}

impl Conn {
    fn send(&self, line: &str) -> std::io::Result<()> {
        let _g = lock(&self.write);
        let mut w = &self.stream;
        w.write_all(line.as_bytes())?;
        w.write_all(b"\n")?;
        w.flush()
    }
}

/// Answers requests on one connection until the client hangs up. `me` is
/// the participant the connection said hello as, and `sub` its
/// subscription.
fn serve(
    conn: &Arc<Conn>,
    state: &State,
    me: &mut Option<String>,
    sub: &mut Option<Arc<Subscriber>>,
) {
    let mut reader = BufReader::new(&conn.stream);
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
        let mut subscribed = None;
        let result = match req {
            Ok(Request::DaemonStatus {}) => Ok(status(state)),
            Ok(Request::DaemonShutdown {}) => {
                shutdown = true;
                Ok(json!({"stopping": true, "pid": std::process::id()}))
            }
            Ok(Request::Subscribe { topics }) => {
                subscribe(state, me.as_deref(), sub.is_some(), &topics).map(|(s, v)| {
                    subscribed = Some(s);
                    v
                })
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
                        let before = live.snapshot();
                        live.goodbye(&old);
                        live.announce(&before);
                    }
                }
                *lock(&state.last) = Instant::now();
                r
            }
            Err(e) => Err(e),
        };
        if conn.send(&rpc_response(id, &result)).is_err() {
            if let Some(s) = subscribed {
                s.close();
            }
            return;
        }
        // Notifications start only after the answer is out.
        if let Some(s) = subscribed {
            let (c, pump) = (Arc::clone(conn), Arc::clone(&s));
            std::thread::spawn(move || {
                while let Some(line) = pump.next() {
                    if c.send(&line).is_err() {
                        pump.close();
                    }
                }
            });
            *sub = Some(s);
        }
        if shutdown {
            // Wait for any request in progress, then go.
            let _busy = lock(&state.core);
            stop(state, "asked to shut down");
        }
    }
}

/// Opens a subscription for the connection's participant.
fn subscribe(
    state: &State,
    me: Option<&str>,
    already: bool,
    names: &[String],
) -> Result<(Arc<Subscriber>, Value)> {
    let Some(me) = me else {
        return Err(Error::new(
            Code::Usage,
            "say session.hello on this connection before subscribe",
        ));
    };
    if already {
        return Err(Error::new(
            Code::Usage,
            "this connection is already subscribed",
        ));
    }
    let topics = notify::topics(names)?;
    let mut core = lock(&state.core);
    let sub = core.live.hub.subscribe(me, &topics);
    let shown = if topics.is_empty() {
        notify::Topic::ALL.to_vec()
    } else {
        topics
    };
    Ok((
        sub,
        json!({"subscribed": shown, "session": core.live.list()}),
    ))
}
