//! ryueda's PDM over project bytes (design 629): parts, content hashes,
//! defaults and every part's properties never panic, and a content hash
//! is the same twice. The bytes choose the project's kind and whether its
//! manifest pins a library's component (bit 1; in another repository with
//! bit 2, at a label the high bits choose), then name files of a ryueda
//! project from a fixed list (`ryueda.lock` among them), each followed by
//! its bytes up to a zero, over a manifest that reads. The lock reads or
//! is refused, reads back as written, and is judged with no label both
//! lacking and unused; the pins and lock diff against the manifest
//! without them. And each design's BOM: totals equal the netlist's, a
//! BOM diffed with itself is empty. Any panic is a bug.

#![no_main]

use libfuzzer_sys::fuzz_target;
use ryupdm::product::Product;
use std::collections::BTreeMap;

/// The files a part's hash, defaults and properties read.
const PATHS: [&str; 17] = [
    "ryueda.json",
    "designs/d.json",
    "blocks/top/block.json",
    "blocks/top/components/r1.json",
    "blocks/top/nets/n.json",
    "blocks/top/instances/i.json",
    "blocks/s/block.json",
    "rails/r.json",
    "classes/c.json",
    "erc.json",
    "library/components/c.json",
    "library/symbols/s.json",
    "library/footprints/f.json",
    "properties/d.json",
    "properties/c.json",
    "ryupdm.json",
    "ryueda.lock",
];

fuzz_target!(|data: &[u8]| {
    let Some((&kind, rest)) = data.split_first() else {
        return;
    };
    let kind = if kind & 1 == 0 {
        ryueda::project::ProjectKind::Design
    } else {
        ryueda::project::ProjectKind::Library
    };
    let pinned = data[0] & 2 != 0;
    let foreign = data[0] & 4 != 0;
    let label = char::from(b'A' + (data[0] >> 3) % 26).to_string();
    let mut files: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    let plain = ryueda::project::Manifest::new(
        ryueda::id::ProjectId::from_bytes([1; 16]),
        ryueda::units::Unit::Mm,
        kind,
    );
    let mut m = plain.clone();
    if pinned {
        let mut a = ryueda::project::Alias::local(if foreign { "lib" } else { "../lib" });
        a.project = Some(ryueda::id::ProjectId::from_bytes([2; 16]));
        if foreign {
            a.url = Some("file:///libs".into());
        }
        for c in ["c", "r"] {
            a.labels
                .insert(ryueda::id::Id::parse(c, "c").unwrap(), label.clone());
        }
        m.libraries
            .insert(ryueda::id::Id::parse("passives", "a").unwrap(), a);
    }
    files.insert("ryueda.json".into(), m.to_text().into_bytes());
    let mut it = rest.split(|b| *b == 0);
    while let Some(head) = it.next() {
        let Some((&at, bytes)) = head.split_first() else {
            continue;
        };
        files.insert(
            PATHS[usize::from(at) % PATHS.len()].to_owned(),
            bytes.to_vec(),
        );
    }
    lock_and_pins(&files, &plain);
    let Ok(parts) = ryupdm::product::part_ids::<ryueda::Eda>(&files) else {
        return;
    };
    for part in &parts {
        let a = <ryueda::Eda as Product>::content(&files, part).ok();
        assert_eq!(a, <ryueda::Eda as Product>::content(&files, part).ok());
        let _ = <ryueda::Eda as Product>::defaults(&files, part);
        for p in [
            "designs/d.json",
            "library/components/c.json",
            "blocks/top/block.json",
        ] {
            let _ = <ryueda::Eda as Product>::part_of(p);
            let _ = <ryueda::Eda as Product>::shared(p);
        }
    }
    let _ = ryupdm::props::Pdm::of_for::<ryueda::Eda>(&files, parts, ryueda::units::Unit::Mm);
    bom(&files);
});

/// The lock read and judged, and the pins and lock diffed against the
/// manifest `plain` (no libraries) with no lock.
fn lock_and_pins(files: &BTreeMap<String, Vec<u8>>, plain: &ryueda::project::Manifest) {
    use std::sync::Arc;
    let manifest = files
        .get("ryueda.json")
        .and_then(|b| ryueda::project::Manifest::read(b).ok());
    if let Some(bytes) = files.get("ryueda.lock")
        && let Ok(lock) = ryueda::libdeps::read_lock(bytes)
    {
        if let Some(text) = ryueda::libdeps::lock_text(&lock) {
            assert_eq!(
                ryueda::libdeps::read_lock(text.as_bytes()).ok(),
                Some(lock.clone()),
                "a lock reads back as written"
            );
        }
        if let Some(m) = &manifest {
            let (unlocked, unused) = ryueda::libdeps::judge(m, &lock);
            assert!(
                unlocked.iter().all(|w| !unused.contains(w)),
                "no label both lacking and unused"
            );
        }
    }
    let shared = |f: &BTreeMap<String, Vec<u8>>| -> BTreeMap<String, Arc<[u8]>> {
        f.iter()
            .map(|(k, v)| (k.clone(), Arc::from(v.as_slice())))
            .collect()
    };
    let before: BTreeMap<String, Arc<[u8]>> = [(
        "ryueda.json".to_owned(),
        Arc::from(plain.to_text().as_bytes()),
    )]
    .into_iter()
    .collect();
    let after = shared(files);
    let _ = ryueda::pdm::diff::pins(&before, &after);
    let _ = ryueda::pdm::diff::lock(&before, &after);
    let _ = ryueda::pdm::diff::lock(&after, &before);
}

/// Each design's BOM over the project's bytes: lines never panic, their
/// populated quantities sum to the netlist's populated occurrences, the
/// BOM diffed with itself changes nothing, and its CSV has a row per line.
fn bom(files: &BTreeMap<String, Vec<u8>>) {
    let root = std::path::Path::new("/fuzz");
    let Ok(p) =
        ryueda::project::Project::load_from(root, files, &BTreeMap::new(), &ryueda::pin::NoPins)
    else {
        return;
    };
    let config = ryupdm::config::Config::default();
    for d in p.conn.designs.keys() {
        let Ok(n) = ryueda::netlist::flatten(&p, d) else {
            continue;
        };
        let occs: Vec<&ryueda::netlist::Occurrence> = n.occurrences.iter().collect();
        let lines = ryueda::pdm::bom::lines_of(&p, &occs, 1);
        let populated = n
            .occurrences
            .iter()
            .filter(|o| ryueda::erc::of_ref(&o.of).is_some() && !ryueda::erc::occurrence_dnp(&p, o))
            .count() as u64;
        assert_eq!(lines.iter().map(|l| l.quantity).sum::<u64>(), populated, "BOM totals equal the netlist's");
        let same = ryupdm::lines::diff(&lines, &lines, &config, &ryueda::pdm::bom::COMPARED);
        assert_eq!(same["summary"]["changed"], 0);
        let csv = ryupdm::lines::csv(&lines, &config, &ryueda::pdm::bom::COLUMNS);
        assert!(csv.lines().count() >= lines.len() + 1 || csv.contains('"'));
        let _ = ryueda::pdm::bom::unidentified(d, &lines);
    }
}
