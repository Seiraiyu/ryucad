//! The window's view of long requests (design 199): its own, with their
//! progress lines and Cancel, and every other participant's, from the
//! `progress` topic. Pure: the status line, the People strip and the
//! activity feed draw from it.

use serde_json::Value;
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

/// How long one of the window's own requests runs before its bar shows:
/// most `mesh.get`s answer sooner, and a bar that flashes says nothing.
pub const SHOW_AFTER: Duration = Duration::from_millis(500);
/// How long another participant's request is shown with no word from it:
/// a lost `end` (its daemon gone) doesn't leave it forever.
pub const STALE_AFTER: Duration = Duration::from_secs(10);

/// One long request as the window last heard of it.
#[derive(Clone, Debug, PartialEq)]
pub struct Running {
    pub token: String,
    /// Its method: `rebuild`, `check`, …
    pub method: String,
    /// `rebuild`, `heal`, `mesh`, `intersect` or `check`, once it has
    /// said.
    pub phase: Option<String>,
    pub done: u64,
    pub total: u64,
    /// `features`, `parts`, `bodies`, `pairs`, `revisions`.
    pub unit: String,
    /// What it is working on: a part and feature, a body, a pair.
    pub item: String,
    pub at: Instant,
}

impl Running {
    fn new(token: &str, method: &str, now: Instant) -> Running {
        Running {
            token: token.to_owned(),
            method: method.to_owned(),
            phase: None,
            done: 0,
            total: 0,
            unit: String::new(),
            item: String::new(),
            at: now,
        }
    }

    /// Takes in a progress line's params.
    fn update(&mut self, p: &Value, now: Instant) {
        self.phase = p["phase"].as_str().map(str::to_owned);
        self.done = p["done"].as_u64().unwrap_or(self.done);
        self.total = p["total"].as_u64().unwrap_or(self.total);
        self.unit = p["unit"].as_str().unwrap_or_default().to_owned();
        self.item = p["item"].as_str().unwrap_or_default().to_owned();
        self.at = now;
    }

    /// How far along, from 0 to 1, once it has counted anything.
    pub fn fraction(&self) -> Option<f32> {
        (self.total > 0).then(|| (self.done as f32 / self.total as f32).min(1.0))
    }

    /// `12 / 40 features`, or nothing before the first line.
    pub fn count(&self) -> String {
        match self.total {
            0 => String::new(),
            t => format!("{} / {t} {}", self.done, self.unit),
        }
    }

    /// `check 3/8 parts`: the People strip's words.
    pub fn short(&self) -> String {
        match self.total {
            0 => self.method.clone(),
            t => format!("{} {}/{t} {}", self.method, self.done, self.unit),
        }
    }
}

/// One of the window's own long requests.
#[derive(Clone, Debug, PartialEq)]
pub struct Mine {
    pub ticket: u64,
    /// What the status line says it is doing: `Rebuilding bracket`.
    pub label: String,
    pub running: Running,
    pub started: Instant,
    /// Cancel was pressed: the request still runs to its next boundary.
    pub cancelling: bool,
}

/// What the window knows of long requests.
#[derive(Clone, Debug, Default)]
pub struct Progress {
    /// The window's own, by ticket, oldest first.
    mine: BTreeMap<u64, Mine>,
    /// Other participants' running requests, by participant ID.
    others: BTreeMap<String, Running>,
    next: u64,
}

impl Progress {
    /// A token for the window's next long request: `w<n>`, unique among
    /// this window's requests.
    pub fn token(&mut self) -> String {
        self.next += 1;
        format!("w{}", self.next)
    }

    /// The window sent long request `ticket`.
    pub fn started(&mut self, ticket: u64, token: &str, method: &str, label: String, now: Instant) {
        self.mine.insert(
            ticket,
            Mine {
                ticket,
                label,
                running: Running::new(token, method, now),
                started: now,
                cancelling: false,
            },
        );
    }

    /// A progress line for the window's request `ticket`.
    pub fn line(&mut self, ticket: u64, p: &Value, now: Instant) {
        if let Some(m) = self.mine.get_mut(&ticket) {
            m.running.update(p, now);
        }
    }

    /// Request `ticket` answered: it is no longer running. Returns it,
    /// with how long it ran.
    pub fn answered(&mut self, ticket: u64) -> Option<Mine> {
        self.mine.remove(&ticket)
    }

    /// The daemon was lost: nothing of the window's is running.
    pub fn lost(&mut self) {
        self.mine.clear();
        self.others.clear();
    }

    /// Marks the window's oldest running request (the one the status
    /// line shows, once it shows) as cancelling and gives its token.
    pub fn cancel(&mut self) -> Option<String> {
        let m = self.mine.values_mut().next()?;
        m.cancelling = true;
        Some(m.running.token.clone())
    }

    /// The window's oldest running request, shown or not yet.
    pub fn oldest(&self) -> Option<&Mine> {
        self.mine.values().next()
    }

    /// The request the status line shows at `now`: the oldest of the
    /// window's own that has run at least [`SHOW_AFTER`].
    pub fn shown(&self, now: Instant) -> Option<&Mine> {
        self.mine
            .values()
            .find(|m| now.saturating_duration_since(m.started) >= SHOW_AFTER)
    }

    /// When the oldest request not yet shown will be: to repaint then.
    pub fn due(&self, now: Instant) -> Option<Duration> {
        self.mine
            .values()
            .map(|m| SHOW_AFTER.saturating_sub(now.saturating_duration_since(m.started)))
            .filter(|d| !d.is_zero())
            .min()
    }

    /// A `progress` notification: another participant's request (the
    /// window's own come on its request connection, and are skipped).
    pub fn published(&mut self, me: &str, p: &Value, now: Instant) {
        let Some(who) = p["participant"].as_str() else {
            return;
        };
        if who == me {
            return;
        }
        if p["end"] == true {
            self.others.remove(who);
            return;
        }
        let token = p["token"].as_str().unwrap_or_default();
        let method = p["method"].as_str().unwrap_or_default();
        let r = self
            .others
            .entry(who.to_owned())
            .or_insert_with(|| Running::new(token, method, now));
        if r.token != token {
            *r = Running::new(token, method, now);
        }
        r.update(p, now);
    }

    /// Another participant's running request, unless it has gone quiet
    /// for [`STALE_AFTER`].
    pub fn of(&self, participant: &str, now: Instant) -> Option<&Running> {
        self.others
            .get(participant)
            .filter(|r| now.saturating_duration_since(r.at) < STALE_AFTER)
    }

    /// The first other participant's running request, while the window
    /// has requests waiting: what they wait for.
    pub fn blocking(&self, now: Instant) -> Option<(&str, &Running)> {
        self.others
            .iter()
            .find(|(_, r)| now.saturating_duration_since(r.at) < STALE_AFTER)
            .map(|(p, r)| (p.as_str(), r))
    }
}

/// What the status line says a long request is doing: `Rebuilding
/// bracket`, `Checking the project`.
pub fn label(method: &str, part: Option<&str>) -> String {
    let what = part.unwrap_or("the project");
    match method {
        "rebuild" => format!("Rebuilding {what}"),
        "check" => format!("Checking {what}"),
        "doctor" => "Running doctor".to_owned(),
        "import" => "Importing".to_owned(),
        "interference" => format!("Checking interference in {what}"),
        "export" => format!("Exporting {what}"),
        "merge.status" => "Reading the merge".to_owned(),
        "diff" => "Comparing".to_owned(),
        "mesh.get" => format!("Meshing {what}"),
        "render" => format!("Rendering {what}"),
        m => m.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn line(done: u64, total: u64, item: &str) -> Value {
        json!({"token": "w1", "phase": "rebuild", "done": done, "total": total,
               "unit": "features", "item": item})
    }

    #[test]
    fn the_windows_own_request_shows_once_it_has_run_a_while() {
        let t0 = Instant::now();
        let mut p = Progress::default();
        let token = p.token();
        p.started(7, &token, "rebuild", label("rebuild", Some("bracket")), t0);
        assert!(p.shown(t0).is_none());
        assert_eq!(p.due(t0), Some(SHOW_AFTER));
        let later = t0 + SHOW_AFTER;
        p.line(7, &line(12, 40, "bracket/fillet1"), later);
        let m = p.shown(later).unwrap();
        assert_eq!(m.label, "Rebuilding bracket");
        assert_eq!(m.running.count(), "12 / 40 features");
        assert_eq!(m.running.fraction(), Some(0.3));
        assert_eq!(p.cancel(), Some("w1".to_owned()));
        assert!(p.shown(later).unwrap().cancelling);
        assert_eq!(p.answered(7).unwrap().ticket, 7);
        assert!(p.shown(later).is_none());
    }

    #[test]
    fn others_requests_come_from_the_topic_and_go_at_their_end() {
        let t0 = Instant::now();
        let mut p = Progress::default();
        let mut note = line(3, 8, "frame/pin");
        note["participant"] = json!("p-2");
        note["method"] = json!("check");
        note["unit"] = json!("parts");
        p.published("p-1", &note, t0);
        assert_eq!(p.of("p-2", t0).unwrap().short(), "check 3/8 parts");
        assert_eq!(p.blocking(t0).unwrap().0, "p-2");
        // Its own lines aren't the window's business twice.
        note["participant"] = json!("p-1");
        p.published("p-1", &note, t0);
        assert!(p.of("p-1", t0).is_none());
        // Quiet too long, it no longer shows.
        assert!(p.of("p-2", t0 + STALE_AFTER).is_none());
        note["participant"] = json!("p-2");
        note["end"] = json!(true);
        p.published("p-1", &note, t0);
        assert!(p.of("p-2", t0).is_none());
    }

    #[test]
    fn labels_name_what_runs() {
        assert_eq!(label("check", None), "Checking the project");
        assert_eq!(label("mesh.get", Some("frame")), "Meshing frame");
        assert_eq!(label("import", Some("x")), "Importing");
    }
}
