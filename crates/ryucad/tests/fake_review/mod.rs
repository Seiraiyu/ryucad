//! A stand-in for the hosted review service: fixed answers by method and
//! path (query included), and a record of every request.

// Each test binary uses its own part of it.
#![allow(dead_code)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug)]
pub struct Seen {
    pub method: String,
    pub path: String,
    pub auth: Option<String>,
    pub body: String,
}

/// An answer: method, path, status, content type, body.
pub type Route = (&'static str, String, u16, &'static str, Vec<u8>);

pub struct Fake {
    pub url: String,
    pub seen: Arc<Mutex<Vec<Seen>>>,
}

impl Fake {
    pub fn start(routes: Vec<Route>) -> Fake {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&seen);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut r = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                if r.read_line(&mut line).is_err() {
                    continue;
                }
                let mut words = line.split_whitespace();
                let method = words.next().unwrap_or("").to_owned();
                let path = words.next().unwrap_or("").to_owned();
                let (mut len, mut auth) = (0usize, None);
                loop {
                    let mut h = String::new();
                    if r.read_line(&mut h).unwrap_or(0) == 0 {
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
                let _ = r.read_exact(&mut body);
                log.lock().unwrap().push(Seen {
                    method: method.clone(),
                    path: path.clone(),
                    auth,
                    body: String::from_utf8_lossy(&body).into_owned(),
                });
                let (status, ty, bytes) = routes
                    .iter()
                    .find(|(m, p, ..)| *m == method && *p == path)
                    .map(|(_, _, s, t, b)| (*s, *t, b.clone()))
                    .unwrap_or((
                        404,
                        "application/json",
                        br#"{"error":{"code":"not_found","message":"no such route"}}"#.to_vec(),
                    ));
                let head = format!(
                    "HTTP/1.1 {status} X\r\ncontent-type: {ty}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                    bytes.len()
                );
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(&bytes);
            }
        });
        Fake { url, seen }
    }
}

/// A JSON answer.
pub fn json(method: &'static str, path: &str, status: u16, body: serde_json::Value) -> Route {
    (
        method,
        path.to_owned(),
        status,
        "application/json",
        body.to_string().into_bytes(),
    )
}
