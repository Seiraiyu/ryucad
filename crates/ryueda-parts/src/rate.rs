//! A live source's rate limits, kept per process (design 347): DigiKey's
//! 120 calls a minute and 1,000 a day. Past one, a call is refused with
//! `parts_rate_limited` and when to try again, never a silent wait; a
//! source's own 429 holds every call until the time it gave.

use crate::error::{Code, Error, Result};
use serde::Serialize;
use std::collections::{BTreeMap, VecDeque};
use std::sync::{Mutex, OnceLock};

const MINUTE: u64 = 60;
const DAY: u64 = 86_400;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Limits {
    pub per_minute: u32,
    pub per_day: u32,
}

#[derive(Default)]
struct Window {
    /// When each call in the last day was made, oldest first.
    calls: VecDeque<u64>,
    /// No call before this (a source's own `Retry-After`).
    held_until: u64,
}

impl Window {
    fn forget(&mut self, now: u64) {
        while self
            .calls
            .front()
            .is_some_and(|t| now.saturating_sub(*t) >= DAY)
        {
            self.calls.pop_front();
        }
    }

    fn in_minute(&self, now: u64) -> usize {
        self.calls
            .iter()
            .rev()
            .take_while(|t| now.saturating_sub(**t) < MINUTE)
            .count()
    }
}

/// Calls made by source, in a sliding minute and day.
#[derive(Default)]
pub struct Limiter {
    windows: Mutex<BTreeMap<String, Window>>,
}

/// How many calls a source has made: in the last minute and day.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Used {
    pub minute: u32,
    pub day: u32,
}

fn limited(source: &str, limit: &str, retry_at: u64, now: u64) -> Error {
    let after = retry_at.saturating_sub(now).max(1);
    Error::new(
        Code::PartsRateLimited,
        format!("{source}'s {limit} rate limit is spent; try again in {after} s"),
    )
    .with("source", source)
    .with("limit", limit)
    .with("retry_after_s", after)
    .with("retry_at", crate::time::rfc3339(now + after))
}

impl Limiter {
    pub fn new() -> Limiter {
        Limiter::default()
    }

    /// This process's.
    pub fn process() -> &'static Limiter {
        static L: OnceLock<Limiter> = OnceLock::new();
        L.get_or_init(Limiter::new)
    }

    /// One call of `source` at `now` (UTC seconds), counted, or
    /// `parts_rate_limited` with `limit` (`source`, `minute`, `day`) and
    /// when to try again.
    pub fn take(&self, source: &str, limits: Limits, now: u64) -> Result<()> {
        let mut w = self.windows.lock().unwrap_or_else(|p| p.into_inner());
        let w = w.entry(source.to_owned()).or_default();
        w.forget(now);
        if now < w.held_until {
            return Err(limited(source, "source", w.held_until, now));
        }
        if w.calls.len() >= limits.per_day as usize {
            let oldest = w.calls.front().copied().unwrap_or(now);
            return Err(limited(source, "day", oldest + DAY, now));
        }
        let minute = w.in_minute(now);
        if minute >= limits.per_minute as usize {
            let first = w.calls[w.calls.len() - minute];
            return Err(limited(source, "minute", first + MINUTE, now));
        }
        w.calls.push_back(now);
        Ok(())
    }

    /// The source said to wait until `until` (UTC seconds).
    pub fn hold(&self, source: &str, until: u64) {
        let mut w = self.windows.lock().unwrap_or_else(|p| p.into_inner());
        let w = w.entry(source.to_owned()).or_default();
        w.held_until = w.held_until.max(until);
    }

    /// `parts_rate_limited` for a source's own 429 at `now`, held for
    /// `retry_after` seconds (60 when it didn't say).
    pub fn refused(&self, source: &str, retry_after: Option<u64>, now: u64) -> Error {
        let until = now + retry_after.unwrap_or(MINUTE).max(1);
        self.hold(source, until);
        limited(source, "source", until, now)
    }

    pub fn used(&self, source: &str, now: u64) -> Used {
        let mut w = self.windows.lock().unwrap_or_else(|p| p.into_inner());
        let Some(w) = w.get_mut(source) else {
            return Used { minute: 0, day: 0 };
        };
        w.forget(now);
        Used {
            minute: w.in_minute(now) as u32,
            day: w.calls.len() as u32,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const L: Limits = Limits {
        per_minute: 3,
        per_day: 5,
    };

    #[test]
    fn calls_past_a_minutes_limit_wait_for_the_oldest_to_age() {
        let l = Limiter::new();
        for t in [100, 110, 120] {
            l.take("dk", L, t).unwrap();
        }
        let e = l.take("dk", L, 130).unwrap_err();
        assert_eq!(e.code, Code::PartsRateLimited);
        assert_eq!(e.details["limit"], "minute");
        assert_eq!(
            e.details["retry_after_s"], 30,
            "the call at 100 ages out at 160"
        );
        assert_eq!(l.used("dk", 130), Used { minute: 3, day: 3 });
        l.take("dk", L, 160).unwrap();
        // Another source counts apart.
        l.take("other", L, 130).unwrap();
    }

    #[test]
    fn a_days_limit_holds_until_its_first_call_is_a_day_old() {
        let l = Limiter::new();
        for t in [0, 100, 200, 300, 400] {
            l.take("dk", L, t).unwrap();
        }
        let e = l.take("dk", L, 1_000).unwrap_err();
        assert_eq!(
            (
                e.details["limit"].as_str(),
                e.details["retry_after_s"].as_u64()
            ),
            (Some("day"), Some(DAY - 1_000))
        );
        l.take("dk", L, DAY).unwrap();
        assert_eq!(l.used("dk", DAY).day, 5);
    }

    #[test]
    fn a_sources_own_refusal_holds_every_call() {
        let l = Limiter::new();
        let e = l.refused("dk", Some(7), 50);
        assert_eq!(
            (
                e.details["limit"].as_str(),
                e.details["retry_after_s"].as_u64()
            ),
            (Some("source"), Some(7))
        );
        assert_eq!(l.take("dk", L, 56).unwrap_err().details["retry_after_s"], 1);
        l.take("dk", L, 57).unwrap();
        assert_eq!(l.refused("dk", None, 60).details["retry_after_s"], 60);
    }
}
