//! What every source of parts is to ryueda: a name, its terms, searches
//! and lookups by key, each under a cancel flag and a deadline. The
//! session holds each as a `Box<dyn Source>`; 6.B adds the JLC catalogue,
//! 6.C DigiKey, and a hosted adapter needs nothing here changed.

use crate::error::{Code, Error, Result};
use crate::query::{Hit, Query};
use crate::record::{Record, Terms};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

/// What a call may take: a cancel flag and a deadline, checked between
/// requests and pages.
#[derive(Clone, Copy, Debug, Default)]
pub struct Call<'a> {
    pub cancel: Option<&'a AtomicBool>,
    pub deadline: Option<Instant>,
}

impl Call<'_> {
    /// `cancelled` once the flag is set or the deadline passed.
    pub fn go_on(&self) -> Result<()> {
        if self.cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
            return Err(Error::new(Code::Cancelled, "cancelled"));
        }
        if self.deadline.is_some_and(|d| Instant::now() >= d) {
            return Err(Error::new(Code::Cancelled, "the call's deadline passed")
                .with("reason", "deadline"));
        }
        Ok(())
    }
}

pub trait Source: Send + Sync {
    /// `library`, `jlc`, `digikey`.
    fn name(&self) -> &str;
    fn terms(&self) -> Terms;
    /// Hits ranked (`query::rank`), at most `q.limit`.
    fn search(&self, q: &Query, call: &Call) -> Result<Vec<Hit>>;
    /// The record of `key` in this source, if it has one.
    fn get(&self, key: &str, call: &Call) -> Result<Option<Record>>;
}

/// A part named `<source>:<key>` (`lcsc:C1525`, `digikey:311-1338-1-ND`,
/// `mpn:CL05B104KO5NNNC`, which asks every source for the part number).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Key {
    pub source: String,
    pub key: String,
}

impl Key {
    pub fn parse(text: &str) -> Result<Key> {
        match text.split_once(':') {
            Some((s, k))
                if !s.is_empty()
                    && !k.is_empty()
                    && s.bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()) =>
            {
                Ok(Key {
                    source: s.to_owned(),
                    key: k.to_owned(),
                })
            }
            _ => Err(Error::new(
                Code::InvalidParam,
                format!(
                    "name a part as <source>:<key> (lcsc:C1525, mpn:CL05B104KO5NNNC), not {text:?}"
                ),
            )
            .with("key", text)),
        }
    }

    pub fn is_mpn(&self) -> bool {
        self.source == "mpn"
    }
}
