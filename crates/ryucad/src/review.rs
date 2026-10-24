//! The `ryucad review` client: a thin client of the hosted review
//! service's API (`/api/v1`, design 65), so an agent reads checks and
//! renders and comments on faces with the CLI it models with. CLI only:
//! it talks to a service, not to the session.
//!
//! The service's error codes become `review_*` codes, so they are stable
//! here too; one this build doesn't know is `review_failed` with the
//! service's code in the details.

use crate::error::{Code, Error, Result};
use serde_json::{Value, json};
use std::time::Duration;

/// Overrides the `review.url` preference.
pub const URL_VAR: &str = "RYUCAD_REVIEW_URL";
/// The GitHub token to send, before `GH_TOKEN`, `GITHUB_TOKEN` and
/// `gh auth token`.
pub const TOKEN_VAR: &str = "RYUCAD_REVIEW_TOKEN";

/// The largest artifact downloaded (renders are well under it).
const MAX_DOWNLOAD: u64 = 64 * 1024 * 1024;

pub struct Client {
    base: String,
    token: String,
    agent: ureq::Agent,
}

impl Client {
    /// A client of the service at `base` (`https://review.seiraiyu.com`).
    pub fn new(base: &str, token: &str) -> Client {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(60)))
            .user_agent(concat!("ryucad/", env!("CARGO_PKG_VERSION")))
            .build()
            .into();
        Client {
            base: base.trim_end_matches('/').to_owned(),
            token: token.to_owned(),
            agent,
        }
    }

    fn api(&self, path: &str) -> String {
        format!("{}/api/v1{path}", self.base)
    }

    fn bearer(&self) -> String {
        format!("Bearer {}", self.token)
    }

    /// `GET /api/v1{path}`.
    pub fn get(&self, path: &str) -> Result<Value> {
        let r = self
            .agent
            .get(&self.api(path))
            .header("Authorization", &self.bearer())
            .header("Accept", "application/json")
            .call();
        self.answer(r)
    }

    /// `POST /api/v1{path}` with a JSON body.
    pub fn post(&self, path: &str, body: &Value) -> Result<Value> {
        let r = self
            .agent
            .post(&self.api(path))
            .header("Authorization", &self.bearer())
            .header("Accept", "application/json")
            .header("Content-Type", "application/json")
            .send(body.to_string());
        self.answer(r)
    }

    /// The bytes at `url`: a path on the service (`/api/v1/artifacts/…`)
    /// or a full URL (a signed one it redirected to). The token goes only
    /// to the service itself.
    pub fn download(&self, url: &str) -> Result<Vec<u8>> {
        let full = if url.starts_with('/') {
            format!("{}{url}", self.base)
        } else {
            url.to_owned()
        };
        let mut req = self.agent.get(&full);
        if full.starts_with(&format!("{}/", self.base)) {
            req = req.header("Authorization", &self.bearer());
        }
        let mut resp = req.call().map_err(|e| self.unreachable(&e))?;
        let status = resp.status().as_u16();
        if !(200..300).contains(&status) {
            let text = resp.body_mut().read_to_string().unwrap_or_default();
            return Err(refusal(
                status,
                &serde_json::from_str(&text).unwrap_or(Value::Null),
            ));
        }
        resp.body_mut()
            .with_config()
            .limit(MAX_DOWNLOAD)
            .read_to_vec()
            .map_err(|e| self.unreachable(&e))
    }

    fn answer(
        &self,
        r: core::result::Result<ureq::http::Response<ureq::Body>, ureq::Error>,
    ) -> Result<Value> {
        let mut resp = r.map_err(|e| self.unreachable(&e))?;
        let status = resp.status().as_u16();
        let text = resp
            .body_mut()
            .read_to_string()
            .map_err(|e| self.unreachable(&e))?;
        let body: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        match status {
            202 => Err(refusal(
                202,
                &json!({"error": {"code": "pending",
                    "message": "the service is still working on it; try again shortly"}}),
            )),
            200..=299 => Ok(body),
            _ => Err(refusal(status, &body)),
        }
    }

    fn unreachable(&self, e: &dyn std::fmt::Display) -> Error {
        Error::new(
            Code::ReviewUnreachable,
            format!("can't reach the review service at {}: {e}", self.base),
        )
        .with("url", &self.base)
    }
}

/// The error for a refusal: the service's code as a `review_*` code, its
/// message, and its details under `service`.
pub fn refusal(status: u16, body: &Value) -> Error {
    let e = &body["error"];
    let code = e["code"].as_str().unwrap_or("");
    let ours = match code {
        "not_installed" => Code::ReviewNotInstalled,
        "not_found" => Code::ReviewNotFound,
        "forbidden" => Code::ReviewForbidden,
        "pending" => Code::ReviewPending,
        "head_moved" => Code::ReviewHeadMoved,
        "base_moved" => Code::ReviewBaseMoved,
        "fork_not_installed" => Code::ReviewForkNotInstalled,
        "limit_exceeded" => Code::ReviewLimitExceeded,
        "reference_invalid" => Code::ReviewReferenceInvalid,
        _ if status == 401 => Code::ReviewUnauthenticated,
        _ if status == 404 => Code::ReviewNotFound,
        _ => Code::ReviewFailed,
    };
    let message = e["message"].as_str().map_or_else(
        || format!("the review service answered {status}"),
        str::to_owned,
    );
    let mut err = Error::new(ours, message).with("status", status);
    if !code.is_empty() {
        err = err.with("service_code", code);
    }
    if !e["details"].is_null() {
        err = err.with("service", &e["details"]);
    }
    err
}

/// The token to send: `RYUCAD_REVIEW_TOKEN`, `GH_TOKEN`, `GITHUB_TOKEN`
/// (the first that isn't empty), else what `gh auth token` prints.
pub fn token() -> Result<String> {
    token_from(
        |k| std::env::var(k).ok(),
        || {
            let out = std::process::Command::new("gh")
                .args(["auth", "token"])
                .output()
                .ok()?;
            let t = String::from_utf8(out.stdout).ok()?.trim().to_owned();
            (out.status.success() && !t.is_empty()).then_some(t)
        },
    )
}

fn token_from(
    env: impl Fn(&str) -> Option<String>,
    gh: impl FnOnce() -> Option<String>,
) -> Result<String> {
    [TOKEN_VAR, "GH_TOKEN", "GITHUB_TOKEN"]
        .iter()
        .find_map(|k| env(k).filter(|v| !v.trim().is_empty()))
        .map(|v| v.trim().to_owned())
        .or_else(gh)
        .ok_or_else(|| {
            Error::new(
                Code::ReviewUnauthenticated,
                format!(
                    "no GitHub token: set {TOKEN_VAR} or GH_TOKEN, or log in with gh auth login"
                ),
            )
        })
}

/// The service's URL: `RYUCAD_REVIEW_URL`, else the `review.url`
/// preference.
pub fn base_url() -> String {
    std::env::var(URL_VAR)
        .ok()
        .filter(|u| !u.is_empty())
        .unwrap_or_else(|| {
            crate::config::user("review.url")
                .as_str()
                .unwrap_or("https://review.seiraiyu.com")
                .to_owned()
        })
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
}

/// Runs `action` for the clone at `here`, asking the service.
pub fn run(here: &std::path::Path, action: Action) -> Result<Value> {
    let repo = crate::git::repo(here)?;
    let origin = repo.origin_url();
    let (owner, name) = origin
        .as_deref()
        .and_then(crate::git::github_repo)
        .ok_or_else(|| {
            Error::new(
                Code::ReviewNoRemote,
                "the clone's origin remote isn't a GitHub repository",
            )
            .with("origin", &origin)
        })?;
    let slug = format!("{owner}/{name}");
    let client = Client::new(&base_url(), &token()?);
    let pull = |pr: Option<u64>| -> Result<u64> {
        if let Some(n) = pr {
            return Ok(n);
        }
        let branch = repo.branch()?.ok_or_else(|| {
            Error::new(
                Code::InvalidParam,
                "no branch checked out; name the pull request",
            )
        })?;
        let found = client.get(&format!("/repos/{slug}/pulls?branch={}", escape(&branch)))?;
        let numbers: Vec<u64> = found["pulls"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|p| p["number"].as_u64())
            .collect();
        match numbers[..] {
            [n] => Ok(n),
            [] => Err(Error::new(
                Code::ReviewNotFound,
                format!("no open pull request has {branch} as its head"),
            )
            .with("branch", &branch)),
            _ => Err(Error::new(
                Code::InvalidParam,
                format!("several pull requests have {branch} as their head; name one"),
            )
            .with("pulls", &numbers)),
        }
    };
    // Answers say which repository and pull request they are about.
    let with = |mut v: Value, pr: Option<u64>| {
        if let Some(o) = v.as_object_mut() {
            o.insert("repo".into(), json!(slug));
            if let Some(n) = pr {
                o.insert("pull".into(), json!(n));
            }
        }
        v
    };
    match action {
        Action::Status { pr } => {
            let n = pull(pr)?;
            Ok(with(
                client.get(&format!("/repos/{slug}/pulls/{n}"))?,
                Some(n),
            ))
        }
        Action::Threads { pr, open } => {
            let n = pull(pr)?;
            let q = if open { "?state=open" } else { "" };
            Ok(with(
                client.get(&format!("/repos/{slug}/pulls/{n}/threads{q}"))?,
                Some(n),
            ))
        }
        Action::Comment {
            reference,
            text,
            pr,
            commit,
        } => {
            let n = pull(pr)?;
            let mut body = json!({"ref": reference, "body": text});
            if let Some(c) = commit {
                body["commit"] = json!(c);
            }
            Ok(with(
                client.post(&format!("/repos/{slug}/pulls/{n}/threads"), &body)?,
                Some(n),
            ))
        }
        Action::Reply { thread, text } => Ok(with(
            client.post(
                &format!("/threads/{}/comments", escape(&thread)),
                &json!({"body": text}),
            )?,
            None,
        )),
        Action::Resolve { thread } => Ok(with(
            client.post(&format!("/threads/{}/resolve", escape(&thread)), &json!({}))?,
            None,
        )),
        Action::Reopen { thread } => Ok(with(
            client.post(&format!("/threads/{}/reopen", escape(&thread)), &json!({}))?,
            None,
        )),
        Action::Update { pr } => {
            let n = pull(pr)?;
            Ok(with(
                client.post(&format!("/repos/{slug}/pulls/{n}/merge-in"), &json!({}))?,
                Some(n),
            ))
        }
        Action::Renders { pr, out } => {
            let n = pull(pr)?;
            let status = client.get(&format!("/repos/{slug}/pulls/{n}"))?;
            std::fs::create_dir_all(&out).map_err(|e| Error::io(&out, &e))?;
            let mut files = Vec::new();
            for r in status["renders"].as_array().into_iter().flatten() {
                let (Some(url), Some(part), Some(side)) =
                    (r["url"].as_str(), r["part"].as_str(), r["side"].as_str())
                else {
                    continue;
                };
                let project = match r["project"].as_str().unwrap_or("") {
                    "" | "." => "root".to_owned(),
                    p => p.replace(['/', '\\'], "_"),
                };
                let path = out.join(format!("{project}-{part}-{side}.png"));
                let bytes = client.download(url)?;
                std::fs::write(&path, bytes).map_err(|e| Error::io(&path, &e))?;
                files.push(path);
            }
            Ok(json!({"repo": slug, "pull": n, "files": files}))
        }
    }
}

/// Percent-encodes a path segment or query value.
fn escape(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |k| {
            pairs
                .iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| (*v).to_owned())
        }
    }

    #[test]
    fn the_token_comes_from_the_first_place_that_has_one() {
        let no_gh = || None;
        assert_eq!(
            token_from(env(&[(TOKEN_VAR, "a"), ("GH_TOKEN", "b")]), no_gh).unwrap(),
            "a"
        );
        assert_eq!(
            token_from(env(&[(TOKEN_VAR, ""), ("GH_TOKEN", "b")]), no_gh).unwrap(),
            "b"
        );
        assert_eq!(
            token_from(env(&[("GITHUB_TOKEN", "c")]), no_gh).unwrap(),
            "c"
        );
        assert_eq!(token_from(env(&[]), || Some("d".to_owned())).unwrap(), "d");
        let e = token_from(env(&[]), no_gh).unwrap_err();
        assert_eq!(e.code, Code::ReviewUnauthenticated);
        assert!(e.message.contains("gh auth login"), "{}", e.message);
    }

    #[test]
    fn refusal_maps_codes() {
        let r = |s, c: &str| refusal(s, &json!({"error": {"code": c, "message": "m"}})).code;
        assert_eq!(r(403, "forbidden"), Code::ReviewForbidden);
        assert_eq!(r(400, "reference_invalid"), Code::ReviewReferenceInvalid);
        assert_eq!(r(409, "someday_new"), Code::ReviewFailed);
        assert_eq!(
            refusal(502, &Value::Null).message,
            "the review service answered 502"
        );
    }
}
