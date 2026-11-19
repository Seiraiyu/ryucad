//! A stand-in for the hosted review service, for products' tests (feature
//! `testing`): fixed answers by method and path (query included), answers
//! a handler makes, or the ledger's routes kept in memory (`Ledger`), and
//! a record of every request.

use serde_json::{Value, json};
use std::collections::BTreeMap;
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

/// What a handler answers: status, content type, body.
pub type Reply = (u16, &'static str, Vec<u8>);

/// `not_found`, as the service answers a path it hasn't.
pub fn not_found() -> Reply {
    (
        404,
        "application/json",
        br#"{"error":{"code":"not_found","message":"no such route"}}"#.to_vec(),
    )
}

pub struct Fake {
    pub url: String,
    pub seen: Arc<Mutex<Vec<Seen>>>,
}

impl Fake {
    /// Fixed answers by method and path (query included).
    pub fn start(routes: Vec<Route>) -> Fake {
        Fake::serve(move |s| {
            routes
                .iter()
                .find(|(m, p, ..)| *m == s.method && *p == s.path)
                .map(|(_, _, st, t, b)| (*st, *t, b.clone()))
                .unwrap_or_else(not_found)
        })
    }

    /// Answers made by `handler` from each request, one at a time, in the
    /// order they come: a stand-in that keeps state (`Ledger`).
    pub fn serve(mut handler: impl FnMut(&Seen) -> Reply + Send + 'static) -> Fake {
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
                let s = Seen {
                    method,
                    path,
                    auth,
                    body: String::from_utf8_lossy(&body).into_owned(),
                };
                let (status, ty, bytes) = handler(&s);
                log.lock().unwrap().push(s);
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

/// When every reservation the fake ledger makes expires.
pub const EXPIRES: &str = "2027-01-01T00:00:00Z";
/// Who the fake ledger says reserved a number.
pub const BY: &str = "alice";

/// One organization's ledger as ryupdm-service answers it (design 936,
/// ryuplm plan 2.A's routes), kept in memory for products' tests: under
/// `/api/v1/repos/<slug>/numbers`, `POST next`, `POST reserve`, `GET
/// {number}` and `GET ?catalogue=`. A sequence is keyed by its template
/// and never goes back; it starts above the floor and above every number
/// held that reads back through it. Every number handed out or reserved is
/// held for its catalogue, and a `bound` number (a test sets it) for an
/// item. Expiry isn't modelled: every reservation is live.
#[derive(Debug, Default)]
pub struct Ledger {
    slug: String,
    /// Template → the next sequence to hand out.
    pub sequences: BTreeMap<String, u64>,
    /// Number → the catalogue it is reserved to.
    pub reserved: BTreeMap<String, String>,
    /// Number → the (catalogue, item) a release branch holds it for.
    pub bound: BTreeMap<String, (String, String)>,
    /// When set, every call is refused with this status and service code.
    pub refuse: Option<(u16, String)>,
    /// How many `next` calls were answered.
    pub next_calls: usize,
}

impl Ledger {
    /// A stand-in answering `slug`'s (`acme/widgets`) numbers routes from
    /// one ledger, and that ledger, for a test to read and change.
    pub fn start(slug: &str) -> (Fake, Arc<Mutex<Ledger>>) {
        let ledger = Arc::new(Mutex::new(Ledger {
            slug: slug.to_owned(),
            ..Ledger::default()
        }));
        let state = Arc::clone(&ledger);
        let base = format!("/api/v1/repos/{slug}/numbers");
        let fake = Fake::serve(move |s| {
            let Some(rest) = s.path.strip_prefix(&base) else {
                return not_found();
            };
            let mut l = state.lock().unwrap();
            if let Some((status, code)) = &l.refuse {
                return reply(*status, &error(code, &format!("refused: {code}")));
            }
            let (status, body) = l.answer(&s.method, rest, &s.body);
            reply(status, &body)
        });
        (fake, ledger)
    }

    fn answer(&mut self, method: &str, rest: &str, body: &str) -> (u16, Value) {
        let b: Value = serde_json::from_str(body).unwrap_or(Value::Null);
        match (method, rest) {
            ("POST", "/next") => self.next(&b),
            ("POST", "/reserve") => self.reserve(&b),
            ("GET", r) if r.is_empty() || r.starts_with('?') => self.list(r.trim_start_matches('?')),
            ("GET", r) if r.len() > 1 && r.starts_with('/') => {
                let n = unescape(&r[1..]);
                (200, json!({"schema": 1, "number": n, "holders": self.holders(&n)}))
            }
            _ => (404, error("not_found", "no such API route")),
        }
    }

    fn next(&mut self, b: &Value) -> (u16, Value) {
        let (Some(cat), Some(text)) = (b["catalogue"].as_str(), b["template"].as_str()) else {
            return (400, error("invalid_request", "catalogue and template are required"));
        };
        let Some(t) = Template::new(text) else {
            return (400, error("invalid_request", "template needs one {seq:N}, N 1 to 12"));
        };
        let count = b["count"].as_u64().unwrap_or(1);
        if !(1..=100_000).contains(&count) {
            return (400, error("invalid_request", "count is 1 to 100000"));
        }
        let floor = b["floor"].as_u64().unwrap_or(0);
        let held = self
            .reserved
            .keys()
            .chain(self.bound.keys())
            .filter_map(|n| t.read(n))
            .max()
            .unwrap_or(0);
        let start = self
            .sequences
            .get(text)
            .copied()
            .unwrap_or(1)
            .max(floor + 1)
            .max(held + 1);
        let numbers: Vec<String> = (start..start + count).map(|s| t.fill(s)).collect();
        self.sequences.insert(text.to_owned(), start + count);
        for n in &numbers {
            self.reserved.insert(n.clone(), cat.to_owned());
        }
        self.next_calls += 1;
        (
            200,
            json!({"schema": 1, "catalogue": cat, "template": text, "numbers": numbers, "expires_at": EXPIRES}),
        )
    }

    fn reserve(&mut self, b: &Value) -> (u16, Value) {
        let Some(cat) = b["catalogue"].as_str() else {
            return (400, error("invalid_request", "catalogue is required"));
        };
        let mut numbers: Vec<String> = b["numbers"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|n| n.as_str().map(str::to_owned))
            .collect();
        numbers.sort();
        numbers.dedup();
        if numbers.is_empty() || numbers.len() > 1000 {
            return (400, error("invalid_request", "numbers holds 1 to 1000"));
        }
        let taken: Vec<Value> = numbers
            .iter()
            .filter_map(|n| {
                let holders: Vec<Value> = self
                    .holders(n)
                    .into_iter()
                    .filter(|h| h["catalogue"] != cat)
                    .collect();
                (!holders.is_empty()).then(|| json!({"number": n, "holders": holders}))
            })
            .collect();
        if !taken.is_empty() {
            let mut e = error(
                "part_number_taken",
                &format!("{} of these numbers are held for something else", taken.len()),
            );
            e["error"]["details"] = json!({"taken": taken});
            return (409, e);
        }
        for n in &numbers {
            self.reserved.insert(n.clone(), cat.to_owned());
        }
        (200, json!({"schema": 1, "catalogue": cat, "numbers": numbers, "expires_at": EXPIRES}))
    }

    /// Who holds `n`: its binding, then its reservation.
    fn holders(&self, n: &str) -> Vec<Value> {
        let mut out = Vec::new();
        if let Some((cat, item)) = self.bound.get(n) {
            out.push(json!({"kind": "bound", "repo": self.slug, "path": "", "catalogue": cat,
                            "item": item, "state": "bound"}));
        }
        if let Some(cat) = self.reserved.get(n) {
            out.push(json!({"kind": "reserved", "catalogue": cat, "by": BY, "expires_at": EXPIRES}));
        }
        out
    }

    /// The live reservations, of one catalogue when the query names it,
    /// 1,000 a page from `after`.
    fn list(&self, query: &str) -> (u16, Value) {
        let arg = |k: &str| {
            query
                .split('&')
                .filter_map(|kv| kv.split_once('='))
                .find(|(key, _)| *key == k)
                .map(|(_, v)| unescape(v))
        };
        let (cat, after) = (arg("catalogue"), arg("after").unwrap_or_default());
        let rows: Vec<Value> = self
            .reserved
            .iter()
            .filter(|(n, c)| n.as_str() > after.as_str() && cat.as_ref().is_none_or(|x| x == *c))
            .take(1000)
            .map(|(n, c)| json!({"number": n, "catalogue": c, "by": BY, "expires_at": EXPIRES}))
            .collect();
        let next = (rows.len() == 1000).then(|| rows[999]["number"].clone());
        (200, json!({"schema": 1, "reservations": rows, "after": next}))
    }
}

/// `{seq:N}` as ryupdm's `config::Template` reads it, for the fake alone
/// (ryu-review doesn't depend on ryupdm).
struct Template {
    before: String,
    width: usize,
    after: String,
}

impl Template {
    fn new(text: &str) -> Option<Template> {
        let start = text.find("{seq:")?;
        let rest = &text[start + 5..];
        let close = rest.find('}')?;
        let width: usize = rest[..close].parse().ok().filter(|w| (1..=12).contains(w))?;
        Some(Template {
            before: text[..start].to_owned(),
            width,
            after: rest[close + 1..].to_owned(),
        })
    }

    fn fill(&self, n: u64) -> String {
        format!("{}{:0w$}{}", self.before, n, self.after, w = self.width)
    }

    fn read(&self, number: &str) -> Option<u64> {
        let d = number.strip_prefix(&self.before)?.strip_suffix(&self.after)?;
        (d.len() >= self.width && d.bytes().all(|c| c.is_ascii_digit()))
            .then(|| d.parse().ok())
            .flatten()
    }
}

fn reply(status: u16, v: &Value) -> Reply {
    (status, "application/json", v.to_string().into_bytes())
}

fn error(code: &str, message: &str) -> Value {
    json!({"error": {"code": code, "message": message}})
}

/// `%XX` decoded, as a path segment or query value arrives.
fn unescape(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Ok(x) = u8::from_str_radix(&s[i + 1..i + 3], 16)
        {
            out.push(x);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}
