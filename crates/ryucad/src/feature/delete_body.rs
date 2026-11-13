//! Deleted bodies: solids taken out of the part's result (ryucad 27.C).

use super::Given;
use super::{BodyUse, FeatureKind, Write};
use crate::doc::files::Feature;
use crate::doc::id::Id;
use crate::doc::project::Part;
use crate::error::{Code, Error, Result};
use crate::part::proposed;
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
    crate::body::delete_body(&q, &solids, id, d, r).map(|_| ())
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
}
