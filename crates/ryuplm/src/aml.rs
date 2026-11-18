//! An item's approved manufacturer list (design 734): manufacturer and
//! part-number pairs, each with a status, named as ryueda's part data
//! names them, so records from a parts source read straight in.

use crate::error::{Error, Result};
use crate::id::Id;
use ryu_doc::files::{Obj, invalid};
use serde_json::{Value, json};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Active,
    Nrnd,
    Obsolete,
    Unknown,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Active => "active",
            Status::Nrnd => "nrnd",
            Status::Obsolete => "obsolete",
            Status::Unknown => "unknown",
        }
    }

    pub fn parse(s: &str) -> Option<Status> {
        [
            Status::Active,
            Status::Nrnd,
            Status::Obsolete,
            Status::Unknown,
        ]
        .into_iter()
        .find(|x| x.as_str() == s)
    }

    /// A parts source's lifecycle: not recommended and last time buy are
    /// both `nrnd`; none known is `unknown`.
    pub fn from_lifecycle(l: Option<ryueda_parts::record::Lifecycle>) -> Status {
        use ryueda_parts::record::Lifecycle::*;
        match l {
            Some(Active) => Status::Active,
            Some(NotRecommended | LastTimeBuy) => Status::Nrnd,
            Some(Obsolete) => Status::Obsolete,
            None => Status::Unknown,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub manufacturer: String,
    pub mpn: String,
    pub status: Status,
    pub preferred: bool,
}

impl Entry {
    pub fn to_json(&self) -> Value {
        let mut v = json!({"manufacturer": self.manufacturer, "mpn": self.mpn, "status": self.status.as_str()});
        if self.preferred {
            v["preferred"] = json!(true);
        }
        v
    }

    /// The entry a part record gives: its maker, part number and lifecycle.
    pub fn from_record(r: &ryueda_parts::record::Record) -> Entry {
        Entry {
            manufacturer: r.manufacturer.clone(),
            mpn: r.mpn.clone(),
            status: Status::from_lifecycle(r.lifecycle),
            preferred: false,
        }
    }

    pub fn from_json(v: &Value, at: &str) -> Result<Entry> {
        let mut o = Obj::new(v, at)?;
        let mut text = |k: &str| -> Result<String> {
            o.take(k)?
                .as_str()
                .filter(|s| !s.trim().is_empty())
                .map(str::to_owned)
                .ok_or_else(|| {
                    invalid(&format!("{at}/{k}"), &format!("{k} must be non-empty text"))
                })
        };
        let manufacturer = text("manufacturer")?;
        let mpn = text("mpn")?;
        let status = o
            .take("status")?
            .as_str()
            .and_then(Status::parse)
            .ok_or_else(|| {
                invalid(
                    &format!("{at}/status"),
                    "status is active, nrnd, obsolete or unknown",
                )
            })?;
        let preferred = match o.take_opt("preferred") {
            None => false,
            Some(p) => p
                .as_bool()
                .ok_or_else(|| invalid(&format!("{at}/preferred"), "preferred is true or false"))?,
        };
        o.finish()?;
        Ok(Entry {
            manufacturer,
            mpn,
            status,
            preferred,
        })
    }
}

/// Every fault of item `item`'s AML, each at its entry's pointer: an entry
/// that doesn't read, a pair listed twice, a second preferred entry.
pub fn judge(item: &Id, aml: &[Value]) -> Vec<Error> {
    let mut out = Vec::new();
    let mut seen: Vec<(String, String)> = Vec::new();
    let mut preferred = false;
    for (i, v) in aml.iter().enumerate() {
        let at = format!("/aml/{i}");
        match Entry::from_json(v, &at) {
            Err(e) => out.push(e.with("item", item)),
            Ok(e) => {
                let pair = (e.manufacturer.clone(), e.mpn.clone());
                if seen.contains(&pair) {
                    out.push(
                        invalid(
                            &at,
                            &format!("{} {} is listed twice", e.manufacturer, e.mpn),
                        )
                        .with("item", item),
                    );
                    continue;
                }
                seen.push(pair);
                if e.preferred && preferred {
                    out.push(
                        invalid(
                            &format!("{at}/preferred"),
                            "only one entry may be preferred",
                        )
                        .with("item", item),
                    );
                }
                preferred |= e.preferred;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn id() -> Id {
        Id::parse("k3v9q2m8x4c1p", "item").unwrap()
    }

    #[test]
    fn statuses_follow_parts_data() {
        use ryueda_parts::record::Lifecycle;
        assert_eq!(
            Status::from_lifecycle(Some(Lifecycle::Active)),
            Status::Active
        );
        assert_eq!(
            Status::from_lifecycle(Some(Lifecycle::NotRecommended)),
            Status::Nrnd
        );
        assert_eq!(
            Status::from_lifecycle(Some(Lifecycle::LastTimeBuy)),
            Status::Nrnd
        );
        assert_eq!(
            Status::from_lifecycle(Some(Lifecycle::Obsolete)),
            Status::Obsolete
        );
        assert_eq!(Status::from_lifecycle(None), Status::Unknown);
        for s in ["active", "nrnd", "obsolete", "unknown"] {
            assert_eq!(Status::parse(s).unwrap().as_str(), s);
        }
    }

    #[test]
    fn the_names_are_ryueda_parts() {
        // An AML entry's names are a part record's: one written from a
        // record reads back.
        let r: ryueda_parts::record::Record = serde_json::from_value(json!({
            "source": "s", "key": "k", "manufacturer": "Acme", "mpn": "BR-42"
        }))
        .unwrap();
        let entry = Entry::from_record(&r);
        assert_eq!(
            entry.to_json(),
            json!({"manufacturer": "Acme", "mpn": "BR-42", "status": "unknown"})
        );
    }

    #[test]
    fn aml_faults_say_where() {
        let ok = vec![
            json!({"manufacturer": "Acme", "mpn": "BR-42", "status": "active", "preferred": true}),
            json!({"manufacturer": "Bolt Co", "mpn": "BR-42", "status": "nrnd"}),
        ];
        assert!(judge(&id(), &ok).is_empty());
        let bad = vec![
            json!({"manufacturer": "Acme", "mpn": "BR-42", "status": "active", "preferred": true}),
            json!({"manufacturer": "Acme", "mpn": "BR-42", "status": "active"}),
            json!({"manufacturer": "B", "mpn": "1", "status": "eol"}),
            json!({"manufacturer": "C", "mpn": "2", "status": "active", "preferred": true}),
            json!({"manufacturer": "", "mpn": "3", "status": "active"}),
            json!({"manufacturer": "D", "mpn": "4", "status": "active", "x": 1}),
        ];
        let at: Vec<String> = judge(&id(), &bad)
            .iter()
            .map(|e| e.details["pointer"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(
            at,
            [
                "/aml/1",
                "/aml/2/status",
                "/aml/3/preferred",
                "/aml/4/manufacturer",
                "/aml/5/x"
            ]
        );
    }
}
