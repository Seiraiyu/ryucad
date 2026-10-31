//! A long request's progress and its cancel. The daemon gives every
//! request of a long method ([`crate::Serve::LONG`]) a [`Job`]: the
//! product reports what it is doing through it ([`Job::begin`],
//! [`Stage::step`]) and stops at its boundaries when it is cancelled
//! ([`Stage::step`] and [`Job::check`] answer `cancelled`). Progress goes
//! to the requesting connection as `progress` notifications (at most every
//! [`LINE_EVERY`], and once more at the end) and to `progress`
//! subscribers (at most every [`TOPIC_EVERY`]). Nothing here stops a
//! kernel call: a request stops only where it asks.

use ryu_doc::error::{Code, Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// How often progress goes to the requesting connection.
pub const LINE_EVERY: Duration = Duration::from_millis(100);
/// How often progress goes to `progress` subscribers: four a second.
pub const TOPIC_EVERY: Duration = Duration::from_millis(250);
/// The longest token a client may give.
pub const TOKEN_MAX: usize = 64;

/// What a long request is doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Rebuild,
    Heal,
    Mesh,
    Intersect,
    Check,
}

/// What a request asked of its job: `progress` and `token`, taken out of
/// its params before the product reads them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Ask {
    pub progress: bool,
    pub token: Option<String>,
}

impl Ask {
    /// Takes `progress` (a boolean) and `token` (1 to [`TOKEN_MAX`]
    /// characters) out of `params`, leaving the rest for the product.
    pub fn take(params: &mut Option<Value>) -> Result<Ask> {
        let Some(Value::Object(map)) = params.as_mut() else {
            return Ok(Ask::default());
        };
        let progress = match map.remove("progress") {
            None => false,
            Some(Value::Bool(b)) => b,
            Some(v) => {
                return Err(
                    Error::new(Code::InvalidParam, "progress is true or false").with("progress", v)
                );
            }
        };
        let token = match map.remove("token") {
            None => None,
            Some(Value::String(t)) if !t.is_empty() && t.chars().count() <= TOKEN_MAX => Some(t),
            Some(v) => {
                return Err(Error::new(
                    Code::InvalidParam,
                    format!("a token is a string of 1 to {TOKEN_MAX} characters"),
                )
                .with("token", v));
            }
        };
        Ok(Ask { progress, token })
    }

    /// Puts `progress` and `token` into a request's params: what a client
    /// sends.
    pub fn put(&self, params: &mut Value) {
        if !params.is_object() {
            *params = json!({});
        }
        if self.progress {
            params["progress"] = json!(true);
        }
        if let Some(t) = &self.token {
            params["token"] = json!(t);
        }
    }
}

/// Where a job's progress goes: given each update's params.
pub type Sink = Box<dyn Fn(&Value) + Send + Sync>;

/// A request's progress and cancel. `Job::default()` is no job: it reports
/// nothing and is never cancelled. Clones share one job.
#[derive(Clone, Default)]
pub struct Job {
    inner: Option<Arc<Inner>>,
}

impl std::fmt::Debug for Job {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.inner {
            Some(i) => write!(f, "Job({} {})", i.method, i.token),
            None => write!(f, "Job(none)"),
        }
    }
}

struct Inner {
    token: String,
    participant: String,
    method: String,
    /// Set by `request.cancel` (or the CLI's Ctrl+C).
    cancel: Arc<AtomicBool>,
    /// The request has written a file: a cancel no longer stops it.
    writing: AtomicBool,
    /// Boundaries reached; a test's job cancels itself at one.
    checks: AtomicU64,
    cancel_at: Option<u64>,
    state: Mutex<Counts>,
    /// The phase every stage reports, when the request names one (an
    /// import's rebuilds are healing).
    phase: Mutex<Option<Phase>>,
    line: Option<Sink>,
    topic: Option<Sink>,
    every: (Duration, Duration),
}

#[derive(Default)]
struct Counts {
    /// Stages begun and not yet ended; only the outermost counts.
    depth: u32,
    phase: Option<Phase>,
    unit: &'static str,
    started: u64,
    total: u64,
    item: String,
    line_at: Option<Instant>,
    topic_at: Option<Instant>,
}

impl Inner {
    /// Whether a cancel came: its flag, or a test's boundary reached.
    fn asked(&self) -> bool {
        let n = self.checks.load(Ordering::SeqCst);
        self.cancel.load(Ordering::SeqCst) || self.cancel_at.is_some_and(|at| n >= at)
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

impl Job {
    /// A job for `participant`'s request `method`, named `token`, stopped
    /// when `cancel` is set.
    pub fn new(token: &str, participant: &str, method: &str, cancel: Arc<AtomicBool>) -> Job {
        Job {
            inner: Some(Arc::new(Inner {
                token: token.to_owned(),
                participant: participant.to_owned(),
                method: method.to_owned(),
                cancel,
                writing: AtomicBool::new(false),
                checks: AtomicU64::new(0),
                cancel_at: None,
                state: Mutex::new(Counts::default()),
                phase: Mutex::new(None),
                line: None,
                topic: None,
                every: (LINE_EVERY, TOPIC_EVERY),
            })),
        }
    }

    /// Sends progress to the requester through `line`, at most every
    /// [`LINE_EVERY`] and once more at the end of the outermost stage.
    pub fn with_line(self, line: Sink) -> Job {
        self.map(|i| i.line = Some(line))
    }

    /// Publishes progress, with its participant and method, through
    /// `topic`, at most every [`TOPIC_EVERY`] and once more at the end.
    pub fn with_topic(self, topic: Sink) -> Job {
        self.map(|i| i.topic = Some(topic))
    }

    /// Sends every update, unthrottled: for tests.
    #[doc(hidden)]
    pub fn unthrottled(self) -> Job {
        self.map(|i| i.every = (Duration::ZERO, Duration::ZERO))
    }

    /// A job that cancels itself at its `n`th boundary (from 1): for tests
    /// of where requests stop.
    #[doc(hidden)]
    pub fn cancelled_at(n: u64) -> Job {
        Job::new("test", "test", "test", Arc::new(AtomicBool::new(false)))
            .map(|i| i.cancel_at = Some(n))
    }

    fn map(self, f: impl FnOnce(&mut Inner)) -> Job {
        let mut inner = Arc::into_inner(self.inner.expect("a job"))
            .expect("a job is configured before it is shared");
        f(&mut inner);
        Job {
            inner: Some(Arc::new(inner)),
        }
    }

    /// The token, if this is a job.
    pub fn token(&self) -> Option<&str> {
        self.inner.as_deref().map(|i| i.token.as_str())
    }

    /// Whether a cancel has been asked for (and still counts).
    pub fn is_cancelled(&self) -> bool {
        self.inner
            .as_deref()
            .is_some_and(|i| !i.writing.load(Ordering::SeqCst) && i.asked())
    }

    /// A boundary: `cancelled` (with `done` and `total`) if a cancel has
    /// been asked for, else nothing.
    pub fn check(&self) -> Result<()> {
        let Some(i) = self.inner.as_deref() else {
            return Ok(());
        };
        i.checks.fetch_add(1, Ordering::SeqCst);
        if !self.is_cancelled() {
            return Ok(());
        }
        let c = lock(&i.state);
        // Serially, every unit begun before this boundary is done.
        let done = c.started.min(c.total);
        let what = if c.unit.is_empty() {
            String::new()
        } else {
            format!(" after {done} of {} {}", c.total, c.unit)
        };
        Err(
            Error::new(Code::Cancelled, format!("{} was cancelled{what}", i.method))
                .with("token", &i.token)
                .with("done", done)
                .with("total", c.total),
        )
    }

    /// A request's answer, once it has one: `cancelled` if a cancel came
    /// that it saw, whatever it answered. A boundary that let the cancel
    /// by (a failure taken as a missing piece) leaves an answer that may
    /// lack what it didn't build; a request that had begun writing keeps
    /// its answer.
    pub fn settle<T>(&self, answer: Result<T>) -> Result<T> {
        match answer {
            Ok(_) if self.is_cancelled() => Err(self.check().expect_err("cancelled")),
            a => a,
        }
    }

    /// The request is about to write its first file: from now on a cancel
    /// is ignored, and [`Job::ignored`] says whether one came.
    pub fn writing(&self) {
        if let Some(i) = self.inner.as_deref() {
            i.writing.store(true, Ordering::SeqCst);
        }
    }

    /// Whether a cancel came after the request began writing.
    pub fn ignored(&self) -> bool {
        self.inner
            .as_deref()
            .is_some_and(|i| i.writing.load(Ordering::SeqCst) && i.asked())
    }

    /// Names the phase every stage of this request reports, whatever
    /// the stage says: an import's rebuilds heal its bodies.
    pub fn phase(&self, phase: Phase) {
        if let Some(i) = self.inner.as_deref() {
            *lock(&i.phase) = Some(phase);
        }
    }

    /// Ends the request: one last update, `end: true`, where progress
    /// goes. The daemon calls it once the request has its answer.
    pub fn finish(&self) {
        if let Some(i) = self.inner.as_deref() {
            let mut c = lock(&i.state);
            if c.phase.is_some() {
                Job::report(i, &mut c, true);
            }
        }
    }

    /// Begins counting `total` `unit`s (`features`, `bodies`, `pairs`,
    /// `parts`) in `phase`. Only the outermost stage counts: a part's
    /// rebuild inside an assembly's reports nothing of its own, but its
    /// boundaries still stop it.
    pub fn begin(&self, phase: Phase, unit: &'static str, total: usize) -> Stage<'_> {
        let Some(i) = self.inner.as_deref() else {
            return Stage {
                job: self,
                counts: false,
            };
        };
        let mut c = lock(&i.state);
        c.depth += 1;
        let counts = c.depth == 1;
        if counts {
            let hint = *lock(&i.phase);
            c.phase = Some(hint.unwrap_or(phase));
            c.unit = unit;
            c.started = 0;
            c.total = total as u64;
            c.item.clear();
        }
        Stage { job: self, counts }
    }

    /// Sends the current counts where they go, if their interval has
    /// passed (or always, at the end).
    fn report(i: &Inner, c: &mut Counts, end: bool) {
        let now = Instant::now();
        let due = |at: Option<Instant>, every: Duration| {
            end || at.is_none_or(|t| now.duration_since(t) >= every)
        };
        let Some(phase) = c.phase else {
            return;
        };
        let done = c.started.saturating_sub(1).min(c.total);
        let mut p = Map::new();
        p.insert("token".into(), json!(i.token));
        p.insert("phase".into(), json!(phase));
        p.insert("done".into(), json!(done));
        p.insert("total".into(), json!(c.total));
        p.insert("unit".into(), json!(c.unit));
        p.insert("item".into(), json!(c.item));
        if end {
            p.insert("end".into(), json!(true));
        }
        if let Some(line) = &i.line
            && due(c.line_at, i.every.0)
        {
            c.line_at = Some(now);
            line(&Value::Object(p.clone()));
        }
        if let Some(topic) = &i.topic
            && due(c.topic_at, i.every.1)
        {
            c.topic_at = Some(now);
            p.insert("participant".into(), json!(i.participant));
            p.insert("method".into(), json!(i.method));
            topic(&Value::Object(p));
        }
    }
}

/// A stage of a job's work, from [`Job::begin`]; reports its end when
/// dropped.
pub struct Stage<'a> {
    job: &'a Job,
    counts: bool,
}

impl Stage<'_> {
    /// A boundary before the next unit, `item` (a part and feature, a
    /// body, a pair): `cancelled` if a cancel has been asked for, else it
    /// counts the unit and reports it.
    pub fn step(&self, item: impl FnOnce() -> String) -> Result<()> {
        self.job.check()?;
        if let (true, Some(i)) = (self.counts, self.job.inner.as_deref()) {
            let mut c = lock(&i.state);
            c.started += 1;
            c.item = item();
            Job::report(i, &mut c, false);
        }
        Ok(())
    }
}

impl Drop for Stage<'_> {
    fn drop(&mut self) {
        let Some(i) = self.job.inner.as_deref() else {
            return;
        };
        let mut c = lock(&i.state);
        if self.counts && !self.job.is_cancelled() {
            c.started = c.total + 1;
            Job::report(i, &mut c, false);
        }
        c.depth = c.depth.saturating_sub(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collecting() -> (Sink, Arc<Mutex<Vec<Value>>>) {
        let got = Arc::new(Mutex::new(Vec::new()));
        let into = Arc::clone(&got);
        (Box::new(move |v: &Value| lock(&into).push(v.clone())), got)
    }

    #[test]
    fn asks_come_out_of_params() {
        let mut params = Some(json!({"part": "p", "progress": true, "token": "t1"}));
        let ask = Ask::take(&mut params).unwrap();
        assert_eq!(
            ask,
            Ask {
                progress: true,
                token: Some("t1".into())
            }
        );
        assert_eq!(params, Some(json!({"part": "p"})));
        assert_eq!(Ask::take(&mut None).unwrap(), Ask::default());
        let long = "x".repeat(TOKEN_MAX + 1);
        for bad in [
            json!({"token": ""}),
            json!({"token": long}),
            json!({"token": 3}),
            json!({"progress": "yes"}),
        ] {
            let e = Ask::take(&mut Some(bad)).unwrap_err();
            assert_eq!(e.code, Code::InvalidParam);
        }
        let mut sent = json!({"part": "p"});
        ask.put(&mut sent);
        assert_eq!(sent, json!({"part": "p", "progress": true, "token": "t1"}));
    }

    #[test]
    fn no_job_reports_nothing_and_never_stops() {
        let job = Job::default();
        let stage = job.begin(Phase::Rebuild, "features", 3);
        for _ in 0..3 {
            stage.step(|| unreachable!()).unwrap();
        }
        assert!(job.check().is_ok() && !job.ignored());
    }

    #[test]
    fn progress_counts_the_outermost_stage_and_ends_once_more() {
        let (line, lines) = collecting();
        let (topic, published) = collecting();
        let flag = Arc::new(AtomicBool::new(false));
        let job = Job::new("t1", "agent-1", "rebuild", flag)
            .with_line(line)
            .with_topic(topic)
            .unthrottled();
        {
            let parts = job.begin(Phase::Rebuild, "parts", 2);
            for p in ["a", "b"] {
                parts.step(|| p.to_owned()).unwrap();
                let inner = job.begin(Phase::Rebuild, "features", 9);
                inner.step(|| "ignored".to_owned()).unwrap();
            }
        }
        job.finish();
        let lines = lock(&lines).clone();
        let at = |done: u64, item: &str| json!({"token": "t1", "phase": "rebuild", "done": done, "total": 2, "unit": "parts", "item": item});
        let mut end = at(2, "b");
        end["end"] = json!(true);
        assert_eq!(lines, [at(0, "a"), at(1, "b"), at(2, "b"), end]);
        let last = lock(&published).last().cloned().unwrap();
        assert_eq!(last["participant"], "agent-1");
        assert_eq!(last["method"], "rebuild");
        assert_eq!(last["end"], true);
    }

    #[test]
    fn a_requests_phase_overrides_its_stages() {
        let (line, lines) = collecting();
        let job = Job::new("t", "p", "import", Arc::new(AtomicBool::new(false)))
            .with_line(line)
            .unthrottled();
        job.phase(Phase::Heal);
        job.begin(Phase::Rebuild, "parts", 1)
            .step(|| "a".into())
            .unwrap();
        assert_eq!(lock(&lines)[0]["phase"], "heal");
    }

    #[test]
    fn progress_is_throttled_but_the_end_always_goes() {
        let (line, lines) = collecting();
        let job = Job::new("t", "p", "check", Arc::new(AtomicBool::new(false))).with_line(line);
        {
            let s = job.begin(Phase::Check, "parts", 1000);
            for n in 0..1000 {
                s.step(|| n.to_string()).unwrap();
            }
        }
        job.finish();
        let lines = lock(&lines).clone();
        // The first and the end at least; a thousand quick steps don't
        // make a thousand lines.
        assert!(lines.len() >= 2 && lines.len() < 50, "{}", lines.len());
        assert_eq!(lines.last().unwrap()["done"], 1000);
    }

    #[test]
    fn a_cancel_stops_at_the_next_boundary_with_the_counts() {
        let flag = Arc::new(AtomicBool::new(false));
        let job = Job::new("t", "p", "rebuild", Arc::clone(&flag));
        let s = job.begin(Phase::Rebuild, "features", 40);
        for n in 0..12 {
            s.step(|| n.to_string()).unwrap();
        }
        flag.store(true, Ordering::SeqCst);
        let e = s.step(|| "fillet1".into()).unwrap_err();
        assert_eq!(e.code, Code::Cancelled);
        assert_eq!(e.message, "rebuild was cancelled after 12 of 40 features");
        assert_eq!(
            (e.details["done"].clone(), e.details["total"].clone()),
            (json!(12), json!(40))
        );
    }

    #[test]
    fn a_cancel_after_the_first_write_is_ignored_and_said_so() {
        let flag = Arc::new(AtomicBool::new(false));
        let job = Job::new("t", "p", "import", Arc::clone(&flag));
        job.writing();
        flag.store(true, Ordering::SeqCst);
        assert!(job.check().is_ok());
        assert!(job.ignored());
    }

    #[test]
    fn an_answer_a_cancel_reached_is_cancelled() {
        let flag = Arc::new(AtomicBool::new(false));
        let job = Job::new("t", "p", "diff", Arc::clone(&flag));
        assert_eq!(job.settle(Ok(1)).unwrap(), 1);
        flag.store(true, Ordering::SeqCst);
        assert_eq!(job.settle(Ok(1)).unwrap_err().code, Code::Cancelled);
        let e = Error::new(Code::NotFound, "x");
        assert_eq!(job.settle::<()>(Err(e)).unwrap_err().code, Code::NotFound);
    }

    #[test]
    fn a_test_job_cancels_itself_at_its_nth_boundary() {
        let job = Job::cancelled_at(3);
        assert!(job.check().is_ok());
        assert!(job.check().is_ok());
        assert_eq!(job.check().unwrap_err().code, Code::Cancelled);
    }
}
