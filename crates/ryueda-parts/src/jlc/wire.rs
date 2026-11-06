//! jlcparts' files as it writes them (`datatables.buildtables`, format 4):
//! the manifest, the attribute lookup table, and gzip.

use crate::error::{Code, Error, Result};
use serde::Deserialize;
use serde_json::Value;
use std::io::Read;

/// `parts_source_refused` about jlcparts' `field`.
pub fn refused(field: &str, why: impl Into<String>) -> Error {
    Error::new(
        Code::PartsSourceRefused,
        format!("jlcparts: {}", why.into()),
    )
    .with("source", super::NAME)
    .with("field", field)
}

/// The most a gzipped file unpacks to: past it, the file is refused.
pub const MAX_UNPACKED: u64 = 1 << 30;

pub fn gunzip(bytes: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    flate2::read::GzDecoder::new(bytes)
        .take(MAX_UNPACKED + 1)
        .read_to_end(&mut out)
        .map_err(|e| refused("gzip", format!("a file doesn't unpack: {e}")))?;
    if out.len() as u64 > MAX_UNPACKED {
        return Err(refused("gzip", "a file unpacks past 1 GiB"));
    }
    Ok(out)
}

#[derive(Clone, Debug, Deserialize)]
pub struct Category {
    pub id: u64,
    pub category: String,
    pub subcategory: String,
    #[serde(rename = "componentCount", default)]
    pub component_count: u64,
    pub shards: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Manifest {
    pub version: u64,
    pub created: String,
    #[serde(rename = "totalComponents")]
    pub total_components: u64,
    #[serde(rename = "attributesLut")]
    pub attributes_lut: String,
    pub categories: Vec<Category>,
}

impl Manifest {
    pub fn read(bytes: &[u8]) -> Result<Manifest> {
        let v: Value = serde_json::from_slice(bytes)
            .map_err(|e| refused("manifest", format!("manifest.json isn't JSON: {e}")))?;
        match v.get("version").and_then(Value::as_u64) {
            Some(super::FORMAT) => {}
            other => {
                return Err(refused(
                    "version",
                    format!(
                        "manifest.json is format {}; this ryueda reads format {}",
                        other.map_or_else(|| "unknown".to_owned(), |n| n.to_string()),
                        super::FORMAT
                    ),
                )
                .with("version", other));
            }
        }
        let m: Manifest = serde_json::from_value(v)
            .map_err(|e| refused("manifest", format!("manifest.json: {e}")))?;
        let names =
            std::iter::once(&m.attributes_lut).chain(m.categories.iter().flat_map(|c| &c.shards));
        for n in names {
            if n.is_empty() || n.contains(['/', '\\', '?', '#', '%']) || n.starts_with('.') {
                return Err(refused("manifest", format!("{n:?} isn't a file name")));
            }
        }
        Ok(m)
    }

    /// Every shard to fetch, with its category.
    pub fn shards(&self) -> impl Iterator<Item = (&Category, &str)> {
        self.categories
            .iter()
            .flat_map(|c| c.shards.iter().map(move |s| (c, s.as_str())))
    }

    /// `created` as UTC seconds, if it reads (`2026-11-06T11:33:30+00:00`).
    pub fn created_unix(&self) -> Option<u64> {
        crate::time::parse_rfc3339(&self.created)
    }
}

/// `[name, value]` by index.
#[derive(Debug)]
pub struct Lut(Vec<(String, Value)>);

impl Lut {
    pub fn read(gzipped: &[u8]) -> Result<Lut> {
        let raw = gunzip(gzipped)?;
        let v: Vec<(String, Value)> = serde_json::from_slice(&raw)
            .map_err(|e| refused("attributes_lut", format!("the attribute table: {e}")))?;
        Ok(Lut(v))
    }

    pub fn get(&self, i: usize) -> Option<&(String, Value)> {
        self.0.get(i)
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jlc::testing;

    #[test]
    fn the_manifest_and_lookup_table_read() {
        let site = testing::site("2026-11-06T11:33:30+00:00");
        let m = Manifest::read(&site[0].1).unwrap();
        assert_eq!(
            (m.version, m.total_components, m.categories.len()),
            (4, 5, 3)
        );
        assert_eq!(
            m.categories[2].subcategory,
            "Linear Voltage Regulators (LDO)"
        );
        assert_eq!(m.shards().count(), 3);
        assert_eq!(m.created_unix(), Some(1_793_964_810));
        let lut = Lut::read(&site[1].1).unwrap();
        assert_eq!(lut.len(), 19);
        assert_eq!(lut.get(4).map(|(n, _)| n.as_str()), Some("Package"));
        // Another format is refused naming it, never misread.
        let v5 = String::from_utf8(site[0].1.clone())
            .unwrap()
            .replace("\"version\":4", "\"version\":5");
        let e = Manifest::read(v5.as_bytes()).unwrap_err();
        assert_eq!(
            (e.code, &e.details["field"]),
            (Code::PartsSourceRefused, &serde_json::json!("version"))
        );
        assert_eq!(e.details["version"], 5);
        assert_eq!(
            Manifest::read(b"{}").unwrap_err().code,
            Code::PartsSourceRefused
        );
        let sneaky = String::from_utf8(site[0].1.clone())
            .unwrap()
            .replace("components-caps-001.jsonl.gz", "../etc/passwd");
        assert_eq!(
            Manifest::read(sneaky.as_bytes()).unwrap_err().code,
            Code::PartsSourceRefused
        );
        assert_eq!(
            Lut::read(b"not gzip").unwrap_err().code,
            Code::PartsSourceRefused
        );
        assert_eq!(
            gunzip(b"\x1f\x8b\x08\x00\x00").unwrap_err().code,
            Code::PartsSourceRefused
        );
    }
}
