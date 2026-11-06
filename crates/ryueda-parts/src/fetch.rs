//! What adapters fetch through: a URL's bytes under a call's cancel and
//! deadline. ryueda gives an HTTPS fetcher (ureq on rustls), tests a
//! [`Fake`]; this crate never opens a connection itself.

use crate::error::{Code, Error, Result};
use crate::source::Call;
use std::collections::BTreeMap;
use std::sync::Mutex;

pub trait Fetch: Send + Sync {
    /// The body of `url` once it answers 2xx; `parts_source_unreachable`
    /// otherwise (`status` in the details when it answered), `cancelled`
    /// when the call stops first.
    fn get(&self, url: &str, call: &Call) -> Result<Vec<u8>>;
}

/// `parts_source_unreachable` for `url`, `why` as said.
pub fn unreachable(url: &str, why: &str) -> Error {
    Error::new(Code::PartsSourceUnreachable, format!("{url}: {why}")).with("url", url)
}

/// URLs and their bodies, for tests; anything else answers 404.
#[derive(Default)]
pub struct Fake {
    bodies: BTreeMap<String, Vec<u8>>,
    calls: Mutex<Vec<String>>,
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
}
