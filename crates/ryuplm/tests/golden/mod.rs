//! Golden hashes: ryu-board's `testing::Golden`, carried here so ryuplm
//! depends on no other product's crate (design 734). Outputs are hashed
//! per named section (FNV-1a, 64 bits) and checked against a committed
//! file of `name hash` lines; `UPDATE_GOLDEN=1` rewrites the file instead.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

pub struct Golden {
    path: PathBuf,
    sections: BTreeMap<String, u64>,
}

/// One section's running hash.
pub struct Section<'a> {
    hash: &'a mut u64,
}

impl Section<'_> {
    pub fn bytes(&mut self, bytes: &[u8]) -> &mut Self {
        for &b in bytes {
            *self.hash ^= u64::from(b);
            *self.hash = self.hash.wrapping_mul(FNV_PRIME);
        }
        self
    }

    #[allow(dead_code)]
    pub fn i64(&mut self, v: i64) -> &mut Self {
        self.bytes(&v.to_le_bytes())
    }

    pub fn str(&mut self, s: &str) -> &mut Self {
        // Length first, so `a` then `bc` never hashes as `ab` then `c`.
        self.bytes(&(s.len() as u64).to_le_bytes());
        self.bytes(s.as_bytes())
    }
}

impl Golden {
    pub fn new(path: impl AsRef<Path>) -> Golden {
        Golden {
            path: path.as_ref().to_path_buf(),
            sections: BTreeMap::new(),
        }
    }

    pub fn section(&mut self, name: &str) -> Section<'_> {
        Section {
            hash: self.sections.entry(name.to_string()).or_insert(FNV_OFFSET),
        }
    }

    fn render(&self) -> String {
        self.sections
            .iter()
            .map(|(n, h)| format!("{n} {h:016x}\n"))
            .collect()
    }

    /// Checks every section against the file, or rewrites it when
    /// `UPDATE_GOLDEN=1`. Panics naming the sections that differ.
    pub fn finish(self) {
        let actual = self.render();
        if std::env::var("UPDATE_GOLDEN").as_deref() == Ok("1") {
            std::fs::write(&self.path, &actual).expect("write golden file");
            return;
        }
        let expected = std::fs::read_to_string(&self.path).unwrap_or_else(|_| {
            panic!("missing {}; run with UPDATE_GOLDEN=1", self.path.display())
        });
        if expected != actual {
            let exp: BTreeMap<_, _> = expected.lines().filter_map(|l| l.split_once(' ')).collect();
            let act: BTreeMap<_, _> = actual.lines().filter_map(|l| l.split_once(' ')).collect();
            let bad: Vec<_> = exp
                .keys()
                .chain(act.keys())
                .filter(|k| exp.get(*k) != act.get(*k))
                .collect();
            panic!("golden mismatch in {bad:?}\nexpected:\n{expected}actual:\n{actual}");
        }
    }
}
