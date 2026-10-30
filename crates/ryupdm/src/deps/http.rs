//! Git's smart HTTP protocol over ureq, for fetching dependencies over
//! `https://` and `http://`: gix's own HTTP backends are curl (C) or
//! reqwest (tokio and aws-lc-rs), and the binary already carries ureq on
//! rustls with ring for `ryucad review`. A request is made when its
//! response is first read, after gix has written any body it posts, so
//! nothing here needs a thread. A 401 is gix's authentication error, so
//! gix asks Git's credential helpers and tries again.

use gix::bstr::BString;
use gix::protocol::transport::client::AuthenticationRequired;
use gix::protocol::transport::client::blocking_io::http::{
    GetResponse, Http, Options, PostBodyDataKind, PostResponse,
};
use std::io::{self, BufReader, Read, Write};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// An HTTP client for gix's transport.
pub struct Ureq {
    agent: ureq::Agent,
    /// `http.extraHeader` lines from Git's configuration.
    extra: Vec<String>,
    /// The base URL a redirect moved the repository to.
    base: Arc<Mutex<Option<String>>>,
}

/// How long one read or write may wait before a fetch fails as stalled:
/// a remote that takes the connection and stops answering (a proxy, a
/// half-dead host) would otherwise hold the fetch, and with it every
/// request waiting on the daemon, forever. Git's servers send progress
/// while they work, so a healthy fetch is never this quiet.
pub const STALL: Duration = Duration::from_secs(60);

impl Default for Ureq {
    fn default() -> Ureq {
        Ureq::stalling_after(STALL)
    }
}

impl Ureq {
    /// A client whose reads and writes fail after waiting `stall`.
    pub fn stalling_after(stall: Duration) -> Ureq {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_connect(Some(Duration::from_secs(20)))
            .timeout_per_read(Some(stall))
            .timeout_per_write(Some(stall))
            .user_agent(concat!("git/2.0 (ryupdm/", env!("CARGO_PKG_VERSION"), ")"))
            .build()
            .into();
        Ureq {
            agent,
            extra: Vec::new(),
            base: Arc::new(Mutex::new(None)),
        }
    }
}

/// One request, made when its response is first read.
struct Call {
    agent: ureq::Agent,
    url: String,
    /// What follows the base URL in `url`, so a redirected base can be
    /// read off the final URL.
    tail: String,
    headers: Vec<String>,
    /// The body to post; `None` for a `GET`.
    post: Option<Vec<u8>>,
    base: Arc<Mutex<Option<String>>>,
    done: Option<Done>,
}

enum Done {
    Ok {
        headers: io::Cursor<Vec<u8>>,
        body: Box<dyn Read + Send>,
    },
    /// The error, given once; then the readers are at their end.
    Err(Option<io::Error>),
}

impl Call {
    fn run(&mut self) {
        if self.done.is_some() {
            return;
        }
        let url = match self.base.lock().expect("not poisoned").as_deref() {
            Some(b) => format!("{b}{}", self.tail),
            None => self.url.clone(),
        };
        let pairs = self
            .headers
            .iter()
            .filter_map(|h| h.split_once(':'))
            .map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()));
        let sent = match self.post.take() {
            Some(body) => {
                let mut req = self.agent.post(&url);
                for (k, v) in pairs {
                    req = req.header(k, v);
                }
                req.send(&body[..])
            }
            None => {
                let mut req = self.agent.get(&url);
                for (k, v) in pairs {
                    req = req.header(k, v);
                }
                req.call()
            }
        };
        self.done = Some(match sent {
            Err(e) => Done::Err(Some(io::Error::other(e))),
            Ok(res) => {
                use ureq::ResponseExt;
                let status = res.status().as_u16();
                let at = res.get_uri().to_string();
                if at != url
                    && let Some(base) = at.strip_suffix(&self.tail)
                {
                    *self.base.lock().expect("not poisoned") = Some(base.to_owned());
                }
                if status == 401 {
                    let www_authenticate: Vec<BString> = res
                        .headers()
                        .get_all("www-authenticate")
                        .iter()
                        .map(|v| v.as_bytes().into())
                        .collect();
                    Done::Err(Some(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        AuthenticationRequired { www_authenticate },
                    )))
                } else if !(200..300).contains(&status) {
                    Done::Err(Some(io::Error::other(format!(
                        "{url} answered HTTP {status}"
                    ))))
                } else {
                    let mut text = Vec::new();
                    for (name, value) in res.headers() {
                        text.extend_from_slice(name.as_str().as_bytes());
                        text.push(b':');
                        text.extend_from_slice(value.as_bytes());
                        text.push(b'\n');
                    }
                    Done::Ok {
                        headers: io::Cursor::new(text),
                        body: Box::new(res.into_body().into_reader()),
                    }
                }
            }
        });
    }
}

/// A response's headers, one `name:value` line each.
pub struct Headers(Arc<Mutex<Call>>);

/// A response's body.
pub struct Body(Arc<Mutex<Call>>);

/// What gix posts, gathered until it reads the response.
pub struct Post(Arc<Mutex<Call>>);

impl Read for Headers {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let mut c = self.0.lock().expect("not poisoned");
        c.run();
        match c.done.as_mut().expect("run") {
            Done::Ok { headers, .. } => headers.read(buf),
            Done::Err(e) => e.take().map_or(Ok(0), Err),
        }
    }
}

impl Read for Body {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let mut c = self.0.lock().expect("not poisoned");
        c.run();
        match c.done.as_mut().expect("run") {
            Done::Ok { body, .. } => body.read(buf),
            Done::Err(e) => e.take().map_or(Ok(0), Err),
        }
    }
}

impl Write for Post {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut c = self.0.lock().expect("not poisoned");
        match c.post.as_mut() {
            Some(body) => body.extend_from_slice(buf),
            None => return Err(io::Error::other("the request was already sent")),
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Ureq {
    fn call(
        &self,
        url: &str,
        base_url: &str,
        headers: impl IntoIterator<Item = impl AsRef<str>>,
        post: Option<Vec<u8>>,
    ) -> Arc<Mutex<Call>> {
        let mut lines: Vec<String> = headers.into_iter().map(|h| h.as_ref().to_owned()).collect();
        lines.extend(self.extra.iter().cloned());
        Arc::new(Mutex::new(Call {
            agent: self.agent.clone(),
            url: url.to_owned(),
            tail: url.strip_prefix(base_url).unwrap_or_default().to_owned(),
            headers: lines,
            post,
            base: Arc::clone(&self.base),
            done: None,
        }))
    }
}

impl Http for Ureq {
    type Headers = BufReader<Headers>;
    type ResponseBody = BufReader<Body>;
    type PostBody = Post;

    fn get(
        &mut self,
        url: &str,
        base_url: &str,
        headers: impl IntoIterator<Item = impl AsRef<str>>,
    ) -> gix::Result<GetResponse<Self::Headers, Self::ResponseBody>> {
        let c = self.call(url, base_url, headers, None);
        Ok(GetResponse {
            headers: BufReader::new(Headers(Arc::clone(&c))),
            body: BufReader::new(Body(c)),
        })
    }

    fn post(
        &mut self,
        url: &str,
        base_url: &str,
        headers: impl IntoIterator<Item = impl AsRef<str>>,
        _body: PostBodyDataKind,
    ) -> gix::Result<PostResponse<Self::Headers, Self::ResponseBody, Self::PostBody>> {
        let c = self.call(url, base_url, headers, Some(Vec::new()));
        Ok(PostResponse {
            post_body: Post(Arc::clone(&c)),
            headers: BufReader::new(Headers(Arc::clone(&c))),
            body: BufReader::new(Body(c)),
        })
    }

    fn configure(&mut self, config: &dyn std::any::Any) -> gix::Result<()> {
        if let Some(o) = config.downcast_ref::<Options>() {
            self.extra = o.extra_headers.clone();
        }
        Ok(())
    }

    fn redirected_base_url(&self) -> Option<String> {
        self.base.lock().expect("not poisoned").clone()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};

    /// Serves the repositories under `root` over Git's smart HTTP on a
    /// loopback port, through `git http-backend`, until the test ends;
    /// returns the base URL. A path under `/private/` answers 401.
    pub(crate) fn serve(root: &Path) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let root: PathBuf = root.to_owned();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let root = root.clone();
                std::thread::spawn(move || {
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    let mut parts = line.split_whitespace();
                    let method = parts.next().unwrap_or_default().to_owned();
                    let target = parts.next().unwrap_or_default().to_owned();
                    let mut length = 0usize;
                    let mut headers: Vec<(String, String)> = Vec::new();
                    loop {
                        let mut h = String::new();
                        reader.read_line(&mut h).unwrap();
                        let h = h.trim_end();
                        if h.is_empty() {
                            break;
                        }
                        if let Some((k, v)) = h.split_once(':') {
                            let (k, v) = (k.trim().to_ascii_lowercase(), v.trim().to_owned());
                            if k == "content-length" {
                                length = v.parse().unwrap();
                            }
                            headers.push((k, v));
                        }
                    }
                    let mut body = vec![0; length];
                    reader.read_exact(&mut body).unwrap();
                    if target.starts_with("/private/") {
                        stream
                            .write_all(b"HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Basic realm=\"x\"\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                            .unwrap();
                        return;
                    }
                    let (path, query) = target.split_once('?').unwrap_or((&target, ""));
                    let header = |k: &str| {
                        headers
                            .iter()
                            .find(|(n, _)| n == k)
                            .map(|(_, v)| v.clone())
                            .unwrap_or_default()
                    };
                    let mut child = Command::new("git")
                        .arg("http-backend")
                        .env("GIT_PROJECT_ROOT", &root)
                        .env("GIT_HTTP_EXPORT_ALL", "1")
                        .env("REQUEST_METHOD", &method)
                        .env("PATH_INFO", path)
                        .env("QUERY_STRING", query)
                        .env("CONTENT_TYPE", header("content-type"))
                        .env("CONTENT_LENGTH", length.to_string())
                        .env("GIT_PROTOCOL", header("git-protocol"))
                        .env("HTTP_GIT_PROTOCOL", header("git-protocol"))
                        .stdin(Stdio::piped())
                        .stdout(Stdio::piped())
                        .spawn()
                        .unwrap();
                    child.stdin.take().unwrap().write_all(&body).unwrap();
                    let out = child.wait_with_output().unwrap().stdout;
                    let split = out
                        .windows(4)
                        .position(|w| w == b"\r\n\r\n")
                        .map(|i| (i, 4))
                        .or_else(|| out.windows(2).position(|w| w == b"\n\n").map(|i| (i, 2)))
                        .unwrap();
                    let (head, rest) = (&out[..split.0], &out[split.0 + split.1..]);
                    let head = String::from_utf8_lossy(head);
                    let mut status = "200 OK".to_owned();
                    let mut reply = String::new();
                    for h in head.lines() {
                        match h.strip_prefix("Status:") {
                            Some(s) => status = s.trim().to_owned(),
                            None => reply.push_str(&format!("{h}\r\n")),
                        }
                    }
                    let msg = format!(
                        "HTTP/1.1 {status}\r\n{reply}Content-Length: {}\r\nConnection: close\r\n\r\n",
                        rest.len()
                    );
                    stream.write_all(msg.as_bytes()).unwrap();
                    stream.write_all(rest).unwrap();
                });
            }
        });
        format!("http://127.0.0.1:{port}")
    }
}
