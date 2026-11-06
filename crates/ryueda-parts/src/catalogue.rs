//! Catalogue connectors (design 347, and the owner's condition on plan
//! 387's question 248): a whole catalogue downloaded onto this machine and
//! searched offline. Each connector lives in its own module and is listed
//! here once, in `CONNECTORS`; nothing else names it, so a connector is
//! dropped or replaced by deleting its module and its line here.

use crate::error::Result;

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
pub type Progress<'a> = &'a mut dyn FnMut(usize, usize, &str) -> Result<()>;
