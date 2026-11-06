//! A sync: the manifest, the lookup table and every category's shards,
//! one file after another (each a step, where a cancel stops it), read
//! into records and written as `next/`, then swapped in. Anything that
//! fails leaves the index that was there.

use super::wire::{Lut, Manifest};
use crate::catalogue::{Progress, Synced};
use crate::error::Result;
use crate::fetch::Fetch;
use crate::source::Call;
use std::path::Path;

/// The catalogue at `base` through `fetch` into the index at `root` (the
/// parts cache's `jlc/`), as of `now`; `progress` is called before each
/// file.
pub fn sync(
    fetch: &dyn Fetch,
    base: &str,
    root: &Path,
    now: u64,
    call: &Call,
    progress: Progress,
) -> Result<Synced> {
    let get = |name: &str| {
        call.go_on()?;
        fetch
            .get(&format!("{base}/{name}"), call)
            .map_err(|e| e.with("file", name))
    };
    progress(0, 0, "manifest.json")?;
    let raw = get("manifest.json")?;
    let m = Manifest::read(&raw).map_err(|e| e.with("file", "manifest.json"))?;
    let total = 2 + m.shards().count();
    let mut bytes = raw.len() as u64;
    progress(1, total, &m.attributes_lut)?;
    let lut_raw = get(&m.attributes_lut)?;
    bytes += lut_raw.len() as u64;
    let lut = Lut::read(&lut_raw).map_err(|e| e.with("file", m.attributes_lut.as_str()))?;
    let mut records = Vec::new();
    let mut skipped = 0u64;
    for (i, (cat, shard)) in m.shards().enumerate() {
        progress(2 + i, total, shard)?;
        let body = get(shard)?;
        bytes += body.len() as u64;
        let rows = super::rows::read_shard(&body, &lut, cat).map_err(|e| e.with("file", shard))?;
        skipped += rows.skipped as u64;
        records.extend(rows.records);
    }
    call.go_on()?;
    super::index::write_next(root, &records, now, &m.created)?;
    super::index::swap(root)?;
    // As the index counts them: a key the catalogue gives twice is kept once.
    let parts = super::index::read(root)?.map_or(0, |i| i.meta.parts);
    Ok(Synced {
        parts,
        files: total as u64,
        bytes,
        skipped_rows: skipped,
        created: m.created.clone(),
        fetched_at: now,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::{Code, Error};
    use crate::jlc::{index, testing};

    #[test]
    fn a_sync_builds_the_index_and_a_stopped_one_keeps_the_old() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("jlc");
        let fake = testing::fake("2026-11-06T11:33:30+00:00");
        let mut steps = Vec::new();
        let out = sync(&fake, testing::BASE, &root, 5_000, &Call::default(), &mut |done, total, item| {
            steps.push((done, total, item.to_owned()));
            Ok(())
        })
        .unwrap();
        assert_eq!((out.parts, out.files, out.skipped_rows), (5, 5, 0));
        assert!(out.bytes > 0);
        assert_eq!(steps.len(), 5);
        assert_eq!(steps[0], (0, 0, "manifest.json".to_owned()), "the total is known once the manifest is in");
        assert_eq!(steps[4], (4, 5, "components-regs-001.jsonl.gz".to_owned()));
        let idx = index::read(&root).unwrap().unwrap();
        assert_eq!((idx.meta.parts, idx.meta.fetched_at), (5, 5_000));
        assert_eq!(idx.meta.categories["capacitor"], 2);
        assert_eq!(idx.meta.categories["regulator"], 1);
        // Stopped part way: the old index stands whole.
        let mut n = 0;
        let e = sync(&fake, testing::BASE, &root, 6_000, &Call::default(), &mut |_, _, _| {
            n += 1;
            if n == 3 { Err(Error::new(Code::Cancelled, "cancelled")) } else { Ok(()) }
        })
        .unwrap_err();
        assert_eq!(e.code, Code::Cancelled);
        assert_eq!(index::read(&root).unwrap().unwrap().meta.fetched_at, 5_000);
        // A shard that doesn't read fails the sync naming it and keeps the old index.
        let mut broken = testing::fake("2026-11-07T00:00:00+00:00");
        broken.put(&format!("{}/components-res-001.jsonl.gz", testing::BASE), b"not gzip".to_vec());
        let e = sync(&broken, testing::BASE, &root, 7_000, &Call::default(), &mut |_, _, _| Ok(())).unwrap_err();
        assert_eq!(e.code, Code::PartsSourceRefused);
        assert_eq!(e.details["file"], "components-res-001.jsonl.gz");
        assert_eq!(index::read(&root).unwrap().unwrap().meta.fetched_at, 5_000);
    }
}
