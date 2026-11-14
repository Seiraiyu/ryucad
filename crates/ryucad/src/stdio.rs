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

/// What a session needs to know.
pub struct Options {
    /// The project's root.
    pub root: PathBuf,
    /// The script's folder, which relative paths mean.
    pub here: PathBuf,
    /// The agent to act as (`--as`, `RYUCAD_PARTICIPANT`); `agent` when
    /// `None`.
    pub name: Option<String>,
    /// Answer in this process (`--no-daemon`, `RYUCAD_NO_DAEMON=1`).
    pub no_daemon: bool,
}

/// How a session ended.
#[derive(Debug, PartialEq, Eq)]
pub enum Ended {
    /// Stdin ended, and every request has its answer.
    Eof,
    /// A Ctrl+C, after the running request's answer.
    Interrupted,
}

/// What answers the session's requests.
// One per session, on the stack of `serve`: a `Session`'s size is no cost.
#[allow(clippy::large_enum_variant)]
enum Via {
    /// The project's daemon. `None` once it closed the connection, until
    /// the next request opens another.
    Daemon(Option<Link>),
    /// A session in this process.
    Local(Option<Session>),
}

/// A request's fate through the daemon.
enum Sent {
    /// Its answer is out, a result or not.
    Answered(bool),
    /// It never reached a daemon: the line, and why.
    Undelivered(Line, String),
}

/// The session's connection to the daemon: what it writes requests to,
/// and the answers its reader thread has copied to stdout.
struct Link {
    writer: SendHalf,
    answers: mpsc::Receiver<Event>,
}

/// What the reader thread tells the loop.
enum Event {
    /// An answer is out, a result or not.
    Answered(bool),
    /// The daemon closed the connection (stopped, idle, replaced).
    Closed,
}

impl Link {
    /// Connects (starting the daemon if none runs), says hello as `name`,
    /// and starts the thread that copies what the daemon sends to `out`.
    /// `Err`: no daemon to be had, and nothing was sent. `Ok(Err(..))`:
    /// the hello was refused.
    fn open<W: Write + Send + 'static>(
        root: &Path,
        name: Option<&str>,
        out: &Out<W>,
    ) -> Result<Result<Link>> {
        let conn = match client::hello_connection(root, name)? {
            Ok(c) => c,
            Err(e) => return Ok(Err(e)),
        };
        let (reader, writer) = conn.into_halves();
        let (tx, answers) = mpsc::channel();
        let out = Arc::clone(out);
        std::thread::spawn(move || copy(reader, &out, &tx));
        Ok(Ok(Link { writer, answers }))
    }

    /// Whether the daemon closed the connection since the last answer.
    fn closed(&self) -> bool {
        !matches!(self.answers.try_recv(), Err(mpsc::TryRecvError::Empty))
    }

    /// Sends `line` and waits until its answer is out. `Err`: it was
    /// never sent. `Ok(None)`: the connection closed before its answer,
    /// and it may have been applied.
    fn ask(&mut self, line: &Line) -> std::io::Result<Option<bool>> {
        self.writer.write_all(line.text().as_bytes())?;
        self.writer.flush()?;
        Ok(match self.answers.recv() {
            Ok(Event::Answered(ok)) => Some(ok),
            Ok(Event::Closed) | Err(_) => None,
        })
    }
}

/// Copies what the daemon sends to `out`, each line with the bytes it
/// announces, until it closes, and tells the loop of each answer. If
/// stdout is gone, nothing more can be said, so the process exits.
fn copy<W: Write>(mut reader: BufReader<RecvHalf>, out: &Out<W>, tx: &mpsc::Sender<Event>) {
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let frame = Frame::of(line.trim_end());
        let mut bytes = vec![0; frame.binary.unwrap_or(0)];
        if std::io::Read::read_exact(&mut reader, &mut bytes).is_err() {
            break;
        }
        if put(&mut *lock(out), line.as_bytes(), &bytes).is_err() {
            std::process::exit(i32::from(crate::error::Code::Io.exit_code()));
        }
        if frame.answer && tx.send(Event::Answered(frame.ok)).is_err() {
            return;
        }
    }
    let _ = tx.send(Event::Closed);
}

/// A line as the daemon wrote it (its break included), then its bytes.
fn put(w: &mut impl Write, line: &[u8], bytes: &[u8]) -> std::io::Result<()> {
    w.write_all(line)?;
    w.write_all(bytes)?;
    w.flush()
}

/// Answers each request line of `input` in order, writing each answer to
/// `out`, until `input` ends or a Ctrl+C (`stop`) has had its answer.
/// `Err`: stdin or stdout failed, or the daemon refused the session's
/// hello.
pub fn serve<R: BufRead, W: Write + Send + 'static>(
    mut input: R,
    out: &Out<W>,
    opts: &Options,
    stop: &Stop,
) -> Result<Ended> {
    let mut me = opts.name.clone();
    let mut via = match opts.no_daemon {
        true => Via::Local(None),
        false => match Link::open(&opts.root, me.as_deref(), out) {
            Ok(Ok(link)) => Via::Daemon(Some(link)),
            // A refused hello is the script's to fix (its --as).
            Ok(Err(e)) => return Err(e),
            Err(e) => {
                warn(&format!("{}; running in process", e.message));
                Via::Local(None)
            }
        },
    };
    let mut text = String::new();
    let ended = loop {
        text.clear();
        if input
            .read_line(&mut text)
            .map_err(|e| Error::io(Path::new("<stdin>"), &e))?
            == 0
        {
            break Ended::Eof;
        }
        if text.trim().is_empty() {
            continue;
        }
        let line = match Line::read(text.trim_end(), &opts.here) {
            Ok(l) => l,
            Err((id, e)) => {
                stdout(write_answer(out, &id, Err(e)))?;
                continue;
            }
        };
        let hello = line.hello_name();
        let sent = match &mut via {
            Via::Local(slot) => Sent::Answered(stdout(answer_local(
                slot,
                opts,
                me.as_deref(),
                line,
                out,
                stop,
            ))?),
            Via::Daemon(link) => answer_daemon(link, opts, me.as_deref(), line, out, stop)?,
        };
        let ok = match sent {
            Sent::Answered(ok) => ok,
            // Never delivered: this request, and the rest of the run, in
            // this process (decision 3).
            Sent::Undelivered(line, why) => {
                warn(&format!("{why}; running in process"));
                let mut slot = None;
                let ok = stdout(answer_local(
                    &mut slot,
                    opts,
                    me.as_deref(),
                    line,
                    out,
                    stop,
                ))?;
                via = Via::Local(slot);
                ok
            }
        };
        // A hello the daemon took renames the session's agent: its
        // cancels and reconnections say the new name.
        if ok && let Some(name) = hello {
            me = name;
        }
        if stop.stopping() {
            break Ended::Interrupted;
        }
    };
    if let Via::Local(slot) = &via {
        flush_remote(slot);
    }
    Ok(ended)
}

/// Answers `line` with the session in `slot` (opened at the first
/// request), as a daemon would: its progress, when it asks, as
/// `progress` lines before its answer. `Ok(ok)`: written, and whether a
/// result.
fn answer_local<W: Write + Send + 'static>(
    slot: &mut Option<Session>,
    opts: &Options,
    me: Option<&str>,
    mut line: Line,
    out: &Out<W>,
    stop: &Stop,
) -> std::io::Result<bool> {
    let token = line.token();
    let mut params = line.params.take();
    let ask = match line.long() {
        true => Ask::take(&mut params),
        false => Ok(Ask::default()),
    };
    let req = ask.and_then(|ask| Ok((ask, Request::from_rpc(&line.method, params)?)));
    let (ask, req) = match req {
        Ok(r) => r,
        Err(e) => return write_answer(out, &line.id, Err(e)),
    };
    let req = client::with_author(req, me);
    let job = match token {
        None => {
            stop.begin(Running::Plain);
            Job::default()
        }
        Some(token) => {
            let flag = cancel::local_flag();
            stop.begin(Running::Local(Arc::clone(&flag)));
            let job = Job::new(&token, me.unwrap_or("agent"), &line.method, flag);
            match ask.progress {
                false => job,
                true => {
                    let out = Arc::clone(out);
                    job.with_line(Box::new(move |p| {
                        let mut w = lock(&out);
                        let _ = writeln!(w, "{}", ryu_session::notify::line("progress", p));
                        let _ = w.flush();
                    }))
                }
            }
        }
    };
    let answer = server::handle_job_raw(slot, None, me, &opts.root, req, &job);
    // Its last progress line, as a daemon sends it.
    job.finish();
    stop.end();
    write_answer(out, &line.id, answer)
}

/// Answers `line` through the daemon. If the daemon closed the
/// connection (an idle daemon exits; `daemon stop`), a new one is opened
/// first. A daemon that can't be had, or a line that can't be sent, is
/// `Undelivered`.
fn answer_daemon<W: Write + Send + 'static>(
    link: &mut Option<Link>,
    opts: &Options,
    me: Option<&str>,
    mut line: Line,
    out: &Out<W>,
    stop: &Stop,
) -> Result<Sent> {
    if link.as_ref().is_none_or(Link::closed) {
        *link = None;
        match Link::open(&opts.root, me, out) {
            Ok(Ok(l)) => *link = Some(l),
            Ok(Err(e)) => {
                return Ok(Sent::Answered(stdout(write_answer(out, &line.id, Err(e)))?));
            }
            Err(e) => return Ok(Sent::Undelivered(line, e.message)),
        }
    }
    let l = link.as_mut().expect("opened above");
    stop.begin(match line.token() {
        Some(token) => Running::Daemon {
            root: opts.root.clone(),
            name: me.map(str::to_owned),
            token,
        },
        None => Running::Plain,
    });
    let asked = l.ask(&line);
    stop.end();
    match asked {
        Ok(Some(ok)) => Ok(Sent::Answered(ok)),
        Ok(None) => {
            *link = None;
            let e = Error::new(
                crate::error::Code::Daemon,
                "the daemon stopped answering; the request may have been applied",
            );
            Ok(Sent::Answered(stdout(write_answer(out, &line.id, Err(e)))?))
        }
        Err(e) => {
            *link = None;
            let why = format!("the daemon couldn't be sent the request ({e})");
            Ok(Sent::Undelivered(line, why))
        }
    }
}

/// What a session in this process built goes to the shared cache before
/// the process exits, as the CLI's in-process requests do.
fn flush_remote(slot: &Option<Session>) {
    if let Some(r) = slot.as_ref().and_then(|s| s.disk()?.remote()) {
        r.flush(crate::cli::REMOTE_FLUSH);
    }
}

/// An answer's write, as the session's error: stdout is gone.
fn stdout(r: std::io::Result<bool>) -> Result<bool> {
    r.map_err(|e| Error::io(Path::new("<stdout>"), &e))
}

fn warn(message: &str) {
    eprintln!("ryucad: warning: {message}");
}

/// `ryucad session --stdio`: serves stdin and stdout until stdin ends
/// (exit 0) or a Ctrl+C (130, a shell's for an interrupt). A session that
/// can't start (no project, a refused hello) or whose stdin or stdout
/// fails writes one refusal with a null id and exits with its code, as
/// the CLI would.
pub fn main(here: PathBuf, name: Option<String>, no_daemon: bool) -> std::process::ExitCode {
    let out: Out<std::io::Stdout> = Arc::new(Mutex::new(std::io::stdout()));
    let failed = |e: Error| {
        let code = e.code.exit_code();
        let _ = write_answer(&out, &Value::Null, Err(e));
        std::process::ExitCode::from(code)
    };
    let root = match crate::doc::project::find_root(&here) {
        Ok(r) => r,
        Err(e) => return failed(e),
    };
    let stop = Arc::new(Stop::default());
    let fired = Arc::clone(&stop);
    // Once per process: this process runs only the session.
    let _ = ctrlc::set_handler(move || {
        if fired.fire() == Fired::Exit {
            std::process::exit(cancel::INTERRUPTED);
        }
    });
    let opts = Options {
        root,
        here,
        name,
        no_daemon,
    };
    match serve(std::io::stdin().lock(), &out, &opts, &stop) {
        Ok(Ended::Eof) => std::process::ExitCode::SUCCESS,
        Ok(Ended::Interrupted) => {
            std::process::ExitCode::from(u8::try_from(cancel::INTERRUPTED).unwrap_or(1))
        }
        Err(e) => failed(e),
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

    /// A project in a temporary folder, and a session's options on it in
    /// this process.
    fn local() -> (tempfile::TempDir, Options) {
        let dir = tempfile::tempdir().unwrap();
        crate::server::init(dir.path(), Some(crate::units::Unit::Mm)).unwrap();
        let root = crate::doc::project::find_root(dir.path()).unwrap();
        let opts = Options {
            root: root.clone(),
            here: root,
            name: None,
            no_daemon: true,
        };
        (dir, opts)
    }

    /// Splits what a session wrote into its lines, each with the bytes
    /// it announces.
    fn frames(mut b: &[u8]) -> Vec<(Value, Vec<u8>)> {
        let mut v = Vec::new();
        while !b.is_empty() {
            let end = b.iter().position(|&c| c == b'\n').expect("a line break");
            let line = std::str::from_utf8(&b[..end]).unwrap();
            let n = Frame::of(line).binary.unwrap_or(0);
            v.push((
                serde_json::from_str(line).unwrap(),
                b[end + 1..end + 1 + n].to_vec(),
            ));
            b = &b[end + 1 + n..];
        }
        v
    }

    /// `lines` through a session: how it ended, and what it wrote.
    fn script(opts: &Options, lines: &[&str]) -> (Ended, Vec<(Value, Vec<u8>)>) {
        let out: Out<Vec<u8>> = Arc::new(Mutex::new(Vec::new()));
        let input = lines.join("\n") + "\n";
        let ended = serve(std::io::Cursor::new(input), &out, opts, &Stop::default()).unwrap();
        let bytes = std::mem::take(&mut *lock(&out));
        (ended, frames(&bytes))
    }

    #[test]
    fn a_script_runs_in_process_answer_by_answer() {
        let (_dir, opts) = local();
        let (ended, got) = script(
            &opts,
            &[
                r#"{"jsonrpc":"2.0","id":1,"method":"part.create","params":{"part":"p"}}"#,
                "",
                "{nope",
                r#"{"jsonrpc":"2.0","id":2,"method":"feature.add","params":{"part":"p","type":"box","id":"plate","params":{"x":"10","y":"20","z":"5"}}}"#,
                r#"{"jsonrpc":"2.0","id":"m","method":"mesh.get","params":{"part":"p","transfer":"binary"}}"#,
                r#"{"jsonrpc":"2.0","id":3,"method":"no.such"}"#,
                r#"{"jsonrpc":"2.0","id":4,"method":"session.hello","params":{"kind":"agent"}}"#,
                r#"{"jsonrpc":"2.0","id":5,"method":"rebuild","params":{"progress":true}}"#,
                r#"{"jsonrpc":"2.0","id":6,"method":"part.list"}"#,
            ],
        );
        assert_eq!(ended, Ended::Eof);
        // Progress lines come only for the request that asked, before its answer.
        let answers: Vec<&(Value, Vec<u8>)> = got
            .iter()
            .filter(|(v, _)| v.get("method").is_none())
            .collect();
        assert!(
            got.iter()
                .all(|(v, _)| v.get("method").is_none() || v["method"] == "progress")
        );
        let ids: Vec<&Value> = answers.iter().map(|(v, _)| &v["id"]).collect();
        assert_eq!(
            ids,
            [
                &json!(1),
                &Value::Null,
                &json!(2),
                &json!("m"),
                &json!(3),
                &json!(4),
                &json!(5),
                &json!(6)
            ]
        );
        assert_eq!(answers[1].0["error"]["data"]["code"], "invalid_json");
        assert!(
            answers[2].0["result"]["revision"].is_string(),
            "{}",
            answers[2].0
        );
        let n = answers[3].0["result"]["binary"]["length"].as_u64().unwrap();
        assert!(n > 0);
        assert_eq!(answers[3].1.len() as u64, n);
        assert_eq!(answers[4].0["error"]["data"]["code"], "usage");
        // No daemon, no participants, as with the CLI's --no-daemon.
        assert_eq!(answers[5].0["error"]["data"]["code"], "no_live_session");
        assert_eq!(
            answers[6].0["result"]["parts"][0]["failed"], 0,
            "{}",
            answers[6].0
        );
        assert_eq!(answers[7].0["result"]["parts"][0]["part"], "p");
    }

    #[test]
    fn relative_paths_mean_the_scripts_folder_in_process_too() {
        let (dir, mut opts) = local();
        let out = dir.path().join("out");
        std::fs::create_dir(&out).unwrap();
        opts.here = out.clone();
        let (_, got) = script(
            &opts,
            &[
                r#"{"jsonrpc":"2.0","id":1,"method":"part.create","params":{"part":"p"}}"#,
                r#"{"jsonrpc":"2.0","id":2,"method":"feature.add","params":{"part":"p","type":"box","id":"plate","params":{"x":"10","y":"20","z":"5"}}}"#,
                r#"{"jsonrpc":"2.0","id":3,"method":"export","params":{"part":"p","out":"p.stl"}}"#,
            ],
        );
        assert!(got[2].0.get("result").is_some(), "{}", got[2].0);
        assert!(out.join("p.stl").is_file());
    }
}
