//! The `ryucad review` client: `ryu-review` (plan 8.A), named for
//! ryucad, with ryucad's own actions (audit and release packages) and
//! where-used across the organization. CLI only: it talks to a service,
//! not to the session.

use crate::error::{Code, Error, Result};
use serde_json::{Value, json};

pub use ryu_review::{Client, refusal};

/// Overrides the `review.url` preference.
pub const URL_VAR: &str = "RYUCAD_REVIEW_URL";
/// The GitHub token to send, before `GH_TOKEN`, `GITHUB_TOKEN` and
/// `gh auth token`.
pub const TOKEN_VAR: &str = "RYUCAD_REVIEW_TOKEN";
/// The largest package downloaded: a large assembly's is tens of MB.
pub const MAX_PACKAGE: u64 = 4 << 30;

/// ryucad as the review client names it.
pub const CAD: ryu_review::Product = ryu_review::Product {
    name: "ryucad",
    version: env!("CARGO_PKG_VERSION"),
    url_var: URL_VAR,
    token_var: TOKEN_VAR,
    url_pref: || {
        crate::config::user(ryu_review::URL_PREF)
            .as_str()
            .map(str::to_owned)
    },
};

/// The token to send ([`ryu_review::Product::token`]).
pub fn token() -> Result<String> {
    CAD.token()
}

/// The service's URL ([`ryu_review::Product::base_url`]).
pub fn base_url() -> String {
    CAD.base_url()
}

/// What `ryucad where-used --org` asks the service: where `part` of
/// `project` is used across the organization's repositories, at any
/// revision or one `label`, `direct` only by assemblies placing it
/// themselves.
pub struct OrgQuery {
    pub project: crate::doc::id::ProjectId,
    pub part: crate::doc::id::Id,
    pub label: Option<String>,
    pub direct: bool,
}

/// `GET /api/v1/where-used` ([`ryu_review::where_used`]): `where-used`'s
/// answer with each row's `repo` (`owner/name`) and `commit`, over the
/// repositories the token can read.
pub fn where_used(client: &Client, q: &OrgQuery) -> Result<Value> {
    ryu_review::where_used(
        client,
        &q.project.to_string(),
        q.part.as_ref(),
        q.label.as_deref(),
        q.direct,
    )
}

/// `ryucad where-used <part> --org` for the project at `root`: its own
/// part (by the project's ID, which it must have), or another project's
/// as `<dependency>:<part>`, asked of the service at [`base_url`] with
/// [`token`].
pub fn where_used_org(
    root: &std::path::Path,
    part: &str,
    label: Option<&str>,
    direct: bool,
) -> Result<Value> {
    let project = crate::doc::project::Project::load(root)?;
    let (of, part) = crate::bom::of_part(&project, part)?;
    let project = match of {
        Some(p) => p,
        None => project.manifest.project.clone().ok_or_else(|| {
            Error::new(
                Code::ProjectIdMissing,
                "this project has no ID yet (`ryucad pdm init` gives it one), so the service can't know it",
            )
        })?,
    };
    let q = OrgQuery {
        project,
        part,
        label: label.map(crate::doc::assembly::label_text).transpose()?,
        direct,
    };
    where_used(&CAD.client()?, &q)
}

/// What `ryucad review` was asked to do.
pub enum Action {
    Status {
        pr: Option<u64>,
    },
    Threads {
        pr: Option<u64>,
        open: bool,
    },
    Comment {
        reference: String,
        text: String,
        pr: Option<u64>,
        commit: Option<String>,
    },
    Reply {
        thread: String,
        text: String,
    },
    Resolve {
        thread: String,
    },
    Reopen {
        thread: String,
    },
    Update {
        pr: Option<u64>,
    },
    Renders {
        pr: Option<u64>,
        out: std::path::PathBuf,
    },
    Approvals {
        pr: Option<u64>,
    },
    Audit {
        part: Option<String>,
        since: Option<String>,
        until: Option<String>,
        csv: bool,
        out: Option<std::path::PathBuf>,
    },
    /// A release's package (design 436): downloaded to `out`; the CLI
    /// verifies it against the clone after.
    PackageGet {
        part: String,
        label: String,
        out: std::path::PathBuf,
    },
    /// Build (or retry) a release's package on the service.
    PackageBuild {
        part: String,
        label: String,
    },
    /// An expiring link to a release's package for someone without access.
    PackageShare {
        part: String,
        label: String,
        days: Option<u64>,
    },
}

/// Runs `action` for the clone at `here`, asking the service.
pub fn run(here: &std::path::Path, action: Action) -> Result<Value> {
    use ryu_review::Action as Shared;
    let shared = match action {
        Action::Status { pr } => Shared::Status { pr },
        Action::Threads { pr, open } => Shared::Threads { pr, open },
        Action::Comment {
            reference,
            text,
            pr,
            commit,
        } => Shared::Comment {
            reference,
            text,
            pr,
            commit,
        },
        Action::Reply { thread, text } => Shared::Reply { thread, text },
        Action::Resolve { thread } => Shared::Resolve { thread },
        Action::Reopen { thread } => Shared::Reopen { thread },
        Action::Update { pr } => Shared::Update { pr },
        Action::Renders { pr, out } => Shared::Renders { pr, out },
        Action::Approvals { pr } => Shared::Approvals { pr },
        own => return run_own(here, own),
    };
    ryu_review::run::<crate::Cad>(here, &CAD, shared)
}

/// ryucad's own actions: audit and release packages.
fn run_own(here: &std::path::Path, action: Action) -> Result<Value> {
    let s = ryu_review::Session::<crate::Cad>::open(here, &CAD)?;
    let (client, slug, escape) = (&s.client, &s.slug, ryu_review::escape);
    let with = |v: Value, pr: Option<u64>| s.with(v, pr);
    match action {
        Action::PackageGet { part, label, out } => {
            let path = format!(
                "/repos/{slug}/releases/{}/{}/package",
                escape(&part),
                escape(&label)
            );
            let row = client.get(&path)?;
            let url = row["url"].as_str().ok_or_else(|| {
                Error::new(
                    Code::ReviewNotFound,
                    format!(
                        "{part} {label}'s package isn't built: it is {}",
                        row["state"].as_str().unwrap_or("unknown")
                    ),
                )
                .with("state", &row["state"])
            })?;
            let bytes = client.download_up_to(url, MAX_PACKAGE)?;
            crate::doc::source::write_atomic(&out, &bytes)?;
            Ok(
                json!({"repo": slug, "part": part, "label": label, "out": out,
                      "bytes": bytes.len(), "service": row}),
            )
        }
        Action::PackageBuild { part, label } => Ok(with(
            client.post(
                &format!(
                    "/repos/{slug}/releases/{}/{}/package",
                    escape(&part),
                    escape(&label)
                ),
                &json!({}),
            )?,
            None,
        )),
        Action::PackageShare { part, label, days } => {
            let mut body = json!({});
            if let Some(d) = days {
                body["days"] = json!(d);
            }
            Ok(with(
                client.post(
                    &format!(
                        "/repos/{slug}/releases/{}/{}/package/share",
                        escape(&part),
                        escape(&label)
                    ),
                    &body,
                )?,
                None,
            ))
        }
        Action::Audit {
            part,
            since,
            until,
            csv,
            out,
        } => {
            let mut q: Vec<String> = [("part", part), ("since", since), ("until", until)]
                .into_iter()
                .filter_map(|(k, v)| Some(format!("{k}={}", escape(&v?))))
                .collect();
            let csv = csv || out.is_some();
            if csv {
                q.push("format=csv".into());
            }
            let path = format!(
                "/repos/{slug}/audit{}{}",
                if q.is_empty() { "" } else { "?" },
                q.join("&")
            );
            if !csv {
                return Ok(with(client.get(&path)?, None));
            }
            let bytes = client.download(&format!("/api/v1{path}"))?;
            match out {
                Some(out) => {
                    crate::doc::source::write_atomic(&out, &bytes)?;
                    Ok(json!({"repo": slug, "out": out, "bytes": bytes.len()}))
                }
                None => Ok(json!({"repo": slug, "csv": String::from_utf8_lossy(&bytes)})),
            }
        }
        _ => unreachable!("the shared actions run through ryu_review::run"),
    }
}
