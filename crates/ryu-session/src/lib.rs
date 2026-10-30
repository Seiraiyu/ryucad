//! What every ryu product's session daemon shares. A product (ryucad,
//! ryueda) is a marker type implementing [`Product`]; the modules here are
//! generic over it, and the product names their types for itself
//! (`ryucad::endpoint::Endpoint` is `Endpoint<ryucad::Cad>`), so its own
//! callers never see the parameter.
//!
//! Nothing here reads a product's files or knows its requests: those stay
//! the product's, behind the trait.

pub mod activity;
pub mod client;
pub mod daemon;
pub mod endpoint;
pub mod fresh;
pub mod job;
pub mod live;
pub mod notify;
pub mod prefs;
pub mod rpc;

use serde::Serialize;
use serde_json::{Map, Value};

/// A product built on these sessions: its name, the variables that move
/// its folders, and what its live sessions hold.
pub trait Product: Sized + Send + Sync + 'static {
    /// The product's name: its binary, and the folder and socket names its
    /// daemons use (`ryucad`). Two products never share a daemon.
    const NAME: &'static str;
    /// The variable that overrides the runtime folder (tests keep their
    /// daemons apart with it): `RYUCAD_RUNTIME_DIR`.
    const RUNTIME_DIR_VAR: &'static str;
    /// What participants select, highlight and work on: an entity
    /// reference, written as a string.
    type Target: Clone + PartialEq + Serialize + std::fmt::Debug + Send + Sync;
    /// What else a participant's window shows (ryucad's camera, compare,
    /// focus and hidden set), from its `Default`.
    type View: Default + Clone + std::fmt::Debug + Send + Sync;
    /// What subscribers were last told about the project, from its
    /// `Default`.
    type Seen: Default + std::fmt::Debug + Send + Sync;
    /// What a subscription can ask for.
    type Topic: notify::Topics + std::fmt::Debug;
    /// The folders (relative, ending in `/`) whose files the activity log
    /// counts against its bulky budget, not the project's: ryucad's
    /// `imports/`, STEP files of a hundred megabytes.
    const BULKY: &'static [&'static str] = &[];
    /// Adds a participant's view to its JSON, beside `id`, `kind`, `name`,
    /// `status`, `selection` and `highlights`.
    fn view_json(view: &Self::View, into: &mut Map<String, Value>);
}

/// Work [`Serve::prepare`] found: run without the daemon's lock, it gives
/// back what goes into the session under it.
pub type Prepared<S> = Box<dyn FnOnce() -> Box<dyn FnOnce(&mut S) + Send> + Send>;

/// A product whose daemon these modules run: its requests, its session
/// and how it answers them.
pub trait Serve: Product + prefs::Schema {
    /// The project's manifest (`ryucad.json`): a daemon whose project has
    /// lost it stops.
    const MANIFEST: &'static str;
    /// Seconds of idle time for daemons the CLI starts (tests shorten it):
    /// `RYUCAD_IDLE_TIMEOUT`.
    const IDLE_VAR: &'static str;
    /// What a daemon keeps of the project between requests, opened on its
    /// first.
    type Session: Send;
    /// Every request the daemon answers.
    type Request: client::Call + Send;
    /// What `version` reports: a CLI and a daemon talk only when it says
    /// they are the same build. Its JSON holds a key named after the
    /// product (ryucad's `ryucad`), which is how a client tells another
    /// product's daemon, which it never replaces.
    type Version: Clone + PartialEq + std::fmt::Debug + Serialize + serde::de::DeserializeOwned;

    /// This binary's version.
    fn version() -> Self::Version;
    /// Whether `other` (a `version` object as JSON) is `me`'s build. A
    /// version of another product never is.
    fn same_build(me: &Self::Version, other: &Value) -> bool;
    /// Reads a request from its JSON-RPC method and params.
    fn parse(method: &str, params: Option<Value>) -> ryu_doc::error::Result<Self::Request>;
    /// What the loop does with `req` before handing it on.
    fn control(req: &Self::Request) -> daemon::Control;
    /// Answers a request, opening the session in `session` if it isn't
    /// yet. A panic inside is the product's to catch.
    fn handle(
        session: &mut Option<Self::Session>,
        live: &mut live::Live<Self>,
        me: Option<&str>,
        root: &std::path::Path,
        req: Self::Request,
    ) -> ryu_doc::error::Result<rpc::Answer>;
    /// Slow work `req` can do without the lock every other request waits
    /// on, found under it (ryucad's display meshes the cache doesn't hold
    /// yet). The loop runs it with the lock released, hands what it made
    /// back to the session under the lock, then calls `handle`, which finds
    /// it done. `None` (the default) answers the request under the lock
    /// alone.
    fn prepare(
        session: &mut Option<Self::Session>,
        req: &Self::Request,
    ) -> Option<Prepared<Self::Session>> {
        let _ = (session, req);
        None
    }
    /// What `daemon status` adds about the session, once it is open
    /// (ryucad's file watch and memory budget): `Null` for nothing.
    fn status(session: &Self::Session) -> Value {
        let _ = session;
        Value::Null
    }
    /// Brings the session and what subscribers know up to date with the
    /// files: before a subscription, and while anyone watches.
    fn settle(
        session: &mut Option<Self::Session>,
        live: &mut live::Live<Self>,
        root: &std::path::Path,
    );
    /// How often a watched daemon checks the files, in milliseconds.
    fn poll_ms() -> u64;
    /// A new app window said hello: ryucad puts the project first among
    /// the recent ones.
    fn window_opened(root: &std::path::Path);
}

/// A product for this crate's tests.
#[cfg(test)]
pub(crate) mod testing {
    use serde::{Deserialize, Serialize};
    use serde_json::{Map, Value, json};

    pub struct Test;

    /// Where [`Test`]'s `slow` request is: 0 not started, 1 waiting
    /// without the lock, 2 let go.
    pub static GATE: (std::sync::Mutex<u8>, std::sync::Condvar) =
        (std::sync::Mutex::new(0), std::sync::Condvar::new());

    /// A product's topics, as ryucad's were when these tests were written.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum Topic {
        Model,
        Project,
        Participants,
        Activity,
        Merge,
        Pdm,
        Progress,
    }

    impl crate::notify::Topics for Topic {
        const ALL: &'static [Topic] = &[
            Topic::Model,
            Topic::Project,
            Topic::Participants,
            Topic::Activity,
            Topic::Merge,
            Topic::Pdm,
            Topic::Progress,
        ];
        const PARTICIPANTS: Topic = Topic::Participants;
        const ACTIVITY: Topic = Topic::Activity;
        const PROGRESS: Option<Topic> = Some(Topic::Progress);
    }

    impl crate::Product for Test {
        const NAME: &'static str = "ryutest";
        const RUNTIME_DIR_VAR: &'static str = "RYUTEST_RUNTIME_DIR";
        type Target = String;
        type View = Option<String>;
        type Seen = ();
        type Topic = Topic;

        fn view_json(view: &Option<String>, into: &mut Map<String, Value>) {
            into.insert("camera".into(), json!(view));
        }
    }

    impl crate::prefs::Schema for Test {
        const CONFIG_DIR_VAR: &'static str = "RYUTEST_CONFIG_DIR";

        fn accepts(name: &str, value: &Value) -> bool {
            name == "display.quality" && value.is_string()
        }

        fn default_of(name: &str) -> Option<Value> {
            (name == "display.quality").then(|| json!("normal"))
        }
    }

    /// A request of the test product: any method and params.
    pub struct Request(pub String, pub Value);

    impl crate::client::Call for Request {
        fn method(&self) -> String {
            self.0.clone()
        }

        fn params(&self) -> Value {
            self.1.clone()
        }
    }

    impl crate::Serve for Test {
        const MANIFEST: &'static str = "ryutest.json";
        const IDLE_VAR: &'static str = "RYUTEST_IDLE_TIMEOUT";
        type Session = Vec<String>;
        type Request = Request;
        type Version = Value;

        fn version() -> Value {
            json!({"ryutest": "0.0.0", "protocol": 1})
        }

        fn same_build(me: &Value, other: &Value) -> bool {
            me == other
        }

        fn parse(method: &str, params: Option<Value>) -> ryu_doc::error::Result<Request> {
            Ok(Request(method.to_owned(), params.unwrap_or(Value::Null)))
        }

        fn control(req: &Request) -> crate::daemon::Control {
            use crate::daemon::Control;
            match req.0.as_str() {
                "daemon.status" => Control::Status,
                "daemon.shutdown" => Control::Shutdown,
                "subscribe" => Control::Subscribe(Vec::new()),
                "session.hello" => Control::Hello { window: false },
                _ => Control::Request,
            }
        }

        fn handle(
            _: &mut Option<Vec<String>>,
            live: &mut crate::live::Live<Test>,
            _: Option<&str>,
            _: &std::path::Path,
            req: Request,
        ) -> ryu_doc::error::Result<crate::rpc::Answer> {
            let id = live.hello(crate::live::Kind::Agent, None, None)?;
            Ok(crate::rpc::Answer::json(
                json!({"method": req.0, "participant": id}),
            ))
        }

        /// `slow` waits at [`GATE`] without the lock until let go, then
        /// notes itself in the session.
        fn prepare(
            _: &mut Option<Vec<String>>,
            req: &Request,
        ) -> Option<crate::Prepared<Vec<String>>> {
            (req.0 == "slow").then(|| -> crate::Prepared<Vec<String>> {
                Box::new(|| {
                    let (gate, open) = &GATE;
                    let mut g = gate.lock().unwrap();
                    *g = 1;
                    open.notify_all();
                    while *g != 2 {
                        g = open.wait(g).unwrap();
                    }
                    Box::new(|s: &mut Vec<String>| s.push("slow".into()))
                })
            })
        }

        fn settle(
            _: &mut Option<Vec<String>>,
            _: &mut crate::live::Live<Test>,
            _: &std::path::Path,
        ) {
        }

        fn poll_ms() -> u64 {
            250
        }

        fn window_opened(_: &std::path::Path) {}
    }
}
