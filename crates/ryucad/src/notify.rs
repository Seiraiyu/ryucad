//! Notifications to subscribed connections: `ryu-session`'s, over
//! ryucad's topics. Each subscriber has its own queue, drained by its
//! connection's writer thread, so a slow client never holds up the daemon.
//! Notifications carry state, not deltas: a client that falls too far
//! behind gets `resync` instead and fetches the state again.

use crate::error::Result;
use serde::{Deserialize, Serialize};

pub use ryu_session::notify::{QUEUE_LIMIT, line};

/// What a subscription can ask for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Topic {
    /// `model.changed`: rebuilt parts.
    Model,
    /// `project.changed`: the manifest, the parts, settings.
    Project,
    /// `participant.changed`: who is attached and what they do.
    Participants,
    /// `activity.appended`: the activity log.
    Activity,
    /// `merge.changed`: a merge in progress started, ended, or what it
    /// leaves to settle changed.
    Merge,
    /// `pdm.changed`: parts' release states or tags changed.
    Pdm,
    /// `progress`: any participant's long request, as it goes.
    Progress,
}

impl Topic {
    pub const ALL: [Topic; 7] = [
        Topic::Model,
        Topic::Project,
        Topic::Participants,
        Topic::Activity,
        Topic::Merge,
        Topic::Pdm,
        Topic::Progress,
    ];
}

impl ryu_session::notify::Topics for Topic {
    const ALL: &'static [Topic] = &Topic::ALL;
    const PARTICIPANTS: Topic = Topic::Participants;
    const ACTIVITY: Topic = Topic::Activity;
    const PROGRESS: Option<Topic> = Some(Topic::Progress);
}

/// A subscribed connection's queue.
pub type Subscriber = ryu_session::notify::Subscriber<Topic>;

/// Every subscription of a daemon.
pub type Hub = ryu_session::notify::Hub<Topic>;

/// Parses `subscribe`'s topic names.
pub fn topics(names: &[String]) -> Result<Vec<Topic>> {
    ryu_session::notify::topics(names)
}
