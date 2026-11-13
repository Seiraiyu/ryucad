//! Requests on the disk and shared caches: stats, sign-in, warming.

use crate::config;
use crate::doc::assembly::Rev;
use crate::doc::files::PartKind;
use crate::doc::id::Id;
use crate::error::Result;
use crate::part::part_id;
use crate::session::Session;
use serde_json::{Value, json};

/// `cache.stats`' answer, and `cache.clear`'s with what it `removed`:
/// whether there is a disk cache, its folder and bound, what it holds,
/// and its shared cache server's state.
pub(crate) fn cache_stats(
    disk: Option<&crate::cache::Disk>,
    removed: Option<crate::cache::Stats>,
) -> Value {
    let mut v = local_stats(disk, removed);
    v["remote"] = remote_stats(disk);
    v
}

/// `cache.login`'s and `cache.logout`'s answer, the token as it now is
/// put behind the daemon's cache: where the token is read from, what the
/// server says it allows (or why it couldn't say), and the remote's state.
/// `cache.logout` adds `removed: true`.
pub(crate) fn cache_login(disk: Option<&crate::cache::Disk>) -> Result<Value> {
    use crate::cache::token;
    if let Some(d) = disk {
        d.set_remote(crate::cache::remote_preference(&d.identity()));
    }
    let from = token::token().map(|(_, o)| o.as_str());
    let mut v = json!({
        "file": token::path()?,
        "token": from,
        "remote": remote_stats(disk),
    });
    // The variable wins over the file.
    if from == Some("variable") {
        v["overridden_by"] = json!(token::TOKEN_VAR);
    }
    if let Some(r) = disk.and_then(crate::cache::Disk::remote) {
        v["server"] = match r.info() {
            Ok(info) => json!({"write": info["write"], "entries": info["entries"]}),
            Err(e) => json!({"error": e}),
        };
    }
    Ok(v)
}

/// `cache.warm`: `part` (or every part) rebuilt, the parts that aren't
/// assemblies side by side and then each assembly's model, which places
/// what it holds. Its remote reads come a depth at a time; what the remote
/// lacks is built and sent behind, as any rebuild's is. A failing part is
/// left to `rebuild` to report.
pub(crate) fn cache_warm(session: &mut Session, part: Option<&str>) -> Result<Value> {
    let parts: Vec<Id> = match part {
        Some(p) => {
            let id = part_id(p)?;
            session.project().part(&id)?;
            vec![id]
        }
        None => session.project().parts.keys().cloned().collect(),
    };
    let disk = session.disk().cloned();
    let counts = |d: &Option<std::sync::Arc<crate::cache::Disk>>| {
        let d = d.as_deref();
        let remote = d.and_then(crate::cache::Disk::remote).map(|r| r.stats());
        let n = |k: &str| remote.as_ref().and_then(|r| r[k].as_u64()).unwrap_or(0);
        (
            d.map_or(0, crate::cache::Disk::stored),
            n("hits"),
            n("bytes_in"),
        )
    };
    let before = counts(&disk);
    session.rebuild_parts(&parts)?;
    for id in &parts {
        let assembly = session
            .project()
            .part(id)?
            .file
            .as_ref()
            .is_ok_and(|f| f.kind == PartKind::Assembly);
        if assembly {
            let _ = session.model(id, &Rev::Working);
        }
    }
    let after = counts(&disk);
    Ok(json!({
        "parts": parts.len(),
        "built": after.0 - before.0,
        "fetched": after.1 - before.1,
        "fetched_bytes": after.2 - before.2,
        "remote": remote_stats(disk.as_deref()),
    }))
}

/// What `cache.stats` says of the remote: what it has done in this
/// process, else why there is none.
fn remote_stats(disk: Option<&crate::cache::Disk>) -> Value {
    if let Some(r) = disk.and_then(crate::cache::Disk::remote) {
        let mut v = r.stats();
        v["enabled"] = json!(true);
        return v;
    }
    let url = config::user("cache.remote");
    let url = url.as_str().unwrap_or_default();
    let note = match disk {
        _ if url.is_empty() => return json!({"enabled": false}),
        None => "there is no disk cache on this machine to put it behind",
        Some(d) if !d.identity().shared => {
            "a development build never uses a shared cache: only released builds share entries"
        }
        Some(_) => "it isn't in use",
    };
    json!({"enabled": false, "url": url, "note": note})
}

fn local_stats(disk: Option<&crate::cache::Disk>, removed: Option<crate::cache::Stats>) -> Value {
    let mut v = match disk {
        Some(d) => {
            let mut v = d.stats().to_json();
            v["enabled"] = json!(true);
            v["folder"] = json!(d.root());
            v["max_bytes"] = json!(d.max_bytes());
            // A release's entries are shared with its builds everywhere.
            v["shared"] = json!(d.identity().shared);
            v
        }
        None => {
            let mut v = crate::cache::Stats::default().to_json();
            v["enabled"] = json!(false);
            v
        }
    };
    if let Some(r) = removed {
        v["removed"] = r.to_json();
    }
    v
}

/// `cache.clear`.
pub(crate) fn cache_clear(session: &mut Session) -> Result<Value> {
    let disk = session.disk().map(|d| d.as_ref());
    let removed = disk.map(crate::cache::Disk::clear).unwrap_or_default();
    Ok(cache_stats(disk, Some(removed)))
}

/// `cache.login`.
pub(crate) fn cache_login_request(session: &mut Session, token: String) -> Result<Value> {
    crate::cache::token::store(Some(token.trim()))?;
    cache_login(session.disk().map(|d| d.as_ref()))
}

/// `cache.logout`.
pub(crate) fn cache_logout(session: &mut Session) -> Result<Value> {
    crate::cache::token::store(None)?;
    let mut v = cache_login(session.disk().map(|d| d.as_ref()))?;
    // So a token the variable still sends isn't read as one saved.
    v["removed"] = json!(true);
    Ok(v)
}
