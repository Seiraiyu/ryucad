//! The JLC catalogue from jlcparts (design 347), a catalogue connector:
//! its published files (format 4) read into records, ryueda's own index
//! of them in the cache, a sync that builds a new index beside the old and
//! swaps it in, and the `jlc` source over the index. Nothing outside this
//! module names it but `catalogue::CONNECTORS`.

pub mod attr;
pub mod category;
pub mod index;
pub mod rows;
pub mod source;
#[cfg(any(test, feature = "testing"))]
pub mod testing;
pub use source::Jlc;
pub mod sync;
pub mod wire;

/// Where jlcparts publishes its files; `RYUEDA_JLC_URL` moves it (tests).
pub const BASE: &str = "https://yaqwsx.github.io/jlcparts/data";
pub const URL_VAR: &str = "RYUEDA_JLC_URL";
/// The one jlcparts format this reads.
pub const FORMAT: u64 = 4;
pub const NAME: &str = "jlc";
pub const ATTRIBUTION: &str = "Data from jlcparts (yaqwsx/jlcparts), from JLCPCB's parts list";

/// jlcparts' base URL: `RYUEDA_JLC_URL`, else [`BASE`].
pub fn base() -> String {
    std::env::var(URL_VAR)
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| BASE.to_owned())
        .trim_end_matches('/')
        .to_owned()
}

fn open(root: &std::path::Path) -> crate::error::Result<Option<crate::catalogue::Shared>> {
    Ok(source::Jlc::open(root)?.map(|j| std::sync::Arc::new(j) as crate::catalogue::Shared))
}

/// The jlc connector, listed in `catalogue::CONNECTORS`: LCSC's numbers
/// are its keys (JLCPCB's are the same).
pub const CONNECTOR: crate::catalogue::Connector = crate::catalogue::Connector {
    name: NAME,
    suppliers: &["lcsc", "jlcpcb", "jlc"],
    attribution: ATTRIBUTION,
    base,
    open,
    fetched_at: |root| Some(index::read(root).ok()??.meta.fetched_at),
    sync: sync::sync,
};
