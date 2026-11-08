//! DigiKey's Product Information API v4 (design 347, plan 6.C), a
//! distributor connector: OAuth client credentials with each user's own
//! keys, keyword search and product details, called live; its records
//! kept at most 24 hours, never written into a library, attributed. It
//! is the source of lifecycle. Nothing outside this module names it but
//! `distributor::DISTRIBUTORS`.

pub mod attr;
pub mod record;
pub mod source;
#[cfg(any(test, feature = "testing"))]
pub mod testing;
pub mod wire;
pub use source::DigiKey;

use crate::keys::{Field, KeySpec};
use crate::rate::Limits;

pub const NAME: &str = "digikey";
/// Its API; `RYUEDA_DIGIKEY_URL` moves it (tests, and DigiKey's sandbox,
/// `https://sandbox-api.digikey.com`).
pub const BASE: &str = "https://api.digikey.com";
pub const URL_VAR: &str = "RYUEDA_DIGIKEY_URL";
pub const ATTRIBUTION: &str = "Data from DigiKey";
/// How an offer names it.
pub const SUPPLIER: &str = "DigiKey";
/// 24 hours (design 347).
pub const TTL: u64 = 86_400;
/// DigiKey's own per app (design 347).
pub const LIMITS: Limits = Limits {
    per_minute: 120,
    per_day: 1_000,
};
pub const KEYS: KeySpec = KeySpec {
    fields: &[
        Field {
            name: "client_id",
            var: "RYUEDA_DIGIKEY_CLIENT_ID",
        },
        Field {
            name: "client_secret",
            var: "RYUEDA_DIGIKEY_CLIENT_SECRET",
        },
    ],
    file: "parts-digikey.keys",
    how: "register an app with the Product Information v4 API at developer.digikey.com, then run `ryueda parts login digikey` with its client ID and secret on two lines of stdin (or set RYUEDA_DIGIKEY_CLIENT_ID and RYUEDA_DIGIKEY_CLIENT_SECRET)",
};

/// DigiKey's API: `RYUEDA_DIGIKEY_URL`, else [`BASE`].
pub fn base() -> String {
    std::env::var(URL_VAR)
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| BASE.to_owned())
        .trim_end_matches('/')
        .to_owned()
}

fn open(o: crate::distributor::Opened) -> Box<dyn crate::distributor::Live> {
    Box::new(DigiKey::new(o))
}

/// The digikey distributor, listed in `distributor::DISTRIBUTORS`.
pub const DISTRIBUTOR: crate::distributor::Distributor = crate::distributor::Distributor {
    name: NAME,
    suppliers: &["digikey", "digi-key", "digi key"],
    attribution: ATTRIBUTION,
    ttl: TTL,
    limits: LIMITS,
    keys: KEYS,
    url_var: URL_VAR,
    base,
    open,
};
