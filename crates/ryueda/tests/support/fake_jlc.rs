//! A catalogue's site as `ryueda_parts::jlc::testing` writes it (our own
//! fixture, in jlcparts' shape), served over HTTP on a local port, for the
//! CLI's scenarios (`RYUEDA_JLC_URL`). Each file waits `delay` first.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::time::Duration;

pub struct Site {
    pub base: String,
}

pub fn start(created: &str, delay: Duration) -> Site {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    let files: Arc<Vec<(String, Vec<u8>)>> = Arc::new(
        ryueda_parts::jlc::testing::site(created)
            .into_iter()
            .map(|(u, b)| (u.rsplit('/').next().unwrap().to_owned(), b))
            .collect(),
    );
    std::thread::spawn(move || {
        for s in l.incoming().flatten() {
            let files = files.clone();
            std::thread::spawn(move || {
                let Ok(read) = s.try_clone() else { return };
                let mut r = BufReader::new(read);
                let mut line = String::new();
                if r.read_line(&mut line).is_err() {
                    return;
                }
                let path = line.split_whitespace().nth(1).unwrap_or("").to_owned();
                loop {
                    let mut h = String::new();
                    if r.read_line(&mut h).is_err() || h.trim().is_empty() {
                        break;
                    }
                }
                std::thread::sleep(delay);
                let name = path.rsplit('/').next().unwrap_or("");
                let mut s = s;
                match files.iter().find(|(n, _)| n == name) {
                    Some((_, body)) => {
                        let _ = write!(
                            s,
                            "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                            body.len()
                        );
                        let _ = s.write_all(body);
                    }
                    None => {
                        let _ = s.write_all(
                            b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
                        );
                    }
                }
            });
        }
    });
    Site {
        base: format!("http://{addr}/data"),
    }
}
