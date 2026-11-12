//! Requests on assemblies: solving at a revision, with proposed mates.

use super::dispatch;
use crate::diff::files_at;
use crate::error::{Error, Result};
use crate::proto::Request;
use crate::session::Session;
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
