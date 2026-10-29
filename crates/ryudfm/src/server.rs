//! The request handlers. The daemon and the in-process CLI both call
//! [`handle_safely`], so they give the same answers.

use crate::error::{Code, Error, Result};
use crate::id::ProjectId;
use crate::notify::Topic;
use crate::project::{ATTRIBUTES, GITATTRIBUTES, MANIFEST, Manifest, Project};
use crate::proto::{Answer, Request, VersionInfo};
use crate::session::Session;
use crate::source::write_atomic;
use crate::units::Unit;
use crate::{Dfm, activity, boards};
use ryu_session::activity::EntryKind;
use ryu_session::live::{self, Live};
use serde_json::{Value, json};
use std::path::Path;

/// The manifests of the projects a ryudfm project never shares a folder
/// with: its own, ryucad's and ryueda's.
const PROJECTS: [(&str, &str); 3] = [
    (MANIFEST, "a ryudfm project"),
    ("ryucad.json", "a ryucad project"),
    ("ryueda.json", "a ryueda project"),
];

/// Creates a project in `root`: the manifest, and `.gitattributes` with
/// the lines boards need (kept beside any it has). Not a session request:
/// there is no project to open until this has run.
pub fn init(root: &Path, unit: Option<Unit>) -> Result<Value> {
    for (file, what) in PROJECTS {
        if root.join(file).exists() {
            return Err(Error::new(
                Code::ProjectExists,
                format!("{} already holds {what}", root.display()),
            )
            .with("path", root));
        }
    }
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes)
        .map_err(|e| Error::new(Code::Internal, format!("no random source: {e}")))?;
    let m = Manifest {
        project: ProjectId::from_bytes(bytes),
        default_unit: unit.unwrap_or(Unit::Mm),
    };
    let attributes = root.join(GITATTRIBUTES);
    let had = match std::fs::read_to_string(&attributes) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(Error::io(&attributes, &e)),
    };
    let missing: Vec<&str> = ATTRIBUTES
        .into_iter()
        .filter(|l| !had.lines().any(|h| h.trim() == *l))
        .collect();
    if !missing.is_empty() {
        let mut text = had.clone();
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        for l in &missing {
            text.push_str(l);
            text.push('\n');
        }
        write_atomic(&attributes, text)?;
    }
    write_atomic(&root.join(MANIFEST), m.to_text())?;
    Ok(json!({
        "root": root,
        "project": m.project,
        "default_unit": m.default_unit,
        "gitattributes": missing,
    }))
}

/// [`answer`] behind a panic boundary. `slot` holds the session between
/// requests; it is opened on first use, and dropped after a panic so the
/// next request starts again from the files.
pub fn handle_safely(
    slot: &mut Option<Session>,
    live: Option<&mut Live<Dfm>>,
    me: Option<&str>,
    root: &Path,
    req: Request,
) -> Result<Answer> {
    guarded(slot, root, |s| answer(s, live, me, req))
}

fn guarded<T>(
    slot: &mut Option<Session>,
    root: &Path,
    f: impl FnOnce(&mut Session) -> Result<T>,
) -> Result<T> {
    let session = match slot {
        Some(s) => s,
        None => slot.insert(Session::open(root)?),
    };
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(session))) {
        Ok(r) => r,
        Err(panic) => {
            *slot = None;
            let msg = panic
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| panic.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "unknown panic".to_owned());
            Err(Error::new(Code::Internal, format!("internal error: {msg}")).with("panic", msg))
        }
    }
}

/// Answers one request against `session`, after bringing it up to date
/// with the files. `live` is the daemon's live session (`None` in
/// process) and `me` the participant asking.
pub fn answer(
    session: &mut Session,
    mut live: Option<&mut Live<Dfm>>,
    me: Option<&str>,
    req: Request,
) -> Result<Answer> {
    let external = session.refresh_changes()?;
    let before = live.as_deref().map(Live::snapshot);
    if let Some(l) = live.as_deref_mut() {
        if !external.is_empty() {
            ryu_session::activity::record_external(l, external);
        }
        if let Some(me) = me {
            l.touch(me);
        }
    }
    // A daemon keeps what its writes change, for the log and for undo;
    // undo and redo log themselves.
    let (method, params) = (req.method(), req.params());
    let logs_itself = matches!(req, Request::Undo { .. } | Request::Redo { .. });
    let files_before = match (&live, req.mutates() && !logs_itself) {
        (Some(_), true) => Some(session.snapshot()),
        _ => None,
    };
    let result = dispatch(session, live.as_deref_mut(), me, req);
    if let (Some(l), Some(files_before), true) = (live.as_deref_mut(), files_before, result.is_ok())
    {
        let files = ryu_session::activity::diff(&files_before, &session.snapshot());
        if !files.is_empty() {
            let who = l.caller(me);
            ryu_session::activity::record(l, EntryKind::Change, &who, &method, params, None, files);
        }
    }
    if let (Some(l), Some(before)) = (live, before) {
        l.announce(&before);
        publish(l, session);
    }
    result
}

fn dispatch(
    session: &mut Session,
    live: Option<&mut Live<Dfm>>,
    me: Option<&str>,
    req: Request,
) -> Result<Answer> {
    let json = |v: Result<Value>| v.map(Answer::json);
    match req {
        Request::ProjectInfo {} => Ok(Answer::json(info(session.project()))),
        Request::Import {
            path,
            board,
            step,
            update,
            dry_run,
            expected_revision,
        } => {
            let path = std::path::PathBuf::from(&path);
            if !path.is_absolute() {
                return Err(Error::new(
                    Code::InvalidParam,
                    format!(
                        "{} isn't an absolute path; the daemon has no folder of yours",
                        path.display()
                    ),
                )
                .with("path", &path));
            }
            json(crate::import::import(
                session,
                &crate::import::Options {
                    path,
                    board,
                    step,
                    update,
                    dry_run,
                },
                expected_revision.as_deref(),
            ))
        }
        Request::BoardList {} => Ok(Answer::json(boards::list(session.project()))),
        Request::BoardShow { board } => json(boards::show(session.project(), &board)),
        Request::BoardEdit {
            board,
            name,
            step,
            expected_revision,
        } => json(boards::edit(
            session,
            &board,
            name.as_deref(),
            step.as_deref(),
            expected_revision.as_deref(),
        )),
        Request::BoardDelete {
            board,
            expected_revision,
        } => json(boards::delete(
            session,
            &board,
            expected_revision.as_deref(),
        )),
        Request::Inspect { board, args } => {
            let b = session.board(&board)?;
            json(crate::inspect::inspect(&b, Some(&board), &args))
        }
        Request::Render { board, args } => {
            let unit = session.project().manifest.default_unit;
            let b = session.board(&board)?;
            crate::render::render(&b, Some(&board), &args, unit)
        }
        Request::Check { board } => json(crate::check::project(session, board.as_deref())),
        Request::ActivityLog { limit, before } => {
            let entries = live.map_or_else(Vec::new, |l| l.log.page(limit.unwrap_or(50), before));
            Ok(Answer::json(json!({ "entries": entries })))
        }
        Request::Undo { entry } => {
            let live = live.ok_or_else(|| live::no_live_session("undo"))?;
            let who = live.caller(me);
            json(activity::undo(live, session, &who, entry, false))
        }
        Request::Redo { entry } => {
            let live = live.ok_or_else(|| live::no_live_session("redo"))?;
            let who = live.caller(me);
            json(activity::undo(live, session, &who, entry, true))
        }
        Request::SessionHello {
            kind,
            name,
            participant,
        } => {
            let live = live.ok_or_else(|| live::no_live_session("session.hello"))?;
            let id = live.hello(kind, name.as_deref(), participant.as_deref())?;
            Ok(Answer::json(
                json!({"participant": id, "session": live.list()}),
            ))
        }
        // With no daemon, nobody is attached.
        Request::ParticipantList {} => Ok(Answer::json(
            live.map_or_else(|| json!({"participants": []}), |l| l.list()),
        )),
        Request::Subscribe { .. } => match live {
            None => Err(live::no_live_session("subscribe")),
            Some(_) => Err(Error::new(Code::Usage, "subscribe needs a connection")),
        },
        Request::DaemonStatus {} | Request::DaemonShutdown {} => Err(Error::new(
            Code::Usage,
            "only a running daemon answers daemon requests",
        )),
    }
}

fn info(p: &Project) -> Value {
    json!({
        "root": p.root,
        "project": p.manifest.project,
        "default_unit": p.manifest.default_unit,
        "revision": p.revision,
        "boards": p
            .boards
            .iter()
            .map(|(id, b)| (id.to_string(), json!(b.revision)))
            .collect::<serde_json::Map<String, Value>>(),
        "version": VersionInfo::current(),
    })
}

/// Tells subscribers what moved since they were last told: the project's
/// revision and its boards'.
pub fn publish(live: &mut Live<Dfm>, session: &Session) {
    let now = session.project().revisions();
    if live.hub.is_watched()
        && let Some(seen) = &live.seen.revisions
        && seen != &now
    {
        let changed: Vec<&String> = now
            .keys()
            .chain(seen.keys())
            .filter(|k| now.get(*k) != seen.get(*k))
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        live.hub.send(
            Topic::Project,
            "project.changed",
            &json!({"revisions": now, "changed": changed}),
        );
    }
    live.seen.revisions = Some(now);
}

/// Brings the session and what subscribers know up to date with the
/// files: before a subscription, and while anyone watches.
pub fn settle(slot: &mut Option<Session>, live: &mut Live<Dfm>, root: &Path) {
    let _ = guarded(slot, root, |s| {
        let before = live.snapshot();
        let external = s.refresh_changes()?;
        if !external.is_empty() {
            ryu_session::activity::record_external(live, external);
        }
        live.announce(&before);
        publish(live, s);
        Ok(())
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_writes_the_manifest_and_attributes_once() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join(GITATTRIBUTES), "*.png binary").unwrap();
        let v = init(root, None).unwrap();
        assert_eq!(v["default_unit"], json!("mm"));
        assert_eq!(v["gitattributes"], json!(ATTRIBUTES));
        assert_eq!(
            std::fs::read_to_string(root.join(GITATTRIBUTES)).unwrap(),
            "*.png binary\n*.json text eol=lf\nboards/*/input.* binary\nboards/*/input/** binary\n"
        );
        let m = Manifest::read(&std::fs::read(root.join(MANIFEST)).unwrap()).unwrap();
        assert_eq!(m.default_unit, Unit::Mm);
        assert_eq!(init(root, None).unwrap_err().code, Code::ProjectExists);
        let other = tempfile::tempdir().unwrap();
        std::fs::write(other.path().join("ryueda.json"), "{}").unwrap();
        assert_eq!(
            init(other.path(), None).unwrap_err().code,
            Code::ProjectExists
        );
    }
}
