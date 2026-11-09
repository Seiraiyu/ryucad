//! HTTPS for part catalogues: ureq on rustls, a minute's wait for an
//! answer or a body, the standard proxy environment (ureq's own reading),
//! a ryueda user agent. Every failure is `parts_source_unreachable` naming
//! the URL alone; nothing secret is in a URL this fetches, and a
//! request's headers and body (keys, tokens) are never in a message.

use crate::error::Result;
use ryueda_parts::fetch::{Fetch, Method, Request, Response, unreachable};
use ryueda_parts::source::Call;
use std::time::Duration;

/// The most one file may be.
pub const MAX_BODY: u64 = 512 << 20;
/// A call this quiet fails (design 347).
pub const QUIET: Duration = Duration::from_secs(60);

pub struct Https {
    agent: ureq::Agent,
}

impl Default for Https {
    fn default() -> Https {
        Https::new()
    }
}

impl Https {
    pub fn new() -> Https {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_connect(Some(Duration::from_secs(30)))
            .timeout_recv_response(Some(QUIET))
            .timeout_recv_body(Some(QUIET))
            .user_agent(concat!("ryueda/", env!("CARGO_PKG_VERSION")))
            .build()
            .into();
        Https { agent }
    }
}

impl Fetch for Https {
    fn get(&self, url: &str, call: &Call) -> Result<Vec<u8>> {
        call.go_on()?;
        let mut resp = self
            .agent
            .get(url)
            .call()
            .map_err(|e| unreachable(url, &e.to_string()))?;
        let status = resp.status().as_u16();
        if !(200..300).contains(&status) {
            return Err(unreachable(url, &format!("answered {status}")).with("status", status));
        }
        call.go_on()?;
        resp.body_mut()
            .with_config()
            .limit(MAX_BODY)
            .read_to_vec()
            .map_err(|e| unreachable(url, &e.to_string()))
    }

    fn send(&self, req: &Request, call: &Call) -> Result<Response> {
        call.go_on()?;
        let url = req.url.as_str();
        // ureq's errors name the URL and what failed, never a header.
        let sent = match req.method {
            Method::Get => {
                let mut r = self.agent.get(url);
                for (k, v) in &req.headers {
                    r = r.header(k.as_str(), v.as_str());
                }
                r.call()
            }
            Method::Post => {
                let mut r = self.agent.post(url);
                for (k, v) in &req.headers {
                    r = r.header(k.as_str(), v.as_str());
                }
                r.send(&req.body[..])
            }
        };
        let mut resp = sent.map_err(|e| unreachable(url, &e.to_string()))?;
        let status = resp.status().as_u16();
        let headers = resp
            .headers()
            .iter()
            .filter_map(|(k, v)| Some((k.as_str().to_owned(), v.to_str().ok()?.to_owned())))
            .collect();
        call.go_on()?;
        let body = resp
            .body_mut()
            .with_config()
            .limit(MAX_BODY)
            .read_to_vec()
            .map_err(|e| unreachable(url, &e.to_string()))?;
        Ok(Response {
            status,
            headers,
            body,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Code;
    use std::io::{Read, Write};

    /// Answers one request with `status` and `body`, on a thread.
    fn serve_once(status: &str, body: &'static [u8]) -> String {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let at = format!("http://{}/f", l.local_addr().unwrap());
        let status = status.to_owned();
        std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            let mut buf = [0u8; 4096];
            let _ = s.read(&mut buf);
            let head = format!(
                "HTTP/1.1 {status}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                body.len()
            );
            s.write_all(head.as_bytes()).unwrap();
            s.write_all(body).unwrap();
        });
        at
    }

    #[test]
    fn https_fetches_bodies_and_says_why_it_cant() {
        let f = Https::new();
        assert_eq!(
            f.get(&serve_once("200 OK", b"hello"), &Call::default())
                .unwrap(),
            b"hello"
        );
        let e = f
            .get(
                &serve_once("503 Service Unavailable", b""),
                &Call::default(),
            )
            .unwrap_err();
        assert_eq!(
            (e.code, &e.details["status"]),
            (Code::PartsSourceUnreachable, &serde_json::json!(503))
        );
        let e = f
            .get(&serve_once("404 Not Found", b""), &Call::default())
            .unwrap_err();
        assert_eq!(e.details["status"], 404);
        // Nothing listening.
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let dead = format!("http://{}/f", l.local_addr().unwrap());
        drop(l);
        let e = f.get(&dead, &Call::default()).unwrap_err();
        assert_eq!(
            (e.code, e.details["url"].as_str()),
            (Code::PartsSourceUnreachable, Some(dead.as_str()))
        );
        let stop = std::sync::atomic::AtomicBool::new(true);
        let call = Call {
            cancel: Some(&stop),
            deadline: None,
        };
        assert_eq!(f.get(&dead, &call).unwrap_err().code, Code::Cancelled);
    }

    /// Answers one request with `head` and `body`, sending back what it
    /// was asked (request line, headers and body) on the channel.
    fn echo_once(
        head: &'static str,
        body: &'static [u8],
    ) -> (String, std::sync::mpsc::Receiver<String>) {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let at = format!("http://{}/t", l.local_addr().unwrap());
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            let mut got = Vec::new();
            let mut buf = [0u8; 4096];
            // Headers, then a content-length's body.
            loop {
                let n = s.read(&mut buf).unwrap();
                got.extend_from_slice(&buf[..n]);
                let text = String::from_utf8_lossy(&got).to_string();
                if let Some(end) = text.find("\r\n\r\n") {
                    let len = text
                        .lines()
                        .find_map(|l| {
                            l.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(|v| v.trim().parse::<usize>().unwrap())
                        })
                        .unwrap_or(0);
                    if got.len() >= end + 4 + len || n == 0 {
                        break;
                    }
                }
            }
            tx.send(String::from_utf8_lossy(&got).to_string()).unwrap();
            let h = format!(
                "HTTP/1.1 {head}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                body.len()
            );
            s.write_all(h.as_bytes()).unwrap();
            s.write_all(body).unwrap();
        });
        (at, rx)
    }

    #[test]
    fn https_sends_requests_and_gives_any_status() {
        let f = Https::new();
        let (at, rx) = echo_once("429 Too Many Requests\r\nretry-after: 7", b"slow down");
        let req = Request::post(&at, b"a=1&b=2".to_vec())
            .header("Content-Type", "application/x-www-form-urlencoded")
            .header("Authorization", "Bearer secret-token");
        let r = f.send(&req, &Call::default()).unwrap();
        assert_eq!(
            (r.status, r.header("Retry-After"), r.body.as_slice()),
            (429, Some("7"), &b"slow down"[..])
        );
        let asked = rx.recv().unwrap();
        assert!(asked.starts_with("POST /t HTTP/1.1"), "{asked}");
        assert!(
            asked
                .to_ascii_lowercase()
                .contains("authorization: bearer secret-token"),
            "{asked}"
        );
        assert!(asked.ends_with("a=1&b=2"), "{asked}");
        let (at, rx) = echo_once("200 OK", b"{}");
        let r = f
            .send(
                &Request::get(&at).header("Accept", "application/json"),
                &Call::default(),
            )
            .unwrap();
        assert_eq!((r.status, r.body.as_slice()), (200, &b"{}"[..]));
        assert!(rx.recv().unwrap().starts_with("GET /t HTTP/1.1"));
        // Nothing listening: the URL is named, the headers aren't.
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let dead = format!("http://{}/t", l.local_addr().unwrap());
        drop(l);
        let e = f
            .send(
                &Request::get(&dead).header("Authorization", "Bearer secret-token"),
                &Call::default(),
            )
            .unwrap_err();
        assert_eq!(e.code, Code::PartsSourceUnreachable);
        assert!(!format!("{e} {}", e.details).contains("secret-token"));
    }
}
