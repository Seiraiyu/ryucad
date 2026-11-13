//! Requests on PDM: properties and releases.

use super::session::participant_name;
use crate::error::Result;
use crate::live::Live;
use crate::session::Session;
use serde_json::Value;

/// `props.get`.
pub(crate) fn props_get(session: &mut Session, part: String, rev: Option<String>) -> Result<Value> {
    let (_, session) = session.at(rev.as_deref())?;
    crate::pdm::get(session, &part)
}

/// `release.propose`.
pub(crate) fn release_propose(
    session: &mut Session,
    live: Option<&mut Live>,
    me: Option<&str>,
    part: String,
    note: Option<String>,
    expected_revision: Option<String>,
    dry_run: bool,
) -> Result<Value> {
    let who = participant_name(live, me);
    crate::release::propose(
        session,
        &who,
        &part,
        note,
        expected_revision.as_deref(),
        dry_run,
    )
}

/// `release.obsolete`.
pub(crate) fn release_obsolete(
    session: &mut Session,
    live: Option<&mut Live>,
    me: Option<&str>,
    part: String,
    label: String,
    note: Option<String>,
    expected_revision: Option<String>,
) -> Result<Value> {
    let who = participant_name(live, me);
    crate::release::obsolete(
        session,
        &who,
        &part,
        &label,
        note,
        expected_revision.as_deref(),
    )
}
