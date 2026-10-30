//! The session daemon: one long-running process per project that keeps the
//! loaded project and its rebuilds in memory. It serves newline-delimited
//! JSON-RPC 2.0 on the project's [`Endpoint`](crate::endpoint::Endpoint),
//! one request at a time (display meshes are made outside its lock):
//! `ryu-session`'s loop, answering ryucad's
//! requests through [`crate::server`].

use crate::Cad;
use crate::error::Result;
use crate::live::{Kind, Live};
use crate::proto::{Answer, Request, VersionInfo};
use crate::session::Session;
use ryu_session::daemon::Control;
use ryu_session::job::Job;
use serde_json::Value;
use std::path::Path;
use std::time::Duration;

pub use ryu_session::daemon::DEFAULT_IDLE;

/// Seconds of idle time for daemons the CLI starts; tests shorten it.
pub const IDLE_VAR: &str = <Cad as ryu_session::Serve>::IDLE_VAR;

/// The requests that may take long: each takes `progress` and `token`,
/// reports its progress and stops at its boundaries when cancelled
/// (design 199).
pub const LONG: [&str; 10] = [
    "rebuild",
    "check",
    "doctor",
    "import",
    "interference",
    "export",
    "merge.status",
    "diff",
    "mesh.get",
    "render",
];

/// Runs the daemon for the project at `root` until it is idle for `idle`,
/// asked to shut down, or the project disappears. Never returns on success.
pub fn run(root: &Path, idle: Duration) -> Result<()> {
    ryu_session::daemon::run::<Cad>(root, idle)
}

impl ryu_session::Serve for Cad {
    const MANIFEST: &'static str = crate::doc::project::MANIFEST;
    const IDLE_VAR: &'static str = "RYUCAD_IDLE_TIMEOUT";
    type Session = Session;
    type Request = Request;
    type Version = VersionInfo;
    const LONG: &'static [&'static str] = &LONG;

    fn version() -> VersionInfo {
        VersionInfo::current()
    }

    fn same_build(me: &VersionInfo, other: &Value) -> bool {
        me.same_build(other)
    }

    fn parse(method: &str, params: Option<Value>) -> Result<Request> {
        Request::from_rpc(method, params)
    }

    fn control(req: &Request) -> Control {
        match req {
            Request::DaemonStatus {} => Control::Status,
            Request::DaemonShutdown {} => Control::Shutdown,
            Request::Subscribe { topics } => Control::Subscribe(topics.clone()),
            Request::SessionHello {
                kind, participant, ..
            } => Control::Hello {
                window: *kind == Kind::Human && participant.is_none(),
            },
            Request::RequestCancel { token, name } => Control::Cancel {
                token: token.clone(),
                name: name.clone(),
            },
            _ => Control::Request,
        }
    }

    fn handle(
        session: &mut Option<Session>,
        live: &mut Live,
        me: Option<&str>,
        root: &Path,
        req: Request,
        job: &Job,
    ) -> Result<Answer> {
        crate::server::handle_job(session, Some(live), me, root, req, job)
    }

    fn prepare(
        session: &mut Option<Session>,
        req: &Request,
        job: &Job,
    ) -> Option<ryu_session::Prepared<Session>> {
        crate::server::prepare_job(session, req, job)
    }

    fn status(session: &Session) -> Value {
        serde_json::json!({ "watch": session.watch_status(), "budget": session.budget_status() })
    }

    fn settle(session: &mut Option<Session>, live: &mut Live, root: &Path) {
        crate::server::settle(session, live, root)
    }

    fn poll_ms() -> u64 {
        crate::config::user("daemon.poll_ms")
            .as_u64()
            .unwrap_or(250)
    }

    /// The project goes first among the recent ones. A preferences file
    /// that can't be written isn't the window's problem.
    fn window_opened(root: &Path) {
        let _ = crate::config::remember_project(root);
    }
}
