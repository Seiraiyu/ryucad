//! Library components: `library/components/<id>.json` (kind
//! `ryueda.libcomp`), what a design places. Each ties a symbol to its
//! footprints, pin to pads, with the part's value, parameters, maker and
//! supplier part numbers. Prices, stock and lifecycle are never in Git.

use crate::error::{Code, Error};
use crate::fault::Fault;
use crate::footprint::Footprint;
use crate::id::Id;
use crate::symbol::{PinType, Symbol, pointer_escape};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const KIND: &str = "ryueda.libcomp";
pub const FORMAT: u64 = 1;

/// A library object a file names: in another library by its alias, or
/// (no `library`) in the library the file is in.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LibRef {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub library: Option<Id>,
    pub id: Id,
}

impl LibRef {
    pub fn local(id: Id) -> LibRef {
        LibRef { library: None, id }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LibComp {
    pub symbol: LibRef,
    /// The reference designator's letters: `R`, `C`, `U`.
    pub prefix: String,
    /// As written: `100nF`, `4k7`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub value: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// A URL.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub datasheet: String,
    /// Each a string as written, a quantity or not (`"50 V"`, `"X7R"`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub parameters: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub manufacturer: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub mpn: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub offers: Vec<Offer>,
    /// The first is the default.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub footprints: Vec<Use>,
}

/// A supplier's part number: an LCSC C-number, a DigiKey part number.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Offer {
    pub supplier: String,
    pub sku: String,
}

/// A footprint the component may take, and which pads each symbol pin
/// connects to. A pin left out connects to the pad of its own number.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Use {
    pub footprint: LibRef,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub pins: BTreeMap<String, Vec<String>>,
}

impl Use {
    /// The pads symbol pin `pin` connects to: its list, else the pad of
    /// its own number.
    pub fn pads_of<'a>(&'a self, pin: &'a str) -> Vec<&'a str> {
        match self.pins.get(pin) {
            Some(pads) => pads.iter().map(String::as_str).collect(),
            None => vec![pin],
        }
    }
}

impl LibComp {
    /// What the component's checks find, given its symbol and footprints
    /// as they resolve (an error for one that doesn't): every symbol pin
    /// on at least one pad of each footprint (`no_connect` pins excepted),
    /// no pad named by two pins, pin maps naming the symbol's pins and the
    /// footprint's pads, and (a warning) numbered pads no pin maps.
    pub fn check(
        &self,
        symbol: Result<&Symbol, Error>,
        footprints: &[Result<&Footprint, Error>],
    ) -> Vec<Fault> {
        let mut out = Vec::new();
        let unresolved = |e: Error, ptr: String| Fault {
            severity: crate::fault::Severity::Error,
            error: e.with("pointer", ptr),
        };
        let symbol = match symbol {
            Ok(s) => Some(s),
            Err(e) => {
                out.push(unresolved(e, "/symbol".into()));
                None
            }
        };
        for (i, (u, f)) in self.footprints.iter().zip(footprints).enumerate() {
            let ptr = format!("/footprints/{i}");
            let f = match f {
                Ok(f) => *f,
                Err(e) => {
                    out.push(unresolved(e.clone(), format!("{ptr}/footprint")));
                    continue;
                }
            };
            let Some(s) = symbol else { continue };
            for pin in u.pins.keys() {
                if !s.pins.contains_key(pin) {
                    out.push(
                        Fault::error(Code::PinNotFound, format!("the symbol has no pin {pin}"))
                            .with("pointer", format!("{ptr}/pins/{}", pointer_escape(pin)))
                            .with("pin", pin),
                    );
                }
            }
            let numbers = f.numbers();
            let mut taken: BTreeMap<&str, &str> = BTreeMap::new();
            for (n, p) in &s.pins {
                let pads = u.pads_of(n);
                let at = |what: &str| match u.pins.contains_key(n) {
                    true => format!("{ptr}/pins/{}{what}", pointer_escape(n)),
                    false => format!("{ptr}/pins"),
                };
                for pad in pads.iter().filter(|p| !numbers.contains(**p)) {
                    if u.pins.contains_key(n) {
                        out.push(
                            Fault::error(
                                Code::NotFound,
                                format!("the footprint has no pad {pad} for pin {n}"),
                            )
                            .with("pointer", at(""))
                            .with("pin", n)
                            .with("pad", pad),
                        );
                    }
                }
                let on: Vec<&str> = pads.into_iter().filter(|p| numbers.contains(*p)).collect();
                if on.is_empty() && p.ty != PinType::NoConnect {
                    out.push(
                        Fault::error(
                            Code::PinWithoutPad,
                            format!("pin {n} connects to no pad of {}", u.footprint.id),
                        )
                        .with("pointer", at(""))
                        .with("pin", n),
                    );
                }
                for pad in on {
                    if let Some(other) = taken.insert(pad, n) {
                        out.push(
                            Fault::error(
                                Code::PadMappedTwice,
                                format!("pad {pad} is mapped by pins {other} and {n}"),
                            )
                            .with("pointer", at(""))
                            .with("pad", pad)
                            .with("pins", [other, n.as_str()]),
                        );
                    }
                }
            }
            let unmapped: BTreeSet<&str> = numbers
                .into_iter()
                .filter(|pad| !taken.contains_key(pad))
                .collect();
            for pad in unmapped {
                out.push(
                    Fault::warning(
                        Code::PadWithoutPin,
                        format!("pad {pad} of {} has no pin", u.footprint.id),
                    )
                    .with("pointer", format!("{ptr}/pins"))
                    .with("pad", pad),
                );
            }
        }
        out
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::doc;
    use crate::footprint::tests::r0402;
    use crate::symbol::tests::resistor;
    use serde_json::json;

    pub fn r_100k() -> LibComp {
        let v = json!({
            "kind": KIND, "format": 1,
            "symbol": {"id": "r"}, "prefix": "R", "value": "100k",
            "parameters": {"tolerance": "1%", "power": "62.5 mW"},
            "manufacturer": "Yageo", "mpn": "RC0402FR-07100KL",
            "offers": [{"supplier": "LCSC", "sku": "C25741"}],
            "footprints": [{"footprint": {"id": "r_0402_1005metric"}}]
        });
        doc::from_value(v, "library/components/r_100k.json", KIND, FORMAT).unwrap()
    }

    fn codes(c: &LibComp, s: &Symbol, f: &Footprint) -> Vec<(Code, String)> {
        c.check(Ok(s), &[Ok(f)])
            .into_iter()
            .map(|x| {
                (
                    x.error.code,
                    x.error.details["pointer"].as_str().unwrap().to_owned(),
                )
            })
            .collect()
    }

    #[test]
    fn components_read_write_and_map_by_identity() {
        let c = r_100k();
        let text = doc::to_string(&c, KIND, FORMAT);
        assert_eq!(
            doc::read::<LibComp>(text.as_bytes(), "c.json", KIND, FORMAT).unwrap(),
            c
        );
        assert!(!text.contains("\"library\""));
        assert!(!text.contains("\"pins\""));
        assert!(codes(&c, &resistor(), &r0402()).is_empty());
    }

    #[test]
    fn checks_find_pins_without_pads_and_pads_mapped_twice() {
        let (s, f) = (resistor(), r0402());
        let mut c = r_100k();
        c.footprints[0].pins = [
            ("1".to_owned(), vec!["1".to_owned(), "2".to_owned()]),
            ("3".to_owned(), vec!["1".to_owned()]),
        ]
        .into();
        assert_eq!(
            codes(&c, &s, &f),
            vec![
                (Code::PinNotFound, "/footprints/0/pins/3".to_owned()),
                (Code::PadMappedTwice, "/footprints/0/pins".to_owned()),
            ]
        );
        c.footprints[0].pins = [("2".to_owned(), vec!["9".to_owned()])].into();
        assert_eq!(
            codes(&c, &s, &f),
            vec![
                (Code::NotFound, "/footprints/0/pins/2".to_owned()),
                (Code::PinWithoutPad, "/footprints/0/pins/2".to_owned()),
                (Code::PadWithoutPin, "/footprints/0/pins".to_owned()),
            ]
        );
        let warn = c.check(Ok(&s), &[Ok(&f)]);
        assert!(!warn.last().unwrap().is_error());
        // A pin that connects to nothing by design needs no pad.
        let mut s = s;
        s.pins.get_mut("2").unwrap().ty = PinType::NoConnect;
        c.footprints[0].pins.clear();
        c.footprints[0].pins.insert("2".into(), vec![]);
        assert_eq!(
            codes(&c, &s, &f),
            vec![(Code::PadWithoutPin, "/footprints/0/pins".to_owned())]
        );
    }

    #[test]
    fn what_doesnt_resolve_is_a_fault_where_it_is_named() {
        let c = r_100k();
        let gone = || Error::new(Code::NotFound, "no such symbol");
        let found = c.check(
            Err(gone()),
            &[Err(Error::new(Code::LibraryMissing, "no library"))],
        );
        let at: Vec<(Code, &str)> = found
            .iter()
            .map(|x| (x.error.code, x.error.details["pointer"].as_str().unwrap()))
            .collect();
        assert_eq!(
            at,
            [
                (Code::NotFound, "/symbol"),
                (Code::LibraryMissing, "/footprints/0/footprint")
            ]
        );
    }
}
