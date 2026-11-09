//! A fake DigiKey (`ryueda_parts::digikey::testing`'s answers, which we
//! wrote in its v4 shape), served over HTTP on a local port for the CLI's
//! scenarios (`RYUEDA_DIGIKEY_URL`): tokens for our test keys alone,
//! keyword searches and product details for bearers of its token, and
//! with `limited` every product call refused 429.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

pub struct Site {
    pub base: String,
    pub limited: Arc<AtomicBool>,
}

fn reply(s: &mut std::net::TcpStream, status: &str, extra: &str, body: &[u8]) {
    let _ = write!(
        s,
        "HTTP/1.1 {status}\r\n{extra}content-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        body.len()
    );
    let _ = s.write_all(body);
}

pub fn start() -> Site {
    use ryueda_parts::digikey::testing as t;
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", l.local_addr().unwrap());
    let limited = Arc::new(AtomicBool::new(false));
    let lim = limited.clone();
    std::thread::spawn(move || {
        for s in l.incoming().flatten() {
            let lim = lim.clone();
            std::thread::spawn(move || {
                let Ok(read) = s.try_clone() else { return };
                let mut r = BufReader::new(read);
                let mut line = String::new();
                if r.read_line(&mut line).is_err() {
                    return;
                }
                let mut parts = line.split_whitespace();
                let (method, path) = (
                    parts.next().unwrap_or("").to_owned(),
                    parts.next().unwrap_or("").to_owned(),
                );
                let (mut len, mut auth) = (0usize, String::new());
                loop {
                    let mut h = String::new();
                    if r.read_line(&mut h).is_err() || h.trim().is_empty() {
                        break;
                    }
                    let lower = h.to_ascii_lowercase();
                    if let Some(v) = lower.strip_prefix("content-length:") {
                        len = v.trim().parse().unwrap_or(0);
                    }
                    if lower.starts_with("authorization:") {
                        auth = h.trim().to_owned();
                    }
                }
                let mut body = vec![0u8; len];
                let _ = r.read_exact(&mut body);
                let mut s = s;
                let body = String::from_utf8_lossy(&body).to_string();
                if method == "POST" && path == "/v1/oauth2/token" {
                    let ours = body.contains("client_id=test-id")
                        && body.contains("client_secret=te%2Bst%26secret");
                    return if ours {
                        reply(&mut s, "200 OK", "", &t::token_answer(1).body);
                    } else {
                        reply(&mut s, "401 Unauthorized", "", b"{}");
                    };
                }
                if !auth.ends_with("Bearer tok-1") {
                    return reply(&mut s, "401 Unauthorized", "", b"{}");
                }
                if lim.load(Ordering::Relaxed) {
                    return reply(
                        &mut s,
                        "429 Too Many Requests",
                        "retry-after: 30\r\n",
                        b"{}",
                    );
                }
                if method == "POST" && path == "/products/v4/search/keyword" {
                    return reply(&mut s, "200 OK", "", &t::keyword_answer().body);
                }
                let fixture = match path.as_str() {
                    "/products/v4/search/1276-1001-1-ND/productdetails" => Some(t::CAP),
                    "/products/v4/search/296-1234-1-ND/productdetails" => Some(t::LDO),
                    _ => None,
                };
                match fixture {
                    Some(f) => reply(&mut s, "200 OK", "", &t::details_answer(f).body),
                    None => reply(&mut s, "404 Not Found", "", b"{}"),
                }
            });
        }
    });
    Site { base, limited }
}
