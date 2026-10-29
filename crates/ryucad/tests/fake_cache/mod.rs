//! A stand-in for a shared cache server: `ryucad.cache.1` (see
//! `ryucad::cache::wire`) over plain HTTP on loopback, entries in memory.
//! It is written apart from the client, from the protocol alone, so the
//! two check each other; ryupdm-service's server must answer as it does.
//! Knobs make it fail, stall or garble, and it counts what it was asked.

// Each test binary uses its own part of it.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The token that may read.
pub const READ: &str = "read-token";
/// The token that may write.
pub const WRITE: &str = "write-token";

#[derive(Debug)]
pub struct State {
    /// Entries by name (`<kind>/<key>`).
    pub entries: BTreeMap<String, Vec<u8>>,
    /// Tokens, and whether each may write.
    pub tokens: BTreeMap<String, bool>,
    pub anonymous_read: bool,
    /// Answer every request with this status.
    pub fail: Option<u16>,
    /// Wait this long before answering.
    pub delay: Duration,
    /// Flip a payload byte of every entry served.
    pub garble: bool,
    /// Answer `has` with each held name twice (a server that does is
    /// wrong, and mustn't confuse the client).
    pub has_twice: bool,
    /// The largest entry taken: the protocol's 256 MiB, smaller in tests.
    pub max_entry: u64,
    /// Every request: method and path.
    pub seen: Vec<(String, String)>,
}

pub struct Fake {
    pub url: String,
    pub state: Arc<Mutex<State>>,
}

impl Fake {
    pub fn start() -> Fake {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let state = Arc::new(Mutex::new(State {
            entries: BTreeMap::new(),
            tokens: [(READ.to_owned(), false), (WRITE.to_owned(), true)].into(),
            anonymous_read: false,
            fail: None,
            delay: Duration::ZERO,
            garble: false,
            has_twice: false,
            max_entry: 256 << 20,
            seen: Vec::new(),
        }));
        let shared = Arc::clone(&state);
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let state = Arc::clone(&shared);
                std::thread::spawn(move || {
                    let _ = serve(stream, &state);
                });
            }
        });
        Fake { url, state }
    }

    pub fn with<T>(&self, f: impl FnOnce(&mut State) -> T) -> T {
        f(&mut self.state.lock().unwrap())
    }

    /// How many requests of `method` to paths starting `prefix` it took.
    pub fn count(&self, method: &str, prefix: &str) -> usize {
        self.with(|s| {
            s.seen
                .iter()
                .filter(|(m, p)| m == method && p.starts_with(prefix))
                .count()
        })
    }

    pub fn held(&self) -> usize {
        self.with(|s| s.entries.len())
    }
}

fn valid_key(k: &str) -> bool {
    k.len() == 64 && k.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn valid_name(n: &str) -> bool {
    n.split_once('/').is_some_and(|(kind, key)| {
        ["features", "tools", "meshes"].contains(&kind) && valid_key(key)
    })
}

/// Whether an entry's bytes are sound: `ryucad.cache.2\n`, a 32-byte
/// identity, the payload's BLAKE3, its length (8 bytes, little-endian)
/// and the payload.
fn sound(bytes: &[u8]) -> bool {
    let Some(rest) = bytes.strip_prefix(b"ryucad.cache.2\n") else {
        return false;
    };
    if rest.len() < 72 {
        return false;
    }
    let (hash, len, payload) = (&rest[32..64], &rest[64..72], &rest[72..]);
    u64::from_le_bytes(len.try_into().unwrap()) == payload.len() as u64
        && blake3::hash(payload).as_bytes() == hash
}

fn garbled(mut bytes: Vec<u8>) -> Vec<u8> {
    if let Some(last) = bytes.last_mut() {
        *last ^= 1;
    }
    bytes
}

/// One request on one connection, answered and closed.
fn serve(stream: TcpStream, state: &Mutex<State>) -> std::io::Result<()> {
    let mut r = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    r.read_line(&mut line)?;
    let mut words = line.split_whitespace();
    let method = words.next().unwrap_or("").to_owned();
    let path = words.next().unwrap_or("").to_owned();
    let (mut len, mut auth) = (0usize, None::<String>);
    loop {
        let mut h = String::new();
        if r.read_line(&mut h)? == 0 {
            break;
        }
        let h = h.trim_end();
        if h.is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            match k.to_ascii_lowercase().as_str() {
                "content-length" => len = v.trim().parse().unwrap_or(0),
                "authorization" => auth = Some(v.trim().to_owned()),
                _ => {}
            }
        }
    }
    let mut body = vec![0; len];
    r.read_exact(&mut body)?;
    let (status, out) = answer(state, &method, &path, auth.as_deref(), body);
    let mut w = stream;
    write!(
        w,
        "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        out.len()
    )?;
    w.write_all(&out)?;
    w.flush()
}

fn answer(
    state: &Mutex<State>,
    method: &str,
    path: &str,
    auth: Option<&str>,
    body: Vec<u8>,
) -> (u16, Vec<u8>) {
    let (delay, fail) = {
        let mut s = state.lock().unwrap();
        s.seen.push((method.to_owned(), path.to_owned()));
        (s.delay, s.fail)
    };
    std::thread::sleep(delay);
    if let Some(code) = fail {
        return (code, Vec::new());
    }
    let mut s = state.lock().unwrap();
    let write = match auth.and_then(|a| a.strip_prefix("Bearer ")) {
        Some(t) => match s.tokens.get(t) {
            Some(w) => *w,
            None => return (401, Vec::new()),
        },
        None if s.anonymous_read => false,
        None => return (401, Vec::new()),
    };
    let names = |body: &[u8], max: usize| -> Option<Vec<String>> {
        let v: Vec<String> = serde_json::from_slice(body).ok()?;
        (v.len() <= max && v.iter().all(|n| valid_name(n))).then_some(v)
    };
    match (method, path) {
        ("GET", "/v1/info") => {
            let bytes: usize = s.entries.values().map(Vec::len).sum();
            let info = serde_json::json!({"protocol": "ryucad.cache.1",
                "entries": s.entries.len(), "bytes": bytes, "write": write});
            (200, info.to_string().into_bytes())
        }
        ("POST", "/v1/has") => match names(&body, 10_000) {
            Some(asked) => {
                let held: Vec<&String> = asked
                    .iter()
                    .filter(|n| s.entries.contains_key(*n))
                    .flat_map(|n| std::iter::repeat_n(n, if s.has_twice { 2 } else { 1 }))
                    .collect();
                (200, serde_json::to_vec(&held).unwrap())
            }
            None => (400, Vec::new()),
        },
        ("POST", "/v1/get") => match names(&body, 256) {
            Some(asked) => {
                let mut out = Vec::new();
                for n in &asked {
                    match s.entries.get(n) {
                        Some(e) => {
                            let e = if s.garble {
                                garbled(e.clone())
                            } else {
                                e.clone()
                            };
                            out.extend_from_slice(&(e.len() as u64).to_le_bytes());
                            out.extend_from_slice(&e);
                        }
                        None => out.extend_from_slice(&u64::MAX.to_le_bytes()),
                    }
                }
                (200, out)
            }
            None => (400, Vec::new()),
        },
        (m, p) if p.starts_with("/v1/") && (m == "GET" || m == "PUT") => {
            let name = &p[4..];
            if !valid_name(name) {
                return (400, Vec::new());
            }
            if m == "GET" {
                return match s.entries.get(name) {
                    Some(e) if s.garble => (200, garbled(e.clone())),
                    Some(e) => (200, e.clone()),
                    None => (404, Vec::new()),
                };
            }
            if !write {
                return (403, Vec::new());
            }
            if body.len() as u64 > s.max_entry {
                return (413, Vec::new());
            }
            if !sound(&body) {
                return (400, Vec::new());
            }
            match s.entries.insert(name.to_owned(), body) {
                Some(_) => (200, Vec::new()),
                None => (201, Vec::new()),
            }
        }
        _ => (404, Vec::new()),
    }
}
