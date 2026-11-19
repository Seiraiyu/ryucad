//! The hosted review service's client for every ryu product (design
//! 667): a thin client of its API (`/api/v1`, design 65), so an agent
//! reads checks and renders and comments with the CLI it models with.
//! Moved out of ryucad unchanged; ryucad and ryueda name themselves
//! through [`Product`]. CLI only: it talks to a service, not a session.
//!
//! The service's error codes become `review_*` codes, so they are stable
//! here too; one this build doesn't know is `review_failed` with the
//! service's code in the details.

use ryu_doc::error::{Code, Error, Result};
use serde_json::{Value, json};
use std::time::Duration;

#[cfg(feature = "testing")]
pub mod testing;

/// The service when no preference or variable names another.
pub const DEFAULT_URL: &str = "https://review.seiraiyu.com";
/// The preference naming the service, in each product's preferences.
pub const URL_PREF: &str = "review.url";
/// The largest artifact downloaded (renders are well under it).
const MAX_DOWNLOAD: u64 = 64 * 1024 * 1024;

/// A product as the client names it: its binary, version, variables and
/// where its preferences keep `review.url`.
pub struct Product {
    /// The binary (`ryucad`): the user agent's first half.
    pub name: &'static str,
    /// Its version: the user agent's second half.
    pub version: &'static str,
    /// Overrides the `review.url` preference (`RYUCAD_REVIEW_URL`).
    pub url_var: &'static str,
    /// The GitHub token to send before `GH_TOKEN`, `GITHUB_TOKEN` and
    /// `gh auth token` (`RYUCAD_REVIEW_TOKEN`).
    pub token_var: &'static str,
    /// The `review.url` preference, if the product's preferences set it.
    pub url_pref: fn() -> Option<String>,
}

impl Product {
    /// The service's URL: the product's variable, else its `review.url`
    /// preference, else [`DEFAULT_URL`].
    pub fn base_url(&self) -> String {
        std::env::var(self.url_var)
            .ok()
            .filter(|u| !u.is_empty())
            .or_else(self.url_pref)
            .unwrap_or_else(|| DEFAULT_URL.to_owned())
    }

    /// The token to send: the product's variable, `GH_TOKEN`,
    /// `GITHUB_TOKEN` (the first that isn't empty), else what `gh auth
    /// token` prints.
    pub fn token(&self) -> Result<String> {
        token_from(self.token_var, |k| std::env::var(k).ok(), gh_token)
    }

    /// A client of [`Product::base_url`] with [`Product::token`], its
    /// user agent `<name>/<version>`.
    pub fn client(&self) -> Result<Client> {
        Ok(Client::with_agent(
            &self.base_url(),
            &self.token()?,
            &format!("{}/{}", self.name, self.version),
        ))
    }
}

fn gh_token() -> Option<String> {
    let out = std::process::Command::new("gh")
        .args(["auth", "token"])
        .output()
        .ok()?;
    let t = String::from_utf8(out.stdout).ok()?.trim().to_owned();
    (out.status.success() && !t.is_empty()).then_some(t)
}

/// The token from `var`, `GH_TOKEN` or `GITHUB_TOKEN` as `env` reads
/// them, else `gh`.
pub fn token_from(
    var: &str,
    env: impl Fn(&str) -> Option<String>,
    gh: impl FnOnce() -> Option<String>,
) -> Result<String> {
    [var, "GH_TOKEN", "GITHUB_TOKEN"]
        .iter()
        .find_map(|k| env(k).filter(|v| !v.trim().is_empty()))
        .map(|v| v.trim().to_owned())
        .or_else(gh)
        .ok_or_else(|| {
            Error::new(
                Code::ReviewUnauthenticated,
                format!("no GitHub token: set {var} or GH_TOKEN, or log in with gh auth login"),
            )
        })
}

/// A client of the hosted review service: its URL, the token it sends
/// and the HTTP agent.
pub struct Client {
    base: String,
    token: String,
    agent: ureq::Agent,
}

impl Client {
    /// A client of the service at `base` (`https://review.seiraiyu.com`).
    pub fn new(base: &str, token: &str) -> Client {
        Client::with_agent(
            base,
            token,
            concat!("ryu-review/", env!("CARGO_PKG_VERSION")),
        )
    }

    /// [`Client::new`] naming itself `agent` (`ryucad/0.10.0`).
    pub fn with_agent(base: &str, token: &str, agent: &str) -> Client {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(60)))
            .user_agent(agent)
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
        self.download_up_to(url, MAX_DOWNLOAD)
    }

    /// [`Client::download`] of at most `limit` bytes.
    pub fn download_up_to(&self, url: &str, limit: u64) -> Result<Vec<u8>> {
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
            .limit(limit)
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
        "not_in_beta" => Code::ReviewNotInBeta,
        "invalid_request" => Code::ReviewInvalidRequest,
        "merge_unavailable" => Code::ReviewMergeUnavailable,
        "github_unreachable" => Code::ReviewGithubUnreachable,
        // The ledger's (ryuplm 2, design 936): its own code everywhere.
        "part_number_taken" => Code::PartNumberTaken,
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

/// Percent-encodes a path segment or query value.
pub fn escape(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// The GitHub owner and repository a remote URL names (https, ssh or
/// scp-like, with or without `.git`), or `None` for other hosts.
pub fn github_repo(url: &str) -> Option<(String, String)> {
    let url = url.trim();
    let path = [
        "https://github.com/",
        "http://github.com/",
        "ssh://git@github.com/",
        "git@github.com:",
    ]
    .iter()
    .find_map(|p| url.strip_prefix(p))?;
    let path = path.trim_end_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let (owner, repo) = path.split_once('/')?;
    let ok = |s: &str| {
        !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
    };
    (ok(owner) && ok(repo)).then(|| (owner.to_owned(), repo.to_owned()))
}

/// An https URL, or an http one to this machine (for tests and a local
/// service): a host, no user, no spaces or control characters.
pub fn url_ok(s: &str) -> bool {
    let (secure, rest) = match (s.strip_prefix("https://"), s.strip_prefix("http://")) {
        (Some(r), _) => (true, r),
        (_, Some(r)) => (false, r),
        _ => return false,
    };
    let authority = &rest[..rest.find('/').unwrap_or(rest.len())];
    if authority.is_empty()
        || authority.contains(['@', '?', '#'])
        || s.contains(' ')
        || s.chars().any(char::is_control)
    {
        return false;
    }
    secure
        || ["localhost", "127.0.0.1", "[::1]"].iter().any(|h| {
            authority == *h
                || authority
                    .strip_prefix(h)
                    .is_some_and(|p| p.starts_with(':'))
        })
}

/// `GET /api/v1/where-used`: where `part` of the project with ID
/// `project` is used across the organization's repositories the token
/// can read, at any revision or one `label`, `direct` only by what places
/// it itself; each row with its `repo` and `commit`. Any schema but 1 is
/// `review_failed`: this build reads only that one.
pub fn where_used(
    client: &Client,
    project: &str,
    part: &str,
    label: Option<&str>,
    direct: bool,
) -> Result<Value> {
    let mut path = format!(
        "/where-used?project={}&part={}",
        escape(project),
        escape(part)
    );
    if let Some(l) = label {
        path.push_str(&format!("&label={}", escape(l)));
    }
    if direct {
        path.push_str("&direct=true");
    }
    let v = client.get(&path)?;
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

/// What every product's `review` command asks (ryucad adds its own:
/// audit and packages).
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
}

/// A clone and the service: its repository, its GitHub `owner/name`, and
/// a client.
pub struct Session<L: ryu_git::Layout> {
    repo: ryu_git::repo::Repo<L>,
    pub slug: String,
    pub client: Client,
}

impl<L: ryu_git::Layout> Session<L> {
    /// The clone holding `here`: `not_a_git_repo` outside one,
    /// `review_no_remote` when its origin isn't a GitHub repository; the
    /// token is looked for after that.
    pub fn open(here: &std::path::Path, product: &Product) -> Result<Session<L>> {
        let repo = ryu_git::repo::repo::<L>(here)?;
        let origin = repo.origin_url();
        let (owner, name) = origin.as_deref().and_then(github_repo).ok_or_else(|| {
            Error::new(
                Code::ReviewNoRemote,
                "the clone's origin remote isn't a GitHub repository",
            )
            .with("origin", &origin)
        })?;
        Ok(Session {
            slug: format!("{owner}/{name}"),
            client: product.client()?,
            repo,
        })
    }

    /// The pull request: `pr`, else the one open with the checked-out
    /// branch as its head.
    pub fn pull(&self, pr: Option<u64>) -> Result<u64> {
        if let Some(n) = pr {
            return Ok(n);
        }
        let branch = self.repo.branch()?.ok_or_else(|| {
            Error::new(
                Code::InvalidParam,
                "no branch checked out; name the pull request",
            )
        })?;
        let found = self.client.get(&format!(
            "/repos/{}/pulls?branch={}",
            self.slug,
            escape(&branch)
        ))?;
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
    }

    /// `v` saying which repository and pull request it is about.
    pub fn with(&self, mut v: Value, pr: Option<u64>) -> Value {
        if let Some(o) = v.as_object_mut() {
            o.insert("repo".into(), json!(self.slug));
            if let Some(n) = pr {
                o.insert("pull".into(), json!(n));
            }
        }
        v
    }

    /// Runs `action`.
    pub fn act(&self, action: Action) -> Result<Value> {
        let (client, slug) = (&self.client, &self.slug);
        match action {
            Action::Status { pr } => {
                let n = self.pull(pr)?;
                Ok(self.with(client.get(&format!("/repos/{slug}/pulls/{n}"))?, Some(n)))
            }
            Action::Threads { pr, open } => {
                let n = self.pull(pr)?;
                let q = if open { "?state=open" } else { "" };
                Ok(self.with(
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
                let n = self.pull(pr)?;
                let mut body = json!({"ref": reference, "body": text});
                if let Some(c) = commit {
                    body["commit"] = json!(c);
                }
                Ok(self.with(
                    client.post(&format!("/repos/{slug}/pulls/{n}/threads"), &body)?,
                    Some(n),
                ))
            }
            Action::Reply { thread, text } => Ok(self.with(
                client.post(
                    &format!("/threads/{}/comments", escape(&thread)),
                    &json!({"body": text}),
                )?,
                None,
            )),
            Action::Resolve { thread } => Ok(self.with(
                client.post(&format!("/threads/{}/resolve", escape(&thread)), &json!({}))?,
                None,
            )),
            Action::Reopen { thread } => Ok(self.with(
                client.post(&format!("/threads/{}/reopen", escape(&thread)), &json!({}))?,
                None,
            )),
            Action::Update { pr } => {
                let n = self.pull(pr)?;
                Ok(self.with(
                    client.post(&format!("/repos/{slug}/pulls/{n}/merge-in"), &json!({}))?,
                    Some(n),
                ))
            }
            Action::Renders { pr, out } => {
                let n = self.pull(pr)?;
                let status = client.get(&format!("/repos/{slug}/pulls/{n}"))?;
                std::fs::create_dir_all(&out).map_err(|e| Error::io(&out, &e))?;
                let mut files = Vec::new();
                for r in status["renders"].as_array().into_iter().flatten() {
                    let (Some(url), Some(side)) = (r["url"].as_str(), r["side"].as_str()) else {
                        continue;
                    };
                    // A part (ryucad's rows), or a block's sheet (ryueda's).
                    let name = match (r["part"].as_str(), r["block"].as_str(), r["sheet"].as_str())
                    {
                        (Some(part), ..) => part.to_owned(),
                        (None, Some(block), Some(sheet)) => format!("{block}-{sheet}"),
                        (None, None, Some(sheet)) => sheet.to_owned(),
                        _ => continue,
                    };
                    let project = match r["project"].as_str().unwrap_or("") {
                        "" | "." => "root".to_owned(),
                        p => p.replace(['/', '\\'], "_"),
                    };
                    let ext = if r["format"] == "svg" { "svg" } else { "png" };
                    let path = out.join(format!("{project}-{name}-{side}.{ext}"));
                    let bytes = client.download(url)?;
                    std::fs::write(&path, bytes).map_err(|e| Error::io(&path, &e))?;
                    files.push(path);
                }
                Ok(json!({"repo": slug, "pull": n, "files": files}))
            }
            Action::Approvals { pr } => {
                let n = self.pull(pr)?;
                Ok(self.with(
                    client.get(&format!("/repos/{slug}/pulls/{n}/approvals"))?,
                    Some(n),
                ))
            }
        }
    }
}

/// [`Session::open`] and [`Session::act`].
pub fn run<L: ryu_git::Layout>(
    here: &std::path::Path,
    product: &Product,
    action: Action,
) -> Result<Value> {
    Session::<L>::open(here, product)?.act(action)
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
        let v = "RYUCAD_REVIEW_TOKEN";
        assert_eq!(
            token_from(
                v,
                env(&[("RYUCAD_REVIEW_TOKEN", "a"), ("GH_TOKEN", "b")]),
                no_gh
            )
            .unwrap(),
            "a"
        );
        assert_eq!(
            token_from(
                v,
                env(&[("RYUCAD_REVIEW_TOKEN", ""), ("GH_TOKEN", "b")]),
                no_gh
            )
            .unwrap(),
            "b"
        );
        assert_eq!(
            token_from(v, env(&[("GITHUB_TOKEN", "c")]), no_gh).unwrap(),
            "c"
        );
        assert_eq!(
            token_from(v, env(&[]), || Some("d".to_owned())).unwrap(),
            "d"
        );
        let e = token_from(v, env(&[]), no_gh).unwrap_err();
        assert_eq!(e.code, Code::ReviewUnauthenticated);
        assert!(
            e.message.contains("gh auth login") && e.message.contains(v),
            "{}",
            e.message
        );
        // Another product's variable is its own.
        assert_eq!(
            token_from(
                "RYUEDA_REVIEW_TOKEN",
                env(&[("RYUCAD_REVIEW_TOKEN", "a"), ("GH_TOKEN", "b")]),
                no_gh
            )
            .unwrap(),
            "b"
        );
    }

    #[test]
    fn refusal_maps_codes() {
        let r = |s, c: &str| refusal(s, &json!({"error": {"code": c, "message": "m"}})).code;
        assert_eq!(r(403, "forbidden"), Code::ReviewForbidden);
        assert_eq!(r(400, "reference_invalid"), Code::ReviewReferenceInvalid);
        assert_eq!(r(403, "not_in_beta"), Code::ReviewNotInBeta);
        assert_eq!(r(400, "invalid_request"), Code::ReviewInvalidRequest);
        assert_eq!(r(502, "github_unreachable"), Code::ReviewGithubUnreachable);
        assert_eq!(r(409, "part_number_taken"), Code::PartNumberTaken);
        let e = refusal(
            409,
            &json!({"error": {"code": "merge_unavailable", "message": "m",
                              "details": {"reason": "conflicts"}}}),
        );
        assert_eq!(
            (e.code, &e.details["service"]["reason"]),
            (Code::ReviewMergeUnavailable, &json!("conflicts"))
        );
        assert_eq!(r(409, "someday_new"), Code::ReviewFailed);
        assert_eq!(
            refusal(502, &Value::Null).message,
            "the review service answered 502"
        );
    }

    #[test]
    fn urls_are_https_or_this_machine() {
        assert!(url_ok("https://review.seiraiyu.com"));
        assert!(url_ok("http://127.0.0.1:8080"));
        assert!(!url_ok("http://example.com"));
        assert!(!url_ok("https://a@b.com"));
        assert!(!url_ok("ftp://x"));
    }

    #[test]
    fn github_repos_from_remote_urls() {
        assert_eq!(
            github_repo("git@github.com:o/r.git"),
            Some(("o".into(), "r".into()))
        );
        assert_eq!(
            github_repo("https://github.com/o/r"),
            Some(("o".into(), "r".into()))
        );
        assert_eq!(github_repo("https://example.com/o/r"), None);
    }
}
