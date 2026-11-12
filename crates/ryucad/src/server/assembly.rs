//! Requests on assemblies: solving at a revision, with proposed mates.

use super::dispatch;
use crate::diff::files_at;
use crate::error::{Error, Result};
use crate::live::{self, CadLive, Live};
use crate::proto::InstanceMove;
use crate::proto::Request;
use crate::session::Session;
use serde_json::Map;
use serde_json::{Value, json};

/// `assembly.solve` answered for `rev` (the working files when `None`):
/// any revision a read takes, a person's draft included, named in the
/// answer's `rev`. `mates` are each run as `feature.add` of a mate in a
/// capture session over those files and solved with the rest, never
/// written; a refusal is `feature.add`'s own, at `/mates/<i>`.
pub(crate) fn solve_at(
    session: &mut Session,
    part: &str,
    moves: Vec<(String, crate::assembly::Move)>,
    component: bool,
    rev: Option<&str>,
    mates: Vec<serde_json::Map<String, Value>>,
) -> Result<Value> {
    let rev = rev.filter(|r| *r != "working");
    let (named, files) = match rev {
        None => (None, None),
        Some(r) => {
            let (n, f) = files_at(session, r)?;
            (Some(n), Some(f))
        }
    };
    let mut v = if mates.is_empty() {
        let (_, s) = session.at(rev)?;
        crate::mate::solve_request(s, part, moves, component)?
    } else {
        let files = files.unwrap_or_else(|| session.snapshot());
        let mut capture = session.capturing(files)?;
        for (i, m) in mates.into_iter().enumerate() {
            let at = |mut e: Error| {
                if let Some(p) = e.details.get("pointer").cloned() {
                    e = e.with("field", p);
                }
                e.with("pointer", format!("/mates/{i}"))
            };
            let mut params = json!({"part": part, "type": "mate"});
            for (k, v) in m {
                params[k] = v;
            }
            let req = Request::from_rpc("feature.add", Some(params)).map_err(at)?;
            dispatch(&mut capture, None, None, req).map_err(at)?;
        }
        crate::mate::solve_request(&mut capture, part, moves, component)?
    };
    if let Some(n) = named {
        v["rev"] = n;
    }
    Ok(v)
}

/// `assembly.solve`.
pub(crate) fn assembly_solve(
    session: &mut Session,
    part: String,
    moves: Vec<InstanceMove>,
    component: bool,
    rev: Option<String>,
    mates: Vec<Map<String, Value>>,
) -> Result<Value> {
    let moves = moves
        .into_iter()
        .map(|m| {
            (
                m.instance,
                crate::assembly::Move {
                    placement: m.placement,
                    translate: m.translate,
                    axis: m.axis,
                    angle: m.angle,
                    about: m.about,
                },
            )
        })
        .collect();
    solve_at(session, &part, moves, component, rev.as_deref(), mates)
}

/// `explode.delete`.
pub(crate) fn explode_delete(
    session: &mut Session,
    live: Option<&mut Live>,
    part: String,
    explode: String,
    expected_revision: Option<String>,
) -> Result<Value> {
    let v = crate::explode::delete(session, &part, &explode, expected_revision.as_deref())?;
    // No window goes on showing it.
    if let Some(l) = live {
        let showing: Vec<String> = l
            .participants()
            .filter(|p| {
                p.view.explode.as_ref().is_some_and(|e| {
                    e.part.as_str() == v["part"] && e.explode.as_str() == v["explode"]
                })
            })
            .map(|p| p.id.clone())
            .collect();
        for id in showing {
            l.set_explode(&id, None)?;
        }
    }
    Ok(v)
}

/// `explode.show`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn explode_show(
    session: &mut Session,
    live: Option<&mut Live>,
    me: Option<&str>,
    part: Option<String>,
    explode: Option<String>,
    fraction: Option<f64>,
    off: bool,
    participant: Option<String>,
) -> Result<Value> {
    let live = live.ok_or_else(|| live::no_live_session("explode.show"))?;
    let id = live.compare_target(participant.as_deref(), me)?;
    let (now, focus) = {
        let v = &live.get(&id)?.view;
        (v.explode.clone(), v.focus.clone())
    };
    let ask = crate::explode::ShowAsk {
        part,
        explode,
        fraction,
        off,
    };
    let shown = crate::explode::show(session, now.as_ref(), focus.as_ref(), &ask)?;
    live.set_explode(&id, shown.clone())?;
    Ok(crate::explode::shown_json(session, &id, shown.as_ref()))
}
