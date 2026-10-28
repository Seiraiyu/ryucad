//! The request handlers. The daemon and the in-process CLI both call
//! [`handle_safely`], so they give the same answers.

use crate::error::{Code, Error, Result};
use crate::fault;
use crate::id::{Id, ProjectId};
use crate::library::ObjKind;
use crate::notify::Topic;
use crate::project::{self, Alias, MANIFEST, Manifest, Project, ProjectKind, SELF};
use crate::proto::{Answer, Request, VersionInfo};
use crate::session::{Session, path_of};
use crate::source::write_atomic;
use crate::units::Unit;
use crate::{Eda, activity, libcmd};
use ryu_session::activity::EntryKind;
use ryu_session::live::{self, Live};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::path::Path;

/// Creates a project (or with `library`, a library project) in `root`.
/// Not a session request: there is no project to open until this has run.
pub fn init(root: &Path, unit: Option<Unit>, library: bool) -> Result<Value> {
    for (file, what) in [
        (MANIFEST, "a ryueda project"),
        ("ryucad.json", "a ryucad project"),
    ] {
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
    let kind = if library {
        ProjectKind::Library
    } else {
        ProjectKind::Design
    };
    let m = Manifest::new(ProjectId::from_bytes(bytes), unit.unwrap_or(Unit::Mm), kind);
    write_atomic(&root.join(MANIFEST), m.to_text())?;
    Ok(json!({
        "root": root,
        "project": m.project,
        "project_kind": m.project_kind,
        "default_unit": m.default_unit,
    }))
}

/// [`answer`] behind a panic boundary. `slot` holds the session between
/// requests; it is opened on first use, and dropped after a panic so the
/// next request starts again from the files.
pub fn handle_safely(
    slot: &mut Option<Session>,
    live: Option<&mut Live<Eda>>,
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
    mut live: Option<&mut Live<Eda>>,
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
    let result = dispatch(session, live.as_deref_mut(), me, req).map(Answer::json);
    if let (Some(l), Some(files_before), true) = (live.as_deref_mut(), files_before, result.is_ok())
    {
        // Only this project's own files: an alias's library project comes
        // into view when it is named, and undoing that must never delete
        // it. Writes never reach another project's files.
        let files: Vec<_> = ryu_session::activity::diff(&files_before, &session.snapshot())
            .into_iter()
            .filter(|f| !f.path.starts_with("../"))
            .collect();
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
    live: Option<&mut Live<Eda>>,
    me: Option<&str>,
    req: Request,
) -> Result<Value> {
    match req {
        Request::ProjectInfo {} => Ok(info(session.project())),
        Request::LibAdd {
            alias,
            path,
            expected_revision,
        } => lib_add(session, &alias, &path, expected_revision.as_deref()),
        Request::LibList {} => Ok(lib_list(session.project())),
        Request::SymbolCreate {
            name,
            id,
            units,
            ops,
            expected_revision,
        } => libcmd::symbol_create(
            session,
            &name,
            id.as_deref(),
            units,
            &ops,
            expected_revision.as_deref(),
        ),
        Request::SymbolEdit {
            symbol,
            name,
            units,
            ops,
            expected_revision,
        } => libcmd::symbol_edit(
            session,
            &symbol,
            name.as_deref(),
            units,
            &ops,
            expected_revision.as_deref(),
        ),
        Request::SymbolShow { symbol } => libcmd::symbol_show(session.project(), &symbol),
        Request::SymbolList { library } => {
            libcmd::symbol_list(session.project(), library.as_deref())
        }
        Request::FootprintShow { footprint } => {
            libcmd::footprint_show(session.project(), &footprint)
        }
        Request::FootprintList { library } => {
            libcmd::footprint_list(session.project(), library.as_deref())
        }
        Request::LibcompCreate {
            name,
            id,
            fields,
            expected_revision,
        } => libcmd::libcomp_create(
            session,
            &name,
            id.as_deref(),
            &fields,
            expected_revision.as_deref(),
        ),
        Request::LibcompEdit {
            libcomp,
            fields,
            expected_revision,
        } => libcmd::libcomp_edit(session, &libcomp, &fields, expected_revision.as_deref()),
        Request::LibcompShow { libcomp } => libcmd::libcomp_show(session.project(), &libcomp),
        Request::LibcompList { library } => {
            libcmd::libcomp_list(session.project(), library.as_deref())
        }
        Request::LibcompDelete {
            libcomp,
            expected_revision,
        } => libcmd::libcomp_delete(session, &libcomp, expected_revision.as_deref()),
        Request::SymbolDelete {
            symbol,
            expected_revision,
        } => libcmd::symbol_delete(session, &symbol, expected_revision.as_deref()),
        Request::ActivityLog { limit, before } => {
            let entries = live.map_or_else(Vec::new, |l| l.log.page(limit.unwrap_or(50), before));
            Ok(json!({ "entries": entries }))
        }
        Request::Undo { entry } => {
            let live = live.ok_or_else(|| live::no_live_session("undo"))?;
            let who = live.caller(me);
            activity::undo(live, session, &who, entry, false)
        }
        Request::Redo { entry } => {
            let live = live.ok_or_else(|| live::no_live_session("redo"))?;
            let who = live.caller(me);
            activity::undo(live, session, &who, entry, true)
        }
        Request::SessionHello {
            kind,
            name,
            participant,
        } => {
            let live = live.ok_or_else(|| live::no_live_session("session.hello"))?;
            let id = live.hello(kind, name.as_deref(), participant.as_deref())?;
            Ok(json!({"participant": id, "session": live.list()}))
        }
        // With no daemon, nobody is attached.
        Request::ParticipantList {} => {
            Ok(live.map_or_else(|| json!({"participants": []}), |l| l.list()))
        }
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

/// Refuses a write when the agent's view of `what` is out of date.
pub fn check_revision(what: &str, actual: &str, expected: Option<&str>) -> Result<()> {
    match expected {
        Some(e) if e != actual => Err(Error::new(
            Code::Conflict,
            format!("{what} changed since revision {e}; read it again and retry"),
        )
        .with("expected", e)
        .with("actual", actual)),
        _ => Ok(()),
    }
}

/// The project's revision and each usable library's, by alias (`project`
/// for the manifest's).
pub fn revisions(p: &Project) -> BTreeMap<String, Value> {
    let mut out: BTreeMap<String, Value> = p
        .libraries
        .iter()
        .map(|(alias, l)| {
            let rev = l.as_ref().map_or(Value::Null, |l| json!(l.revision));
            (format!("lib/{alias}"), rev)
        })
        .collect();
    out.insert("project".into(), json!(p.revision));
    for (id, b) in &p.conn.blocks {
        out.insert(format!("block/{id}"), json!(b.revision));
    }
    out
}

fn library_json(p: &Project, alias: &str) -> Value {
    let path = match alias {
        SELF => String::new(),
        a => p
            .manifest
            .libraries
            .iter()
            .find(|(k, _)| k.as_str() == a)
            .map(|(_, v)| v.path.clone())
            .unwrap_or_default(),
    };
    match p.library(alias) {
        Ok(l) => json!({"alias": alias, "path": path, "revision": l.revision}),
        Err(e) => json!({"alias": alias, "path": path, "error": e}),
    }
}

fn info(p: &Project) -> Value {
    let libraries: Map<String, Value> = p
        .libraries
        .keys()
        .map(|a| (a.clone(), library_json(p, a)))
        .collect();
    json!({
        "root": p.root,
        "project": p.manifest.project,
        "project_kind": p.manifest.project_kind,
        "default_unit": p.manifest.default_unit,
        "revision": p.revision,
        "libraries": libraries,
        "designs": p.conn.designs.keys().collect::<Vec<_>>(),
        "blocks": p
            .conn
            .blocks
            .iter()
            .map(|(id, b)| (id.to_string(), json!(b.revision)))
            .collect::<Map<String, Value>>(),
        "version": VersionInfo::current(),
    })
}

fn lib_list(p: &Project) -> Value {
    let faults = p.faults();
    let list: Vec<Value> = p
        .libraries
        .keys()
        .map(|alias| {
            let mut v = library_json(p, alias);
            if let Ok(l) = p.library(alias) {
                for kind in [ObjKind::Symbol, ObjKind::Footprint, ObjKind::Libcomp] {
                    v[format!("{}s", kind.as_str())] = json!(l.ids(kind).len());
                }
                v["models"] = json!(l.models.len());
                let prefix = crate::source::join(&l.base, crate::library::DIR);
                let mine: Vec<fault::Fault> = faults
                    .iter()
                    .filter(|f| {
                        f.error.details["path"]
                            .as_str()
                            .is_some_and(|p| p.starts_with(&format!("{prefix}/")))
                    })
                    .cloned()
                    .collect();
                v["faults"] = fault::list(&mine);
            }
            v
        })
        .collect();
    json!({ "libraries": list })
}

fn lib_add(
    session: &mut Session,
    alias: &str,
    path: &str,
    expected: Option<&str>,
) -> Result<Value> {
    let p = session.project();
    let id = Id::parse(alias, "library alias")?;
    if alias == SELF {
        return Err(Error::new(
            Code::InvalidParam,
            "self is the project's own library; name the library project otherwise",
        )
        .with("param", "alias"));
    }
    if p.manifest.project_kind == ProjectKind::Library {
        return Err(Error::new(
            Code::LibraryNested,
            "a library project can't name libraries of its own",
        ));
    }
    if p.manifest.libraries.contains_key(&id) {
        return Err(Error::new(
            Code::DuplicateId,
            format!("the project already names library {alias}"),
        )
        .with("alias", alias));
    }
    check_revision("the project", &p.revision, expected)?;
    let mut m = p.manifest.clone();
    m.libraries.insert(
        id.clone(),
        Alias {
            path: path.to_owned(),
        },
    );
    // Refused now, rather than written to fail on every load.
    project::resolve(session.root(), &m)
        .remove(&id)
        .expect("just added")?;
    write_atomic(&path_of(session.root(), MANIFEST), m.to_text())?;
    session.reload()?;
    let p = session.project();
    Ok(json!({
        "library": library_json(p, alias),
        "revision": p.revision,
    }))
}

/// Tells subscribers what moved since they were last told: the project's
/// revision and its libraries'.
pub fn publish(live: &mut Live<Eda>, session: &Session) {
    let now = revisions(session.project());
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
pub fn settle(slot: &mut Option<Session>, live: &mut Live<Eda>, root: &Path) {
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
