//! `ryuplm review` (design 938): the hosted service's client,
//! `ryu-review`, named for ryuplm. Comments pin to references
//! (`item/<id>`, `item/<id>/line/<line>`, as `ryuplm resolve` reads
//! them). CLI only: it talks to a service, not to the session.

use crate::error::{Code, Error, Result};
use serde_json::{Value, json};

// ryuplm as the service's client names it: the ledger's (design 936),
// one `review.url` and one token for both.
use crate::ledger::PLM;

/// Runs `action` for the clone holding `here`. A comment's reference is
/// read here first, so a mistyped one never reaches the service.
pub fn run(here: &std::path::Path, action: ryu_review::Action) -> Result<Value> {
    if let ryu_review::Action::Comment { reference, .. } = &action
        && crate::refs::Ref::parse(reference).is_none()
    {
        return Err(Error::new(
            Code::InvalidParam,
            format!("{reference:?} is no reference: item/<id> or item/<id>/line/<line>"),
        )
        .with("param", "ref")
        .with("ref", reference));
    }
    ryu_review::run::<crate::Plm>(here, &PLM, action)
}

/// `ryuplm where-used <item> --org` in the catalogue at `root`: asked of
/// the service, by the catalogue's project ID and the item's ID, across
/// every repository of the organization the token can read. `item` is an
/// ID or a number of this catalogue's; an ID this catalogue no longer
/// has is still asked about.
pub fn where_used_org(root: &std::path::Path, item: &str) -> Result<Value> {
    let cat = crate::load::Catalogue::load(&crate::source::Folder(root))?;
    let id = cat
        .find(item)
        .or_else(|e| crate::id::Id::parse(item, "item").map_err(|_| e))?;
    let client = PLM.client()?;
    let v = client.get(&format!(
        "/where-used?project={}&item={}",
        ryu_review::escape(&cat.manifest.project.to_string()),
        ryu_review::escape(id.as_str()),
    ))?;
    if v["schema"] != json!(1) {
        return Err(Error::new(
            Code::ReviewFailed,
            format!(
                "the review service answered where-used in schema {}, which this build doesn't read",
                v["schema"]
            ),
        )
        .with("schema", &v["schema"]));
    }
    Ok(v)
}
