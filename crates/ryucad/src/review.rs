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
