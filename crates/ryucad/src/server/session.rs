//! Requests on the live session: its participants.

use crate::activity::{self};
use crate::config;
use crate::doc::id::Id;
use crate::doc::project::MANIFEST;
use crate::entity::{self};
use crate::error::{Code, Error, Result};
use crate::live::CadLive;
use crate::live::Kind;
use crate::live::{self, Live};
use crate::notify::Topic;
use crate::part::part_id;
use crate::session::Session;
use serde_json::Map;
use serde_json::{Value, json};

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

/// `session.hello`.
pub(crate) fn session_hello(
    live: Option<&mut Live>,
    kind: Kind,
    name: Option<String>,
    participant: Option<String>,
) -> Result<Value> {
    let live = live.ok_or_else(|| live::no_live_session("session.hello"))?;
    let id = live.hello(kind, name.as_deref(), participant.as_deref())?;
    Ok(json!({"participant": id, "session": live.list()}))
}

/// `subscribe`.
pub(crate) fn subscribe(live: Option<&mut Live>) -> Result<Value> {
    match live {
        None => Err(live::no_live_session("subscribe")),
        Some(_) => Err(Error::new(Code::Usage, "subscribe needs a connection")),
    }
}

/// `participant.mode`.
pub(crate) fn participant_mode(
    live: Option<&mut Live>,
    me: Option<&str>,
    mode: crate::live::Mode,
) -> Result<Value> {
    let live = live.ok_or_else(|| live::no_live_session("participant.mode"))?;
    let id = live.caller(me);
    live.set_mode(&id, mode)?;
    Ok(json!({"participant": id, "mode": mode}))
}

/// `activity.log`.
pub(crate) fn activity_log(
    live: Option<&mut Live>,
    limit: Option<usize>,
    before: Option<u64>,
) -> Result<Value> {
    let entries = live.map_or_else(Vec::new, |l| l.log.page(limit.unwrap_or(50), before));
    Ok(json!({ "entries": entries }))
}

/// `undo`.
pub(crate) fn undo(
    session: &mut Session,
    live: Option<&mut Live>,
    me: Option<&str>,
    entry: Option<u64>,
) -> Result<Value> {
    let live = live.ok_or_else(|| live::no_live_session("undo"))?;
    let who = live.caller(me);
    activity::undo(live, session, &who, entry, false)
}

/// `redo`.
pub(crate) fn redo(
    session: &mut Session,
    live: Option<&mut Live>,
    me: Option<&str>,
    entry: Option<u64>,
) -> Result<Value> {
    let live = live.ok_or_else(|| live::no_live_session("redo"))?;
    let who = live.caller(me);
    activity::undo(live, session, &who, entry, true)
}

/// `doctor`.
pub(crate) fn doctor(session: &mut Session) -> Result<Value> {
    let root = session.root().to_owned();
    crate::doctor::report_in(&mut crate::doctor::Builds::live(&root, session))
}

/// `doctor.repair`.
pub(crate) fn doctor_repair(
    session: &mut Session,
    repairs: Vec<String>,
    params: Map<String, Value>,
) -> Result<Value> {
    let params = crate::doctor::Params::from_json(&params)?;
    let r = crate::doctor::repair_with(
        session.root(),
        &repairs,
        &params,
        session.deps_cache.clone(),
    )?;
    session.reload()?;
    Ok(r)
}

/// `history.log`.
pub(crate) fn history_log(
    session: &mut Session,
    part: Option<String>,
    limit: Option<usize>,
) -> Result<Value> {
    let part = part.as_deref().map(part_id).transpose()?;
    if let Some(p) = &part {
        session.project().part(p)?;
    }
    let files = session.snapshot();
    crate::git::history(
        session.root(),
        &files,
        part.as_ref().map(Id::as_str),
        limit.unwrap_or(50),
    )
}

/// `config.get`.
pub(crate) fn config_get(
    session: &mut Session,
    key: Option<String>,
    scope: Option<String>,
) -> Result<Value> {
    let scope = scope.as_deref().map(config::Scope::parse).transpose()?;
    let mut v = config::get(&session.project().manifest, key.as_deref(), scope)?;
    v["project_revision"] = json!(session.project_revision()?);
    Ok(v)
}

/// `config.set`.
pub(crate) fn config_set(
    session: &mut Session,
    live: Option<&mut Live>,
    key: String,
    value: Value,
    scope: Option<String>,
    expected_revision: Option<String>,
) -> Result<Value> {
    let scope = scope.as_deref().map(config::Scope::parse).transpose()?;
    let v = match config::validate(&key, &value, scope)? {
        config::Scope::Project => {
            session.check_project_revision(expected_revision.as_deref())?;
            // The rest of the manifest (the project's ID) stays.
            let m = config::set_project(&session.project().manifest, &key, &value);
            session.write_json(&session.root().join(MANIFEST), &m.to_json())?;
            session.reload()?;
            let mut v = config::get(&m, Some(&key), None)?;
            v["project_revision"] = json!(session.project_revision()?);
            v
        }
        config::Scope::User => config::set_user(&key, value)?,
    };
    if key == "cache.max_bytes"
        && let Some(d) = session.disk()
    {
        d.set_max_bytes(
            v["value"]
                .as_u64()
                .unwrap_or(crate::cache::DEFAULT_MAX_BYTES),
        );
        d.trim_in_background();
    }
    // A daemon's cache takes the shared cache as it is now set.
    if key.starts_with("cache.remote")
        && let Some(d) = session.disk()
    {
        d.set_remote(crate::cache::remote_preference(&d.identity()));
    }
    if let Some(live) = live {
        if key == "daemon.poll_ms" {
            live.poll_ms = v["value"].as_u64().unwrap_or(250);
        }
        let note = json!({"key": key, "scope": v["scope"], "value": v["value"],
                          "default": v["default"]});
        live.hub.send(Topic::Project, "config.changed", &note);
    }
    Ok(v)
}

/// `app.open`.
pub(crate) fn app_open(live: Option<&mut Live>, participant: Option<String>) -> Result<Value> {
    let live = live.ok_or_else(|| live::no_live_session("app.open"))?;
    let asked: Vec<String> = match participant {
        Some(id) => {
            let p = live.get(&id)?;
            if p.kind != live::Kind::Human {
                return Err(Error::new(
                    Code::InvalidParam,
                    format!("participant {id} is not an app window"),
                )
                .with("participant", id));
            }
            vec![id]
        }
        None => live.humans().map(|p| p.id.clone()).collect(),
    };
    for id in &asked {
        live.hub.send_to(id, "app.open", &json!({}));
    }
    Ok(json!({ "asked": asked }))
}

/// `activity.set`.
pub(crate) fn activity_set(
    session: &mut Session,
    live: Option<&mut Live>,
    me: Option<&str>,
    status: String,
    entities: Vec<String>,
) -> Result<Value> {
    let live = live.ok_or_else(|| live::no_live_session("activity.set"))?;
    let refs = entity::parse_all(session, &entities)?;
    let id = live.caller(me);
    live.set_status(&id, &status, refs)?;
    Ok(json!({"participant": id, "status": live.get(&id)?.status}))
}
