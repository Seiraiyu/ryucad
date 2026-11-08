//! Distributor connectors (design 347, plan 6.C): live sources called with
//! each user's own keys, their records kept at most a day, never written
//! into a library, attributed. Each lives in its own module and is listed
//! here once, in `DISTRIBUTORS`; nothing else names it, so a distributor
//! is dropped or replaced by deleting its module and its line, as a
//! catalogue is (`catalogue::CONNECTORS`).

use crate::error::Result;
use crate::fetch::Fetch;
use crate::keys::{KeySpec, Keys};
use crate::rate::{Limiter, Limits};
use crate::record::{Record, Store, Terms};
use crate::source::{Call, Source};
use std::sync::Arc;

/// A live source: a `Source` that can also find a maker part number.
pub trait Live: Source {
    /// Every record whose maker part number is `mpn` (`record::squash`).
    fn by_mpn(&self, mpn: &str, call: &Call) -> Result<Vec<Record>>;
}

/// UTC seconds now, as a source's calls read it.
pub type Clock = Arc<dyn Fn() -> u64 + Send + Sync>;

/// What a distributor's source is opened with.
pub struct Opened {
    pub keys: Keys,
    pub fetch: Arc<dyn Fetch>,
    /// Where its API is (`base`, or a test's server).
    pub base: String,
    pub limiter: &'static Limiter,
    pub clock: Clock,
}

/// A distributor: its name (`--in <name>`, a key's `<name>:`), the
/// supplier names (lower case) whose offers' SKUs are its keys, its
/// attribution, how long its records keep, its rate limits, the keys it
/// needs, where its API is, and how to open its source.
pub struct Distributor {
    pub name: &'static str,
    pub suppliers: &'static [&'static str],
    pub attribution: &'static str,
    /// Seconds a record keeps in the cache.
    pub ttl: u64,
    pub limits: Limits,
    pub keys: KeySpec,
    /// The variable that moves its API (tests, a sandbox).
    pub url_var: &'static str,
    pub base: fn() -> String,
    pub open: fn(Opened) -> Box<dyn Live>,
}

impl Distributor {
    /// Kept at most `ttl`, never written into a library, attributed.
    pub fn terms(&self) -> Terms {
        Terms {
            store: Store::Ttl(self.ttl),
            write_parameters: false,
            attribution: self.attribution.to_owned(),
        }
    }
}

/// Every distributor this ryueda has.
pub const DISTRIBUTORS: &[Distributor] = &[crate::digikey::DISTRIBUTOR];

/// Every variable any distributor reads from the environment: its keys'
/// and the one moving its API. Test harnesses remove them all, so no test
/// ever takes a person's real keys or reaches a real distributor.
pub fn env_vars() -> impl Iterator<Item = &'static str> {
    DISTRIBUTORS.iter().flat_map(|d| {
        d.keys
            .fields
            .iter()
            .map(|f| f.var)
            .chain(std::iter::once(d.url_var))
    })
}

pub fn distributor(name: &str) -> Option<&'static Distributor> {
    DISTRIBUTORS.iter().find(|d| d.name == name)
}

/// The distributors whose keys a `<supplier>` (an offer's, or a key's
/// source) names, without regard to case.
pub fn for_supplier(supplier: &str) -> impl Iterator<Item = &'static Distributor> {
    let s = supplier.trim().to_ascii_lowercase();
    DISTRIBUTORS
        .iter()
        .filter(move |d| d.name == s || d.suppliers.contains(&s.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_vars_name_every_key_and_url_variable() {
        let vars: Vec<_> = env_vars().collect();
        for v in [
            "RYUEDA_DIGIKEY_CLIENT_ID",
            "RYUEDA_DIGIKEY_CLIENT_SECRET",
            "RYUEDA_DIGIKEY_URL",
        ] {
            assert!(vars.contains(&v), "{v} missing from {vars:?}");
        }
    }

    #[test]
    fn distributors_are_found_by_name_and_supplier() {
        let names = |s: &str| for_supplier(s).map(|d| d.name).collect::<Vec<_>>();
        assert_eq!(distributor("digikey").map(|d| d.name), Some("digikey"));
        assert!(distributor("jlc").is_none());
        assert_eq!(names("DigiKey"), ["digikey"]);
        assert_eq!(names(" Digi-Key "), ["digikey"]);
        assert!(names("LCSC").is_empty());
        let t = DISTRIBUTORS[0].terms();
        assert_eq!(t.store, Store::Ttl(86_400));
        assert!(!t.parameters_writable());
        // No distributor is also a catalogue.
        for d in DISTRIBUTORS {
            assert!(crate::catalogue::connector(d.name).is_none(), "{}", d.name);
        }
    }
}
