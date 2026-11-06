//! What a source says of one part, stamped with when it was fetched, and
//! the terms its data may be kept under (design 347). One shape for every
//! source; a library's records have no supply fields.

use crate::category::Category;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A supplier's part number: an LCSC C-number, a DigiKey part number.
/// Library components write the same shape.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Offer {
    pub supplier: String,
    pub sku: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PriceBreak {
    pub quantity: u64,
    /// Decimal text, as the source gives it.
    pub price: String,
    pub currency: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Lifecycle {
    Active,
    NotRecommended,
    LastTimeBuy,
    Obsolete,
}

/// How long a source's records may be kept: until refreshed, at most a
/// number of seconds, or never written to disk.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Store {
    Cache,
    Ttl(u64),
    Never,
}

/// What a source's data may be used for, which the cache and writers
/// enforce whatever an adapter does.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Terms {
    pub store: Store,
    pub write_parameters: bool,
    /// What every answer showing the source's data carries.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub attribution: String,
}

impl Terms {
    /// Whether a record fetched at `fetched_at` (UTC seconds) may still be
    /// used at `now`.
    pub fn fresh(&self, fetched_at: Option<u64>, now: u64) -> bool {
        match self.store {
            Store::Never => false,
            Store::Cache => true,
            Store::Ttl(s) => fetched_at.is_some_and(|f| now.saturating_sub(f) <= s),
        }
    }

    /// Whether a writer may copy the source's parameters into a library:
    /// never, for every source (the owner's decision in design 347), so
    /// this ignores `write_parameters`.
    pub fn parameters_writable(&self) -> bool {
        false
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Record {
    /// `library`, `jlc`, `digikey`.
    pub source: String,
    /// The part's key in its source.
    pub key: String,
    /// A library result's component: `lib/<alias>/libcomp/<id>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub manufacturer: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub mpn: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<Category>,
    /// As the source writes them, under the vocabulary's names where it
    /// has one (`param::name_for`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub parameters: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub offers: Vec<Offer>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stock: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub price_breaks: Vec<PriceBreak>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lifecycle: Option<Lifecycle>,
    /// JLC Basic (or Preferred).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub basic: Option<bool>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub datasheet: String,
    /// When it was fetched, UTC seconds; `None` for a library's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fetched_at: Option<u64>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub attribution: String,
}

impl Record {
    /// A record of `source`'s `key` saying nothing else yet.
    pub fn new(source: &str, key: &str) -> Record {
        Record {
            source: source.to_owned(),
            key: key.to_owned(),
            component: None,
            manufacturer: String::new(),
            mpn: String::new(),
            description: String::new(),
            category: None,
            parameters: BTreeMap::new(),
            offers: Vec::new(),
            stock: None,
            price_breaks: Vec::new(),
            lifecycle: None,
            basic: None,
            datasheet: String::new(),
            fetched_at: None,
            attribution: String::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn records_write_only_what_they_hold_and_read_back() {
        let mut r = Record::new("library", "lib/self/libcomp/r_100k");
        r.mpn = "RC0402FR-07100KL".into();
        r.category = Some(Category::Resistor);
        assert_eq!(
            serde_json::to_value(&r).unwrap(),
            json!({"source": "library", "key": "lib/self/libcomp/r_100k",
                   "mpn": "RC0402FR-07100KL", "category": "resistor"})
        );
        r.stock = Some(12_000);
        r.price_breaks.push(PriceBreak { quantity: 100, price: "0.0012".into(), currency: "USD".into() });
        r.lifecycle = Some(Lifecycle::LastTimeBuy);
        r.basic = Some(true);
        r.fetched_at = Some(1_762_400_000);
        let back: Record = serde_json::from_value(serde_json::to_value(&r).unwrap()).unwrap();
        assert_eq!(back, r);
        let e = serde_json::from_value::<Record>(json!({"source": "x", "key": "y", "colour": "red"}));
        assert!(e.is_err());
    }

    #[test]
    fn terms_say_how_long_a_record_keeps() {
        let ttl = Terms { store: Store::Ttl(100), write_parameters: false, attribution: "Data from DigiKey".into() };
        assert_eq!(serde_json::to_value(&ttl).unwrap()["store"], json!({"ttl": 100}));
        assert!(ttl.fresh(Some(1_000), 1_100) && !ttl.fresh(Some(1_000), 1_101) && !ttl.fresh(None, 0));
        let cache = Terms { store: Store::Cache, write_parameters: true, attribution: String::new() };
        assert_eq!(serde_json::to_value(&cache).unwrap(), json!({"store": "cache", "write_parameters": true}));
        assert!(cache.fresh(None, u64::MAX));
        assert!(!Terms { store: Store::Never, ..cache.clone() }.fresh(Some(5), 5));
        // Whatever an adapter says, no source's parameters are written into a library.
        assert!(!cache.parameters_writable());
    }
}
