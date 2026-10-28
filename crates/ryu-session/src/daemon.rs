//! A product's session daemon: one long-running process per project that
//! keeps the loaded project in memory. It serves newline-delimited
//! JSON-RPC 2.0 on the project's [`Endpoint`], one request at a time
//! (but for slow work a request prepares without the lock, [`Serve::prepare`]). The
//! requests and the session are the product's ([`Serve`]); the loop, the
//! subscriptions, idling out and shutting down are the same for every
//! product.

use crate::Serve;
use crate::endpoint::Endpoint;
use crate::live::Live;
use crate::notify::{self, Subscriber, Topics};
use crate::rpc::{Answer, parse_request_line, rpc_response};
use interprocess::local_socket::{ListenerOptions, Stream, prelude::*};
use ryu_doc::error::{Code, Error, Result};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// How long a daemon waits for a request before it exits.
pub const DEFAULT_IDLE: Duration = Duration::from_secs(30 * 60);

/// What the loop does with a request before the product sees it.
pub enum Control {
    /// `daemon.status`: answered by the loop.
    Status,
    /// `daemon.shutdown`: answered, then the daemon stops.
    Shutdown,
    /// `subscribe` to these topics (all when empty).
    Subscribe(Vec<String>),
    /// `session.hello`: the product answers it with `participant`, which
    /// the connection then is. `window` when it is a new app window (a
    /// human, not joining a participant it already has).
    Hello { window: bool },
    /// Anything else: the product's.
    Request,
}

struct State<P: Serve> {
    root: PathBuf,
    endpoint: Endpoint<P>,
    idle: Duration,
    started: Instant,
    last: Mutex<Instant>,
    core: Mutex<Core<P>>,
}

/// What requests work on, under one lock: the project's session (opened on
/// first use) and the live session of who is attached.
struct Core<P: Serve> {
    session: Option<P::Session>,
    live: Live<P>,
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
pub fn run<P: Serve>(root: &Path, idle: Duration) -> Result<()> {
    let endpoint = Endpoint::<P>::for_project(root)?;
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
            live: Live::with_poll(P::poll_ms()),
        }),
    });
    let watcher = Arc::clone(&state);
    std::thread::spawn(move || {
        loop {
            let poll = Duration::from_millis(lock(&watcher.core).live.poll_ms);
            std::thread::sleep(poll.min(watcher.idle));
            if !watcher.root.join(P::MANIFEST).is_file() {
                stop(&watcher, "the project is gone");
            }
            // Hold the lock so no request is mid-flight.
            let mut core = lock(&watcher.core);
            let before = core.live.snapshot();
            core.live.expire(Instant::now());
            core.live.announce(&before);
            // Someone watching keeps the daemon alive, and sees changes
            // made behind its back.
            if core.live.hub.is_watched() {
                *lock(&watcher.last) = Instant::now();
                let Core { session, live } = &mut *core;
                P::settle(session, live, &watcher.root);
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
fn bind<P>(endpoint: &Endpoint<P>) -> Result<interprocess::local_socket::Listener> {
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

fn stop<P: Serve>(state: &State<P>, why: &str) -> ! {
    log(&format!("stopping: {why}"));
    #[cfg(unix)]
    let _ = std::fs::remove_file(&state.endpoint.address);
    #[cfg(windows)]
    let _ = &state.endpoint;
    std::process::exit(0)
}

fn status<P: Serve>(state: &State<P>) -> Value {
    json!({
        "pid": std::process::id(),
        "root": state.root,
        "uptime_s": state.started.elapsed().as_secs(),
        "idle_timeout_s": state.idle.as_secs(),
        "version": P::version(),
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
        self.send_with(line, None)
    }

    /// A line, then the bytes it announces.
    fn send_with(&self, line: &str, bytes: Option<&[u8]>) -> std::io::Result<()> {
        let _g = lock(&self.write);
        let mut w = &self.stream;
        w.write_all(line.as_bytes())?;
        w.write_all(b"\n")?;
        if let Some(b) = bytes {
            w.write_all(b)?;
        }
        w.flush()
    }
}

/// Answers requests on one connection until the client hangs up. `me` is
/// the participant the connection said hello as, and `sub` its
/// subscription.
fn serve<P: Serve>(
    conn: &Arc<Conn>,
    state: &State<P>,
    me: &mut Option<String>,
    sub: &mut Option<Arc<Subscriber<P::Topic>>>,
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
        let (id, req) = parse_request_line(line.trim_end());
        let req = req.and_then(|(method, params)| P::parse(&method, params));
        let mut shutdown = false;
        let mut subscribed = None;
        let result = match req.map(|r| (P::control(&r), r)) {
            Ok((Control::Status, _)) => Ok(Answer::json(status(state))),
            Ok((Control::Shutdown, _)) => {
                shutdown = true;
                Ok(Answer::json(
                    json!({"stopping": true, "pid": std::process::id()}),
                ))
            }
            Ok((Control::Subscribe(topics), _)) => {
                subscribe(state, me.as_deref(), sub.is_some(), &topics).map(|(s, v)| {
                    subscribed = Some(s);
                    Answer::json(v)
                })
            }
            Ok((control, req)) => {
                let hello = matches!(control, Control::Hello { .. });
                // A new app window, rather than its second connection.
                let window = matches!(control, Control::Hello { window: true });
                let mut core = lock_prepared(&state.core, &req);
                let Core { session, live } = &mut *core;
                let r = P::handle(session, live, me.as_deref(), &state.root, req);
                if window && r.is_ok() {
                    P::window_opened(&state.root);
                }
                if hello && let Ok(a) = &r {
                    let id = a.result["participant"].as_str().map(str::to_owned);
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
        let (result, bytes) = match result {
            Ok(a) => (Ok(a.result), a.binary),
            Err(e) => (Err(e), None),
        };
        if conn
            .send_with(&rpc_response(id, &result), bytes.as_deref())
            .is_err()
        {
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

/// The lock, for answering `req`, once the slow work it prepares
/// ([`Serve::prepare`]) is done without it, so other connections' requests
/// don't wait on that work (#91).
fn lock_prepared<'a, P: Serve>(
    core: &'a Mutex<Core<P>>,
    req: &P::Request,
) -> MutexGuard<'a, Core<P>> {
    let prepared = P::prepare(&mut lock(core).session, req);
    let made =
        prepared.and_then(|job| std::panic::catch_unwind(std::panic::AssertUnwindSafe(job)).ok());
    let mut core = lock(core);
    if let (Some(put), Some(s)) = (made, core.session.as_mut()) {
        put(s);
    }
    core
}

/// Opens a subscription for the connection's participant.
fn subscribe<P: Serve>(
    state: &State<P>,
    me: Option<&str>,
    already: bool,
    names: &[String],
) -> Result<(Arc<Subscriber<P::Topic>>, Value)> {
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
    let topics = notify::topics::<P::Topic>(names)?;
    let mut core = lock(&state.core);
    let Core { session, live } = &mut *core;
    // What happened before the subscription isn't news to it.
    P::settle(session, live, &state.root);
    let sub = live.hub.subscribe(me, &topics);
    // Now watched: what is news to it is measured from here.
    P::settle(session, live, &state.root);
    let shown = if topics.is_empty() {
        P::Topic::ALL.to_vec()
    } else {
        topics
    };
    Ok((
        sub,
        json!({"subscribed": shown, "session": core.live.list()}),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{GATE, Request, Test};

    #[test]
    fn prepared_work_runs_without_the_lock() {
        let core = Arc::new(Mutex::new(Core::<Test> {
            session: Some(Vec::new()),
            live: Live::with_poll(250),
        }));
        let slow = {
            let core = Arc::clone(&core);
            std::thread::spawn(move || {
                let core = lock_prepared(&core, &Request("slow".into(), Value::Null));
                core.session.clone().unwrap()
            })
        };
        // Once the slow request's work waits at the gate, another request
        // takes the lock.
        let (gate, open) = &GATE;
        let mut g = lock(gate);
        while *g != 1 {
            g = open.wait(g).unwrap();
        }
        let other = lock_prepared(&core, &Request("quick".into(), Value::Null));
        assert!(other.session.as_ref().unwrap().is_empty());
        drop(other);
        *g = 2;
        open.notify_all();
        drop(g);
        assert_eq!(slow.join().unwrap(), vec!["slow".to_owned()]);
    }
}
