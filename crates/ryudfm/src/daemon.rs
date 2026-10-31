//! The project's daemon: `ryu-session`'s loop, answering ryudfm's
//! requests through [`crate::server`].

use crate::Dfm;
use crate::error::Result;
use crate::proto::{Answer, Request, VersionInfo};
use crate::session::Session;
use ryu_session::daemon::Control;
use ryu_session::live::{Kind, Live};
use serde_json::Value;
use std::path::Path;
use std::time::Duration;

pub use ryu_session::daemon::DEFAULT_IDLE;

/// Runs the daemon for the project at `root` until it is idle for `idle`,
/// asked to shut down, or the project disappears.
pub fn run(root: &Path, idle: Duration) -> Result<()> {
    ryu_session::daemon::run::<Dfm>(root, idle)
}

impl ryu_session::Serve for Dfm {
    const MANIFEST: &'static str = crate::project::MANIFEST;
    const IDLE_VAR: &'static str = "RYUDFM_IDLE_TIMEOUT";
    type Session = Session;
    type Request = Request;
    type Version = VersionInfo;

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
            _ => Control::Request,
        }
    }

    fn handle(
        session: &mut Option<Session>,
        live: &mut Live<Dfm>,
        me: Option<&str>,
        root: &Path,
        req: Request,
        _: &ryu_session::job::Job,
    ) -> Result<Answer> {
        crate::server::handle_safely(session, Some(live), me, root, req)
    }

    fn settle(session: &mut Option<Session>, live: &mut Live<Dfm>, root: &Path) {
        crate::server::settle(session, live, root)
    }

    fn poll_ms() -> u64 {
        ryu_session::prefs::Preferences::<Dfm>::load()
            .ok()
            .and_then(|p| p.values.get("daemon.poll_ms")?.as_u64())
            .unwrap_or(250)
    }

    /// There is no app yet, and no list of recent projects.
    fn window_opened(_: &Path) {}
}
