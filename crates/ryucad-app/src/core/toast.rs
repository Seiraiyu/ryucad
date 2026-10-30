//! Toasts (design 199): what other participants change, the window's own
//! failures, the daemon lost and found again, and a long request of the
//! window's finishing while it isn't focused. At most [`SHOWN`] at once,
//! newest at the bottom; each goes after [`LIFE`] unless hovered, and
//! errors stay until closed. `ui.notify` says which show. Pure: `ui::
//! feedback` draws them.

use ryucad::error::Error;
use serde_json::Value;
use std::collections::BTreeSet;
use std::time::{Duration, Instant};

/// The most toasts shown at once.
pub const SHOWN: usize = 4;
/// How long a toast stays, unhovered; errors stay until closed.
pub const LIFE: Duration = Duration::from_secs(6);
/// One participant's changes this close together are one toast.
pub const BATCH: Duration = Duration::from_secs(1);

/// `ui.notify`: which toasts show.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Notify {
    #[default]
    All,
    Errors,
    Off,
}

impl Notify {
    /// The preference's value; anything else is the default.
    pub fn parse(v: &Value) -> Notify {
        match v.as_str() {
            Some("errors") => Notify::Errors,
            Some("off") => Notify::Off,
            _ => Notify::All,
        }
    }
}

/// What a toast says it is about, for its icon and its stripe.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Another participant's change: an agent's (robot) or a person's.
    Change { participant: String, agent: bool },
    /// A failure: it stays until closed.
    Error,
    /// The daemon lost (a failure) or found again, or a request done.
    Note,
}

/// What a toast's links do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// Choose and frame these features (`<part>/<feature>`).
    Show(Vec<String>),
    /// Undo these activity entries, newest first.
    Undo(Vec<u64>),
    /// Open the error card of the window's failure at this index.
    Details(usize),
}

impl Action {
    pub fn title(&self) -> &'static str {
        match self {
            Action::Show(_) => "Show",
            Action::Undo(_) => "Undo",
            Action::Details(_) => "Details",
        }
    }
}

/// One toast.
#[derive(Clone, Debug, PartialEq)]
pub struct Toast {
    pub id: u64,
    pub kind: Kind,
    /// Who, in bold, before `text`; `None` for a title alone.
    pub who: Option<String>,
    /// The line: `edited 3 features in bracket`, or a title.
    pub text: String,
    /// A second, weaker line: an error's message.
    pub detail: Option<String>,
    pub actions: Vec<Action>,
    at: Instant,
    /// A batch of changes, still taking more for [`BATCH`].
    batch: Option<Batch>,
}

#[derive(Clone, Debug, PartialEq)]
struct Batch {
    started: Instant,
    verbs: BTreeSet<String>,
    features: BTreeSet<String>,
    parts: BTreeSet<String>,
    entries: Vec<u64>,
    undoable: Vec<u64>,
}

/// Every toast the window holds.
#[derive(Clone, Debug, Default)]
pub struct Toasts {
    list: Vec<Toast>,
    next: u64,
    pub notify: Notify,
}

impl Toasts {
    /// The toasts to draw, oldest first.
    pub fn shown(&self) -> &[Toast] {
        let n = self.list.len();
        &self.list[n.saturating_sub(SHOWN)..]
    }

    fn push(&mut self, mut t: Toast) {
        self.next += 1;
        t.id = self.next;
        self.list.push(t);
    }

    fn wants(&self, kind: &Kind) -> bool {
        match self.notify {
            Notify::All => true,
            Notify::Errors => *kind == Kind::Error,
            Notify::Off => false,
        }
    }

    /// An `activity.appended` entry: a change another participant made,
    /// batched with their changes of the last [`BATCH`]. The window's own
    /// and changes from outside ryucad make none.
    pub fn activity(&mut self, entry: &Value, me: &str, now: Instant) {
        let p = &entry["participant"];
        let (Some(id), Some(name)) = (p["id"].as_str(), p["name"].as_str()) else {
            return;
        };
        if id == me {
            return;
        }
        let kind = Kind::Change {
            participant: id.to_owned(),
            agent: p["kind"] == "agent",
        };
        if !self.wants(&kind) {
            return;
        }
        let seq = entry["seq"].as_u64().unwrap_or_default();
        let verb = verb(entry["method"].as_str().unwrap_or_default(), &entry["kind"]);
        let undoable = crate::core::activity::can_undo(entry);
        let touched = features_of(entry);
        let open = self.list.iter().rposition(|t| {
            t.kind == kind
                && t.batch
                    .as_ref()
                    .is_some_and(|b| now.saturating_duration_since(b.started) < BATCH)
        });
        let i = match open {
            Some(i) => i,
            None => {
                self.push(Toast {
                    id: 0,
                    kind,
                    who: Some(name.to_owned()),
                    text: String::new(),
                    detail: None,
                    actions: Vec::new(),
                    at: now,
                    batch: Some(Batch {
                        started: now,
                        verbs: BTreeSet::new(),
                        features: BTreeSet::new(),
                        parts: BTreeSet::new(),
                        entries: Vec::new(),
                        undoable: Vec::new(),
                    }),
                });
                self.list.len() - 1
            }
        };
        let t = &mut self.list[i];
        let b = t.batch.as_mut().expect("a batch");
        b.verbs.insert(verb);
        for (part, feature) in touched {
            b.parts.insert(part.clone());
            b.features.insert(format!("{part}/{feature}"));
        }
        b.entries.push(seq);
        if undoable {
            b.undoable.insert(0, seq);
        }
        t.at = now;
        t.text = batch_text(b);
        t.actions.clear();
        if !b.features.is_empty() {
            t.actions
                .push(Action::Show(b.features.iter().cloned().collect()));
        }
        if !b.undoable.is_empty() {
            t.actions.push(Action::Undo(b.undoable.clone()));
        }
    }

    /// The window's failure `index` (in its list of failures), `e`: its
    /// title and message, with Details for its card. Stays until closed.
    pub fn error(&mut self, title: &str, e: &Error, index: usize, now: Instant) {
        if !self.wants(&Kind::Error) {
            return;
        }
        self.push(Toast {
            id: 0,
            kind: Kind::Error,
            who: None,
            text: title.to_owned(),
            detail: Some(e.message.clone()),
            actions: vec![Action::Details(index)],
            at: now,
            batch: None,
        });
    }

    /// A note: the daemon lost (an error, kept until closed) or found
    /// again, or a long request done while the window wasn't focused.
    pub fn note(&mut self, text: &str, failure: bool, now: Instant) {
        let kind = if failure { Kind::Error } else { Kind::Note };
        if !self.wants(&kind) {
            return;
        }
        self.push(Toast {
            id: 0,
            kind,
            who: None,
            text: text.to_owned(),
            detail: None,
            actions: Vec::new(),
            at: now,
            batch: None,
        });
    }

    /// The pointer is over toast `id`: it stays a [`LIFE`] from now.
    pub fn hover(&mut self, id: u64, now: Instant) {
        if let Some(t) = self.list.iter_mut().find(|t| t.id == id) {
            t.at = now;
        }
    }

    pub fn close(&mut self, id: u64) {
        self.list.retain(|t| t.id != id);
    }

    /// Drops what has had its [`LIFE`] (never an error), and what no
    /// longer fits.
    pub fn expire(&mut self, now: Instant) {
        self.list
            .retain(|t| t.kind == Kind::Error || now.saturating_duration_since(t.at) < LIFE);
        if self.list.len() > 16 {
            let extra = self.list.len() - 16;
            self.list.drain(..extra);
        }
    }

    /// When the next toast goes: to repaint then.
    pub fn due(&self, now: Instant) -> Option<Duration> {
        self.shown()
            .iter()
            .filter(|t| t.kind != Kind::Error)
            .map(|t| LIFE.saturating_sub(now.saturating_duration_since(t.at)))
            .min()
    }
}

/// An activity method in words: `feature.edit` is `edited`.
pub fn verb(method: &str, kind: &Value) -> String {
    match kind.as_str() {
        Some("undo") => return "undid".into(),
        Some("redo") => return "redid".into(),
        _ => {}
    }
    match method {
        "feature.add" => "added",
        "feature.edit" => "edited",
        "feature.delete" => "deleted",
        "feature.move" => "reordered",
        "part.create" => "created",
        "part.delete" => "deleted",
        "param.set" | "param.delete" | "param.rename" => "set parameters",
        "sketch.edit" => "edited",
        "assembly.move" => "moved",
        "import" => "imported",
        "merge.resolve" => "resolved",
        "appearance.set" | "appearance.clear" => "coloured",
        "view.save" | "view.delete" | "view.rename" => "saved views",
        "doctor.repair" => "repaired",
        "config.set" => "changed settings",
        m => m,
    }
    .to_owned()
}

/// The features an entry's files are: `parts/<part>/features/<f>.json`,
/// as `(part, feature)`.
pub fn features_of(entry: &Value) -> Vec<(String, String)> {
    entry["files"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|f| {
            let path = f["path"].as_str()?;
            let rest = path.strip_prefix("parts/")?;
            let (part, file) = rest.split_once("/features/")?;
            let feature = file.strip_suffix(".json")?;
            Some((part.to_owned(), feature.to_owned()))
        })
        .collect()
}

/// `edited 3 features in bracket`, `moved pin in frame`, `made 4 changes
/// in 2 parts`.
fn batch_text(b: &Batch) -> String {
    let verb = match (b.verbs.len(), b.verbs.first()) {
        (1, Some(v)) => v.clone(),
        _ => "changed".into(),
    };
    let n = b.features.len();
    let parts: Vec<&String> = b.parts.iter().collect();
    match (n, parts.as_slice()) {
        (0, _) => match b.entries.len() {
            1 => verb,
            k => format!("made {k} changes"),
        },
        (1, [part]) => {
            let f = b.features.first().expect("one");
            let name = f.rsplit('/').next().unwrap_or(f);
            format!("{verb} {name} in {part}")
        }
        (n, [part]) => format!("{verb} {n} features in {part}"),
        (n, ps) => format!("{verb} {n} features in {} parts", ps.len()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ryucad::error::Code;
    use serde_json::json;

    fn entry(seq: u64, who: &str, method: &str, files: &[&str]) -> Value {
        let files: Vec<Value> = files.iter().map(|p| json!({"path": p})).collect();
        json!({"seq": seq, "kind": "change", "method": method, "undone_by": null,
               "participant": {"id": who, "kind": "agent", "name": "agent"}, "files": files})
    }

    #[test]
    fn an_agents_changes_within_a_second_are_one_toast_with_show_and_undo() {
        let t0 = Instant::now();
        let mut t = Toasts::default();
        for (i, f) in ["f1", "c1", "e1"].iter().enumerate() {
            let e = entry(
                i as u64 + 1,
                "p-2",
                "feature.edit",
                &[&format!("parts/bracket/features/{f}.json")],
            );
            t.activity(&e, "p-1", t0 + Duration::from_millis(100 * i as u64));
        }
        let shown = t.shown();
        assert_eq!(shown.len(), 1);
        assert_eq!(shown[0].who.as_deref(), Some("agent"));
        assert_eq!(shown[0].text, "edited 3 features in bracket");
        assert_eq!(
            shown[0].actions,
            [
                Action::Show(vec![
                    "bracket/c1".into(),
                    "bracket/e1".into(),
                    "bracket/f1".into()
                ]),
                Action::Undo(vec![3, 2, 1]),
            ]
        );
        // A second later, a new toast.
        let e = entry(
            4,
            "p-2",
            "assembly.move",
            &["parts/frame/features/pin.json"],
        );
        t.activity(&e, "p-1", t0 + Duration::from_secs(2));
        assert_eq!(t.shown()[1].text, "moved pin in frame");
    }

    #[test]
    fn the_windows_own_changes_and_outside_ones_make_no_toast() {
        let mut t = Toasts::default();
        t.activity(&entry(1, "p-1", "feature.edit", &[]), "p-1", Instant::now());
        let outside = json!({"seq": 2, "kind": "external", "files": []});
        t.activity(&outside, "p-1", Instant::now());
        assert!(t.shown().is_empty());
    }

    #[test]
    fn toasts_go_after_their_life_unless_hovered_and_errors_stay() {
        let t0 = Instant::now();
        let mut t = Toasts::default();
        t.note("Connected to the daemon again", false, t0);
        let e = Error::new(Code::UnitMismatch, "length takes a length, not an angle");
        t.error("Unit mismatch", &e, 0, t0);
        let id = t.shown()[0].id;
        t.hover(id, t0 + Duration::from_secs(5));
        t.expire(t0 + LIFE);
        assert_eq!(t.shown().len(), 2);
        t.expire(t0 + Duration::from_secs(5) + LIFE);
        assert_eq!(t.shown().len(), 1);
        assert_eq!(t.shown()[0].kind, Kind::Error);
        assert_eq!(t.due(t0), None);
        let id = t.shown()[0].id;
        t.close(id);
        assert!(t.shown().is_empty());
    }

    #[test]
    fn at_most_four_show_and_notify_chooses_which() {
        let t0 = Instant::now();
        let mut t = Toasts::default();
        for i in 0..6 {
            t.note(&format!("n{i}"), false, t0);
        }
        let texts: Vec<&str> = t.shown().iter().map(|t| t.text.as_str()).collect();
        assert_eq!(texts, ["n2", "n3", "n4", "n5"]);
        let mut t = Toasts {
            notify: Notify::Errors,
            ..Default::default()
        };
        t.activity(&entry(1, "p-2", "feature.edit", &[]), "p-1", t0);
        t.note("Connected", false, t0);
        t.note("Lost the daemon", true, t0);
        assert_eq!(t.shown().len(), 1);
        t.notify = Notify::Off;
        t.note("Lost the daemon", true, t0);
        assert_eq!(t.shown().len(), 1);
        assert_eq!(Notify::parse(&json!("off")), Notify::Off);
        assert_eq!(Notify::parse(&json!("loud")), Notify::All);
    }
}
