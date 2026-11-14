//! Toasts (design 199): what other participants change, the window's own
//! failures, the daemon lost and found again, and a long request of the
//! window's finishing while it isn't focused. At most [`SHOWN`] at once,
//! newest at the bottom; each goes after [`LIFE`] unless hovered, and
//! errors stay until closed. `ui.notify` says which show. What a change
//! is called is the product's ([`Words`]). Pure: a window draws them.
//! Moved from ryucad-app by ryueda 3.C.

use ryu_doc::error::Error;
use serde_json::Value;
use std::collections::BTreeSet;
use std::marker::PhantomData;
use std::time::{Duration, Instant};

/// How a product names an activity entry's change.
pub trait Words: 'static {
    /// What a toast calls the objects and the groups (`features`,
    /// `parts`) in `edited 3 features in bracket`.
    const OBJECTS: &'static str;
    const GROUPS: &'static str;
    /// Whether an entry makes no change toast (ryucad: annotations' own
    /// writes, which toast for their authors).
    fn quiet(entry: &Value) -> bool {
        let _ = entry;
        false
    }
    /// An activity method in words: `feature.edit` is `edited`.
    fn verb(method: &str, kind: &Value) -> String;
    /// The objects the entry wrote, as `(group, object)`.
    fn objects(entry: &Value) -> Vec<(String, String)>;
    /// The groups the entry's files are in, objects or not.
    fn groups(entry: &Value) -> BTreeSet<String>;
    /// Whether the entry can be undone.
    fn undoable(entry: &Value) -> bool;
}

/// The most toasts shown at once.
pub const SHOWN: usize = 4;
/// How long a toast stays, unhovered; errors stay until closed.
pub const LIFE: Duration = Duration::from_secs(6);
/// One participant's changes this close together are one toast.
pub const BATCH: Duration = Duration::from_secs(1);

/// The lost daemon's toast: an error, kept until the window is connected
/// again (#732).
pub const LOST: &str = "Lost the daemon: reconnecting";

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
    /// Choose and frame these objects (`<group>/<object>`).
    Show(Vec<String>),
    /// Undo these activity entries, newest first.
    Undo(Vec<u64>),
    /// Open the error card of the window's failure at this index.
    Details(usize),
    /// Choose this annotation and open the Annotations tile.
    Annotation(String),
}

impl Action {
    pub fn title(&self) -> &'static str {
        match self {
            Action::Show(_) => "Show",
            Action::Undo(_) => "Undo",
            Action::Details(_) => "Details",
            Action::Annotation(_) => "Show",
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
    objects: BTreeSet<String>,
    groups: BTreeSet<String>,
    entries: Vec<u64>,
    undoable: Vec<u64>,
}

/// Every toast the window holds.
pub struct Toasts<W: Words> {
    list: Vec<Toast>,
    next: u64,
    pub notify: Notify,
    words: PhantomData<fn() -> W>,
}

impl<W: Words> Default for Toasts<W> {
    fn default() -> Self {
        Toasts {
            list: Vec::new(),
            next: 0,
            notify: Notify::default(),
            words: PhantomData,
        }
    }
}

impl<W: Words> Clone for Toasts<W> {
    fn clone(&self) -> Self {
        Toasts {
            list: self.list.clone(),
            next: self.next,
            notify: self.notify,
            words: PhantomData,
        }
    }
}

impl<W: Words> std::fmt::Debug for Toasts<W> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Toasts")
            .field("list", &self.list)
            .field("next", &self.next)
            .field("notify", &self.notify)
            .finish()
    }
}

impl<W: Words> Toasts<W> {
    /// None yet, showing what `notify` says.
    pub fn with(notify: Notify) -> Self {
        Toasts {
            notify,
            ..Self::default()
        }
    }

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

    /// Someone's reply on, or closing of, one of this window's person's
    /// annotations.
    pub fn annotation(
        &mut self,
        who: &str,
        agent: bool,
        text: &str,
        detail: &str,
        id: &str,
        now: Instant,
    ) {
        let kind = Kind::Change {
            participant: who.to_owned(),
            agent,
        };
        if !self.wants(&kind) {
            return;
        }
        self.push(Toast {
            id: 0,
            kind,
            who: Some(who.to_owned()),
            text: text.to_owned(),
            detail: (!detail.is_empty()).then(|| detail.to_owned()),
            actions: vec![Action::Annotation(id.to_owned())],
            at: now,
            batch: None,
        });
    }

    /// An `activity.appended` entry: a change another participant made,
    /// batched with their changes of the last [`BATCH`]. The window's own,
    /// changes from outside the product and [`Words::quiet`] ones make
    /// none.
    pub fn activity(&mut self, entry: &Value, me: &str, now: Instant) {
        if W::quiet(entry) {
            return;
        }
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
        let verb = W::verb(entry["method"].as_str().unwrap_or_default(), &entry["kind"]);
        let undoable = W::undoable(entry);
        let touched = W::objects(entry);
        let in_groups = W::groups(entry);
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
                        objects: BTreeSet::new(),
                        groups: BTreeSet::new(),
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
        for (group, object) in touched {
            b.groups.insert(group.clone());
            b.objects.insert(format!("{group}/{object}"));
        }
        // A change of no object still names its group (#426: an agent's
        // `part create` read "agent created").
        b.groups.extend(in_groups);
        b.entries.push(seq);
        if undoable {
            b.undoable.insert(0, seq);
        }
        t.at = now;
        t.text = batch_text::<W>(b);
        t.actions.clear();
        if !b.objects.is_empty() {
            t.actions
                .push(Action::Show(b.objects.iter().cloned().collect()));
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

    /// The daemon lost: a failure, kept until [`Toasts::found`].
    pub fn lost(&mut self, now: Instant) {
        self.note(LOST, true, now);
    }

    /// Connected again: every lost toast goes, and a note says so.
    pub fn found(&mut self, now: Instant) {
        self.list
            .retain(|t| !(t.kind == Kind::Error && t.text == LOST));
        self.note("Connected to the daemon again", false, now);
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

/// `edited 3 features in bracket`, `moved pin in frame`, `made 4 changes
/// in 2 parts`.
fn batch_text<W: Words>(b: &Batch) -> String {
    let verb = match (b.verbs.len(), b.verbs.first()) {
        (1, Some(v)) => v.clone(),
        _ => "changed".into(),
    };
    let n = b.objects.len();
    let parts: Vec<&String> = b.groups.iter().collect();
    match (n, parts.as_slice()) {
        (0, _) => match (b.entries.len(), parts.as_slice()) {
            (1, []) => verb,
            (1, [part]) => format!("{verb} {part}"),
            (1, ps) => format!("{verb} {} {}", ps.len(), W::GROUPS),
            (k, _) => format!("made {k} changes"),
        },
        (1, [part]) => {
            let f = b.objects.first().expect("one");
            let name = f.rsplit('/').next().unwrap_or(f);
            format!("{verb} {name} in {part}")
        }
        (n, [part]) => format!("{verb} {n} {} in {part}", W::OBJECTS),
        (n, ps) => format!("{verb} {n} {} in {} {}", W::OBJECTS, ps.len(), W::GROUPS),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ryu_doc::error::{Code, Error};
    use serde_json::json;

    /// A toy product whose files are `blocks/<block>/<object>.json`.
    struct Toy;

    impl Words for Toy {
        const OBJECTS: &'static str = "parts";
        const GROUPS: &'static str = "blocks";
        fn verb(method: &str, _kind: &Value) -> String {
            if method == "part.edit" {
                "edited".into()
            } else {
                method.into()
            }
        }
        fn objects(entry: &Value) -> Vec<(String, String)> {
            entry["files"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|f| {
                    let rest = f["path"].as_str()?.strip_prefix("blocks/")?;
                    let (b, o) = rest.split_once('/')?;
                    Some((b.to_owned(), o.strip_suffix(".json")?.to_owned()))
                })
                .collect()
        }
        fn groups(entry: &Value) -> BTreeSet<String> {
            Self::objects(entry).into_iter().map(|(b, _)| b).collect()
        }
        fn undoable(entry: &Value) -> bool {
            entry["undone_by"].is_null()
        }
    }

    fn entry(seq: u64, files: &[&str]) -> Value {
        let files: Vec<Value> = files.iter().map(|p| json!({"path": p})).collect();
        json!({"seq": seq, "kind": "change", "method": "part.edit", "undone_by": null,
               "participant": {"id": "p-2", "kind": "agent", "name": "agent"}, "files": files})
    }

    #[test]
    fn a_product_s_changes_batch_in_its_own_words() {
        let t0 = Instant::now();
        let mut t = Toasts::<Toy>::default();
        t.activity(&entry(1, &["blocks/power/U1.json"]), "p-1", t0);
        t.activity(
            &entry(2, &["blocks/power/U2.json"]),
            "p-1",
            t0 + Duration::from_millis(100),
        );
        assert_eq!(t.shown().len(), 1);
        assert_eq!(t.shown()[0].text, "edited 2 parts in power");
        assert_eq!(
            t.shown()[0].actions,
            [
                Action::Show(vec!["power/U1".into(), "power/U2".into()]),
                Action::Undo(vec![2, 1]),
            ]
        );
        // The window's own make none.
        t.activity(&entry(3, &["blocks/io/J1.json"]), "p-2", t0);
        assert_eq!(t.shown().len(), 1);
    }

    #[test]
    fn errors_stay_notes_go_and_notify_chooses() {
        let t0 = Instant::now();
        let mut t = Toasts::<Toy>::with(Notify::Errors);
        t.note("done", false, t0);
        assert!(t.shown().is_empty());
        t.error(
            "Failed",
            &Error::new(Code::Daemon, "the daemon stopped answering"),
            0,
            t0,
        );
        t.expire(t0 + LIFE * 2);
        assert_eq!(t.shown().len(), 1);
        t.lost(t0);
        t.found(t0);
        assert!(t.shown().iter().all(|x| x.text != LOST));
    }
}
