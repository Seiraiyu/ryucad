//! The project's sourcing (design 347): `parts.json` beside the manifest
//! (kind `ryueda.parts`), written only when set: the assembly house, how
//! many boards a build makes, the sources searches ask, and supply
//! findings' severities. `parts.configure` writes it.

use crate::error::Code;
use crate::fault::Fault;
use crate::project::Project;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const KIND: &str = "ryueda.parts";
pub const FORMAT: u64 = 1;

/// The supply findings `severity` may set.
pub const SUPPLY_CODES: [Code; 6] = [
    Code::PartOutOfStock,
    Code::PartExtended,
    Code::PartNoData,
    Code::PartsMismatch,
    Code::PartsCacheStale,
    Code::PartsSourceUnreachable,
];

/// A supply code's name, as files write it.
pub fn code_name(code: Code) -> String {
    serde_json::json!(code)
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

/// The assembly house a project's boards go to (whose Basic and
/// Extended parts `part_extended` judges).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Assembly {
    Jlcpcb,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assembly: Option<Assembly>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_quantity: Option<u64>,
    /// Which sources a search without `in` asks, in order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub severity: BTreeMap<String, crate::erc::Level>,
}

impl Config {
    /// How many boards a build makes: 1 unless set.
    pub fn build_quantity(&self) -> u64 {
        self.build_quantity.unwrap_or(1)
    }

    /// The severity `parts.json` sets for `code`, if any.
    pub fn level(&self, code: Code) -> Option<crate::erc::Level> {
        self.severity.get(&code_name(code)).copied()
    }

    pub fn check(&self) -> Vec<Fault> {
        let mut out = Vec::new();
        if self.build_quantity == Some(0) {
            out.push(
                Fault::error(Code::InvalidDocument, "build_quantity is at least 1")
                    .with("pointer", "/build_quantity"),
            );
        }
        for name in self.severity.keys() {
            if !SUPPLY_CODES.iter().any(|c| code_name(*c) == *name) {
                out.push(
                    Fault::error(
                        Code::InvalidDocument,
                        format!("{name} isn't a supply finding"),
                    )
                    .with("pointer", format!("/severity/{name}")),
                );
            }
        }
        let known = crate::parts::sources_known();
        for (i, s) in self.sources.iter().enumerate() {
            if !known.contains(&s.as_str()) {
                out.push(
                    Fault::error(
                        Code::InvalidDocument,
                        format!("{s} isn't a source: {}", known.join(", ")),
                    )
                    .with("pointer", format!("/sources/{i}")),
                );
            }
        }
        out
    }
}

/// The project's sourcing; the defaults where `parts.json` doesn't read
/// (a fault of the project).
pub fn config(p: &Project) -> Config {
    p.parts.item.clone().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::netlist::tests::project;
    use serde_json::json;

    #[test]
    fn parts_json_reads_checks_and_counts_in_the_revision() {
        let (_d0, none) = project(&[]);
        assert_eq!(config(&none), Config::default());
        let good = json!({"assembly": "jlcpcb", "build_quantity": 5, "format": 1, "kind": "ryueda.parts",
                          "severity": {"part_extended": "off"}, "sources": ["library", "jlc"]});
        let (d1, p) = project(&[("parts.json", good)]);
        let c = config(&p);
        assert_eq!(
            (c.assembly, c.build_quantity()),
            (Some(Assembly::Jlcpcb), 5)
        );
        assert_eq!(c.sources, ["library", "jlc"]);
        assert_eq!(c.level(Code::PartExtended), Some(crate::erc::Level::Off));
        assert_eq!(c.level(Code::PartOutOfStock), None);
        assert_ne!(p.revision, none.revision);
        assert!(p.faults().is_empty(), "{:?}", p.faults());
        let bad = json!({"build_quantity": 0, "format": 1, "kind": "ryueda.parts",
                         "severity": {"pin_conflict": "off"}, "sources": ["digikey"]});
        let (_d2, p) = project(&[("parts.json", bad)]);
        let pointers: Vec<String> = p
            .faults()
            .iter()
            .map(|f| f.error.details["pointer"].as_str().unwrap_or("").to_owned())
            .collect();
        assert_eq!(
            pointers,
            ["/build_quantity", "/severity/pin_conflict", "/sources/0"]
        );
        let (_d3, p) = project(&[(
            "parts.json",
            json!({"format": 1, "kind": "ryueda.parts", "colour": 1}),
        )]);
        assert_eq!(p.faults()[0].error.details["path"], "parts.json");
        assert_eq!(config(&p), Config::default());
        assert!(
            crate::project::Project::tracked(&d1.path().join("board"))
                .iter()
                .any(|(r, _)| r == "parts.json")
        );
    }
}
