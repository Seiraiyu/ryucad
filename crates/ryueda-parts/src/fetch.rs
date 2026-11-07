//! What adapters fetch through: a URL's bytes under a call's cancel and
//! deadline, or (6.C) any request and whatever status answers it. ryueda
//! gives an HTTPS fetcher (ureq on rustls), tests a [`Fake`]; this crate
//! never opens a connection itself.
//!
//! A request's headers and body may hold secrets (a client id, a bearer
//! token, a client secret in a form), so nothing here puts them in an
//! error or a `Debug` line: only the method and the URL, and no URL holds
//! a secret.

use crate::error::{Code, Error, Result};
use crate::source::Call;
use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::sync::Mutex;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Method {
    Get,
    Post,
}

impl Method {
    pub fn as_str(self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Post => "POST",
        }
    }
}

/// One request. `Debug` writes its method, URL and header names, never
/// their values or the body.
#[derive(Clone, PartialEq, Eq)]
pub struct Request {
    pub method: Method,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl fmt::Debug for Request {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let names: Vec<&str> = self.headers.iter().map(|(k, _)| k.as_str()).collect();
        write!(
            f,
            "{} {} (headers {names:?}, {} bytes)",
            self.method.as_str(),
            self.url,
            self.body.len()
        )
    }
}

impl Request {
    pub fn get(url: &str) -> Request {
        Request {
            method: Method::Get,
            url: url.to_owned(),
            headers: Vec::new(),
            body: Vec::new(),
        }
    }

    pub fn post(url: &str, body: Vec<u8>) -> Request {
        Request {
            method: Method::Post,
            url: url.to_owned(),
            headers: Vec::new(),
            body,
        }
    }

    pub fn header(mut self, name: &str, value: &str) -> Request {
        self.headers.push((name.to_owned(), value.to_owned()));
        self
    }

    /// A header's value, its name compared without regard to case.
    pub fn header_value(&self, name: &str) -> Option<&str> {
        find(&self.headers, name)
    }
}

fn find<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

/// What answered: any status, so an adapter reads 401 and 429 itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Response {
    pub fn new(status: u16, body: impl Into<Vec<u8>>) -> Response {
        Response {
            status,
            headers: Vec::new(),
            body: body.into(),
        }
    }

    pub fn with_header(mut self, name: &str, value: &str) -> Response {
        self.headers.push((name.to_owned(), value.to_owned()));
        self
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        find(&self.headers, name)
    }

    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

pub trait Fetch: Send + Sync {
    /// The body of `url` once it answers 2xx; `parts_source_unreachable`
    /// otherwise (`status` in the details when it answered), `cancelled`
    /// when the call stops first.
    fn get(&self, url: &str, call: &Call) -> Result<Vec<u8>>;

    /// `req` sent and whatever answered it, any status;
    /// `parts_source_unreachable` only where nothing answered (naming the
    /// URL alone), `cancelled` when the call stops first.
    fn send(&self, req: &Request, call: &Call) -> Result<Response>;
}

/// `parts_source_unreachable` for `url`, `why` as said.
pub fn unreachable(url: &str, why: &str) -> Error {
    Error::new(Code::PartsSourceUnreachable, format!("{url}: {why}")).with("url", url)
}

/// URLs and their bodies, and answers queued by method and URL, for
/// tests; anything else answers 404.
#[derive(Default)]
pub struct Fake {
    bodies: BTreeMap<String, Vec<u8>>,
    calls: Mutex<Vec<String>>,
    answers: Mutex<BTreeMap<(Method, String), VecDeque<Response>>>,
    sent: Mutex<Vec<Request>>,
}

impl Fake {
    pub fn with(mut self, url: &str, body: Vec<u8>) -> Fake {
        self.bodies.insert(url.to_owned(), body);
        self
    }

    pub fn put(&mut self, url: &str, body: Vec<u8>) {
        self.bodies.insert(url.to_owned(), body);
    }

    /// Every URL asked for, in order.
    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }

    /// Queues `r` for `method` and `url`: answers go in order, the last
    /// one again for every request after.
    pub fn answer(&self, method: Method, url: &str, r: Response) {
        self.answers
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .entry((method, url.to_owned()))
            .or_default()
            .push_back(r);
    }

    /// Replaces every answer queued for `method` and `url` with `r`.
    pub fn set(&self, method: Method, url: &str, r: Response) {
        self.answers
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert((method, url.to_owned()), VecDeque::from([r]));
    }

    /// Every request sent, in order, headers and all (tests only).
    pub fn sent(&self) -> Vec<Request> {
        self.sent.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }
}

impl Fetch for Fake {
    fn get(&self, url: &str, call: &Call) -> Result<Vec<u8>> {
        call.go_on()?;
        self.calls
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(url.to_owned());
        self.bodies
            .get(url)
            .cloned()
            .ok_or_else(|| unreachable(url, "404 Not Found").with("status", 404))
    }

    fn send(&self, req: &Request, call: &Call) -> Result<Response> {
        call.go_on()?;
        self.calls
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(req.url.clone());
        self.sent
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(req.clone());
        let mut answers = self.answers.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(q) = answers.get_mut(&(req.method, req.url.clone())) {
            let r = if q.len() > 1 {
                q.pop_front()
            } else {
                q.front().cloned()
            };
            if let Some(r) = r {
                return Ok(r);
            }
        }
        Ok(match (req.method, self.bodies.get(&req.url)) {
            (Method::Get, Some(b)) => Response::new(200, b.clone()),
            _ => Response::new(404, Vec::new()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fake_serves_what_it_holds_and_counts_calls() {
        let f = Fake::default().with("https://x/a", b"one".to_vec());
        assert_eq!(f.get("https://x/a", &Call::default()).unwrap(), b"one");
        let e = f.get("https://x/b", &Call::default()).unwrap_err();
        assert_eq!(e.code, Code::PartsSourceUnreachable);
        assert_eq!(e.details["status"], 404);
        assert_eq!(f.calls(), ["https://x/a", "https://x/b"]);
        let stop = std::sync::atomic::AtomicBool::new(true);
        let call = Call {
            cancel: Some(&stop),
            deadline: None,
        };
        assert_eq!(
            f.get("https://x/a", &call).unwrap_err().code,
            Code::Cancelled
        );
        assert_eq!(
            unreachable("https://x/a", "refused").details["url"],
            "https://x/a"
        );
    }

    #[test]
    fn a_fake_answers_requests_in_order_and_keeps_them() {
        let f = Fake::default().with("https://x/a", b"one".to_vec());
        f.answer(Method::Post, "https://x/t", Response::new(401, "no"));
        f.answer(Method::Post, "https://x/t", Response::new(200, "yes"));
        let req = Request::post("https://x/t", b"secret=s3".to_vec())
            .header("Authorization", "Bearer t0k");
        let call = Call::default();
        assert_eq!(f.send(&req, &call).unwrap().status, 401);
        assert_eq!(f.send(&req, &call).unwrap().body, b"yes");
        assert_eq!(
            f.send(&req, &call).unwrap().body,
            b"yes",
            "the last answer repeats"
        );
        assert_eq!(
            f.send(&Request::get("https://x/a"), &call).unwrap().body,
            b"one"
        );
        assert_eq!(
            f.send(&Request::get("https://x/b"), &call).unwrap().status,
            404
        );
        assert_eq!(
            f.sent()[0].header_value("authorization"),
            Some("Bearer t0k")
        );
        // Secrets never show in a request's Debug.
        let d = format!("{req:?}");
        assert!(
            d.contains("POST https://x/t") && d.contains("Authorization"),
            "{d}"
        );
        assert!(!d.contains("t0k") && !d.contains("s3"), "{d}");
        f.set(Method::Post, "https://x/t", Response::new(503, ""));
        assert_eq!(
            f.send(&req, &call).unwrap().status,
            503,
            "set replaces the queue"
        );
        let r = Response::new(429, "").with_header("Retry-After", "7");
        assert_eq!((r.header("retry-after"), r.ok()), (Some("7"), false));
    }
}
