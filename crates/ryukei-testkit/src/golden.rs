use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// Hashes the exact bits of f64 outputs per named section and checks them
/// against a committed file of `name hash` lines.
///
/// Set `UPDATE_GOLDEN=1` to rewrite the file instead of checking it.
pub struct Golden {
    path: PathBuf,
    sections: BTreeMap<String, u64>,
}

/// One section's running FNV-1a hash.
pub struct Section<'a> {
    hash: &'a mut u64,
}

impl Section<'_> {
    pub fn f64(&mut self, x: f64) {
        self.u64(x.to_bits());
    }

    pub fn u64(&mut self, x: u64) {
        self.bytes(&x.to_le_bytes());
    }

    /// Text, such as a body's canonical dump, by its UTF-8 bytes.
    pub fn str(&mut self, s: &str) {
        self.bytes(s.as_bytes());
    }

    fn bytes(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            *self.hash ^= u64::from(byte);
            *self.hash = self.hash.wrapping_mul(FNV_PRIME);
        }
    }
}

impl Golden {
    pub fn new(path: impl AsRef<Path>) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
            sections: BTreeMap::new(),
        }
    }

    /// Starts (or continues) the named section.
    pub fn section(&mut self, name: &str) -> Section<'_> {
        Section {
            hash: self.sections.entry(name.to_string()).or_insert(FNV_OFFSET),
        }
    }

    fn render(&self) -> String {
        self.sections
            .iter()
            .map(|(name, hash)| format!("{name} {hash:016x}\n"))
            .collect()
    }

    /// Checks every section against the committed file, or rewrites it
    /// when `UPDATE_GOLDEN=1`. Panics with every mismatching section named.
    pub fn finish(self) {
        let actual = self.render();
        if std::env::var("UPDATE_GOLDEN").as_deref() == Ok("1") {
            std::fs::write(&self.path, &actual).expect("write golden file");
            return;
        }
        let expected = std::fs::read_to_string(&self.path).unwrap_or_else(|_| {
            panic!(
                "missing golden file {}; run with UPDATE_GOLDEN=1",
                self.path.display()
            )
        });
        if expected != actual {
            let exp: BTreeMap<_, _> = expected.lines().filter_map(|l| l.split_once(' ')).collect();
            let act: BTreeMap<_, _> = actual.lines().filter_map(|l| l.split_once(' ')).collect();
            let mut bad: Vec<&str> = exp
                .keys()
                .chain(act.keys())
                .filter(|k| exp.get(*k) != act.get(*k))
                .copied()
                .collect();
            bad.dedup();
            panic!(
                "determinism mismatch in sections {bad:?}\nexpected:\n{expected}actual:\n{actual}"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fnv_known_value() {
        // FNV-1a 64 of the 8 bytes of 1.0f64 (little-endian 0x3ff0000000000000).
        let mut g = Golden::new("unused");
        g.section("s").f64(1.0);
        assert_eq!(g.render(), "s aab1693229ba1db8\n");
    }

    #[test]
    fn str_hashes_its_bytes() {
        // FNV-1a 64 of "a" is af63dc4c8601ec8c.
        let mut g = Golden::new("unused");
        g.section("s").str("a");
        assert_eq!(g.render(), "s af63dc4c8601ec8c\n");
    }
}
