//! Requests on the live session: its participants.

use crate::live::{self, Live};

/// The name of the participant asking: the live session's, else the one
/// the caller gave (the CLI's `--as`), else the default agent's.
pub(crate) fn participant_name(live: Option<&mut Live>, me: Option<&str>) -> String {
    match live {
        Some(l) => {
            let id = l.caller(me);
            l.get(&id)
                .map_or_else(|_| live::DEFAULT_AGENT.to_owned(), |p| p.name.clone())
        }
        None => me.unwrap_or(live::DEFAULT_AGENT).to_owned(),
    }
}
