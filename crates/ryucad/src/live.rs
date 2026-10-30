//! The live session: who is attached to a daemon, and what each of them is
//! looking at and doing: `ryu-session`'s, with what ryucad's windows show
//! besides a selection ([`View`]). It lives only in the daemon's memory;
//! the files never hold it, and a `--no-daemon` run has none.

use crate::Cad;
use crate::doc::id::Id;
use crate::entity::EntityRef;
use crate::error::Result;
use crate::view::Camera;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

pub use ryu_session::live::{
    AGENT_EXPIRY, DEFAULT_AGENT, Kind, STATUS_MAX, SelectMode, Snapshot, Style, no_live_session,
};

/// A group of highlighted entities with one style and label.
pub type Highlight = ryu_session::live::Highlight<EntityRef>;

/// What a participant says it is doing, and the entities it is working on.
pub type Status = ryu_session::live::Status<EntityRef>;

/// One human (an app window) or agent attached to the session.
pub type Participant = ryu_session::live::Participant<Cad>;

/// Everything the daemon knows about who is attached, and the
/// subscriptions it tells about changes.
pub type Live = ryu_session::live::Live<Cad>;

/// What a participant's window shows besides its selection, highlights
/// and status.
#[derive(Clone, Debug, Default)]
pub struct View {
    pub camera: Option<Camera>,
    /// The Git revision the participant compares the working files
    /// against, as it was given (`HEAD`, a branch, a commit); `None` when
    /// it isn't comparing.
    pub compare: Option<String>,
    /// The part or assembly the participant's window shows; `None` for
    /// the project's model ([`crate::assembly::shown_parts`]).
    pub focus: Option<Id>,
    /// What the participant's window hides: occurrences and solids, as
    /// [`crate::visibility::leaves`] writes them.
    pub hidden: BTreeSet<String>,
}

/// The project and part revisions subscribers were last told about, and
/// the merge in progress (its state's key; `None` when there is none).
#[derive(Debug, Default)]
pub struct Seen {
    pub project: Value,
    pub parts: BTreeMap<Id, String>,
    pub merge: Option<String>,
    /// Each part's release state and tags, while anyone watches.
    pub pdm: Option<BTreeMap<Id, Value>>,
    /// What `parts` and `pdm` were last worked out from, while anyone
    /// watches: the session's epoch and the release branch's refs. While
    /// neither moves, there is nothing new to tell.
    pub at: Option<(u64, Option<String>)>,
    /// The other worktrees participants compare with, by revision
    /// (`worktree:<path>`), and a digest of their files: a window hears
    /// `compare.changed` when one moves, which no file of this project
    /// says.
    pub compared: BTreeMap<String, String>,
}

/// Setting what ryucad's windows show.
pub trait CadLive {
    /// Sets what participant `id` compares the working files against, or
    /// (with `None`) stops it comparing.
    fn set_compare(&mut self, id: &str, rev: Option<String>) -> Result<()>;
    /// Sets what participant `id`'s window shows: a part or assembly, or
    /// (with `None`) the project's model.
    fn set_focus(&mut self, id: &str, part: Option<Id>) -> Result<()>;
    /// Sets what participant `id`'s window hides.
    fn set_hidden(&mut self, id: &str, hidden: BTreeSet<String>) -> Result<()>;
}

impl CadLive for Live {
    fn set_compare(&mut self, id: &str, rev: Option<String>) -> Result<()> {
        self.get_mut(id)?.view.compare = rev;
        Ok(())
    }

    fn set_focus(&mut self, id: &str, part: Option<Id>) -> Result<()> {
        self.get_mut(id)?.view.focus = part;
        Ok(())
    }

    fn set_hidden(&mut self, id: &str, hidden: BTreeSet<String>) -> Result<()> {
        self.get_mut(id)?.view.hidden = hidden;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Code;
    use serde_json::json;

    #[test]
    fn a_participant_reads_as_it_always_has() {
        let mut live = Live::new();
        let h = live.hello(Kind::Human, Some("Dana"), None).unwrap();
        let mut v = live.get(&h).unwrap().to_json();
        assert_eq!(
            v,
            json!({
                "id": h,
                "kind": "human",
                "name": "Dana",
                "status": null,
                "selection": [],
                "camera": null,
                "highlights": [],
                "compare": null,
                "focus": null,
                "hidden": [],
            })
        );
        live.set_focus(&h, Some(Id::parse("frame", "part").unwrap()))
            .unwrap();
        live.set_hidden(&h, BTreeSet::from(["frame/pin/s".to_owned()]))
            .unwrap();
        v = live.get(&h).unwrap().to_json();
        assert_eq!(v["focus"], "frame");
        assert_eq!(v["hidden"], json!(["frame/pin/s"]));
    }

    #[test]
    fn a_compare_goes_to_the_one_human_unless_one_is_named() {
        let mut live = Live::new();
        let a = live.hello(Kind::Agent, None, None).unwrap();
        // Nobody to show: the agent compares itself.
        assert_eq!(live.compare_target(None, Some(&a)).unwrap(), a);
        let h = live.hello(Kind::Human, Some("Dana"), None).unwrap();
        assert_eq!(live.compare_target(None, Some(&a)).unwrap(), h);
        assert_eq!(live.compare_target(Some(&a), Some(&a)).unwrap(), a);
        live.set_compare(&h, Some("HEAD~1".into())).unwrap();
        assert_eq!(
            live.get(&h).unwrap().to_json()["compare"],
            json!({"rev": "HEAD~1"})
        );
        live.set_compare(&h, None).unwrap();
        assert_eq!(live.get(&h).unwrap().to_json()["compare"], Value::Null);
        live.hello(Kind::Human, None, None).unwrap();
        let e = live.compare_target(None, Some(&a)).unwrap_err();
        assert_eq!(e.code, Code::AmbiguousParticipant);
        let e = live.compare_target(Some("p-ffffffff"), None).unwrap_err();
        assert_eq!(e.code, Code::NotFound);
    }
}
