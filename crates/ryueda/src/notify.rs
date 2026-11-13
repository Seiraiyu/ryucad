//! What subscribers hear: the topics a subscription asks for, and what
//! the daemon last told them, so it says only what moved.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// What a subscription can ask for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Topic {
    /// `project.changed`: the project's and its libraries' revisions.
    Project,
    /// `participant.changed`: who is attached and what they do.
    Participants,
    /// `activity.appended`: the activity log.
    Activity,
    /// `progress`: long requests' progress, any participant's.
    Progress,
    /// `merge.changed`: a merge in progress started, ended, or what it
    /// leaves to settle changed (design 518).
    Merge,
}

impl ryu_session::notify::Topics for Topic {
    const ALL: &'static [Topic] = &[
        Topic::Project,
        Topic::Participants,
        Topic::Activity,
        Topic::Progress,
        Topic::Merge,
    ];
    const PARTICIPANTS: Topic = Topic::Participants;
    const ACTIVITY: Topic = Topic::Activity;
    const PROGRESS: Option<Topic> = Some(Topic::Progress);
}

/// What subscribers were last told: the project's revision and each
/// library's (`null` for one that can't be used), by `project` and
/// `lib/<alias>`. `None` until the first telling.
#[derive(Debug, Default)]
pub struct Seen {
    pub revisions: Option<BTreeMap<String, serde_json::Value>>,
    /// The merge status's key last told ([`crate::validate::key`]).
    pub merge: Option<String>,
    /// Git's waiting files when the last status told failed: told again
    /// once they move.
    pub merge_failed: Option<String>,
}
