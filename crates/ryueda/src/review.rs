//! `ryueda review` (design 667): the hosted review service's client,
//! `ryu-review`, named for ryueda. Comments pin to thread anchors
//! (`<design>:<block>/<object>`, as `ryueda resolve` reads them). CLI
//! only: it talks to a service, not to the session.

use crate::error::Result;
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
