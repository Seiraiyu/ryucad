//! The `library` source: a project's library components as records, built
//! by ryueda from its files and searched here, deterministically and with
//! no network. Its records are never cached: the files are their truth.

use crate::error::Result;
use crate::query::{Hit, Query, rank};
use crate::record::{Record, Store, Terms};
use crate::source::{Call, Source};
use rayon::prelude::*;

pub struct Library {
    /// By key.
    records: Vec<Record>,
}

impl Library {
    pub const NAME: &'static str = "library";

    pub fn new(mut records: Vec<Record>) -> Library {
        records.sort_by(|a, b| a.key.cmp(&b.key));
        Library { records }
    }

    pub fn records(&self) -> &[Record] {
        &self.records
    }
}

impl Source for Library {
    fn name(&self) -> &str {
        Library::NAME
    }

    fn terms(&self) -> Terms {
        Terms {
            store: Store::Never,
            write_parameters: false,
            attribution: String::new(),
        }
    }

    fn search(&self, q: &Query, call: &Call) -> Result<Vec<Hit>> {
        call.go_on()?;
        let mut hits: Vec<Hit> = self.records.par_iter().filter_map(|r| q.judge(r)).collect();
        rank(&mut hits);
        hits.truncate(q.limit);
        Ok(hits)
    }

    fn get(&self, key: &str, call: &Call) -> Result<Option<Record>> {
        call.go_on()?;
        Ok(self
            .records
            .binary_search_by(|r| r.key.as_str().cmp(key))
            .ok()
            .map(|i| self.records[i].clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::category::Category;
    use crate::query::{Condition, LIMIT};
    use crate::source::Key;
    use std::sync::atomic::AtomicBool;

    fn records(n: usize) -> Vec<Record> {
        (0..n)
            .map(|i| {
                let mut r = Record::new(Library::NAME, &format!("lib/self/libcomp/c{i:04}"));
                r.category = Some(Category::Capacitor);
                r.parameters.insert(
                    "capacitance".into(),
                    if i % 3 == 0 {
                        "100 nF".into()
                    } else {
                        "1 uF".into()
                    },
                );
                if i % 2 == 0 {
                    r.parameters.insert("voltage_rating".into(), "16 V".into());
                }
                r
            })
            .rev()
            .collect()
    }

    #[test]
    fn the_library_searches_alike_on_one_thread_and_eight() {
        let lib = Library::new(records(300));
        let q = Query {
            category: Some(Category::Capacitor),
            conditions: vec![
                Condition::parse("capacitance=0.1uF").unwrap(),
                Condition::parse("voltage_rating>=10V").unwrap(),
            ],
            stock: None,
            basic: false,
            limit: LIMIT,
        };
        let run = |n: usize| {
            rayon::ThreadPoolBuilder::new()
                .num_threads(n)
                .build()
                .unwrap()
                .install(|| lib.search(&q, &Call::default()).unwrap())
        };
        let one = run(1);
        assert_eq!(run(8), one);
        assert_eq!(one.len(), LIMIT);
        // Those stating both first, in key order; then those stating one.
        assert_eq!(one[0].record.key, "lib/self/libcomp/c0000");
        assert_eq!(one[0].meets, 2);
        assert_eq!(one[1].record.key, "lib/self/libcomp/c0006");
        assert_eq!(
            lib.get("lib/self/libcomp/c0007", &Call::default())
                .unwrap()
                .unwrap()
                .key,
            "lib/self/libcomp/c0007"
        );
        assert!(lib.get("nope", &Call::default()).unwrap().is_none());
        assert_eq!(lib.terms().store, crate::record::Store::Never);
        let stop = AtomicBool::new(true);
        let e = lib
            .search(
                &q,
                &Call {
                    cancel: Some(&stop),
                    deadline: None,
                },
            )
            .unwrap_err();
        assert_eq!(e.code, crate::error::Code::Cancelled);
    }

    #[test]
    fn keys_name_a_source_and_its_part() {
        let k = Key::parse("lcsc:C1525").unwrap();
        assert_eq!(
            (k.source.as_str(), k.key.as_str(), k.is_mpn()),
            ("lcsc", "C1525", false)
        );
        assert!(Key::parse("mpn:CL05B104KO5NNNC").unwrap().is_mpn());
        assert_eq!(
            Key::parse("digikey:311-1338-1-ND").unwrap().key,
            "311-1338-1-ND"
        );
        for bad in ["C1525", ":C1525", "lcsc:", "LCSC:C1525"] {
            assert_eq!(
                Key::parse(bad).unwrap_err().code,
                crate::error::Code::InvalidParam,
                "{bad}"
            );
        }
    }
}
