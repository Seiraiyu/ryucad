//! The remote tier: a shared cache server (`ryucad.cache.1`, see
//! [`super::wire`]) behind the disk cache. A local miss asks it, and what
//! it answers is checked (header, identity, payload hash) before the disk
//! cache stores and uses it; what is built locally is sent behind
//! requests, never in their way.
//!
//! A remote never makes a request fail and never makes it slow: one that
//! doesn't answer within `cache.remote_timeout_ms`, answers with a server
//! error or refuses the token is skipped for a while (doubling up to five
//! minutes), with one warning per process, and an entry that fails its
//! checks is dropped and counted, never used. Only `cache push` reports
//! the remote's errors as its own.

use super::{Kind, check, wire};
use crate::error::{Code, Error, Result};
use serde_json::{Value, json};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// The longest a failing remote is skipped.
const MAX_BACKOFF: Duration = Duration::from_secs(300);

/// The most a `get` batch's answer may hold.
const MAX_BATCH_BYTES: u64 = 1 << 30;

/// How a remote is reached.
#[derive(Clone, Debug)]
pub struct Options {
    /// The server, `https://…` (or `http://` on loopback).
    pub url: String,
    pub token: Option<String>,
    /// Whether to send what is built (`cache.remote_write`).
    pub write: bool,
    /// `cache.remote_timeout_ms`.
    pub timeout: Duration,
    /// How long a remote is first skipped after a failure (then doubling).
    pub backoff: Duration,
    /// The identity entries must have been written by: the disk cache's.
    pub identity: [u8; 32],
}

/// What a remote has done, since the process started.
#[derive(Debug, Default)]
struct Counters {
    hits: AtomicU64,
    misses: AtomicU64,
    bytes_in: AtomicU64,
    bytes_out: AtomicU64,
    puts: AtomicU64,
    bad: AtomicU64,
}

#[derive(Debug, Default)]
struct Health {
    /// Skipped until then, after a failure.
    skip_until: Option<Instant>,
    /// How long the next failure skips it.
    backoff: Duration,
    /// The last failure, while it lasts.
    problem: Option<Error>,
    /// The codes already warned about.
    warned: Vec<Code>,
}

/// A shared cache server, as one process uses it.
#[derive(Debug)]
pub struct Remote {
    url: String,
    token: Option<String>,
    identity: [u8; 32],
    base_backoff: Duration,
    agent: ureq::Agent,
    /// Whether this machine sends what it builds: `cache.remote_write`,
    /// until the server says the token may not.
    write: AtomicBool,
    health: Mutex<Health>,
    counters: Counters,
    /// Whether the note about a bad entry has been given.
    noted: AtomicBool,
}

impl Remote {
    pub fn new(o: Options) -> Remote {
        let body = o.timeout.max(Duration::from_secs(60));
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_connect(Some(o.timeout))
            .timeout_recv_response(Some(o.timeout))
            .timeout_send_body(Some(body))
            .timeout_recv_body(Some(body))
            .user_agent(concat!("ryucad/", env!("CARGO_PKG_VERSION")))
            .build()
            .into();
        Remote {
            url: o.url.trim_end_matches('/').to_owned(),
            token: o.token,
            identity: o.identity,
            base_backoff: o.backoff,
            agent,
            write: AtomicBool::new(o.write),
            health: Mutex::new(Health {
                backoff: o.backoff,
                ..Health::default()
            }),
            counters: Counters::default(),
            noted: AtomicBool::new(false),
        }
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    /// Whether what is built is sent.
    pub fn writes(&self) -> bool {
        self.write.load(Ordering::Relaxed)
    }

    fn health(&self) -> std::sync::MutexGuard<'_, Health> {
        self.health.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Whether the remote is being asked (not skipped after a failure).
    pub fn available(&self) -> bool {
        self.health().skip_until.is_none_or(|t| Instant::now() >= t)
    }

    /// Skips the remote for a while after `e`, warning once per code.
    fn failed(&self, e: Error) {
        let mut h = self.health();
        let wait = match e.code {
            // A token won't start working by itself.
            Code::CacheRemoteRefused => MAX_BACKOFF,
            _ => h.backoff,
        };
        h.skip_until = Some(Instant::now() + wait);
        h.backoff = (h.backoff * 2).min(MAX_BACKOFF);
        if !h.warned.contains(&e.code) {
            h.warned.push(e.code);
            eprintln!(
                "ryucad: the shared cache is skipped for now ({}): {}",
                serde_json::to_value(e.code).unwrap_or_default(),
                e.message
            );
        }
        h.problem = Some(e);
    }

    fn succeeded(&self) {
        let mut h = self.health();
        h.skip_until = None;
        h.backoff = self.base_backoff;
        h.problem = None;
    }

    /// Counts an entry that failed its checks, noting it once.
    pub fn bad(&self, what: &str) {
        self.counters.bad.fetch_add(1, Ordering::Relaxed);
        if !self.noted.swap(true, Ordering::Relaxed) {
            eprintln!(
                "ryucad: dropped an entry from the shared cache at {} ({what}); it is never used",
                self.url
            );
        }
    }

    fn unreachable(&self, e: &dyn std::fmt::Display) -> Error {
        Error::new(
            Code::CacheRemoteUnreachable,
            format!("can't reach the shared cache at {}: {e}", self.url),
        )
        .with("url", &self.url)
    }

    /// The error for an answer's status, if it is one.
    fn refusal(&self, status: u16) -> Option<Error> {
        match status {
            401 => Some(
                Error::new(
                    Code::CacheRemoteRefused,
                    format!("the shared cache at {} refused the token", self.url),
                )
                .with("url", &self.url)
                .with("status", status),
            ),
            403 => Some(
                Error::new(
                    Code::CacheRemoteRefused,
                    format!(
                        "the token may not write to the shared cache at {}",
                        self.url
                    ),
                )
                .with("url", &self.url)
                .with("status", status),
            ),
            500.. => Some(
                self.unreachable(&format!("it answered {status}"))
                    .with("status", status),
            ),
            _ => None,
        }
    }

    fn request(&self, method: &str, path: &str) -> ureq::RequestBuilder<ureq::typestate::WithBody> {
        let mut r = match method {
            "PUT" => self.agent.put(format!("{}{path}", self.url)),
            _ => self.agent.post(format!("{}{path}", self.url)),
        };
        if let Some(t) = &self.token {
            r = r.header("Authorization", format!("Bearer {t}"));
        }
        r
    }

    /// Sends a request, giving the status and body (read up to `limit`).
    fn send(
        &self,
        r: core::result::Result<ureq::http::Response<ureq::Body>, ureq::Error>,
        limit: u64,
    ) -> Result<(u16, Vec<u8>)> {
        let mut resp = r.map_err(|e| self.unreachable(&e))?;
        let status = resp.status().as_u16();
        let body = resp
            .body_mut()
            .with_config()
            .limit(limit)
            .read_to_vec()
            .map_err(|e| self.unreachable(&e))?;
        self.counters
            .bytes_in
            .fetch_add(body.len() as u64, Ordering::Relaxed);
        match self.refusal(status) {
            Some(e) => Err(e),
            None => Ok((status, body)),
        }
    }

    /// Runs `f` unless the remote is skipped; a failure skips it.
    fn guarded<T>(&self, f: impl FnOnce() -> Result<T>) -> Option<T> {
        if !self.available() {
            return None;
        }
        match f() {
            Ok(v) => {
                self.succeeded();
                Some(v)
            }
            Err(e) => {
                self.failed(e);
                None
            }
        }
    }

    /// The entry of `kind` under `key`, checked, if the remote holds a
    /// sound one and answers.
    pub fn get(&self, kind: Kind, key: &str) -> Option<Vec<u8>> {
        let got = self.guarded(|| {
            let mut r = self
                .agent
                .get(format!("{}{}", self.url, wire::path(kind, key)));
            if let Some(t) = &self.token {
                r = r.header("Authorization", format!("Bearer {t}"));
            }
            self.send(r.call(), wire::MAX_ENTRY)
        })?;
        match got {
            (200, bytes) => self.checked(bytes),
            _ => {
                self.counters.misses.fetch_add(1, Ordering::Relaxed);
                None
            }
        }
    }

    /// `bytes` if they are a sound entry this build's identity wrote.
    fn checked(&self, bytes: Vec<u8>) -> Option<Vec<u8>> {
        if check(&bytes) == Some(self.identity) {
            self.counters.hits.fetch_add(1, Ordering::Relaxed);
            Some(bytes)
        } else {
            self.bad("its header, identity or hash is wrong");
            None
        }
    }

    /// Which of `names` the remote holds (`has`, in batches); `None` when
    /// it can't be asked.
    pub fn has(&self, names: &[String]) -> Option<Vec<String>> {
        let mut held = Vec::new();
        for chunk in names.chunks(wire::MAX_HAS) {
            let answer = self.guarded(|| {
                let body = serde_json::to_vec(chunk).expect("names serialize");
                self.send(
                    self.request("POST", "/v1/has")
                        .header("Content-Type", "application/json")
                        .send(&body[..]),
                    64 << 20,
                )
            })?;
            match answer {
                (200, bytes) => match serde_json::from_slice::<Vec<String>>(&bytes) {
                    Ok(v) => held.extend(v.into_iter().filter(|n| chunk.contains(n))),
                    Err(_) => {
                        self.failed(self.unreachable(&"its answer to has isn't a list of names"));
                        return None;
                    }
                },
                (s, _) => {
                    self.failed(self.unreachable(&format!("it answered has with {s}")));
                    return None;
                }
            }
        }
        Some(held)
    }

    /// The entries of `wanted` the remote holds, checked: `has` first, then
    /// `get` for what it holds, in batches. What it doesn't hold, or can't
    /// be asked for, is left out.
    pub fn fetch(&self, wanted: &[(Kind, String)]) -> Vec<(Kind, String, Vec<u8>)> {
        let names: Vec<String> = wanted.iter().map(|(k, key)| wire::name(*k, key)).collect();
        let Some(held) = self.has(&names) else {
            return Vec::new();
        };
        let misses = (names.len() - held.len()) as u64;
        self.counters.misses.fetch_add(misses, Ordering::Relaxed);
        let mut out = Vec::new();
        for chunk in held.chunks(wire::MAX_GET) {
            let Some(answer) = self.guarded(|| {
                let body = serde_json::to_vec(chunk).expect("names serialize");
                self.send(
                    self.request("POST", "/v1/get")
                        .header("Content-Type", "application/json")
                        .send(&body[..]),
                    MAX_BATCH_BYTES,
                )
            }) else {
                break;
            };
            let entries = match answer {
                (200, bytes) => wire::decode_batch(&bytes, chunk.len()),
                _ => None,
            };
            let Some(entries) = entries else {
                self.failed(self.unreachable(&"its answer to get isn't the entries asked for"));
                break;
            };
            for (name, e) in chunk.iter().zip(entries) {
                let (kind, key) = wire::parse_name(name).expect("names it was given");
                match e.and_then(|b| self.checked(b)) {
                    Some(b) => out.push((kind, key.to_owned(), b)),
                    None => {
                        self.counters.misses.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        }
        out
    }

    /// Sends one entry; whether the remote stored it (`false`: it held it
    /// already). Errors are the remote's, as `cache push` reports them; a
    /// 403 stops this process writing.
    pub fn put(&self, kind: Kind, key: &str, bytes: &[u8]) -> Result<bool> {
        let r = self.send(
            self.request("PUT", &wire::path(kind, key))
                .header("Content-Type", "application/octet-stream")
                .send(bytes),
            1 << 20,
        );
        match r {
            Ok((201, _)) | Ok((200, _)) => {
                self.counters.puts.fetch_add(1, Ordering::Relaxed);
                self.counters
                    .bytes_out
                    .fetch_add(bytes.len() as u64, Ordering::Relaxed);
                Ok(matches!(r, Ok((201, _))))
            }
            Ok((status, _)) => Err(Error::new(
                Code::CacheRemoteUnreachable,
                format!(
                    "the shared cache at {} refused an entry ({status})",
                    self.url
                ),
            )
            .with("url", &self.url)
            .with("status", status)
            .with("key", key)),
            Err(e) => {
                if e.details["status"] == 403 {
                    self.write.store(false, Ordering::Relaxed);
                }
                Err(e)
            }
        }
    }

    /// `GET /v1/info`: the protocol, what the server holds, and whether the
    /// token may write.
    pub fn info(&self) -> Result<Value> {
        let mut r = self.agent.get(format!("{}/v1/info", self.url));
        if let Some(t) = &self.token {
            r = r.header("Authorization", format!("Bearer {t}"));
        }
        let (status, body) = self.send(r.call(), 1 << 20)?;
        let info: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
        if status != 200 || info["protocol"] != wire::PROTOCOL {
            return Err(self.unreachable(&format!(
                "it doesn't speak {} (it answered {status})",
                wire::PROTOCOL
            )));
        }
        Ok(info)
    }

    /// What `cache.stats` says of the remote.
    pub fn stats(&self) -> Value {
        let c = &self.counters;
        let n = |a: &AtomicU64| a.load(Ordering::Relaxed);
        let h = self.health();
        json!({
            "url": self.url,
            "token": self.token.is_some(),
            "reachable": h.problem.is_none(),
            "write": self.writes(),
            "hits": n(&c.hits),
            "misses": n(&c.misses),
            "bytes_in": n(&c.bytes_in),
            "bytes_out": n(&c.bytes_out),
            "puts": n(&c.puts),
            "bad": n(&c.bad),
            "problem": h.problem,
        })
    }
}

#[cfg(test)]
#[path = "../../tests/fake_cache/mod.rs"]
pub(crate) mod fake;

#[cfg(test)]
pub(crate) mod tests {
    use super::fake::{Fake, READ, WRITE};
    use super::*;
    use crate::cache::entry;

    pub const ME: [u8; 32] = [3; 32];

    pub fn options(fake: &Fake, token: Option<&str>) -> Options {
        Options {
            url: format!("{}/", fake.url),
            token: token.map(str::to_owned),
            write: true,
            timeout: Duration::from_millis(500),
            backoff: Duration::from_millis(100),
            identity: ME,
        }
    }

    fn key(i: u32) -> String {
        format!("{:064x}", i)
    }

    #[test]
    fn entries_go_up_and_come_back_checked() {
        let fake = Fake::start();
        let r = Remote::new(options(&fake, Some(WRITE)));
        assert_eq!(r.info().unwrap()["write"], true);
        let e = entry(&ME, b"payload");
        assert!(r.put(Kind::Feature, &key(1), &e).unwrap(), "stored");
        assert!(!r.put(Kind::Feature, &key(1), &e).unwrap(), "held already");
        assert_eq!(r.get(Kind::Feature, &key(1)).as_deref(), Some(&e[..]));
        assert_eq!(r.get(Kind::Tool, &key(1)), None, "kinds are apart");
        assert_eq!(r.get(Kind::Feature, &key(2)), None);
        // Another identity's entry under this key is never used.
        let theirs = entry(&[4; 32], b"payload");
        r.put(Kind::Mesh, &key(5), &theirs).unwrap();
        assert_eq!(r.get(Kind::Mesh, &key(5)), None);
        // Nor one the server garbles.
        fake.with(|s| s.garble = true);
        assert_eq!(r.get(Kind::Feature, &key(1)), None);
        let s = r.stats();
        assert_eq!(
            (s["hits"].as_u64(), s["misses"].as_u64()),
            (Some(1), Some(2))
        );
        assert_eq!(s["bad"], 2);
        assert_eq!(s["puts"], 3);
        assert_eq!(s["reachable"], true);
        assert!(r.available(), "a bad entry doesn't skip the remote");
    }

    #[test]
    fn a_read_token_reads_and_may_not_write() {
        let fake = Fake::start();
        let w = Remote::new(options(&fake, Some(WRITE)));
        w.put(Kind::Tool, &key(7), &entry(&ME, b"t")).unwrap();
        let r = Remote::new(options(&fake, Some(READ)));
        assert_eq!(r.info().unwrap()["write"], false);
        assert!(r.get(Kind::Tool, &key(7)).is_some());
        let e = r.put(Kind::Tool, &key(8), &entry(&ME, b"t")).unwrap_err();
        assert_eq!(e.code, Code::CacheRemoteRefused);
        assert_eq!(e.details["status"], 403);
        assert!(!r.writes(), "it stops writing");
        // An entry the server finds unsound is refused as such.
        let mut broken = entry(&ME, b"t");
        broken[super::super::HEADER_LEN] ^= 1;
        let e = w.put(Kind::Tool, &key(9), &broken).unwrap_err();
        assert_eq!(e.details["status"], 400);
    }

    #[test]
    fn a_refused_token_skips_the_remote() {
        let fake = Fake::start();
        let r = Remote::new(options(&fake, Some("not-a-token")));
        assert_eq!(r.get(Kind::Feature, &key(1)), None);
        assert!(!r.available());
        assert_eq!(r.stats()["problem"]["code"], "cache_remote_refused");
        let asked = fake.count("GET", "/v1/");
        assert_eq!(r.get(Kind::Feature, &key(1)), None);
        assert_eq!(fake.count("GET", "/v1/"), asked, "not asked while skipped");
        let none = Remote::new(options(&fake, None));
        assert_eq!(none.get(Kind::Feature, &key(1)), None);
        assert_eq!(none.stats()["problem"]["code"], "cache_remote_refused");
        fake.with(|s| s.anonymous_read = true);
        let anyone = Remote::new(options(&fake, None));
        assert_eq!(anyone.get(Kind::Feature, &key(1)), None);
        assert!(anyone.available(), "a miss, read anonymously");
    }

    #[test]
    fn a_failing_remote_is_skipped_for_longer_each_time() {
        let fake = Fake::start();
        fake.with(|s| s.fail = Some(503));
        let r = Remote::new(options(&fake, Some(READ)));
        assert_eq!(r.get(Kind::Feature, &key(1)), None);
        assert_eq!(r.stats()["problem"]["code"], "cache_remote_unreachable");
        assert_eq!(r.stats()["reachable"], false);
        assert_eq!(r.health().backoff, Duration::from_millis(200), "doubled");
        let asked = fake.count("GET", "/v1/");
        assert_eq!(r.get(Kind::Feature, &key(1)), None);
        assert_eq!(fake.count("GET", "/v1/"), asked);
        std::thread::sleep(Duration::from_millis(150));
        fake.with(|s| s.fail = None);
        assert_eq!(r.get(Kind::Feature, &key(1)), None, "a miss now");
        assert!(r.available());
        assert_eq!(r.stats()["reachable"], true);
        assert_eq!(r.health().backoff, Duration::from_millis(100), "reset");
    }

    #[test]
    fn a_slow_remote_times_out() {
        let fake = Fake::start();
        fake.with(|s| s.delay = Duration::from_secs(3));
        let r = Remote::new(options(&fake, Some(READ)));
        let start = Instant::now();
        assert_eq!(r.get(Kind::Feature, &key(1)), None);
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "{:?}",
            start.elapsed()
        );
        assert!(!r.available());
        // Nothing listens there at all.
        let gone = Remote::new(Options {
            url: "http://127.0.0.1:1".to_owned(),
            ..options(&fake, Some(READ))
        });
        assert_eq!(gone.get(Kind::Feature, &key(1)), None);
        assert_eq!(gone.stats()["problem"]["code"], "cache_remote_unreachable");
    }

    #[test]
    fn batches_ask_what_is_held_then_get_it() {
        let fake = Fake::start();
        let w = Remote::new(options(&fake, Some(WRITE)));
        // 300 held of 600 asked: two `get`s of at most 256.
        let mut wanted = Vec::new();
        for i in 0..600 {
            if i % 2 == 0 {
                w.put(Kind::Mesh, &key(i), &entry(&ME, &i.to_le_bytes()))
                    .unwrap();
            }
            wanted.push((Kind::Mesh, key(i)));
        }
        let r = Remote::new(options(&fake, Some(READ)));
        let got = r.fetch(&wanted);
        assert_eq!(got.len(), 300);
        assert_eq!(got[1].1, key(2));
        assert_eq!(&got[1].2[super::super::HEADER_LEN..], &2u32.to_le_bytes());
        assert_eq!(fake.count("POST", "/v1/has"), 1);
        assert_eq!(fake.count("POST", "/v1/get"), 2);
        let s = r.stats();
        assert_eq!(
            (s["hits"].as_u64(), s["misses"].as_u64()),
            (Some(300), Some(300))
        );
        // Garbled in a batch: dropped, counted.
        fake.with(|s| s.garble = true);
        assert!(r.fetch(&wanted[..4]).is_empty());
        assert_eq!(r.stats()["bad"], 2);
        // Nothing asked for, nothing sent.
        assert!(r.fetch(&[]).is_empty());
        assert_eq!(fake.count("POST", "/v1/has"), 2);
    }
}
