//! ryueda's index of the catalogue, in the parts cache: `current/` read,
//! `next/` built by a sync and swapped in (`current` to `old`, `next` to
//! `current`, `old` removed), so a stopped sync leaves the old index
//! whole. `meta.json` (kind `ryueda.catalogue`) says what it holds, and
//! each ryueda category is one gzipped file of records, a JSON line each,
//! without the meta's source, time and attribution. `keys.tsv.gz` lists
//! every key with its category and part number, so a lookup loads one
//! category, not the catalogue. Syncs of one index share `next/`, so
//! [`install`] holds `jlc/sync.lock` (an OS file lock, gone with its
//! process) across writing it and swapping it in: a second sync waits.

use crate::category::Category;
use crate::error::{Error, Result};
use crate::record::Record;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

pub const KIND: &str = "ryueda.catalogue";
pub const FORMAT: u64 = 1;
const KEYS: &str = "keys.tsv.gz";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Meta {
    pub kind: String,
    pub format: u64,
    pub source: String,
    /// When the sync fetched it, UTC seconds.
    pub fetched_at: u64,
    /// The catalogue's own date, as it gives it.
    pub created: String,
    pub parts: u64,
    pub categories: BTreeMap<String, u64>,
    pub attribution: String,
}

#[derive(Debug)]
pub struct Index {
    pub dir: PathBuf,
    pub meta: Meta,
}

fn io(p: &Path) -> impl Fn(std::io::Error) -> Error + '_ {
    move |e| Error::io(p, &e)
}

fn file(dir: &Path, c: Category) -> PathBuf {
    dir.join(format!("{}.jsonl.gz", c.as_str()))
}

/// Writes `records` as `root/next`, replacing any `next` a stopped sync
/// left; records of one key are kept once (the first).
pub fn write_next(
    root: &Path,
    records: &[Record],
    fetched_at: u64,
    created: &str,
) -> Result<PathBuf> {
    let next = root.join("next");
    if next.exists() {
        std::fs::remove_dir_all(&next).map_err(io(&next))?;
    }
    std::fs::create_dir_all(&next).map_err(io(&next))?;
    let mut by: BTreeMap<Category, Vec<&Record>> = BTreeMap::new();
    for r in records {
        by.entry(r.category.unwrap_or(Category::Other))
            .or_default()
            .push(r);
    }
    let mut counts = BTreeMap::new();
    let mut keys: Vec<(String, Category, String)> = Vec::new();
    for (c, mut list) in by {
        list.sort_by(|a, b| a.key.cmp(&b.key));
        list.dedup_by(|a, b| a.key == b.key);
        let path = file(&next, c);
        let f = std::fs::File::create(&path).map_err(io(&path))?;
        let mut gz =
            flate2::write::GzEncoder::new(std::io::BufWriter::new(f), flate2::Compression::fast());
        for r in &list {
            let mut slim = (*r).clone();
            slim.source = String::new();
            slim.fetched_at = None;
            slim.attribution = String::new();
            serde_json::to_writer(&mut gz, &slim)
                .map_err(|e| Error::io(&path, &std::io::Error::other(e)))?;
            gz.write_all(b"\n").map_err(io(&path))?;
        }
        gz.finish().and_then(|mut w| w.flush()).map_err(io(&path))?;
        counts.insert(c.as_str().to_owned(), list.len() as u64);
        keys.extend(
            list.iter()
                .map(|r| (r.key.clone(), c, crate::record::squash(&r.mpn))),
        );
    }
    keys.sort();
    let path = next.join(KEYS);
    let f = std::fs::File::create(&path).map_err(io(&path))?;
    let mut gz =
        flate2::write::GzEncoder::new(std::io::BufWriter::new(f), flate2::Compression::fast());
    for (k, c, m) in &keys {
        // Keys and part numbers squashed hold no tab or line break.
        if !k.contains(['\t', '\n', '\r']) {
            writeln!(gz, "{k}\t{}\t{m}", c.as_str()).map_err(io(&path))?;
        }
    }
    gz.finish().and_then(|mut w| w.flush()).map_err(io(&path))?;
    let meta = Meta {
        kind: KIND.to_owned(),
        format: FORMAT,
        source: super::NAME.to_owned(),
        fetched_at,
        created: created.to_owned(),
        parts: counts.values().sum(),
        categories: counts,
        attribution: super::ATTRIBUTION.to_owned(),
    };
    let path = next.join("meta.json");
    let v = serde_json::to_value(&meta).expect("meta serializes");
    std::fs::write(&path, ryu_doc::canonical::to_string(&v)).map_err(io(&path))?;
    Ok(next)
}

/// The lock every sync of the index at `root` holds while it writes
/// `next/` and swaps it in; dropping it lets the next sync go.
pub struct SyncLock {
    _file: std::fs::File,
}

/// Waits for `root/sync.lock`, making `root` if it isn't there.
pub fn lock(root: &Path) -> Result<SyncLock> {
    std::fs::create_dir_all(root).map_err(io(root))?;
    let path = root.join("sync.lock");
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .map_err(io(&path))?;
    file.lock().map_err(io(&path))?;
    Ok(SyncLock { _file: file })
}

/// `records` written as the index and swapped in under the sync lock, so
/// two syncs of one index at once leave one of them whole.
pub fn install(root: &Path, records: &[Record], fetched_at: u64, created: &str) -> Result<()> {
    let _held = lock(root)?;
    write_next(root, records, fetched_at, created)?;
    swap(root)
}

/// Puts `root/next` in place of `root/current`.
pub fn swap(root: &Path) -> Result<()> {
    let (cur, next, old) = (root.join("current"), root.join("next"), root.join("old"));
    if old.exists() && cur.exists() {
        std::fs::remove_dir_all(&old).map_err(io(&old))?;
    }
    if cur.exists() {
        std::fs::rename(&cur, &old).map_err(io(&cur))?;
    }
    std::fs::rename(&next, &cur).map_err(io(&next))?;
    if old.exists() {
        std::fs::remove_dir_all(&old).map_err(io(&old))?;
    }
    Ok(())
}

fn read_meta(dir: &Path) -> Option<Meta> {
    let text = std::fs::read_to_string(dir.join("meta.json")).ok()?;
    let m: Meta = serde_json::from_str(&text).ok()?;
    (m.kind == KIND && m.format == FORMAT).then_some(m)
}

/// The index swapped in, else the old one a stopped swap left; `None`
/// where neither reads (never synced, or a newer ryueda's).
pub fn read(root: &Path) -> Result<Option<Index>> {
    for d in ["current", "old"] {
        let dir = root.join(d);
        if let Some(meta) = read_meta(&dir) {
            return Ok(Some(Index { dir, meta }));
        }
    }
    Ok(None)
}

/// Every key of an index with its category and squashed part number.
pub struct Keys {
    /// By key.
    pub by_key: Vec<(String, Category)>,
    /// By squashed part number, then key.
    pub by_mpn: Vec<(String, String)>,
}

impl Index {
    /// The index's keys and part numbers.
    pub fn keys(&self) -> Result<Keys> {
        let path = self.dir.join(KEYS);
        let mut text = String::new();
        if let Ok(f) = std::fs::File::open(&path) {
            flate2::read::GzDecoder::new(std::io::BufReader::new(f))
                .read_to_string(&mut text)
                .map_err(io(&path))?;
        }
        let (mut by_key, mut by_mpn) = (Vec::new(), Vec::new());
        for l in text.lines() {
            let mut f = l.split('\t');
            let (Some(k), Some(c), m) = (f.next(), f.next().and_then(Category::parse), f.next())
            else {
                continue;
            };
            by_key.push((k.to_owned(), c));
            if let Some(m) = m.filter(|m| !m.is_empty()) {
                by_mpn.push((m.to_owned(), k.to_owned()));
            }
        }
        by_key.sort();
        by_mpn.sort();
        Ok(Keys { by_key, by_mpn })
    }

    /// Every record of `c`, by key, each with the meta's source, time and
    /// attribution. A line that doesn't read is left out.
    pub fn category(&self, c: Category) -> Result<Vec<Record>> {
        let path = file(&self.dir, c);
        let Ok(f) = std::fs::File::open(&path) else {
            return Ok(Vec::new());
        };
        let mut text = String::new();
        flate2::read::GzDecoder::new(std::io::BufReader::new(f))
            .read_to_string(&mut text)
            .map_err(io(&path))?;
        Ok(text
            .lines()
            .filter_map(|l| serde_json::from_str::<Record>(l).ok())
            .map(|mut r| {
                r.source.clone_from(&self.meta.source);
                r.fetched_at = Some(self.meta.fetched_at);
                r.attribution.clone_from(&self.meta.attribution);
                r
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::Record;

    fn rec(key: &str, cat: Category) -> Record {
        let mut r = Record::new("jlc", key);
        r.category = Some(cat);
        r.mpn = format!("M{key}");
        r.stock = Some(5);
        r
    }

    #[test]
    fn an_index_writes_swaps_and_reads_back() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("jlc");
        let recs = vec![
            rec("C2", Category::Resistor),
            rec("C1", Category::Capacitor),
            rec("C3", Category::Capacitor),
            rec("C1", Category::Capacitor),
        ];
        let next = write_next(&root, &recs, 1_000, "2026-11-06T11:33:30+00:00").unwrap();
        assert!(next.ends_with("next"));
        assert!(
            read(&root).unwrap().is_none(),
            "nothing until it is swapped in"
        );
        swap(&root).unwrap();
        let idx = read(&root).unwrap().unwrap();
        assert_eq!((idx.meta.parts, idx.meta.fetched_at), (3, 1_000));
        let caps = idx.category(Category::Capacitor).unwrap();
        assert_eq!(
            caps.iter().map(|r| r.key.as_str()).collect::<Vec<_>>(),
            ["C1", "C3"]
        );
        assert_eq!(
            (caps[0].source.as_str(), caps[0].fetched_at),
            ("jlc", Some(1_000))
        );
        assert_eq!(caps[0].attribution, crate::jlc::ATTRIBUTION);
        assert!(idx.category(Category::Fuse).unwrap().is_empty());
        let keys = idx.keys().unwrap();
        assert_eq!(keys.by_key.len(), 3);
        assert_eq!(keys.by_key[1], ("C2".to_owned(), Category::Resistor));
        assert_eq!(keys.by_mpn[0], ("mc1".to_owned(), "C1".to_owned()));
        // A second sync replaces it whole; a stopped swap leaves old/ readable.
        write_next(&root, &recs[..1], 2_000, "x").unwrap();
        std::fs::rename(root.join("current"), root.join("old")).unwrap();
        assert_eq!(read(&root).unwrap().unwrap().meta.fetched_at, 1_000);
        swap(&root).unwrap();
        assert_eq!(read(&root).unwrap().unwrap().meta.parts, 1);
        assert!(!root.join("old").exists() && !root.join("next").exists());
        // A newer ryueda's index is no index.
        let meta = root.join("current/meta.json");
        let t = std::fs::read_to_string(&meta)
            .unwrap()
            .replace("\"format\": 1", "\"format\": 2");
        std::fs::write(&meta, t).unwrap();
        assert!(read(&root).unwrap().is_none());
    }

    /// Every category file holds what the meta counts, and the keys list
    /// every record: the index is one sync's, whole.
    fn whole(root: &Path) -> Meta {
        let idx = read(root).unwrap().expect("an index");
        let mut n = 0;
        for (c, count) in &idx.meta.categories {
            let got = idx.category(Category::parse(c).unwrap()).unwrap();
            assert_eq!(got.len() as u64, *count, "{c}");
            n += got.len();
        }
        assert_eq!(idx.keys().unwrap().by_key.len(), n);
        assert_eq!(n as u64, idx.meta.parts);
        idx.meta
    }

    #[test]
    fn two_syncs_at_once_leave_one_index_whole() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("jlc");
        let batch = |tag: &str, n: usize| -> Vec<Record> {
            (0..n)
                .map(|i| {
                    let cat = [Category::Resistor, Category::Capacitor, Category::Diode][i % 3];
                    rec(&format!("C{tag}{i}"), cat)
                })
                .collect()
        };
        let (a, b) = (batch("a", 3_000), batch("b", 1_500));
        for round in 0..8 {
            std::thread::scope(|s| {
                let ha = s.spawn(|| install(&root, &a, 1, "a"));
                let hb = s.spawn(|| install(&root, &b, 2, "b"));
                ha.join().unwrap().unwrap();
                hb.join().unwrap().unwrap();
            });
            let m = whole(&root);
            assert!(
                (m.parts, m.created.as_str()) == (3_000, "a")
                    || (m.parts, m.created.as_str()) == (1_500, "b"),
                "round {round}: {m:?}"
            );
            assert!(!root.join("next").exists() && !root.join("old").exists());
        }
    }
}
