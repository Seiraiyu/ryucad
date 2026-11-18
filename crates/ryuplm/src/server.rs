//! The request handlers. The daemon and the in-process CLI both call
//! [`handle_safely`], so they give the same answers.

use crate::catalogue::{MANIFEST, Manifest};
use crate::error::{Code, Error, Result};
use crate::id::ProjectId;
use crate::notify::Topic;
use crate::proto::{Answer, Request, VersionInfo};
use crate::session::Session;
use crate::source::write_atomic;
use crate::units::Unit;
use crate::{Plm, activity};
use ryu_session::activity::EntryKind;
use ryu_session::live::{self, Live};
use serde_json::{Value, json};
use std::path::Path;

/// The manifests of the projects a catalogue never shares a folder with.
const PROJECTS: [(&str, &str); 4] = [
    (MANIFEST, "a ryuplm catalogue"),
    ("ryucad.json", "a ryucad project"),
    ("ryueda.json", "a ryueda project"),
    ("ryudfm.json", "a ryudfm project"),
];

/// Creates a catalogue in `root`: the manifest with a fresh project ID,
/// and `.gitattributes` with ryuplm's lines (kept beside any it has).
pub fn init(root: &Path, unit: Option<Unit>) -> Result<Value> {
    for (file, what) in PROJECTS {
        if root.join(file).exists() {
            return Err(Error::new(
                Code::ProjectExists,
                format!("{} already holds {what}", root.display()),
            )
            .with("path", root.join(file)));
        }
    }
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes)
        .map_err(|e| Error::new(Code::Internal, format!("no random source: {e}")))?;
    let m = Manifest::new(ProjectId::from_bytes(bytes), unit.unwrap_or(Unit::Mm));
    let missing = crate::drivers::write_attributes(root)?;
    write_atomic(&root.join(MANIFEST), m.to_text())?;
    Ok(
        json!({"root": root, "project": m.project, "default_unit": m.default_unit, "gitattributes": missing}),
    )
}

fn info(session: &Session) -> Value {
    let c = session.catalogue();
    json!({
        "root": session.root(),
        "project": c.manifest.project,
        "default_unit": c.manifest.default_unit,
        "classes": c.manifest.classes.keys().collect::<Vec<_>>(),
        "items": c.items.len(),
        "revision": session.revision(),
        "version": VersionInfo::current(),
    })
}

/// [`answer`] behind a panic boundary. `slot` holds the session between
/// requests; it is opened on first use, and dropped after a panic so the
/// next request starts again from the files.
pub fn handle_safely(
    slot: &mut Option<Session>,
    live: Option<&mut Live<Plm>>,
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
        None => {
            let s = slot.insert(Session::open(root)?);
            s.opened_now();
            s
        }
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
    mut live: Option<&mut Live<Plm>>,
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
        (Some(_), true) => Some(session.files()),
        _ => None,
    };
    let result = dispatch(session, live.as_deref_mut(), me, req);
    if let (Some(l), Some(files_before), true) = (live.as_deref_mut(), files_before, result.is_ok())
    {
        let files = ryu_session::activity::diff(&files_before, &session.files());
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
    live: Option<&mut Live<Plm>>,
    me: Option<&str>,
    req: Request,
) -> Result<Answer> {
    use crate::{check, diff, items, release, settings};
    let v = match req {
        Request::ProjectInfo {} => info(session),
        Request::ItemCreate {
            fields,
            number,
            next,
            expected_revision,
        } => items::create(session, fields, number, next, expected_revision.as_deref())?,
        Request::ItemShow { item } => items::show(session, &item)?,
        Request::ItemList {
            number,
            class,
            text,
            limit,
        } => items::list(session, number, class, text, limit),
        Request::ItemEdit {
            item,
            fields,
            unset,
            expected_revision,
        } => items::edit(session, &item, fields, unset, expected_revision.as_deref())?,
        Request::ItemRenumber {
            item,
            number,
            next,
            expected_revision,
        } => items::renumber(session, &item, number, next, expected_revision.as_deref())?,
        Request::ItemDelete {
            item,
            expected_revision,
        } => items::delete(session, &item, expected_revision.as_deref())?,
        Request::ItemLink {
            item,
            product,
            project,
            part,
            expected_revision,
        } => crate::links::link(
            session,
            &item,
            &product,
            &project,
            &part,
            expected_revision.as_deref(),
        )?,
        Request::ItemUnlink {
            item,
            product,
            expected_revision,
        } => crate::links::unlink(session, &item, &product, expected_revision.as_deref())?,
        Request::BomAdd {
            item,
            fields,
            expected_revision,
        } => crate::bom::add(session, &item, fields, expected_revision.as_deref())?,
        Request::BomEdit {
            item,
            line,
            fields,
            unset,
            expected_revision,
        } => crate::bom::edit(
            session,
            &item,
            &line,
            fields,
            unset,
            expected_revision.as_deref(),
        )?,
        Request::BomRemove {
            item,
            line,
            expected_revision,
        } => crate::bom::remove(session, &item, &line, expected_revision.as_deref())?,
        Request::BomGet {
            item,
            rev,
            depth,
            within,
        } => crate::bom::show(session, &item, rev.as_deref(), depth, &within)?,
        Request::BomDiff {
            item,
            from,
            to,
            within,
        } => crate::bom_diff::diff(
            session,
            item.as_deref(),
            from.as_deref(),
            to.as_deref(),
            &within,
        )?,
        Request::BomWhereUsed {
            item,
            within,
            no_dependents,
            working,
        } => crate::where_used::answer(
            session,
            &crate::where_used::Ask {
                item: &item,
                within: &within,
                no_dependents,
                working,
            },
        )?,
        Request::ImportGuess {
            file,
            sheet,
            delimiter,
            encoding,
            save,
        } => crate::import::guess::answer(
            session,
            std::path::Path::new(&file),
            sheet,
            delimiter,
            encoding,
            save,
        )?,
        Request::ImportList {} => crate::import::list(session)?,
        Request::ImportCheck { name, file, limit } => {
            crate::import::preview::check(session, &name, std::path::Path::new(&file), limit)?
        }
        Request::ImportRun {
            name,
            file,
            dry_run,
            prefer,
            limit,
            expected_revision,
        } => {
            let prefer = prefer
                .as_deref()
                .map(crate::import::run::Prefer::parse)
                .transpose()?;
            crate::import::run::run(
                session,
                &name,
                std::path::Path::new(&file),
                dry_run,
                prefer,
                limit,
                expected_revision.as_deref(),
            )?
        }
        Request::SettingsShow {} => settings::show(session)?,
        Request::SettingsEdit {
            change,
            expected_revision,
        } => settings::edit(session, change, expected_revision.as_deref())?,
        Request::Check { within } => check::check(session, &within)?,
        Request::ReleasePropose {
            items,
            all,
            dry_run,
            note,
            expected_revision,
        } => release::propose(
            session,
            &items,
            all,
            dry_run,
            note,
            me,
            expected_revision.as_deref(),
        )?,
        Request::ReleaseObsolete {
            item,
            label,
            note,
            expected_revision,
        } => release::obsolete(
            session,
            &item,
            &label,
            note,
            me,
            expected_revision.as_deref(),
        )?,
        Request::ReleaseList { item } => release::list(session, item.as_deref())?,
        Request::ReleaseCheck {} => release::check(session)?,
        Request::ReleaseSync {} => release::sync(session)?,
        Request::Diff { from, to } => diff::diff(session, from.as_deref(), to.as_deref())?,
        Request::ActivityLog { limit, before } => {
            let entries = live.map_or_else(Vec::new, |l| l.log.page(limit.unwrap_or(50), before));
            json!({ "entries": entries })
        }
        Request::Undo { entry } => {
            let live = live.ok_or_else(|| live::no_live_session("undo"))?;
            let who = live.caller(me);
            activity::undo(live, session, &who, entry, false)?
        }
        Request::Redo { entry } => {
            let live = live.ok_or_else(|| live::no_live_session("redo"))?;
            let who = live.caller(me);
            activity::undo(live, session, &who, entry, true)?
        }
        Request::SessionHello {
            kind,
            name,
            participant,
        } => {
            let live = live.ok_or_else(|| live::no_live_session("session.hello"))?;
            let id = live.hello(kind, name.as_deref(), participant.as_deref())?;
            json!({"participant": id, "session": live.list()})
        }
        // With no daemon, nobody is attached.
        Request::ParticipantList {} => {
            live.map_or_else(|| json!({"participants": []}), |l| l.list())
        }
        Request::Subscribe { .. } => {
            return match live {
                None => Err(live::no_live_session("subscribe")),
                Some(_) => Err(Error::new(Code::Usage, "subscribe needs a connection")),
            };
        }
        Request::DaemonStatus {} | Request::DaemonShutdown {} => {
            return Err(Error::new(
                Code::Usage,
                "only a running daemon answers daemon requests",
            ));
        }
    };
    Ok(Answer::json(v))
}

/// Tells subscribers what moved since they were last told: the catalogue's
/// revision and its items'.
pub fn publish(live: &mut Live<Plm>, session: &Session) {
    let now = session.revisions();
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
pub fn settle(slot: &mut Option<Session>, live: &mut Live<Plm>, root: &Path) {
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
pub(crate) mod tests {
    use super::*;

    /// A fresh catalogue in a temporary folder, IDs seeded.
    pub(crate) fn catalogue() -> (tempfile::TempDir, std::path::PathBuf) {
        // SAFETY: tests that read the seed set the same value.
        unsafe { std::env::set_var(crate::ids::SEED_VAR, "test") };
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        init(&root, None).unwrap();
        (dir, root)
    }

    pub(crate) fn ask(root: &Path, req: Request) -> Result<Value> {
        handle_safely(&mut None, None, None, root, req).map(|a| a.result)
    }

    #[test]
    fn init_writes_the_manifest_and_attributes_once() {
        let (_d, root) = catalogue();
        let m = Manifest::read(&std::fs::read(root.join(MANIFEST)).unwrap()).unwrap();
        assert_eq!(m.default_unit, Unit::Mm);
        let attrs = std::fs::read_to_string(root.join(".gitattributes")).unwrap();
        assert!(
            attrs.contains("items/*/*.json merge=ryuplm diff=ryuplm text eol=lf"),
            "{attrs}"
        );
        assert_eq!(init(&root, None).unwrap_err().code, Code::ProjectExists);
        let other = tempfile::tempdir().unwrap();
        std::fs::write(other.path().join("ryucad.json"), "{}").unwrap();
        assert_eq!(
            init(other.path(), None).unwrap_err().code,
            Code::ProjectExists
        );
    }

    #[test]
    fn info_names_the_catalogue() {
        let (_d, root) = catalogue();
        let v = ask(&root, Request::ProjectInfo {}).unwrap();
        assert_eq!(v["items"], 0);
        assert_eq!(v["revision"].as_str().unwrap().len(), 32);
        assert_eq!(v["default_unit"], "mm");
    }
}
