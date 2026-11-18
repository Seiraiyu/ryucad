//! When a watched daemon tells its subscribers what changed (#849, design
//! 698): a request that changed something marks it pending and answers.
//! A lone change, one marked at least `quiet` after the change before it,
//! wakes the watcher and is published as soon as the request lets go of
//! the session (#960: holding it for the quiet window put a lone edit
//! over design 192's 300 ms bar). Only a burst
//! coalesces: a change marked within `quiet` of another is published once
//! the daemon has been quiet for `quiet` (no request started or finished),
//! or at the latest `max` after the first unpublished change, so a long
//! burst still updates a window about once a second. A publish covers every change since the last.
//! While nothing is pending, the watcher checks for changes made behind
//! the daemon's back every `poll_ms`, as before.

use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// `daemon.publish_quiet_ms`'s default (question 1009).
pub const QUIET_MS: u64 = 150;
/// `daemon.publish_max_ms`'s default (question 1009).
pub const MAX_MS: u64 = 1000;

#[derive(Clone, Debug)]
pub struct Coalesce {
    pub quiet: Duration,
    pub max: Duration,
    /// The first change not yet published.
    since: Option<Instant>,
    /// Whether that change came alone, `quiet` or more after the change
    /// before it: published without waiting (#960).
    lone: bool,
    /// When the last change was marked, published or not.
    last: Option<Instant>,
    /// Wakes the watcher when a lone change is marked, so it publishes
    /// as soon as the request lets go of the session, not a tick later.
    wake: Wake,
}

/// What the daemon's watcher sleeps on between looks: woken early by a
/// lone change (#960).
#[derive(Clone, Debug, Default)]
pub struct Wake(Arc<(Mutex<bool>, Condvar)>);

impl Wake {
    /// Sleeps for `tick`, or until a lone change is marked.
    pub fn sleep(&self, tick: Duration) {
        let (lock, cv) = &*self.0;
        let woken = lock.lock().unwrap_or_else(|p| p.into_inner());
        let mut woken = cv
            .wait_timeout_while(woken, tick, |w| !*w)
            .unwrap_or_else(|p| p.into_inner())
            .0;
        *woken = false;
    }

    fn nudge(&self) {
        let (lock, cv) = &*self.0;
        *lock.lock().unwrap_or_else(|p| p.into_inner()) = true;
        cv.notify_all();
    }
}

/// Why the watcher settles now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Due {
    /// Requests changed something: publish it (`daemon.publish`).
    Publish,
    /// Nothing pending; time to look for outside changes (`daemon.poll`).
    Poll,
}

impl Default for Coalesce {
    fn default() -> Self {
        Coalesce::new(QUIET_MS, MAX_MS)
    }
}

impl Coalesce {
    pub fn new(quiet_ms: u64, max_ms: u64) -> Coalesce {
        Coalesce {
            quiet: Duration::from_millis(quiet_ms),
            max: Duration::from_millis(max_ms),
            since: None,
            lone: false,
            last: None,
            wake: Wake::default(),
        }
    }

    /// A request changed something subscribers will hear of.
    pub fn mark(&mut self, now: Instant) {
        if self.since.is_none() {
            self.since = Some(now);
            self.lone = self
                .last
                .is_none_or(|l| now.saturating_duration_since(l) >= self.quiet);
            if self.lone {
                self.wake.nudge();
            }
        }
        self.last = Some(now);
    }

    /// What the watcher sleeps on.
    pub fn wake(&self) -> Wake {
        self.wake.clone()
    }

    /// When the first unpublished change was marked.
    pub fn pending(&self) -> Option<Instant> {
        self.since
    }

    /// Everything marked has been published.
    pub fn clear(&mut self) {
        self.since = None;
    }

    /// Whether the watcher settles now, and why: a lone change at once, a
    /// burst once quiet or once its first change is `max` old. `last_request` is when a
    /// request last started or finished, `last_settle` when the watcher
    /// last settled.
    pub fn due(
        &self,
        now: Instant,
        last_request: Instant,
        last_settle: Instant,
        poll: Duration,
    ) -> Option<Due> {
        let since = |t: Instant| now.saturating_duration_since(t);
        match self.since {
            Some(first) => {
                (self.lone || since(last_request) >= self.quiet || since(first) >= self.max)
                    .then_some(Due::Publish)
            }
            None => (since(last_settle) >= poll).then_some(Due::Poll),
        }
    }

    /// How long the watcher sleeps between looks: the poll, or the quiet
    /// interval when that is shorter, never under 10 ms.
    pub fn tick(&self, poll: Duration) -> Duration {
        poll.min(self.quiet).max(Duration::from_millis(10))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_change_is_published_once_quiet_or_once_old() {
        let c0 = Instant::now();
        let ms = Duration::from_millis;
        let poll = ms(250);
        let mut c = Coalesce::default();
        // Nothing pending: the poll's own pace.
        assert_eq!(c.due(c0 + ms(100), c0, c0, poll), None);
        assert_eq!(c.due(c0 + ms(250), c0, c0, poll), Some(Due::Poll));
        c.mark(c0);
        c.mark(c0 + ms(50)); // the first mark stands
        assert_eq!(c.pending(), Some(c0));
        // The first change came alone: published at once (#960).
        assert_eq!(
            c.due(c0 + ms(50), c0 + ms(50), c0, poll),
            Some(Due::Publish)
        );
        c.clear();
        // The burst goes on: within `quiet` of the change before it.
        c.mark(c0 + ms(100));
        assert_eq!(c.pending(), Some(c0 + ms(100)));
        // Requests keep coming: not quiet, not yet a second.
        assert_eq!(c.due(c0 + ms(500), c0 + ms(450), c0, poll), None);
        // Quiet for 150 ms.
        assert_eq!(
            c.due(c0 + ms(600), c0 + ms(450), c0, poll),
            Some(Due::Publish)
        );
        // Never quiet, but a second since the first change.
        c.mark(c0 + ms(1090));
        assert_eq!(
            c.due(c0 + ms(1100), c0 + ms(1090), c0, poll),
            Some(Due::Publish)
        );
        c.clear();
        assert_eq!(c.pending(), None);
        // Quiet since the burst's last change: the next comes alone.
        c.mark(c0 + ms(1090) + ms(150));
        assert_eq!(
            c.due(c0 + ms(1240), c0 + ms(1240), c0, poll),
            Some(Due::Publish)
        );
        c.clear();
        assert_eq!(c.tick(poll), ms(150));
        assert_eq!(Coalesce::new(5, 1000).tick(poll), ms(10));
    }

    /// A lone change wakes the watcher's sleep; a burst's later ones
    /// leave it to its tick (#960).
    #[test]
    fn a_lone_change_wakes_the_watcher() {
        let mut c = Coalesce::new(3_600_000, 3_600_000);
        let wake = c.wake();
        let t = Instant::now();
        let sleeper = std::thread::spawn(move || wake.sleep(Duration::from_secs(3600)));
        std::thread::sleep(Duration::from_millis(20));
        c.mark(Instant::now());
        sleeper.join().unwrap();
        assert!(t.elapsed() < Duration::from_secs(60));
        c.clear();
        // Within `quiet` of the last: no nudge, the sleep runs its tick.
        c.mark(Instant::now());
        let t = Instant::now();
        c.wake().sleep(Duration::from_millis(50));
        assert!(t.elapsed() >= Duration::from_millis(50));
    }
}
