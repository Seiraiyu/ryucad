//! `ryucad session --stdio` (design 603, ryucad 31.A): a script's one
//! connection. It reads one JSON-RPC 2.0 request a line on stdin and
//! writes each answer on stdout in order, the daemon's own response line
//! and the bytes it announces. One process and one `session.hello`
//! serve the whole run, relative paths mean the script's folder, and
//! with no daemon one session in this process answers instead, in the
//! same framing. See `docs/claude/ryucad-scripting.md`.

use crate::cancel;
use crate::client;
use crate::daemon::LONG;
use crate::error::{Error, Result};
use crate::proto::Request;
use crate::server;
use crate::session::Session;
use interprocess::local_socket::{RecvHalf, SendHalf};
use ryu_session::job::{Ask, Job};
use ryu_session::rpc::{Answer, Raw, parse_request_line, rpc_response, write_raw_response};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

/// Where answers go (stdout), shared by the loop and the thread copying
/// what the daemon sends, written a whole answer at a time.
pub type Out<W> = Arc<Mutex<W>>;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// One of the script's request lines, read.
#[derive(Debug, PartialEq)]
pub struct Line {
    /// Its `id`, given back with its answer (null when it has none).
    pub id: Value,
    pub method: String,
    pub params: Option<Value>,
}

impl Line {
    /// Reads `text` (one line, without its break) as the daemon reads a
    /// line, with its paths made absolute against `here`
    /// ([`client::absolute_params`]). `Err` holds the id to answer (null
    /// when the line isn't JSON) and why it isn't a request.
    pub fn read(text: &str, here: &Path) -> std::result::Result<Line, (Value, Error)> {
        let (id, parsed) = parse_request_line(text);
        let (method, mut params) = match parsed {
            Ok(p) => p,
            Err(e) => return Err((id, e)),
        };
        if let Some(p) = params.as_mut() {
            client::absolute_params(&method, p, here);
        }
        Ok(Line { id, method, params })
    }

    /// Whether it is one of the long requests, which take a token.
    pub fn long(&self) -> bool {
        LONG.contains(&self.method.as_str())
    }

    /// A long request's token: the script's own, else one given it here,
    /// so that Ctrl+C can cancel it. `None` for other requests, and for
    /// params that aren't an object (the request's checks refuse them).
    pub fn token(&mut self) -> Option<String> {
        if !self.long() {
            return None;
        }
        let map = self
            .params
            .get_or_insert_with(|| json!({}))
            .as_object_mut()?;
        match map.get("token") {
            Some(Value::String(t)) => Some(t.clone()),
            Some(_) => None,
            None => {
                let t = next_token();
                map.insert("token".into(), json!(t));
                Some(t)
            }
        }
    }

    /// The line the daemon is sent, with its break.
    pub fn text(&self) -> String {
        let mut v = json!({"jsonrpc": "2.0", "id": self.id, "method": self.method});
        if let Some(p) = &self.params {
            v["params"] = p.clone();
        }
        let mut t = v.to_string();
        t.push('\n');
        t
    }

    /// The agent a `session.hello` line names, when it is one
    /// (`Some(None)`: the default agent).
    pub fn hello_name(&self) -> Option<Option<String>> {
        (self.method == "session.hello").then(|| {
            self.params
                .as_ref()
                .and_then(|p| p["name"].as_str())
                .map(str::to_owned)
        })
    }
}

/// A token for one of this session's long requests: its process and a
/// count, unique among its participant's requests.
fn next_token() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let n = NEXT.fetch_add(1, Ordering::SeqCst);
    format!("stdio-{}-{n}", std::process::id())
}

/// Writes `answer` to the request `id` as the daemon writes it: the
/// response line, then the bytes it announces. `Ok(true)` when it is a
/// result, not a refusal.
pub fn write_answer<W: Write>(
    out: &Out<W>,
    id: &Value,
    answer: Result<Answer>,
) -> std::io::Result<bool> {
    let mut w = lock(out);
    let (ok, bytes) = match answer {
        Ok(Answer {
            raw: Some(raw),
            binary,
            ..
        }) => {
            write_raw_response(&mut *w, id, &raw)?;
            (!matches!(raw, Raw::Error { .. }), binary)
        }
        Ok(a) => {
            w.write_all(rpc_response(id.clone(), &Ok(a.result)).as_bytes())?;
            (true, a.binary)
        }
        Err(e) => {
            w.write_all(rpc_response(id.clone(), &Err(e)).as_bytes())?;
            (false, None)
        }
    };
    w.write_all(b"\n")?;
    if let Some(b) = bytes {
        w.write_all(&b)?;
    }
    w.flush()?;
    Ok(ok)
}

/// What the session needs to know of a line the daemon sent, read
/// without building its result (a rebuild's report may be most of a
/// gigabyte, #533).
#[derive(Debug, PartialEq, Eq)]
pub struct Frame {
    /// An answer, not a notification (which has a method).
    pub answer: bool,
    /// An answer that is a result, not a refusal.
    pub ok: bool,
    /// How many bytes follow the line.
    pub binary: Option<usize>,
}

impl Frame {
    pub fn of(line: &str) -> Frame {
        #[derive(serde::Deserialize)]
        struct Peek<'a> {
            #[serde(default)]
            method: Option<serde::de::IgnoredAny>,
            #[serde(default, borrow)]
            result: Option<&'a serde_json::value::RawValue>,
            #[serde(default)]
            error: Option<serde::de::IgnoredAny>,
        }
        #[derive(serde::Deserialize)]
        struct Announced {
            #[serde(default)]
            binary: Option<Length>,
        }
        #[derive(serde::Deserialize)]
        struct Length {
            length: usize,
        }
        let Ok(p) = serde_json::from_str::<Peek>(line) else {
            // Not ours to judge: an answer that failed.
            return Frame {
                answer: true,
                ok: false,
                binary: None,
            };
        };
        let binary = p
            .result
            .filter(|r| r.get().starts_with('{'))
            .and_then(|r| serde_json::from_str::<Announced>(r.get()).ok())
            .and_then(|a| a.binary)
            .map(|l| l.length);
        Frame {
            answer: p.method.is_none(),
            ok: p.error.is_none(),
            binary,
        }
    }
}

/// What the session's Ctrl+C does. The first cancels the running
/// request if it is long (another finishes), and the session ends once
/// that request has its answer. With nothing running, or the second
/// time, the process exits at once.
#[derive(Default)]
pub struct Stop {
    fired: AtomicU8,
    running: Mutex<Option<Running>>,
}

/// The request being answered, as a Ctrl+C cancels it.
pub enum Running {
    /// A long request through the daemon: `request.cancel` of its token
    /// as the agent `name`, on a connection of its own, as the CLI's.
    Daemon {
        root: PathBuf,
        name: Option<String>,
        token: String,
    },
    /// A long request in this process: its job's flag.
    Local(Arc<AtomicBool>),
    /// Any other request: it finishes.
    Plain,
}

/// What a Ctrl+C leaves to do.
#[derive(Debug, PartialEq, Eq)]
pub enum Fired {
    /// Exit now.
    Exit,
    /// End the session once the running request has its answer.
    AfterAnswer,
}

impl Stop {
    /// A request starts being answered.
    pub fn begin(&self, r: Running) {
        *lock(&self.running) = Some(r);
    }

    /// The request has its answer.
    pub fn end(&self) {
        lock(&self.running).take();
    }

    /// Whether a Ctrl+C came: the session ends after the answer it is
    /// writing.
    pub fn stopping(&self) -> bool {
        self.fired.load(Ordering::SeqCst) > 0
    }

    /// A Ctrl+C.
    pub fn fire(&self) -> Fired {
        if self.fired.fetch_add(1, Ordering::SeqCst) > 0 {
            return Fired::Exit;
        }
        let Some(running) = lock(&self.running).take() else {
            return Fired::Exit;
        };
        match running {
            // Nothing to cancel if it finished meanwhile; the answer says.
            Running::Daemon { root, name, token } => {
                let _ = client::cancel(&root, name.as_deref(), &token);
            }
            Running::Local(flag) => flag.store(true, Ordering::SeqCst),
            Running::Plain => {}
        }
        Fired::AfterAnswer
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Code;
    use ryu_session::rpc::rpc_response;
    use serde_json::{Value, json};
    use std::path::Path;
    use std::sync::{Arc, Mutex};

    #[test]
    fn a_line_is_read_as_the_daemon_reads_it_with_its_paths_absolute() {
        let here = std::env::temp_dir().join("script");
        let l = Line::read(
            r#"{"jsonrpc":"2.0","id":7,"method":"export","params":{"part":"p","out":"o/p.stl"}}"#,
            &here,
        )
        .unwrap();
        assert_eq!((&l.id, l.method.as_str()), (&json!(7), "export"));
        let want = std::path::absolute(here.join("o/p.stl")).unwrap();
        assert_eq!(
            l.params.as_ref().unwrap()["out"],
            json!(want.to_string_lossy())
        );
        let (id, e) = Line::read("{nope", &here).unwrap_err();
        assert_eq!((id, e.code), (Value::Null, Code::InvalidJson));
        let (id, e) = Line::read(r#"{"id":3,"method":"part.list"}"#, &here).unwrap_err();
        assert_eq!((id, e.code), (json!(3), Code::Usage));
    }

    #[test]
    fn long_requests_get_a_token_and_keep_their_own() {
        let here = Path::new("/");
        let mut l = Line::read(r#"{"jsonrpc":"2.0","id":1,"method":"rebuild"}"#, here).unwrap();
        let t = l.token().unwrap();
        assert!(t.starts_with("stdio-"), "{t}");
        assert_eq!(l.params.as_ref().unwrap()["token"], json!(t));
        let mut again = Line::read(r#"{"jsonrpc":"2.0","id":2,"method":"rebuild"}"#, here).unwrap();
        assert_ne!(again.token(), Some(t));
        let mut mine = Line::read(
            r#"{"jsonrpc":"2.0","id":3,"method":"rebuild","params":{"token":"mine"}}"#,
            here,
        )
        .unwrap();
        assert_eq!(mine.token().as_deref(), Some("mine"));
        let mut short =
            Line::read(r#"{"jsonrpc":"2.0","id":4,"method":"part.list"}"#, here).unwrap();
        assert_eq!(short.token(), None);
        assert_eq!(short.params, None);
    }

    #[test]
    fn the_daemon_is_sent_the_scripts_id_and_params() {
        let l = Line::read(
            r#"{"jsonrpc":"2.0","id":"a","method":"part.list"}"#,
            Path::new("/"),
        )
        .unwrap();
        let text = l.text();
        assert!(text.ends_with('\n'));
        let back: Value = serde_json::from_str(text.trim_end()).unwrap();
        assert_eq!(
            back,
            json!({"jsonrpc": "2.0", "id": "a", "method": "part.list"})
        );
        let hello = Line::read(
            r#"{"jsonrpc":"2.0","id":1,"method":"session.hello","params":{"kind":"agent","name":"b"}}"#,
            Path::new("/"),
        )
        .unwrap();
        assert_eq!(hello.hello_name(), Some(Some("b".to_owned())));
        assert_eq!(l.hello_name(), None);
    }

    #[test]
    fn frames_say_answer_error_or_notification_and_their_bytes() {
        let ok = Frame {
            answer: true,
            ok: true,
            binary: None,
        };
        assert_eq!(
            Frame::of(r#"{"jsonrpc":"2.0","id":1,"result":{"parts":[]}}"#),
            ok
        );
        assert_eq!(Frame::of(r#"{"jsonrpc":"2.0","id":1,"result":null}"#), ok);
        assert_eq!(Frame::of(r#"{"jsonrpc":"2.0","id":1,"result":[1,2]}"#), ok);
        let png =
            r#"{"jsonrpc":"2.0","id":1,"result":{"binary":{"encoding":"image/png","length":12}}}"#;
        assert_eq!(Frame::of(png).binary, Some(12));
        let refused = r#"{"error":{"code":-32000,"data":{"code":"usage","message":"m"},"message":"m"},"id":null,"jsonrpc":"2.0"}"#;
        assert_eq!(
            Frame::of(refused),
            Frame {
                answer: true,
                ok: false,
                binary: None
            }
        );
        let note = r#"{"jsonrpc":"2.0","method":"progress","params":{"done":1}}"#;
        assert!(!Frame::of(note).answer);
    }

    #[test]
    fn answers_are_written_as_the_daemon_writes_them() {
        let out: Out<Vec<u8>> = Arc::new(Mutex::new(Vec::new()));
        let png = Answer::bytes(json!({}), "image/png", vec![0, 10, 255]);
        assert!(write_answer(&out, &json!(1), Ok(Answer::json(json!({"a": 1})))).unwrap());
        assert!(write_answer(&out, &json!(2), Ok(png)).unwrap());
        assert!(!write_answer(&out, &json!(3), Err(Error::new(Code::Usage, "no"))).unwrap());
        let mut want: Vec<u8> = Vec::new();
        want.extend(rpc_response(json!(1), &Ok(json!({"a": 1}))).as_bytes());
        want.push(b'\n');
        let announced = json!({"binary": {"encoding": "image/png", "length": 3}});
        want.extend(rpc_response(json!(2), &Ok(announced)).as_bytes());
        want.push(b'\n');
        want.extend([0, 10, 255]);
        want.extend(rpc_response(json!(3), &Err(Error::new(Code::Usage, "no"))).as_bytes());
        want.push(b'\n');
        assert_eq!(*lock(&out), want);
    }

    #[test]
    fn ctrl_c_exits_when_idle_and_the_second_time() {
        let s = Stop::default();
        assert_eq!(s.fire(), Fired::Exit);
        let s = Stop::default();
        s.begin(Running::Plain);
        assert_eq!(s.fire(), Fired::AfterAnswer);
        assert!(s.stopping());
        assert_eq!(s.fire(), Fired::Exit);
    }

    #[test]
    fn ctrl_c_sets_a_local_requests_flag() {
        let s = Stop::default();
        let flag = crate::cancel::local_flag();
        s.begin(Running::Local(Arc::clone(&flag)));
        assert_eq!(s.fire(), Fired::AfterAnswer);
        assert!(flag.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[test]
    fn ctrl_c_after_an_answer_finds_nothing_running() {
        let s = Stop::default();
        s.begin(Running::Plain);
        s.end();
        assert!(!s.stopping());
        assert_eq!(s.fire(), Fired::Exit);
    }
}
