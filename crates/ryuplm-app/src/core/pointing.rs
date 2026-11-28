//! Pointing (design 938): this window's selection and view go to the
//! daemon at most every [`SELECT_EVERY`], the last value winning, and a
//! view a participant shows is adopted unless it is the window's own
//! echo (the last [`SENT_VIEWS`] `view.set`s), as ryueda-app's.

use crate::core::people::ItemView;
use std::time::{Duration, Instant};

pub const SELECT_EVERY: Duration = Duration::from_millis(150);
/// How many `view.set`s a window remembers.
pub const SENT_VIEWS: usize = 8;

/// Sends the last value at most every [`SELECT_EVERY`]. One kept
/// [`Throttle::ordered`] also waits for each value's answer before the
/// next goes, so values sent on different connections are still applied
/// in the order they were sent.
#[derive(Debug, Default)]
pub struct Throttle {
    due: bool,
    sent: Option<Instant>,
    ordered: bool,
    out: bool,
}

impl Throttle {
    /// A throttle whose values wait for the last one's answer.
    pub fn ordered() -> Throttle {
        Throttle {
            ordered: true,
            ..Throttle::default()
        }
    }

    /// The value sent was answered (or its connection lost): the next may go.
    pub fn answered(&mut self) {
        self.out = false;
    }

    pub fn due(&mut self) {
        self.due = true;
    }

    /// Whether to send now; if so, it counts as sent.
    pub fn ready(&mut self, now: Instant) -> bool {
        let ok = self.due
            && !self.out
            && self
                .sent
                .is_none_or(|t| now.saturating_duration_since(t) >= SELECT_EVERY);
        if ok {
            self.due = false;
            self.sent = Some(now);
            self.out = self.ordered;
        }
        ok
    }

    /// How long until a due value may go.
    pub fn wait(&self, now: Instant) -> Option<Duration> {
        let t = self.sent.filter(|_| self.due && !self.out)?;
        Some(SELECT_EVERY.saturating_sub(now.saturating_duration_since(t))).filter(|d| !d.is_zero())
    }

    pub fn reset(&mut self) {
        self.sent = None;
        self.out = false;
    }
}

/// The views a window sent with `view.set`, newest last.
#[derive(Debug, Default)]
pub struct Sent {
    views: Vec<ItemView>,
}

fn place(v: &ItemView) -> (Option<String>, Option<String>, ryuplm::view::List) {
    (v.item.clone(), v.tab.clone(), v.list.clone())
}

impl Sent {
    pub fn set(&mut self, v: ItemView) {
        self.views.retain(|s| *s != v);
        self.views.push(v);
        if self.views.len() > SENT_VIEWS {
            self.views.remove(0);
        }
    }

    /// The window sent its own Go to: what comes back next is the show.
    pub fn show(&mut self) {
        self.views.clear();
    }

    /// Whether the window adopts its participant's view as a
    /// `participant.changed` brings it (`before` and `now`): when it
    /// moved, and it is the window's own Go to or (with `follow_shows`)
    /// another's show. A view marked the window's own that is one of its
    /// recent `view.set`s is an echo, never adopted.
    pub fn adopts(
        &self,
        before: Option<&ItemView>,
        now: &ItemView,
        me: &str,
        follow_shows: bool,
    ) -> bool {
        if now.shown_by.is_none() || before == Some(now) {
            return false;
        }
        if now.shown_by.as_deref() == Some(me) {
            return !self.views.iter().any(|s| place(s) == place(now));
        }
        follow_shows
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn a_throttle_sends_the_last_value_at_most_every_150_ms() {
        let t0 = Instant::now();
        let mut t = Throttle::default();
        assert!(!t.ready(t0), "nothing due");
        t.due();
        assert!(t.ready(t0));
        t.due();
        assert!(!t.ready(t0 + Duration::from_millis(100)));
        assert_eq!(
            t.wait(t0 + Duration::from_millis(100)),
            Some(Duration::from_millis(50))
        );
        assert!(t.ready(t0 + SELECT_EVERY));
    }

    fn at(item: &str, tab: &str, by: Option<&str>) -> ItemView {
        ItemView {
            item: Some(item.into()),
            tab: Some(tab.into()),
            list: Default::default(),
            shown_by: by.map(str::to_owned),
        }
    }

    #[test]
    fn an_ordered_throttle_sends_nothing_while_its_last_value_is_out() {
        let t0 = Instant::now();
        let mut t = Throttle::ordered();
        t.due();
        assert!(t.ready(t0));
        t.due();
        let later = t0 + Duration::from_secs(1);
        assert!(!t.ready(later), "the first pick isn't answered yet");
        assert_eq!(t.wait(later), None, "its answer wakes the window");
        t.answered();
        assert!(t.ready(later), "the last value goes once it is");
        t.answered();
        assert!(!t.ready(later), "nothing more due");
        t.due();
        t.reset();
        assert!(t.ready(later), "a lost connection's value never blocks");
    }

    #[test]
    fn a_window_adopts_shows_it_didn_t_send() {
        let mut s = Sent::default();
        let mine = at("a", "bom", Some("me"));
        s.set(mine.clone());
        assert!(!s.adopts(None, &mine, "me", true), "its own echo");
        let agents = at("b", "bom", Some("agent"));
        assert!(s.adopts(Some(&mine), &agents, "me", true));
        assert!(
            !s.adopts(Some(&mine), &agents, "me", false),
            "follow_shows off"
        );
        assert!(
            !s.adopts(Some(&agents), &agents, "me", true),
            "nothing moved"
        );
        // Its own Go to, even to a place it sent once.
        s.show();
        assert!(s.adopts(Some(&agents), &mine, "me", false));
    }

    #[test]
    fn an_older_send_s_echo_is_never_adopted() {
        let mut s = Sent::default();
        s.set(at("a", "bom", Some("me")));
        s.set(at("b", "bom", Some("me")));
        assert!(!s.adopts(
            Some(&at("b", "bom", Some("me"))),
            &at("a", "bom", Some("me")),
            "me",
            true
        ));
        for k in 0..SENT_VIEWS {
            s.set(at(&format!("x{k}"), "bom", Some("me")));
        }
        assert!(
            s.adopts(None, &at("a", "bom", Some("me")), "me", true),
            "only the last eight"
        );
    }
}
