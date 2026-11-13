//! Requests on Git: diff and compare.

use super::versioned;
use crate::diff::diff;
use crate::error::{Code, Error, Result};
use crate::live::{self, CadLive, Live};
use crate::session::Session;
use serde_json::{Value, json};

/// `diff`.
pub(crate) fn diff_request(
    session: &mut Session,
    from: Option<String>,
    to: Option<String>,
    part: Option<String>,
    model: bool,
    positions: bool,
    all: bool,
) -> Result<Value> {
    let opts = crate::diff::Options {
        part,
        positions,
        all,
    };
    versioned(diff(
        session,
        from.as_deref().unwrap_or("HEAD"),
        to.as_deref().unwrap_or("working"),
        model,
        &opts,
    ))
}

/// `compare.get`.
pub(crate) fn compare_get(
    session: &mut Session,
    live: Option<&mut Live>,
    participant: Option<String>,
) -> Result<Value> {
    let target = match &live {
        Some(l) => l.target(participant.as_deref())?,
        None => None,
    };
    let Some(p) = target else {
        return Ok(json!({"participant": null, "compare": null}));
    };
    let (id, rev) = (p.id.clone(), p.view.compare.clone());
    // The commit the revision names now; none if it no longer
    // names one.
    let compare = rev.map(|rev| {
        if crate::session::worktree_path(&rev).is_some() {
            return json!({"rev": rev, "commit": null});
        }
        let commit = crate::git::repo(session.root())
            .and_then(|r| r.locate(&rev))
            .ok()
            .map(|(c, _)| c);
        json!({"rev": rev, "commit": commit})
    });
    Ok(json!({"participant": id, "compare": compare}))
}

/// `compare.set`.
pub(crate) fn compare_set(
    session: &mut Session,
    live: Option<&mut Live>,
    me: Option<&str>,
    rev: Option<String>,
    off: bool,
    participant: Option<String>,
) -> Result<Value> {
    let live = live.ok_or_else(|| live::no_live_session("compare.set"))?;
    let rev = match (rev, off) {
        (Some(r), false) => Some(r),
        (None, true) => None,
        _ => {
            return Err(Error::new(
                Code::Usage,
                "give a revision to compare with, or off, not both",
            ));
        }
    };
    if rev.as_deref() == Some("working") {
        return Err(Error::new(
            Code::InvalidParam,
            "a compare is against a Git revision: the working files are what it compares",
        )
        .with("rev", "working"));
    }
    let id = live.compare_target(participant.as_deref(), me)?;
    // Another worktree is named by its project folder, however the
    // caller named it.
    let rev = match rev.as_deref().map(|r| session.worktree_files(r)) {
        Some(found) => match found? {
            Some((w, ..)) => Some(format!("{}{}", crate::session::WORKTREE, w.root.display())),
            None => rev,
        },
        None => None,
    };
    // Refused unless the revision has the project; it is then
    // ready for the diff and meshes the window asks for next.
    let commit = match &rev {
        Some(r) => session.at(Some(r))?.0,
        None => None,
    };
    live.set_compare(&id, rev.clone())?;
    let compare = rev.map(|rev| json!({"rev": rev, "commit": commit}));
    Ok(json!({"participant": id, "compare": compare}))
}
