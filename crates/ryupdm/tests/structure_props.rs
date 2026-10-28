//! BOMs and where-used of seeded random projects
//! (`ryucad_doc::doc::fixtures::structure`) against sums worked out from
//! what the generator says it wrote: the flat BOM's totals, a diff's
//! rows, and where-used's counts.

use ryucad_doc::doc::assembly::Rev;
use ryucad_doc::doc::fixtures::{Structure, structure};
use ryucad_doc::doc::id::Id;
use ryucad_doc::doc::project::Project;
use ryupdm::bom::{diff, flat, tree};
use ryupdm::config::Config;
use ryupdm::structure::{Cache, Reader};
use ryupdm::where_used::{Query, where_used};
use std::collections::BTreeMap;
use std::path::Path;

/// Seeded bytes: xorshift64*.
fn bytes(seed: u64) -> Vec<u8> {
    let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    (0..96)
        .map(|_| {
            x ^= x >> 12;
            x ^= x << 25;
            x ^= x >> 27;
            (x.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 56) as u8
        })
        .collect()
}

fn id(s: &str) -> Id {
    Id::parse(s, "part").unwrap()
}

/// How many times `target` is placed under `part`, at any depth, looking
/// inside every assembly but `target`.
fn uses(s: &Structure, part: &str, target: &str) -> u64 {
    let Some((_, children)) = s.assemblies.iter().find(|(a, _)| a == part) else {
        return 0;
    };
    children
        .iter()
        .map(|(c, n)| {
            if c == target {
                *n
            } else {
                n * uses(s, c, target)
            }
        })
        .sum()
}

/// The flat BOM's totals, read from the files.
fn read(s: &Structure, root: &str) -> BTreeMap<String, u64> {
    let project = Project::load_from(Path::new(""), &s.files).unwrap();
    let mut cache = Cache::new();
    let mut r = Reader::new(&s.files, &project, None, None, &mut cache);
    let row = tree(&mut r, &id(root), &Rev::Working, None).unwrap();
    assert!(ryupdm::bom::problems(&row).is_empty());
    let mut out = BTreeMap::new();
    for f in flat(&row, &Config::default()) {
        *out.entry(f.row.part.to_string()).or_default() += f.total;
    }
    out
}

#[test]
fn flat_totals_are_the_sums_along_every_path() {
    let (mut patterns, mut bought, mut metres) = (0, 0, 0);
    for seed in 0..400 {
        let s = structure(&bytes(seed));
        let has = |what: &str| {
            s.files
                .values()
                .any(|b| String::from_utf8_lossy(b).contains(what))
        };
        patterns += has("instance_pattern") as u32;
        bought += !s.bought.is_empty() as u32;
        metres += has("\"amount\"") as u32;
        let root = &s.assemblies.last().unwrap().0;
        assert_eq!(read(&s, root), s.totals(root), "seed {seed}");
    }
    // The seeds reach every case.
    assert!(
        patterns > 100 && bought > 50 && metres > 100,
        "{patterns} {bought} {metres}"
    );
}

#[test]
fn a_diff_lists_exactly_the_rows_that_differ() {
    let config = Config::default();
    for seed in 0..200 {
        let (a, b) = (structure(&bytes(seed)), structure(&bytes(seed + 1000)));
        let root = "a0";
        let (ta, tb) = (read(&a, root), read(&b, root));
        let load = |s: &Structure| {
            let project = Project::load_from(Path::new(""), &s.files).unwrap();
            let mut cache = Cache::new();
            let mut r = Reader::new(&s.files, &project, None, None, &mut cache);
            tree(&mut r, &id(root), &Rev::Working, None).unwrap()
        };
        let d = diff(Some(&load(&a)), Some(&load(&b)), &config);
        let list = |k: &str| -> Vec<String> {
            d["flat"][k]
                .as_array()
                .unwrap()
                .iter()
                .map(|r| r["part"].as_str().unwrap().to_owned())
                .collect()
        };
        let mut added: Vec<String> = tb
            .keys()
            .filter(|k| !ta.contains_key(*k))
            .cloned()
            .collect();
        let mut removed: Vec<String> = ta
            .keys()
            .filter(|k| !tb.contains_key(*k))
            .cloned()
            .collect();
        added.sort();
        removed.sort();
        let (mut got_added, mut got_removed) = (list("added"), list("removed"));
        got_added.sort();
        got_removed.sort();
        assert_eq!((got_added, got_removed), (added, removed), "seed {seed}");
        // Every row both have whose total differs is changed.
        for part in list("changed") {
            assert!(
                ta.contains_key(&part) && tb.contains_key(&part),
                "seed {seed}"
            );
        }
        for (part, n) in &ta {
            if tb.get(part).is_some_and(|m| m != n) {
                assert!(list("changed").contains(part), "seed {seed}: {part}");
            }
        }
    }
}

#[test]
fn where_used_counts_what_the_flat_bom_counts() {
    for seed in 0..200 {
        let s = structure(&bytes(seed));
        let root = s.assemblies.last().unwrap().0.clone();
        if s.bought.contains(&root) {
            continue;
        }
        let project = Project::load_from(Path::new(""), &s.files).unwrap();
        let mut cache = Cache::new();
        let mut r = Reader::new(&s.files, &project, None, None, &mut cache);
        for (leaf, n) in read(&s, &root) {
            let q = Query {
                part: id(&leaf),
                rev: None,
                direct: false,
            };
            let v = where_used(&mut r, &q, &Config::default());
            let at_root: u64 = v["working"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|w| w["assembly"] == root.as_str())
                .map(|w| w["count"].as_u64().unwrap())
                .sum();
            // Where-used looks inside bought assemblies too.
            assert_eq!(at_root, uses(&s, &root, &leaf), "seed {seed}: {leaf}");
            assert!(n > 0);
        }
    }
}
