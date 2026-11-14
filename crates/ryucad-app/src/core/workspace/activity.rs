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
        self.send(req, Pending::Activity(activity::Asked::History));
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

/// What one of this sub-state's requests was for (design 541,
/// "Typed handles"): `Workspace::answer` routes its answer here.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Asked {
    Activity,
    History,
}

impl Activity {
    /// Takes the answer to one of this sub-state's requests (design
    /// 541, "Typed handles"); what it does elsewhere goes in `fx`.
    pub(super) fn answer(&mut self, why: Asked, answer: Result<Answer>, fx: &mut Effects) {
        let history = matches!(why, Asked::History);
        match (why, answer) {
            (Asked::Activity, Ok(a)) => self.reset(&a.result["entries"]),
            (Asked::History, Ok(a)) => self.history = Some(a.result),
            // Said in the tab, not left spinning.
            (Asked::History, Err(e)) => {
                self.history = Some(json!({"error": e.message}));
                fx.fail(e);
            }
            (_, Err(e)) => fx.fail(e),
        }
        // The history wanted again while this reading was on its way.
        if history {
            self.history_waiting = false;
            if std::mem::take(&mut self.history_again) {
                fx.then(Then::LoadHistory);
            }
        }
    }
}

impl Activity {
    pub(super) fn on_lost(&mut self) {
        self.history_waiting = false;
        self.history_again = false;
    }
}
