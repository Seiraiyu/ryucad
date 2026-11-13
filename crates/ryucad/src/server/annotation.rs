//! Requests on annotations.

use super::view::pick_from;
use crate::error::{Code, Error, Result};
use crate::live::{self, Live};
use crate::part::part_id;
use crate::proto::{MarksArg, PinArg};
use crate::session::Session;
use serde_json::Value;

/// `annotation.apply`.
pub(crate) fn annotation_apply(
    session: &mut Session,
    live: Option<&mut Live>,
    me: Option<&str>,
    id: String,
    keep_open: bool,
    expect: Option<String>,
    author: Option<String>,
) -> Result<Value> {
    let by = crate::annotation::author(live.as_deref(), me, author.as_deref());
    crate::annotation::propose::apply(session, &id, keep_open, expect.as_deref(), &by)
}

/// `annotation.create`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn annotation_create(
    session: &mut Session,
    live: Option<&mut Live>,
    me: Option<&str>,
    pins: Vec<PinArg>,
    comment: String,
    camera: Option<crate::view::CameraArg>,
    with_view: bool,
    marks: Option<MarksArg>,
    named: Option<String>,
    author: Option<String>,
) -> Result<Value> {
    let by = crate::annotation::author(live.as_deref(), me, author.as_deref());
    let unit = session.project().manifest.default_unit;
    let view = match (camera, with_view, named) {
        (Some(c), false, None) => Some(c.resolve(unit)?.to_view_file()),
        (None, false, Some(v)) => {
            let part = marks
                .as_ref()
                .map(|m| m.part.clone())
                .or_else(|| {
                    pins.first().and_then(|p| {
                        let r = p.reference.clone().or(p.part.clone())?;
                        Some(r.split('/').next().unwrap_or_default().to_owned())
                    })
                })
                .ok_or_else(|| {
                    Error::new(
                        Code::Usage,
                        "a named view is of the marks' part: give marks or a pin",
                    )
                })?;
            Some(crate::views::view_file(session, &part_id(&part)?, &v)?)
        }
        (None, true, None) => {
            let l = live
                .as_deref()
                .ok_or_else(|| live::no_live_session("annotation.create --view"))?;
            let camera = me
                .and_then(|m| l.get(m).ok())
                .and_then(|p| p.view.camera.clone())
                .ok_or_else(|| {
                    Error::new(
                        Code::InvalidParam,
                        "your participant has no camera to keep (camera.set first)",
                    )
                })?;
            Some(camera.to_view_file())
        }
        (None, false, None) => None,
        _ => {
            return Err(Error::new(
                Code::Usage,
                "give one of camera, --view and --of",
            ));
        }
    };
    let from = pick_from(live.as_deref(), me);
    crate::annotation::create(session, by, &pins, &comment, view, marks.as_ref(), &from)
}

/// `annotation.reply`.
pub(crate) fn annotation_reply(
    session: &mut Session,
    live: Option<&mut Live>,
    me: Option<&str>,
    id: String,
    text: String,
    expect: Option<String>,
    author: Option<String>,
) -> Result<Value> {
    let by = crate::annotation::author(live.as_deref(), me, author.as_deref());
    crate::annotation::reply(session, &id, &text, expect.as_deref(), by)
}

/// `annotation.claim`.
pub(crate) fn annotation_claim(
    session: &mut Session,
    live: Option<&mut Live>,
    me: Option<&str>,
    id: String,
    expect: Option<String>,
    author: Option<String>,
) -> Result<Value> {
    let by = crate::annotation::author(live.as_deref(), me, author.as_deref());
    crate::annotation::claim(session, &id, expect.as_deref(), by)
}

/// `annotation.resolve`.
pub(crate) fn annotation_resolve(
    session: &mut Session,
    live: Option<&mut Live>,
    me: Option<&str>,
    id: String,
    note: Option<String>,
    expect: Option<String>,
    author: Option<String>,
) -> Result<Value> {
    let by = crate::annotation::author(live.as_deref(), me, author.as_deref());
    crate::annotation::close(session, &id, None, note.as_deref(), expect.as_deref(), by)
}

/// `annotation.reject`.
pub(crate) fn annotation_reject(
    session: &mut Session,
    live: Option<&mut Live>,
    me: Option<&str>,
    id: String,
    reason: String,
    expect: Option<String>,
    author: Option<String>,
) -> Result<Value> {
    let by = crate::annotation::author(live.as_deref(), me, author.as_deref());
    crate::annotation::close(session, &id, Some(&reason), None, expect.as_deref(), by)
}

/// `annotation.edit`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn annotation_edit(
    session: &mut Session,
    live: Option<&mut Live>,
    me: Option<&str>,
    id: String,
    comment: Option<String>,
    pins: Option<Vec<PinArg>>,
    marks: Option<MarksArg>,
    no_marks: bool,
    expect: Option<String>,
    author: Option<String>,
) -> Result<Value> {
    if no_marks && marks.is_some() {
        return Err(Error::new(
            Code::Usage,
            "--no-marks takes them away: give no marks with it",
        ));
    }
    let by = crate::annotation::author(live.as_deref(), me, author.as_deref());
    let from = pick_from(live.as_deref(), me);
    crate::annotation::edit(
        session,
        &id,
        comment.as_deref(),
        pins.as_deref(),
        marks.as_ref(),
        no_marks,
        &from,
        expect.as_deref(),
        by,
    )
}

/// `annotation.delete`.
pub(crate) fn annotation_delete(
    session: &mut Session,
    live: Option<&mut Live>,
    me: Option<&str>,
    id: String,
    expect: Option<String>,
    author: Option<String>,
) -> Result<Value> {
    let by = crate::annotation::author(live.as_deref(), me, author.as_deref());
    crate::annotation::delete(session, &id, expect.as_deref(), by)
}

/// `annotation.propose`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn annotation_propose(
    session: &mut Session,
    live: Option<&mut Live>,
    me: Option<&str>,
    request: Value,
    comment: Option<String>,
    pins: Vec<PinArg>,
    rough: bool,
    author: Option<String>,
) -> Result<Value> {
    let by = crate::annotation::author(live.as_deref(), me, author.as_deref());
    crate::annotation::propose::propose(
        session,
        live,
        me,
        crate::annotation::propose::Ask {
            request,
            comment,
            pins,
            rough,
            by,
        },
    )
}
