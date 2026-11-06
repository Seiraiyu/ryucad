//! The `jlc` source: the index on this machine, loaded a category at a
//! time and kept, its records' values typed once (`query::Typed`).

use super::index::{self, Index, Keys};
use crate::category::Category;
use crate::error::Result;
use crate::query::{Hit, Query, Typed, rank};
use crate::record::{Record, Store, Terms, squash};
use crate::source::{Call, Source};
use rayon::prelude::*;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

type Loaded = Arc<Vec<(Record, Typed)>>;

pub struct Jlc {
    index: Index,
    loaded: Mutex<BTreeMap<Category, Loaded>>,
    keys: OnceLock<Keys>,
}

impl Jlc {
    /// The index at `root` (the cache's `jlc/`), `None` where there is none.
    pub fn open(root: &Path) -> Result<Option<Jlc>> {
        Ok(index::read(root)?.map(|index| Jlc {
            index,
            loaded: Mutex::default(),
            keys: OnceLock::new(),
        }))
    }

    /// `c`'s records, loaded once (searches in parallel then share them).
    fn category(&self, c: Category) -> Result<Loaded> {
        if let Some(l) = self
            .loaded
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(&c)
        {
            return Ok(l.clone());
        }
        let recs = self.index.category(c)?;
        let typed: Vec<(Record, Typed)> = recs
            .into_par_iter()
            .map(|r| {
                let t = Typed::of(&r);
                (r, t)
            })
            .collect();
        let mut loaded = self.loaded.lock().unwrap_or_else(|p| p.into_inner());
        Ok(loaded.entry(c).or_insert_with(|| Arc::new(typed)).clone())
    }

    fn categories(&self) -> Vec<Category> {
        self.index
            .meta
            .categories
            .keys()
            .filter_map(|n| Category::parse(n))
            .collect()
    }

    fn keys(&self) -> Result<&Keys> {
        if let Some(k) = self.keys.get() {
            return Ok(k);
        }
        let k = self.index.keys()?;
        Ok(self.keys.get_or_init(|| k))
    }

    fn record(&self, key: &str) -> Result<Option<Record>> {
        let keys = self.keys()?;
        let Ok(i) = keys.by_key.binary_search_by(|(k, _)| k.as_str().cmp(key)) else {
            return Ok(None);
        };
        let l = self.category(keys.by_key[i].1)?;
        Ok(l.binary_search_by(|(r, _)| r.key.as_str().cmp(key))
            .ok()
            .map(|j| l[j].0.clone()))
    }
}

impl crate::catalogue::Catalogue for Jlc {
    fn fetched_at(&self) -> u64 {
        self.index.meta.fetched_at
    }

    fn created(&self) -> &str {
        &self.index.meta.created
    }

    fn parts(&self) -> u64 {
        self.index.meta.parts
    }

    fn by_mpn(&self, mpn: &str) -> Result<Vec<Record>> {
        let want = squash(mpn);
        if want.is_empty() {
            return Ok(Vec::new());
        }
        let keys = self.keys()?;
        let from = keys
            .by_mpn
            .partition_point(|(m, _)| m.as_str() < want.as_str());
        let mut out = Vec::new();
        for (_, k) in keys.by_mpn[from..].iter().take_while(|(m, _)| *m == want) {
            out.extend(self.record(k)?);
        }
        Ok(out)
    }
}

impl Source for Jlc {
    fn name(&self) -> &str {
        super::NAME
    }

    fn terms(&self) -> Terms {
        Terms {
            store: Store::Cache,
            write_parameters: false,
            attribution: super::ATTRIBUTION.to_owned(),
        }
    }

    fn search(&self, q: &Query, call: &Call) -> Result<Vec<Hit>> {
        let cats = match q.category {
            Some(c) => vec![c],
            None => self.categories(),
        };
        let mut hits = Vec::new();
        for c in cats {
            call.go_on()?;
            let l = self.category(c)?;
            hits.extend(
                l.par_iter()
                    .filter_map(|(r, t)| q.judge_typed(r, t))
                    .collect::<Vec<_>>(),
            );
        }
        rank(&mut hits);
        hits.truncate(q.limit);
        Ok(hits)
    }

    fn get(&self, key: &str, call: &Call) -> Result<Option<Record>> {
        call.go_on()?;
        self.record(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalogue::Catalogue;
    use crate::jlc::{sync::sync, testing};
    use crate::query::{Condition, LIMIT};

    fn synced() -> (tempfile::TempDir, Jlc) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("jlc");
        sync(
            &testing::fake("2026-11-06T11:33:30+00:00"),
            testing::BASE,
            &root,
            5_000,
            &Call::default(),
            &mut |_, _, _| Ok(()),
        )
        .unwrap();
        let jlc = Jlc::open(&root).unwrap().unwrap();
        (dir, jlc)
    }

    #[test]
    fn the_catalogue_searches_and_finds_by_key_and_part_number() {
        let (d, jlc) = synced();
        assert_eq!(
            (jlc.name(), jlc.fetched_at(), jlc.parts()),
            ("jlc", 5_000, 5)
        );
        assert_eq!(jlc.created(), "2026-11-06T11:33:30+00:00");
        let q = Query {
            category: Some(Category::Capacitor),
            conditions: vec![
                Condition::parse("capacitance=0.1uF").unwrap(),
                Condition::parse("package=0402").unwrap(),
            ],
            stock: Some(1000),
            basic: true,
            limit: LIMIT,
        };
        let one = rayon::ThreadPoolBuilder::new()
            .num_threads(1)
            .build()
            .unwrap()
            .install(|| jlc.search(&q, &Call::default()).unwrap());
        assert_eq!(jlc.search(&q, &Call::default()).unwrap(), one);
        assert_eq!(
            one.iter()
                .map(|h| h.record.key.as_str())
                .collect::<Vec<_>>(),
            ["C1525"]
        );
        assert_eq!(one[0].meets, 4);
        // No category: every category of the index.
        let all = jlc
            .search(
                &Query {
                    category: None,
                    conditions: vec![],
                    stock: None,
                    basic: false,
                    limit: 50,
                },
                &Call::default(),
            )
            .unwrap();
        assert_eq!(all.len(), 5);
        assert_eq!(all[0].record.key, "C25741", "most in stock first");
        assert_eq!(
            jlc.get("C5446", &Call::default()).unwrap().unwrap().mpn,
            "TPS7A2033PDBVR"
        );
        assert!(jlc.get("C9", &Call::default()).unwrap().is_none());
        assert_eq!(jlc.by_mpn("cl05b104ko5nnnc").unwrap()[0].key, "C1525");
        assert!(jlc.by_mpn("").unwrap().is_empty());
        assert_eq!(jlc.terms().store, Store::Cache);
        assert!(Jlc::open(&d.path().join("nothing")).unwrap().is_none());
    }
}
