//! HTTPS for part catalogues: ureq on rustls, a minute's wait for an
//! answer or a body, the standard proxy environment (ureq's own reading),
//! a ryueda user agent. Every failure is `parts_source_unreachable` naming
//! the URL; nothing secret is in a URL this fetches.

use crate::error::Result;
use ryueda_parts::fetch::{Fetch, unreachable};
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
}
