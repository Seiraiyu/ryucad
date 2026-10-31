//! One `ryucad` command in a process of its own: in process
//! (`RYUCAD_NO_DAEMON`), on one rayon thread, with private runtime, config
//! and cache folders, killed when it passes the timeout.

use std::ffi::OsStr;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

/// The binary under test and how to run it.
#[derive(Clone, Debug)]
pub struct Ryucad {
    pub exe: PathBuf,
    pub timeout: Duration,
}

/// How a command ended.
#[derive(Clone, Debug, PartialEq)]
pub enum Ended {
    /// It printed an envelope (`{"ok": …}`) and exited 0 to 4.
    Answered(Value),
    /// It exited otherwise (a panic, a signal) or printed no envelope.
    Crashed {
        exit: Option<i32>,
        stderr: String,
    },
    TimedOut,
}

/// A command's end and how long it took.
#[derive(Clone, Debug, PartialEq)]
pub struct Call {
    pub seconds: f64,
    pub ended: Ended,
}

/// At most the last `n` bytes of `bytes`, as text.
fn tail(bytes: &[u8], n: usize) -> String {
    String::from_utf8_lossy(&bytes[bytes.len().saturating_sub(n)..]).into_owned()
}

impl Ryucad {
    /// Runs `ryucad -C <dir> <args>` for `stage`, with the case's private
    /// folders under `home`. `STEP_HUNT_STAGE` names the stage to the
    /// process (ryucad ignores it; the test double reads it).
    pub fn call<S: AsRef<OsStr>>(&self, home: &Path, dir: &Path, stage: &str, args: &[S]) -> Call {
        let mut cmd = Command::new(&self.exe);
        cmd.arg("-C")
            .arg(dir)
            .args(args)
            .env("RYUCAD_NO_DAEMON", "1")
            .env("RAYON_NUM_THREADS", "1")
            .env("RYUCAD_RUNTIME_DIR", home.join("run"))
            .env("RYUCAD_CONFIG_DIR", home.join("config"))
            .env("RYUCAD_CACHE_DIR", home.join("cache"))
            .env("STEP_HUNT_STAGE", stage)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let start = Instant::now();
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                return Call {
                    seconds: 0.0,
                    ended: Ended::Crashed {
                        exit: None,
                        stderr: format!("spawning {}: {e}", self.exe.display()),
                    },
                };
            }
        };
        // Read both pipes on threads of their own, so a full pipe never
        // stops the child.
        let mut out = child.stdout.take().expect("piped");
        let mut err = child.stderr.take().expect("piped");
        let out = std::thread::spawn(move || {
            let mut b = Vec::new();
            let _ = out.read_to_end(&mut b);
            b
        });
        let err = std::thread::spawn(move || {
            let mut b = Vec::new();
            let _ = err.read_to_end(&mut b);
            b
        });
        let status = loop {
            match child.try_wait() {
                Ok(Some(s)) => break Some(s),
                Ok(None) => {}
                Err(_) => break None,
            }
            if start.elapsed() >= self.timeout {
                let _ = child.kill();
                let _ = child.wait();
                let _ = (out.join(), err.join());
                return Call {
                    seconds: start.elapsed().as_secs_f64(),
                    ended: Ended::TimedOut,
                };
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        let seconds = start.elapsed().as_secs_f64();
        let stdout = out.join().unwrap_or_default();
        let stderr = err.join().unwrap_or_default();
        let exit = status.and_then(|s| s.code());
        let envelope = serde_json::from_slice::<Value>(&stdout)
            .ok()
            .filter(|v| v["ok"].is_boolean());
        let ended = match (exit, envelope) {
            (Some(0..=4), Some(v)) => Ended::Answered(v),
            _ => Ended::Crashed {
                exit,
                stderr: tail(&stderr, 4096),
            },
        };
        Call { seconds, ended }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tails_keep_the_end() {
        assert_eq!(tail(b"abcdef", 3), "def");
        assert_eq!(tail(b"ab", 3), "ab");
    }
}
