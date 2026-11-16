//! `ryueda review` (design 667): the hosted review service's client,
//! `ryu-review`, named for ryueda. Comments pin to thread anchors
//! (`<design>:<block>/<object>`, as `ryueda resolve` reads them). CLI
//! only: it talks to a service, not to the session.

use crate::error::{Code, Error, Result};
use serde_json::Value;

/// Overrides the `review.url` preference.
pub const URL_VAR: &str = "RYUEDA_REVIEW_URL";
/// The GitHub token to send, before `GH_TOKEN`, `GITHUB_TOKEN` and `gh
/// auth token`.
pub const TOKEN_VAR: &str = "RYUEDA_REVIEW_TOKEN";

/// ryueda as the review client names it.
pub const EDA: ryu_review::Product = ryu_review::Product {
    name: "ryueda",
    version: env!("CARGO_PKG_VERSION"),
    url_var: URL_VAR,
    token_var: TOKEN_VAR,
    url_pref,
};

/// The `review.url` preference, when the preferences set it.
fn url_pref() -> Option<String> {
    ryu_session::prefs::Preferences::<crate::Eda>::load()
        .ok()?
        .values
        .get(ryu_review::URL_PREF)?
        .as_str()
        .map(str::to_owned)
}

/// Runs `action` for the clone holding `here`.
pub fn run(here: &std::path::Path, action: ryu_review::Action) -> Result<Value> {
    ryu_review::run::<crate::Eda>(here, &EDA, action)
}

/// `ryueda where-used <component> --org` in the library project at
/// `root`: asked of the service, by the project's ID, at any revision or
/// one `label`.
pub fn where_used_org(
    root: &std::path::Path,
    part: &str,
    label: Option<&str>,
    direct: bool,
) -> Result<Value> {
    let path = root.join(crate::project::MANIFEST);
    let bytes = std::fs::read(&path).map_err(|e| Error::io(&path, &e))?;
    let m = crate::project::Manifest::read(&bytes)?;
    if m.project_kind != crate::project::ProjectKind::Library {
        return Err(Error::new(
            Code::InvalidParam,
            "where-used asks about a library project's component: run it in the library project",
        )
        .with("param", "part"));
    }
    let component = crate::id::Id::parse(part, "component")?;
    ryu_review::where_used(
        &EDA.client()?,
        &m.project.to_string(),
        component.as_str(),
        label,
        direct,
    )
}
