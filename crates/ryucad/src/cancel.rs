//! The CLI's side of long requests: the token it sends, `--progress` as
//! text on stderr, and Ctrl+C. The first Ctrl+C cancels the request (a
//! `request.cancel` on a connection of its own, or the job's flag in
//! process) and waits up to [`CANCEL_WAIT`] for its answer; the second
//! exits at once.

use crate::error::{Code, Error};
use ryu_session::job::Ask;
use serde_json::{Value, json};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How long the CLI waits for a cancelled request's own answer.
pub const CANCEL_WAIT: Duration = Duration::from_secs(5);
/// How often `--progress` prints a line.
pub const PRINT_EVERY: Duration = Duration::from_secs(1);
/// The exit code of a second Ctrl+C: a shell's for SIGINT.
pub const INTERRUPTED: i32 = 130;

/// A token for one CLI request: its process and the time, unique enough
/// among one participant's requests.
pub fn token() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    format!("cli-{}-{nanos:08x}", std::process::id())
}

/// What the CLI asks of a long request: always a token, and progress
/// with `--progress`.
pub fn ask(progress: bool) -> Ask {
    Ask {
        progress,
        token: Some(token()),
    }
}

/// One line of `--progress` text for a `progress` notification's params:
/// `rebuild: 12/40 features (p/fillet1)`.
pub fn text(method: &str, p: &Value) -> String {
    let item = p["item"].as_str().unwrap_or_default();
    let item = if item.is_empty() {
        String::new()
    } else {
        format!(" ({item})")
    };
    format!(
        "{method}: {}/{} {}{item}",
        p["done"],
        p["total"],
        p["unit"].as_str().unwrap_or_default()
    )
}

/// Prints progress to stderr at most once a [`PRINT_EVERY`], and the end
/// always.
pub struct Printer {
    method: String,
    last: Mutex<Option<Instant>>,
}

impl Printer {
    pub fn new(method: &str) -> Printer {
        Printer {
            method: method.to_owned(),
            last: Mutex::new(None),
        }
    }

    /// The line to print for `p` now, if one is due.
    pub fn due(&self, p: &Value, now: Instant) -> Option<String> {
        let mut last = self.last.lock().unwrap_or_else(|e| e.into_inner());
        let end = p["end"] == json!(true);
        if !end && last.is_some_and(|t| now.duration_since(t) < PRINT_EVERY) {
            return None;
        }
        *last = Some(now);
        Some(text(&self.method, p))
    }

    pub fn print(&self, p: &Value) {
        if let Some(line) = self.due(p, Instant::now()) {
            eprintln!("{line}");
        }
    }
}

/// What the first Ctrl+C does once its cancel is sent, by whether the
/// request said it had begun writing (#268).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fallback {
    /// Wait [`CANCEL_WAIT`] for the request's own answer, then say
    /// `cancelled` and exit 4.
    Exit,
    /// Say why on stderr and wait for the answer however long it takes:
    /// a request that writes finishes (a second Ctrl+C still exits).
    Wait(&'static str),
}

/// The first Ctrl+C's fallback, `writing` whether the request said it
/// had begun writing.
pub fn fallback(writing: bool) -> Fallback {
    if writing {
        Fallback::Wait("the request is writing its files; it finishes first (Ctrl+C again to exit)")
    } else {
        Fallback::Exit
    }
}

/// The cancel the first Ctrl+C sends: it answers whether the request had
/// begun writing, so a cancel no longer stops it.
pub type Cancel = Box<dyn FnOnce() -> bool + Send>;

/// What the Ctrl+C handler does: how many times it has fired, and the
/// cancel to send on the first.
pub struct Interrupt {
    count: AtomicU8,
    cancel: Mutex<Option<Cancel>>,
}

impl Interrupt {
    /// Installs the process's Ctrl+C handler. `cancel` runs on the first
    /// Ctrl+C; if the process is still running [`CANCEL_WAIT`] later, it
    /// prints `cancelled` itself and exits 4. The second exits at once.
    pub fn install(cancel: Cancel, pretty: bool) -> Arc<Interrupt> {
        let me = Arc::new(Interrupt {
            count: AtomicU8::new(0),
            cancel: Mutex::new(Some(cancel)),
        });
        let handler = Arc::clone(&me);
        // Installed once per process; a second install (tests) keeps the
        // first.
        let _ = ctrlc::set_handler(move || handler.fire(pretty));
        me
    }

    fn fire(&self, pretty: bool) {
        if self.count.fetch_add(1, Ordering::SeqCst) > 0 {
            std::process::exit(INTERRUPTED);
        }
        let writing = match self.cancel.lock().unwrap_or_else(|e| e.into_inner()).take() {
            Some(cancel) => cancel(),
            None => false,
        };
        if let Fallback::Wait(why) = fallback(writing) {
            eprintln!("{why}");
            return;
        }
        std::thread::spawn(move || {
            std::thread::sleep(CANCEL_WAIT);
            let e = Error::new(
                Code::Cancelled,
                "cancel sent; the daemon is still finishing a kernel call",
            )
            .with("answered", false);
            let doc = json!({"ok": false, "error": e, "warnings": []});
            let s = if pretty {
                serde_json::to_string_pretty(&doc)
            } else {
                serde_json::to_string(&doc)
            };
            println!("{}", s.expect("JSON values always serialize"));
            std::process::exit(i32::from(Code::Cancelled.exit_code()));
        });
    }
}

/// A flag for a request run in this process, which Ctrl+C sets.
pub fn local_flag() -> Arc<AtomicBool> {
    Arc::new(AtomicBool::new(false))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_reads_as_one_line() {
        let p = json!({"done": 12, "total": 40, "unit": "features", "item": "p/fillet1"});
        assert_eq!(text("rebuild", &p), "rebuild: 12/40 features (p/fillet1)");
        let p = json!({"done": 0, "total": 3, "unit": "pairs", "item": ""});
        assert_eq!(text("interference", &p), "interference: 0/3 pairs");
    }

    #[test]
    fn progress_prints_once_a_second_and_the_end_always() {
        let printer = Printer::new("check");
        let t0 = Instant::now();
        let p = json!({"done": 1, "total": 9, "unit": "parts", "item": "a"});
        assert!(printer.due(&p, t0).is_some());
        assert!(printer.due(&p, t0 + Duration::from_millis(400)).is_none());
        assert!(printer.due(&p, t0 + Duration::from_millis(1000)).is_some());
        let end = json!({"done": 9, "total": 9, "unit": "parts", "item": "i", "end": true});
        assert!(
            printer
                .due(&end, t0 + Duration::from_millis(1100))
                .is_some()
        );
    }

    /// #268: a request that said it writes isn't given the fallback.
    #[test]
    fn the_fallback_waits_for_a_request_that_writes() {
        assert!(matches!(fallback(true), Fallback::Wait(_)));
        assert_eq!(fallback(false), Fallback::Exit);
    }

    #[test]
    fn tokens_fit_the_protocol() {
        let t = token();
        assert!(t.starts_with("cli-") && t.len() <= ryu_session::job::TOKEN_MAX);
    }
}
