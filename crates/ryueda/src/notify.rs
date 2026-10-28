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
}

impl ryu_session::notify::Topics for Topic {
    const ALL: &'static [Topic] = &[Topic::Project, Topic::Participants, Topic::Activity];
    const PARTICIPANTS: Topic = Topic::Participants;
    const ACTIVITY: Topic = Topic::Activity;
}

/// What subscribers were last told: the project's revision and each
/// library's, by alias. `None` until the first telling.
#[derive(Debug, Default)]
pub struct Seen {
    pub revisions: Option<BTreeMap<String, String>>,
}
