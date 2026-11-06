//! Catalogue connectors (design 347, and the owner's condition on plan
//! 387's question 248): a whole catalogue downloaded onto this machine and
//! searched offline. Each connector lives in its own module and is listed
//! here once, in `CONNECTORS`; nothing else names it, so a connector is
//! dropped or replaced by deleting its module and its line here.

use crate::error::Result;
use crate::fetch::Fetch;
use crate::record::Record;
use crate::source::{Call, Source};
use std::path::Path;
use std::sync::Arc;

pub trait Catalogue: Source {
    /// When this machine synced it, UTC seconds.
    fn fetched_at(&self) -> u64;
    /// The catalogue's own date, as it gives it.
    fn created(&self) -> &str;
    fn parts(&self) -> u64;
    /// Every record whose maker part number is `mpn` (`record::squash`).
    fn by_mpn(&self, mpn: &str) -> Result<Vec<Record>>;
}

/// What a sync reports, whatever the connector.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct Synced {
    pub parts: u64,
    pub files: u64,
    pub bytes: u64,
    /// Rows of the catalogue that didn't read, left out.
    pub skipped_rows: u64,
    pub created: String,
    pub fetched_at: u64,
}

/// `(done, total, item)` before each file a sync fetches (`total` 0 until
/// it is known); an `Err` (a cancel) stops the sync.
/// A synced catalogue, shared.
pub type Shared = Arc<dyn Catalogue>;

pub type Progress<'a> = &'a mut dyn FnMut(usize, usize, &str) -> Result<()>;

/// A connector: its name (`parts sync <name>`, `--in <name>`, a key's
/// `<name>:`), the supplier names (lower case) whose offers' SKUs are its
/// keys, its attribution, where it downloads from, and how to open and
/// sync its index in the parts cache's `<name>/`.
pub struct Connector {
    pub name: &'static str,
    pub suppliers: &'static [&'static str],
    pub attribution: &'static str,
    pub base: fn() -> String,
    pub open: fn(&Path) -> Result<Option<Shared>>,
    pub sync: fn(&dyn Fetch, &str, &Path, u64, &Call, Progress) -> Result<Synced>,
}

/// Every connector this ryueda has.
pub const CONNECTORS: &[Connector] = &[crate::jlc::CONNECTOR];

pub fn connector(name: &str) -> Option<&'static Connector> {
    CONNECTORS.iter().find(|c| c.name == name)
}

/// The connectors whose keys a `<supplier>` (an offer's, or a key's
/// source) names, without regard to case.
pub fn for_supplier(supplier: &str) -> impl Iterator<Item = &'static Connector> {
    let s = supplier.trim().to_ascii_lowercase();
    CONNECTORS
        .iter()
        .filter(move |c| c.name == s || c.suppliers.contains(&s.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connectors_are_found_by_name_and_supplier() {
        let names = |s: &str| for_supplier(s).map(|c| c.name).collect::<Vec<_>>();
        assert_eq!(connector("jlc").map(|c| c.name), Some("jlc"));
        assert!(connector("digikey").is_none());
        assert_eq!(names("LCSC"), ["jlc"]);
        assert_eq!(names(" jlc "), ["jlc"]);
        assert!(names("DigiKey").is_empty());
        let dir = tempfile::tempdir().unwrap();
        assert!((CONNECTORS[0].open)(dir.path()).unwrap().is_none());
    }
}
