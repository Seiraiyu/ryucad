use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// A STEP file's text with the writer's version in its header (`'ryustep
/// <version>'`) written as a development build's, `0.0.0`. Releases stamp
/// their own version in, and golden hashes must hold for every build of
/// one commit.
pub fn unversioned(step: &str) -> String {
    let stamped = format!("'ryustep {}'", ryukei::VERSION);
    assert_eq!(
        step.matches(&stamped).count(),
        1,
        "one version in the header"
    );
    step.replacen(&stamped, "'ryustep 0.0.0'", 1)
}

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

    /// Raw bytes, such as a body's encoding.
    pub fn bytes(&mut self, bytes: &[u8]) {
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

/// Opens a golden file for reading and writing and holds an exclusive lock
/// on it, so tests that each own some of its lines (one process each under
/// nextest, or threads under `cargo test`) update it one at a time.
fn locked(path: &Path, write: bool) -> std::fs::File {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(write)
        .create(write)
        .truncate(false)
        .open(path)
        .unwrap_or_else(|_| {
            panic!(
                "missing golden file {}; run with UPDATE_GOLDEN=1",
                path.display()
            )
        });
    if write {
        file.lock().expect("lock golden file");
    } else {
        file.lock_shared().expect("lock golden file");
    }
    file
}

/// The `name hash` lines of a golden file.
fn read_lines(file: &mut std::fs::File) -> BTreeMap<String, String> {
    use std::io::Read;
    let mut text = String::new();
    file.read_to_string(&mut text).expect("read golden file");
    text.lines()
        .filter_map(|l| l.split_once(' '))
        .map(|(n, h)| (n.to_string(), h.to_string()))
        .collect()
}

/// Writes a golden file's lines back in place, sorted by name.
fn write_lines(file: &mut std::fs::File, lines: &BTreeMap<String, String>) {
    use std::io::{Seek, Write};
    let text: String = lines.iter().map(|(n, h)| format!("{n} {h}\n")).collect();
    file.set_len(0).expect("truncate golden file");
    file.seek(std::io::SeekFrom::Start(0))
        .expect("seek golden file");
    file.write_all(text.as_bytes()).expect("write golden file");
}

impl Golden {
    /// Checks only this golden's own sections against their lines of the
    /// committed file, leaving every other line to the tests that own it,
    /// or with `UPDATE_GOLDEN=1` rewrites just those lines, under a lock on
    /// the file. So one file's sections can be checked by many tests, run
    /// in parallel; [`Golden::check_names`] keeps the file to the sections
    /// they own.
    pub fn finish_part(self) {
        let update = std::env::var("UPDATE_GOLDEN").as_deref() == Ok("1");
        let mut file = locked(&self.path, update);
        let mut lines = read_lines(&mut file);
        let actual: BTreeMap<String, String> = self
            .sections
            .iter()
            .map(|(n, h)| (n.clone(), format!("{h:016x}")))
            .collect();
        if update {
            lines.extend(actual);
            write_lines(&mut file, &lines);
            return;
        }
        let bad: Vec<String> = actual
            .iter()
            .filter(|(n, h)| lines.get(*n) != Some(*h))
            .map(|(n, h)| {
                let expected = lines.get(n).map_or("(none)", String::as_str);
                format!("{n}: expected {expected}, actual {h}")
            })
            .collect();
        assert!(
            bad.is_empty(),
            "determinism mismatch in {}:\n{}",
            self.path.display(),
            bad.join("\n")
        );
    }

    /// Checks that the golden file at `path` holds exactly the sections
    /// `names` lists (the sections the tests checking it with
    /// [`Golden::finish_part`] own), or with `UPDATE_GOLDEN=1` drops the
    /// lines of sections no longer listed.
    pub fn check_names(path: impl AsRef<Path>, names: &[&str]) {
        let path = path.as_ref();
        let update = std::env::var("UPDATE_GOLDEN").as_deref() == Ok("1");
        let mut file = locked(path, update);
        let mut lines = read_lines(&mut file);
        if update {
            lines.retain(|n, _| names.contains(&n.as_str()));
            write_lines(&mut file, &lines);
            return;
        }
        let mut listed: Vec<&str> = names.to_vec();
        listed.sort_unstable();
        let held: Vec<&str> = lines.keys().map(String::as_str).collect();
        assert_eq!(
            held,
            listed,
            "{} doesn't hold exactly the listed sections; run with UPDATE_GOLDEN=1",
            path.display()
        );
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
