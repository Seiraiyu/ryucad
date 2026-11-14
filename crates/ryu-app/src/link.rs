//! A window's connection to a project's daemon: one connection for
//! requests, one aside for requests that may wait on the network, and
//! one for notifications, all as the same human participant. Requests
//! run on threads of their own, so the window never waits on the daemon;
//! answers and notifications arrive as one stream of events. Generic over
//! the product's [`Client`]. Moved from ryucad-app by ryueda 3.C.

use ryu_doc::error::{Code, Error, Result};
use ryu_session::client::{Call, Connection, Daemon, Subscription, find_program};
use ryu_session::job::Ask;
use ryu_session::live::Kind;
use ryu_session::rpc::Answer;
use ryu_session::{Product, Serve};
use serde_json::{Value, json};
use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

/// A product's requests.
pub type Req<A> = <<A as Client>::Product as Serve>::Request;
/// A product's version.
pub type Version<A> = <<A as Client>::Product as Serve>::Version;

/// What a window's link needs of its product beyond `Serve`.
///
/// ```no_run
/// fn drain<A: ryu_app::link::Client>(l: &mut ryu_app::link::Link<A>) -> usize {
///     let mut n = 0;
///     while l.try_event().is_some() {
///         n += 1;
///     }
///     n
/// }
/// ```
pub trait Client: 'static {
    type Product: Serve;
    /// The binary daemons are started from (`ryucad`).
    const PROGRAM: &'static str;
    /// `session.hello`.
    fn hello(kind: Kind, name: Option<String>, participant: Option<String>) -> Req<Self>;
    /// `request.cancel` for one of this window's long requests.
    fn cancel(token: String) -> Req<Self>;
    /// A build as people read it: `ryucad 0.9.0 (abc1234)`.
    fn build(v: &Version<Self>) -> String;
}

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

/// The product's binary the app starts daemons from: beside the app,
/// else on `PATH`, and of the app's own build.
pub fn find<A: Client>() -> Result<Daemon<A::Product>> {
    let exe = find_program(A::PROGRAM).ok_or_else(|| {
        Error::new(
            Code::Daemon,
            format!(
                "can't find the {} command beside the app or on PATH",
                A::PROGRAM
            ),
        )
    })?;
    let daemon = Daemon::at(&exe)?;
    check_build::<A>(&daemon)?;
    Ok(daemon)
}

/// Refuses a daemon binary of another build than the app.
pub fn check_build<A: Client>(daemon: &Daemon<A::Product>) -> Result<()> {
    let app = <A::Product as Serve>::version();
    if <A::Product as Serve>::same_build(&app, &json!(daemon.version)) {
        return Ok(());
    }
    Err(Error::new(
        Code::Daemon,
        format!(
            "this app is {} but {} is {}; install them together",
            A::build(&app),
            daemon.exe.display(),
            A::build(&daemon.version)
        ),
    )
    .with("app", &app)
    .with(<A::Product as Product>::NAME, &daemon.version))
}

/// A live connection to a project's daemon.
pub struct Link<A: Client> {
    /// This window's participant ID.
    pub participant: String,
    /// The session as it was when the window subscribed.
    pub session: Value,
    jobs: Sender<(u64, Req<A>, Ask)>,
    /// Requests that may wait on the network (fetching dependencies), on
    /// a connection and thread of their own, so the window's other
    /// requests never wait behind them.
    aside: Sender<(u64, Req<A>, Ask)>,
    events: Receiver<Event>,
    next: u64,
    /// Dropped with the link, which closes the subscription; it also
    /// carries cancels, since the request connection is busy with what
    /// they cancel.
    sub: Subscription,
}

impl<A: Client> Link<A>
where
    Req<A>: 'static,
{
    /// Connects to the daemon of the project at `root` (canonical),
    /// starting it from `daemon` if needed, says hello as the human `name`
    /// and subscribes to everything. `wake` is called for every event.
    pub fn open(root: &Path, daemon: &Daemon<A::Product>, name: &str, wake: Wake) -> Result<Self> {
        let mut requests = daemon.connect(root)?;
        let mut notes = daemon.connect(root)?;
        let mut aside = daemon.connect(root)?;
        let hello = requests
            .call(&A::hello(Kind::Human, Some(name.to_owned()), None))
            .map_err(lost)??;
        let participant = hello["participant"].as_str().unwrap_or_default().to_owned();
        notes
            .call(&A::hello(Kind::Human, None, Some(participant.clone())))
            .map_err(lost)??;
        aside
            .call(&A::hello(Kind::Human, None, Some(participant.clone())))
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
        spawn_requests(requests, todo, tx.clone(), Arc::clone(&wake));
        let (aside_jobs, aside_todo) = mpsc::channel();
        spawn_requests(aside, aside_todo, tx, wake);
        Ok(Link {
            participant,
            session: subscribed["session"].clone(),
            jobs,
            aside: aside_jobs,
            events,
            next: 1,
            sub,
        })
    }

    /// Queues a request; its answer arrives as [`Event::Answer`] with the
    /// returned ticket.
    pub fn send(&mut self, req: Req<A>) -> u64 {
        self.send_job(req, Ask::default())
    }

    /// [`Link::send`] for a long request: `ask`'s `progress` and `token`
    /// go with it, and its progress lines arrive as [`Event::Progress`].
    pub fn send_job(&mut self, req: Req<A>, ask: Ask) -> u64 {
        let ticket = self.next;
        self.next += 1;
        // A dead request thread has already sent `Lost`.
        let _ = self.jobs.send((ticket, req, ask));
        ticket
    }

    /// [`Link::send_job`] on the second request connection: for requests
    /// that wait on another repository, which mustn't hold up the
    /// window's others. Tickets are shared, so answers land as any do.
    pub fn send_aside(&mut self, req: Req<A>, ask: Ask) -> u64 {
        let ticket = self.next;
        self.next += 1;
        let _ = self.aside.send((ticket, req, ask));
        ticket
    }

    /// Asks the daemon to cancel this window's long request `token`; its
    /// answer (`cancelled`) comes as the request's own.
    pub fn cancel(&mut self, token: &str) {
        let req = A::cancel(token.to_owned());
        // A lost daemon has nothing running.
        let _ = self.sub.send(&req);
    }

    /// Waits up to `timeout` for the answer to `ticket`, dropping every
    /// other event on the way: for a window about to close, which has no
    /// use for them. Whether the daemon answered (a refusal is an answer);
    /// false once the link is lost or the time is up.
    pub fn wait_answer(&self, ticket: u64, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.wait_event(left) {
                Some(Event::Answer { ticket: t, .. }) if t == ticket => return true,
                Some(Event::Lost) | None => return false,
                Some(_) => {}
            }
        }
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

fn spawn_requests<R: Call + Send + 'static>(
    mut conn: Connection,
    todo: Receiver<(u64, R, Ask)>,
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
