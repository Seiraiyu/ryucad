//! When a watched daemon tells its subscribers what changed (#849, design
//! 698): a request that changed something marks it pending and answers.
//! The daemon's watcher publishes once the daemon has been quiet for
//! `quiet` (no request started or finished), or at the latest `max` after
//! the first unpublished change, so a long burst still updates a window
//! about once a second. A publish covers every change since the last.
//! While nothing is pending, the watcher checks for changes made behind
//! the daemon's back every `poll_ms`, as before.

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
        }
    }

    /// A request changed something subscribers will hear of.
    pub fn mark(&mut self, now: Instant) {
        self.since.get_or_insert(now);
    }

    /// When the first unpublished change was marked.
    pub fn pending(&self) -> Option<Instant> {
        self.since
    }

    /// Everything marked has been published.
    pub fn clear(&mut self) {
        self.since = None;
    }

    /// Whether the watcher settles now, and why. `last_request` is when a
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
            Some(first) => (since(last_request) >= self.quiet || since(first) >= self.max)
                .then_some(Due::Publish),
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
        // Requests keep coming: not quiet, not yet a second.
        assert_eq!(c.due(c0 + ms(500), c0 + ms(450), c0, poll), None);
        // Quiet for 150 ms.
        assert_eq!(
            c.due(c0 + ms(600), c0 + ms(450), c0, poll),
            Some(Due::Publish)
        );
        // Never quiet, but a second since the first change.
        assert_eq!(
            c.due(c0 + ms(1000), c0 + ms(990), c0, poll),
            Some(Due::Publish)
        );
        c.clear();
        assert_eq!(c.pending(), None);
        assert_eq!(c.tick(poll), ms(150));
        assert_eq!(Coalesce::new(5, 1000).tick(poll), ms(10));
    }
}
