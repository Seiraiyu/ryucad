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

/// What a participant's window lets its person do (design 374): nothing
/// (`view`, how every window opens), annotate, or write the model
/// directly (`edit`, never on by default). The daemon refuses a human's
/// writes their mode doesn't allow; an agent's mode is shown, never
/// enforced.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    #[default]
    View,
    Annotate,
    Edit,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::View => "view",
            Mode::Annotate => "annotate",
            Mode::Edit => "edit",
        }
    }
}

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
    /// What the participant's window cuts away: up to
    /// [`crate::section::MAX_PLANES`] planes in the shown model's frame.
    pub sections: Vec<crate::section::Plane>,
    /// The exploded view the participant's window shows, if any.
    pub explode: Option<crate::explode::Shown>,
    /// The motion the participant's window shows, if any. Never with an
    /// explode: showing one takes the other away.
    pub motion: Option<crate::playback::Shown>,
    /// What the participant's window lets its person do: View until they
    /// choose another, every time it connects.
    pub mode: Mode,
}

/// The project and part revisions subscribers were last told about, and
/// the merge in progress (its state's key; `None` when there is none).
#[derive(Debug, Default)]
pub struct Seen {
    pub project: Value,
    pub parts: BTreeMap<Id, String>,
    pub merge: Option<String>,
    /// While the last status announced failed, what it is asked again on
    /// ([`crate::validate::retry_stamp`]): a failure is said again once
    /// its inputs move, though its key reads the same.
    pub merge_failed: Option<String>,
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
    /// Each annotation as listings give it, while anyone watches: what
    /// `annotation.changed` was last worked out from.
    pub annotations: Option<BTreeMap<String, Value>>,
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
    /// Sets what participant `id`'s window cuts away.
    fn set_sections(&mut self, id: &str, sections: Vec<crate::section::Plane>) -> Result<()>;
    /// Sets the exploded view participant `id`'s window shows.
    fn set_explode(&mut self, id: &str, explode: Option<crate::explode::Shown>) -> Result<()>;
    /// Sets the motion participant `id`'s window shows. A motion shown
    /// takes away the explode, and an explode shown the motion.
    fn set_motion(&mut self, id: &str, motion: Option<crate::playback::Shown>) -> Result<()>;
    /// Sets what participant `id`'s window lets its person do.
    fn set_mode(&mut self, id: &str, mode: Mode) -> Result<()>;
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

    fn set_sections(&mut self, id: &str, sections: Vec<crate::section::Plane>) -> Result<()> {
        self.get_mut(id)?.view.sections = sections;
        Ok(())
    }

    fn set_explode(&mut self, id: &str, explode: Option<crate::explode::Shown>) -> Result<()> {
        let v = &mut self.get_mut(id)?.view;
        if explode.is_some() {
            v.motion = None;
        }
        v.explode = explode;
        Ok(())
    }

    fn set_motion(&mut self, id: &str, motion: Option<crate::playback::Shown>) -> Result<()> {
        let v = &mut self.get_mut(id)?.view;
        if motion.is_some() {
            v.explode = None;
        }
        v.motion = motion;
        Ok(())
    }

    fn set_mode(&mut self, id: &str, mode: Mode) -> Result<()> {
        self.get_mut(id)?.view.mode = mode;
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
                "sections": [],
                "explode": null,
                "motion": null,
                "mode": "view",
            })
        );
        live.set_focus(&h, Some(Id::parse("frame", "part").unwrap()))
            .unwrap();
        live.set_hidden(&h, BTreeSet::from(["frame/pin/s".to_owned()]))
            .unwrap();
        v = live.get(&h).unwrap().to_json();
        assert_eq!(v["focus"], "frame");
        assert_eq!(v["hidden"], json!(["frame/pin/s"]));
        let cut = crate::section::Plane::new([0.0, 0.0, 0.01], [0.0, 0.0, 1.0]).unwrap();
        live.set_sections(&h, vec![cut]).unwrap();
        assert_eq!(
            live.get(&h).unwrap().to_json()["sections"],
            json!([{"origin_m": [0.0, 0.0, 0.01], "normal": [0.0, 0.0, 1.0]}])
        );
    }

    #[test]
    fn a_window_shows_an_explode_or_a_motion_never_both() {
        let mut live = Live::new();
        let h = live.hello(Kind::Human, Some("Dana"), None).unwrap();
        let frame = Id::parse("frame", "part").unwrap();
        let explode = crate::explode::Shown {
            part: frame.clone(),
            explode: Id::parse("apart", "explode").unwrap(),
            fraction: 1.0,
        };
        let motion = crate::playback::Shown {
            part: frame,
            motion: Id::parse("swing", "motion").unwrap(),
            fraction: 0.25,
            playing: true,
        };
        live.set_explode(&h, Some(explode.clone())).unwrap();
        live.set_motion(&h, Some(motion.clone())).unwrap();
        let v = live.get(&h).unwrap().to_json();
        assert_eq!(v["explode"], Value::Null, "{v}");
        assert_eq!(
            v["motion"],
            json!({"part": "frame", "motion": "swing", "fraction": 0.25, "playing": true})
        );
        // Showing an explode takes the motion away.
        live.set_explode(&h, Some(explode)).unwrap();
        assert_eq!(live.get(&h).unwrap().view.motion, None);
        // Clearing one leaves the other.
        live.set_motion(&h, None).unwrap();
        assert!(live.get(&h).unwrap().view.explode.is_some());
        live.set_motion(&h, Some(motion)).unwrap();
        live.set_explode(&h, None).unwrap();
        assert!(live.get(&h).unwrap().view.motion.is_some());
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
