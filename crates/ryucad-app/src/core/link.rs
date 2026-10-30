//! The app's connection to a project's daemon: one connection for
//! requests and one for notifications, both as the same human participant.
//! Requests run on a thread of their own, so the window never waits on
//! the daemon; answers and notifications arrive as one stream of events.

use ryucad::client::Ask;
use ryucad::client::{Connection, Daemon, Subscription, find_program};
use ryucad::error::{Code, Error, Result};
use ryucad::live::Kind;
use ryucad::proto::{Answer, Request, VersionInfo};
use serde_json::{Value, json};
use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::Duration;

/// Called whenever an event arrives: the window repaints.
pub type Wake = Arc<dyn Fn() + Send + Sync>;

/// What the link hears.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// The answer to the request `send` gave this ticket.
    Answer { ticket: u64, answer: Result<Answer> },
    /// A notification.
    Note { method: String, params: Value },
    /// A progress line for the long request `send_job` gave this ticket.
    Progress { ticket: u64, params: Value },
    /// The daemon went away; the link is dead.
    Lost,
}

/// The `ryucad` binary the app starts daemons from: beside the app, else
/// on `PATH`, and of the app's own build.
pub fn find_ryucad() -> Result<Daemon> {
    let exe = find_program("ryucad").ok_or_else(|| {
        Error::new(
            Code::Daemon,
            "can't find the ryucad command beside the app or on PATH",
        )
    })?;
    let daemon = Daemon::at(&exe)?;
    check_build(&daemon)?;
    Ok(daemon)
}

/// Refuses a `ryucad` of another build than the app.
pub fn check_build(daemon: &Daemon) -> Result<()> {
    let app = VersionInfo::current();
    if app.same_build(&json!(daemon.version)) {
        return Ok(());
    }
    Err(Error::new(
        Code::Daemon,
        format!(
            "this app is ryucad {} ({}) but {} is ryucad {} ({}); install them together",
            app.ryucad,
            app.commit,
            daemon.exe.display(),
            daemon.version.ryucad,
            daemon.version.commit
        ),
    )
    .with("app", &app)
    .with("ryucad", &daemon.version))
}

/// A live connection to a project's daemon.
pub struct Link {
    /// This window's participant ID.
    pub participant: String,
    /// The session as it was when the window subscribed.
    pub session: Value,
    jobs: Sender<(u64, Request, Ask)>,
    events: Receiver<Event>,
    next: u64,
    /// Dropped with the link, which closes the subscription; it also
    /// carries cancels, since the request connection is busy with what
    /// they cancel.
    sub: Subscription,
}

impl Link {
    /// Connects to the daemon of the project at `root` (canonical),
    /// starting it from `daemon` if needed, says hello as the human `name`
    /// and subscribes to everything. `wake` is called for every event.
    pub fn open(root: &Path, daemon: &Daemon, name: &str, wake: Wake) -> Result<Link> {
        let mut requests = daemon.connect(root)?;
        let mut notes = daemon.connect(root)?;
        let hello = requests
            .call(&Request::SessionHello {
                kind: Kind::Human,
                name: Some(name.to_owned()),
                participant: None,
            })
            .map_err(lost)??;
        let participant = hello["participant"].as_str().unwrap_or_default().to_owned();
        notes
            .call(&Request::SessionHello {
                kind: Kind::Human,
                name: None,
                participant: Some(participant.clone()),
            })
            .map_err(lost)??;
        let (tx, events) = mpsc::channel();
        let notes_tx = tx.clone();
        let notes_wake = Arc::clone(&wake);
        let (subscribed, sub) = notes.subscribe(&[], move |v| {
            let event = match v {
                Some(v) => Event::Note {
                    method: v["method"].as_str().unwrap_or_default().to_owned(),
                    params: v["params"].clone(),
                },
                None => Event::Lost,
            };
            if notes_tx.send(event).is_ok() {
                notes_wake();
            }
        })?;
        let (jobs, todo) = mpsc::channel();
        spawn_requests(requests, todo, tx, wake);
        Ok(Link {
            participant,
            session: subscribed["session"].clone(),
            jobs,
            events,
            next: 1,
            sub,
        })
    }

    /// Queues a request; its answer arrives as [`Event::Answer`] with the
    /// returned ticket.
    pub fn send(&mut self, req: Request) -> u64 {
        self.send_job(req, Ask::default())
    }

    /// [`Link::send`] for a long request: `ask`'s `progress` and `token`
    /// go with it, and its progress lines arrive as [`Event::Progress`].
    pub fn send_job(&mut self, req: Request, ask: Ask) -> u64 {
        let ticket = self.next;
        self.next += 1;
        // A dead request thread has already sent `Lost`.
        let _ = self.jobs.send((ticket, req, ask));
        ticket
    }

    /// Asks the daemon to cancel this window's long request `token`; its
    /// answer (`cancelled`) comes as the request's own.
    pub fn cancel(&mut self, token: &str) {
        let req = Request::RequestCancel {
            token: token.to_owned(),
            name: None,
        };
        // A lost daemon has nothing running.
        let _ = self.sub.send(&req);
    }

    /// The next event, if one has arrived.
    pub fn try_event(&self) -> Option<Event> {
        self.events.try_recv().ok()
    }

    /// The next event, waiting up to `timeout`; `Lost` once both threads
    /// have stopped.
    pub fn wait_event(&self, timeout: Duration) -> Option<Event> {
        match self.events.recv_timeout(timeout) {
            Ok(e) => Some(e),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => Some(Event::Lost),
        }
    }
}

fn lost(e: std::io::Error) -> Error {
    Error::new(Code::Daemon, format!("the daemon stopped answering: {e}"))
}

fn spawn_requests(
    mut conn: Connection,
    todo: Receiver<(u64, Request, Ask)>,
    tx: Sender<Event>,
    wake: Wake,
) {
    std::thread::spawn(move || {
        for (ticket, req, ask) in todo {
            let mut note = |p: &Value| {
                let params = p.clone();
                if tx.send(Event::Progress { ticket, params }).is_ok() {
                    wake();
                }
            };
            let event = match conn.call_noting(&req, &ask, &mut note) {
                Ok(answer) => Event::Answer { ticket, answer },
                Err(_) => Event::Lost,
            };
            let dead = event == Event::Lost;
            if tx.send(event).is_err() {
                return;
            }
            wake();
            if dead {
                return;
            }
        }
    });
}
