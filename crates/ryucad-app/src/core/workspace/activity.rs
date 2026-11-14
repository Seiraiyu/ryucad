//! The Activity and History tabs: what the window asks the daemon for it (an `impl Workspace`
//! block) and, from 27.F's Task 7 on, what it does with the answers.
//! Its fields live on `core::activity::Activity`.

use super::*;

impl Workspace {
    /// Reads the project's Git history again.
    pub fn load_history(&mut self) {
        self.activity.history_at = Some(Instant::now());
        self.activity.history_waiting = true;
        let req = Request::HistoryLog {
            part: None,
            limit: Some(100),
        };
        self.send(req, Pending::History);
    }

    /// Reads the history again after the project changed, once it was
    /// asked for: at most one reading on its way, and one more after it.
    pub(super) fn reload_history(&mut self) {
        if self.activity.history_waiting {
            self.activity.history_again = true;
        } else if self.activity.history.is_some() {
            self.load_history();
        }
    }

    /// The History tab is shown: read the history again when the last
    /// reading is older than [`HISTORY_EVERY`]. Returns when to look again.
    pub fn history_shown(&mut self, now: Instant) -> Duration {
        let due = self.activity.history_at.map_or(now, |t| t + HISTORY_EVERY);
        if now >= due && !self.activity.history_waiting {
            self.load_history();
            return HISTORY_EVERY;
        }
        due.saturating_duration_since(now)
            .max(Duration::from_millis(100))
    }
}
