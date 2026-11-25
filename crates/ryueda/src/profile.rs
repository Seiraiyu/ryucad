//! Stage timings of a request (ryueda 7.E): `ryueda profile <stage> <ms>`
//! lines, in order, when `RYUEDA_PROFILE` is set: to stderr for `1`, else
//! appended to the file it names (a daemon's stderr goes to its log).
//! Unset, a stage costs one relaxed load.

use std::io::Write;
use std::sync::OnceLock;
use std::time::Instant;

fn sink() -> Option<&'static str> {
    static SINK: OnceLock<Option<String>> = OnceLock::new();
    SINK.get_or_init(|| std::env::var("RYUEDA_PROFILE").ok().filter(|v| !v.is_empty()))
        .as_deref()
}

/// A stage's timer: prints its time when dropped.
pub struct Stage(&'static str, Option<Instant>);

pub fn stage(name: &'static str) -> Stage {
    Stage(name, sink().map(|_| Instant::now()))
}

impl Drop for Stage {
    fn drop(&mut self) {
        let (Some(t), Some(to)) = (self.1, sink()) else {
            return;
        };
        let line = format!(
            "ryueda profile {} {:.3}\n",
            self.0,
            t.elapsed().as_secs_f64() * 1e3
        );
        if to == "1" {
            eprint!("{line}");
        } else if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(to) {
            let _ = f.write_all(line.as_bytes());
        }
    }
}
