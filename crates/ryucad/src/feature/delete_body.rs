//! Deleted bodies: solids taken out of the part's result (ryucad 27.C).

use super::Given;
use super::{BodyUse, FeatureKind, Write};
use crate::doc::files::Feature;
use crate::doc::id::Id;
use crate::doc::project::Part;
use crate::error::{Code, Error, Result};
use crate::part::proposed;
use crate::rebuild::{Made, Results};
use crate::session::Session;
use crate::solid::Solids;
use serde_json::Value;
use serde_json::json;

pub(crate) const DELETE_BODY: [&str; 2] = ["solids", "solid"];

/// A `delete_body` from what it is given: `solids` (or one `solid`, as
/// the CLI sends a lone `--solid`), read as its file reads them.
pub(crate) fn delete_from(given: &Given) -> Result<crate::doc::body::DeleteBody> {
    let solids: Vec<String> = match (&given.fields.solids, &given.fields.solid) {
        (Some(s), None) => s.clone(),
        (None, Some(s)) => vec![s.clone()],
        (Some(_), Some(_)) => {
            return Err(Error::new(
                Code::InvalidParam,
                "a delete names its solids once: solids, or one solid",
            )
            .with("param", "solid"));
        }
        (None, None) => Vec::new(),
    };
    if solids.is_empty() {
        return Err(Error::new(
            Code::InvalidParam,
            "a delete names the solids it deletes: solids",
        )
        .with("param", "solids"));
    }
    let v = json!({"solids": solids});
    let mut o = crate::doc::files::Obj::new(&v, "")?;
    let d =
        crate::doc::body::DeleteBody::from_fields(&mut o).map_err(|e| e.with("param", "solids"))?;
    o.finish()?;
    Ok(d)
}

/// Refuses a `delete_body` its rebuild would refuse, against the part as
/// the write would leave it: the solids before it as they are now.
pub(crate) fn check_delete(
    session: &mut Session,
    pid: &Id,
    list: &[Id],
    id: &Id,
    f: &Feature,
) -> Result<()> {
    let Feature::DeleteBody(d) = f else {
        return Ok(());
    };
    let q = proposed(session.project().part(pid)?, list, &[(id, f)]);
    let solids = Solids::of(&q);
    let r = session.rebuilt(pid)?;
    delete_body(&q, &solids, id, d, r).map(|_| ())
}

/// Deletes: solids taken out of the part's result (ryucad 25).
pub(crate) struct Deletes;

impl FeatureKind for Deletes {
    fn types(&self) -> &'static [&'static str] {
        &["delete_body"]
    }

    fn fields(&self, _ty: &str) -> &'static [&'static str] {
        &DELETE_BODY
    }

    fn body(&self, _ty: &str) -> BodyUse {
        BodyUse::NONE
    }

    fn from_given(
        &self,
        _ty: &str,
        current: Option<&Feature>,
        cx: &mut Write<'_>,
    ) -> Result<Feature> {
        let named = cx.given.fields.solids.is_some() || cx.given.fields.solid.is_some();
        match current {
            // An edit naming no solids keeps them.
            Some(Feature::DeleteBody(d)) if !named => Ok(Feature::DeleteBody(d.clone())),
            _ => Ok(Feature::DeleteBody(Box::new(delete_from(&cx.given)?))),
        }
    }

    fn report(&self, _part: &Part, _unit: crate::units::Unit, f: &Feature) -> Value {
        let Feature::DeleteBody(d) = f else {
            unreachable!("a delete_body")
        };
        d.fields()
    }

    /// Ends the chains it names; makes nothing (design 521).
    fn rebuild(&self, cx: &super::RebuildCx<'_>, f: &Feature) -> super::Outcome {
        let Feature::DeleteBody(d) = f else {
            unreachable!("a delete")
        };
        super::made(delete_body(cx.part, cx.solids, cx.id, d, cx.before))
    }
}

/// A `delete_body`: each solid it names must be a solid of the part as
/// it sees it (`not_found`), not consumed (`body_consumed`) nor deleted
/// already (`body_deleted`). It builds to no solids: [`Solids`] ends the
/// chains, so what follows can't read them and what came before reads
/// them as it did. An insert's ID names every solid it founded.
pub fn delete_body(
    part: &Part,
    solids: &Solids,
    id: &Id,
    d: &crate::doc::body::DeleteBody,
    before: &dyn Results,
) -> Result<Made> {
    for (i, t) in d.solids.iter().enumerate() {
        let at = format!("/solids/{i}");
        if let Some(by) = solids.consumed_by(t, id) {
            return Err(crate::rebuild::consumed(id, t, by).with("pointer", at));
        }
        // Deleted by an earlier delete: this one records its own list at
        // its own place, so it is never before itself.
        if let Some(by) = solids.deleted_by(t, id).filter(|by| *by != id) {
            return Err(crate::rebuild::deleted(id, t, by).with("pointer", at));
        }
        let is_insert = part
            .features
            .get(t)
            .is_some_and(|f| f.as_ref().is_ok_and(Feature::inserts));
        if !(solids.is_solid(t) || is_insert) {
            return Err(Error::new(
                Code::NotFound,
                format!("{id} deletes {t}, which is no solid of {}", part.id),
            )
            .with("feature", id)
            .with("solid", t)
            .with("pointer", at));
        }
        // A key below an insert must be one its source shows.
        if t.is_path()
            && let Some(view) = solids.view(t, id)
            && let Ok(out) = crate::graph::input(part, id, view, before)
            && out.solid(t).is_none()
        {
            return Err(crate::rebuild::not_inserted(
                id,
                "solids",
                t,
                &t.founder(),
                out.solids().iter().map(|(k, _)| k).collect(),
            )
            .with("pointer", at));
        }
    }
    Ok(Made::new(Vec::new()))
}

/// A delete's report: `solids` as written, an insert's ID expanded to the
/// keys it founded (design 521).
pub fn delete_report(d: &crate::doc::body::DeleteBody, results: &dyn Results) -> Value {
    let mut out = Vec::new();
    for t in &d.solids {
        match results.result(t).and_then(|r| r.as_ref().ok()) {
            Some(o) if o.inserted().is_some() => {
                out.extend(o.solids().iter().map(|(k, _)| json!(k)));
            }
            _ => out.push(json!(t)),
        }
    }
    json!({"solids": out})
}
