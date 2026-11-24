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
use ryu_session::live::{self, Live, SelectMode};
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
        "schema": crate::OUTPUT_SCHEMA,
        "root": session.root(),
        "project": c.manifest.project,
        "default_unit": c.manifest.default_unit,
        "classes": c.manifest.classes.keys().collect::<Vec<_>>(),
        "items": c.items.len(),
        "documents": c.documents.len(),
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
            // What a change from outside took away leaves every
            // selection, highlight and status before the request reads
            // them (design 938).
            l.prune(|t| crate::refs::stands(session, t));
        }
        if let Some(me) = me {
            l.touch(me);
        }
    }
    // A daemon keeps what its writes change, for the log and for undo;
    // undo and redo log themselves.
    let (method, params) = (req.method(), req.params());
    let writes = req.mutates();
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
        // References a write took away drop out of every selection,
        // highlight and status.
        if writes && result.is_ok() {
            l.prune(|t| crate::refs::stands(session, t));
        }
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
            state,
            make_buy,
            faults,
            source,
            counts,
            sort,
            offset,
            limit,
        } => items::list(
            session,
            &items::ListAsk {
                number,
                class,
                text,
                state,
                make_buy,
                faults,
                source,
                counts,
                sort,
                offset,
                limit,
            },
        )?,
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
        Request::DocumentAdd {
            file,
            number,
            next,
            fields,
            expected_revision,
        } => crate::documents::add(
            session,
            Path::new(&file),
            number,
            next,
            fields,
            expected_revision.as_deref(),
        )?,
        Request::DocumentShow { document } => crate::documents::show(session, &document)?,
        Request::DocumentList {
            doc_type,
            number,
            text,
            limit,
        } => crate::documents::list(session, doc_type, number, text, limit),
        Request::DocumentEdit {
            document,
            fields,
            unset,
            expected_revision,
        } => crate::documents::edit(
            session,
            &document,
            fields,
            unset,
            expected_revision.as_deref(),
        )?,
        Request::DocumentRenumber {
            document,
            number,
            next,
            expected_revision,
        } => crate::documents::renumber(
            session,
            &document,
            number,
            next,
            expected_revision.as_deref(),
        )?,
        Request::DocumentReplace {
            document,
            file,
            media_type,
            expected_revision,
        } => crate::documents::replace(
            session,
            &document,
            Path::new(&file),
            media_type,
            expected_revision.as_deref(),
        )?,
        Request::DocumentRemove {
            document,
            detach,
            expected_revision,
        } => crate::documents::remove(session, &document, detach, expected_revision.as_deref())?,
        Request::DocumentGet { document, rev, out } => {
            crate::documents::get(session, &document, rev.as_deref(), Path::new(&out))?
        }
        Request::DocumentAttach {
            document,
            items,
            expected_revision,
        } => crate::documents::attach(
            session,
            &document,
            &items,
            true,
            expected_revision.as_deref(),
        )?,
        Request::DocumentDetach {
            document,
            items,
            expected_revision,
        } => crate::documents::attach(
            session,
            &document,
            &items,
            false,
            expected_revision.as_deref(),
        )?,
        Request::DocumentWhereUsed {
            document,
            within,
            no_dependents,
            working,
        } => crate::documents::where_used(
            session,
            &crate::documents::Ask {
                document: &document,
                within: &within,
                no_dependents,
                working,
            },
        )?,
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
        Request::RepsList { rev, within } => crate::reach::list(session, rev.as_deref(), &within)?,
        Request::Index { out, rev, within } => {
            crate::index::write(session, &out, rev.as_deref(), &within)?
        }
        Request::ImportGuess {
            file,
            sheet,
            delimiter,
            encoding,
            records,
            save,
        } => crate::import::guess::answer(
            session,
            std::path::Path::new(&file),
            sheet,
            delimiter,
            encoding,
            records,
            save,
        )?,
        Request::ImportList {} => crate::import::list(session)?,
        Request::NumberLocal { next, class, item } => {
            crate::numbers::local(session, next.as_deref(), class.as_deref(), item.as_deref())?
        }
        Request::ImportCheck { name, file, limit } => {
            crate::import::preview::check(session, &name, std::path::Path::new(&file), limit)?
        }
        Request::ImportRun {
            name,
            file,
            dry_run,
            prefer,
            limit,
            numbers,
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
                &numbers,
                expected_revision.as_deref(),
            )?
        }
        Request::SettingsShow {} => settings::show(session)?,
        Request::SettingsEdit {
            change,
            expected_revision,
        } => settings::edit(session, change, expected_revision.as_deref())?,
        Request::Check { within, against } => check::check(session, &within, against.as_deref())?,
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
        Request::Resolve { refs, rev } => crate::refs::resolve(session, &refs, rev.as_deref())?,
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
        Request::SelectionGet { participant } => {
            let live = live.ok_or_else(|| live::no_live_session("selection.get"))?;
            match live.target(participant.as_deref())? {
                Some(p) => json!({
                    "participant": p.id,
                    "selection": p.selection,
                    "objects": crate::refs::objects(session, &p.selection),
                }),
                None => json!({"participant": null, "selection": [], "objects": []}),
            }
        }
        Request::SelectionSet { refs, mode } => {
            let live = live.ok_or_else(|| live::no_live_session("selection.set"))?;
            // What is taken out may be gone already: kept as given when it
            // no longer reads.
            let refs = if mode == SelectMode::Remove {
                refs.iter()
                    .map(|r| crate::refs::target(session, r).unwrap_or_else(|_| r.clone()))
                    .collect()
            } else {
                crate::refs::targets(session, &refs)?
            };
            let id = live.caller(me);
            let selection = live.select(&id, refs, mode)?.to_vec();
            json!({"participant": id, "selection": selection})
        }
        Request::HighlightSet { refs, style, label } => {
            let live = live.ok_or_else(|| live::no_live_session("highlight.set"))?;
            let refs = crate::refs::targets(session, &refs)?;
            let id = live.caller(me);
            let highlights = live.highlight(&id, style, label, refs)?.to_vec();
            json!({"participant": id, "highlights": highlights})
        }
        Request::HighlightClear { style } => {
            let live = live.ok_or_else(|| live::no_live_session("highlight.clear"))?;
            let id = live.caller(me);
            let highlights = live.clear_highlights(&id, style)?.to_vec();
            json!({"participant": id, "highlights": highlights})
        }
        Request::ActivitySet { status, refs } => {
            let live = live.ok_or_else(|| live::no_live_session("activity.set"))?;
            let refs = crate::refs::targets(session, &refs)?;
            let id = live.caller(me);
            live.set_status(&id, &status, refs)?;
            json!({"participant": id, "status": live.get(&id)?.status})
        }
        Request::ViewGet { participant } => {
            let live = live.ok_or_else(|| live::no_live_session("view.get"))?;
            match live.target(participant.as_deref())? {
                Some(p) => json!({"participant": p.id, "view": crate::view::to_json(&p.view)}),
                None => json!({"participant": null, "view": null}),
            }
        }
        Request::ViewSet { item, tab, list } => {
            let live = live.ok_or_else(|| live::no_live_session("view.set"))?;
            let item = item.map(|i| session.catalogue().find(&i)).transpose()?;
            if let Some(t) = &tab {
                crate::view::check_tab(t)?;
            }
            let list = list.unwrap_or_default();
            list.check()?;
            let id = live.caller(me);
            let v = &mut live.get_mut(&id)?.view;
            v.item = item.map(|i| i.to_string());
            v.tab = tab;
            v.list = list;
            v.shown_by = Some(id.clone());
            json!({"participant": id, "view": crate::view::to_json(v)})
        }
        Request::ViewShow {
            reference,
            participant,
        } => {
            let live = live.ok_or_else(|| live::no_live_session("view.show"))?;
            let r = crate::refs::target(session, &reference)?;
            let by = live.caller(me);
            let who: Vec<String> = match &participant {
                Some(p) => vec![live.get(p)?.id.clone()],
                None => live.humans().map(|h| h.id.clone()).collect(),
            };
            let (item, tab) = crate::view::shown_at(&r);
            for id in &who {
                live.select(id, vec![r.clone()], SelectMode::Replace)?;
                let v = &mut live.get_mut(id)?.view;
                if item.is_some() {
                    v.item = item.clone();
                }
                if let Some(t) = tab {
                    v.tab = Some(t.to_owned());
                }
                v.shown_by = Some(by.clone());
            }
            let object = crate::refs::objects(session, std::slice::from_ref(&r)).remove(0);
            json!({"ref": r, "shown": who, "item": item, "tab": tab, "object": object})
        }
        // In process there is nothing running to cancel; the daemon's
        // loop answers it there.
        Request::RequestCancel { .. } => return Err(live::no_live_session("request.cancel")),
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
        // The same generation's map is the same allocation: nothing moved.
        && !std::sync::Arc::ptr_eq(seen, &now)
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
            &json!({"revisions": *now, "changed": changed}),
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
            live.prune(|t| crate::refs::stands(s, t));
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
        assert!(
            attrs.contains("documents/*/*.json merge=ryuplm diff=ryuplm text eol=lf"),
            "{attrs}"
        );
        assert!(
            attrs.contains("documents/*/*/** -text -diff -merge"),
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
        assert_eq!(v["schema"], 1);
    }

    #[test]
    fn people_point_and_agents_show() {
        use ryu_session::live::{Kind, SelectMode, Style};
        let (_d, root) = catalogue();
        for n in ["A-1", "B-1"] {
            ask(
                &root,
                Request::ItemCreate {
                    fields: Default::default(),
                    number: Some(n.into()),
                    next: None,
                    expected_revision: None,
                },
            )
            .unwrap();
        }
        let line = ask(
            &root,
            Request::BomAdd {
                item: "A-1".into(),
                fields: serde_json::from_value(json!({"child": "B-1", "quantity": "2"})).unwrap(),
                expected_revision: None,
            },
        )
        .unwrap()["line"]
            .as_str()
            .unwrap()
            .to_owned();
        let a = ask(&root, Request::ItemShow { item: "A-1".into() }).unwrap()["item"]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let mut slot = None;
        let mut live = Live::<Plm>::new();
        let dana = live.hello(Kind::Human, Some("Dana"), None).unwrap();
        let mut at = |live: &mut Live<Plm>, me: Option<&str>, r: Request| {
            handle_safely(&mut slot, Some(live), me, &root, r).map(|a| a.result)
        };
        // The person picks by number; the session keeps the ID.
        let s = at(
            &mut live,
            Some(&dana),
            Request::SelectionSet {
                refs: vec!["item/A-1".into()],
                mode: SelectMode::Replace,
            },
        )
        .unwrap();
        assert_eq!(s["selection"], json!([format!("item/{a}")]));
        // An agent reads what the one person points at.
        let g = at(&mut live, None, Request::SelectionGet { participant: None }).unwrap();
        assert_eq!(g["participant"], json!(dana));
        assert_eq!(g["objects"][0]["number"], "A-1");
        // An agent shows a line: the person's item opens at its BOM.
        let v = at(
            &mut live,
            None,
            Request::ViewShow {
                reference: format!("item/A-1/line/{line}"),
                participant: None,
            },
        )
        .unwrap();
        assert_eq!(v["shown"], json!([dana]));
        assert_eq!(v["tab"], "bom");
        let view = at(&mut live, None, Request::ViewGet { participant: None }).unwrap();
        assert_eq!(view["view"]["item"], json!(a));
        assert_eq!(view["view"]["tab"], "bom");
        assert_ne!(view["view"]["shown_by"], json!(dana));
        assert_eq!(
            live.get(&dana).unwrap().selection,
            [format!("item/{a}/line/{line}")]
        );
        // The person's own view, its filter checked.
        let e = at(
            &mut live,
            Some(&dana),
            Request::ViewSet {
                item: None,
                tab: Some("scene".into()),
                list: None,
            },
        )
        .unwrap_err();
        assert_eq!(e.code, Code::InvalidParam);
        let mine = at(
            &mut live,
            Some(&dana),
            Request::ViewSet {
                item: Some("B-1".into()),
                tab: Some("findings".into()),
                list: Some(crate::view::List {
                    sort: Some("number".into()),
                    ..Default::default()
                }),
            },
        )
        .unwrap();
        assert_eq!(mine["view"]["shown_by"], json!(dana));
        // Highlights and status take references the same way.
        let h = at(
            &mut live,
            None,
            Request::HighlightSet {
                refs: vec!["item/A-1".into()],
                style: Style::Pending,
                label: None,
            },
        )
        .unwrap();
        assert_eq!(h["highlights"][0]["entities"], json!([format!("item/{a}")]));
        let st = at(
            &mut live,
            None,
            Request::ActivitySet {
                status: "renumbering".into(),
                refs: vec!["item/A-1".into()],
            },
        )
        .unwrap();
        assert_eq!(st["status"]["text"], "renumbering");
        assert!(
            at(&mut live, None, Request::HighlightClear { style: None }).unwrap()["highlights"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        // Nothing of it in process.
        for r in [
            Request::SelectionGet { participant: None },
            Request::ViewShow {
                reference: "item/A-1".into(),
                participant: None,
            },
            Request::RequestCancel {
                token: "t".into(),
                name: None,
            },
        ] {
            assert_eq!(ask(&root, r).unwrap_err().code, Code::NoLiveSession);
        }
    }

    #[test]
    fn revisions_are_computed_once_per_generation() {
        let (_d, root) = catalogue();
        let mut slot = None;
        handle_safely(&mut slot, None, None, &root, Request::ProjectInfo {}).unwrap();
        let s = slot.as_ref().unwrap();
        let a = s.revisions();
        assert!(
            std::sync::Arc::ptr_eq(&a, &s.revisions()),
            "kept while nothing moves"
        );
        handle_safely(
            &mut slot,
            None,
            None,
            &root,
            Request::ItemCreate {
                fields: Default::default(),
                number: Some("A-1".into()),
                next: None,
                expected_revision: None,
            },
        )
        .unwrap();
        handle_safely(&mut slot, None, None, &root, Request::ProjectInfo {}).unwrap();
        let b = slot.as_ref().unwrap().revisions();
        assert!(!std::sync::Arc::ptr_eq(&a, &b));
        assert_eq!(b.len(), a.len() + 1, "the new item is in it");
    }
}
