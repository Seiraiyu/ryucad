//! ryueda's PDM over project bytes (design 629): parts, content hashes,
//! defaults and every part's properties never panic, and a content hash
//! is the same twice. The bytes choose the project's kind, then name
//! files of a ryueda project from a fixed list, each followed by its
//! bytes up to a zero, over a manifest that reads. Any panic is a bug.

#![no_main]

use libfuzzer_sys::fuzz_target;
use ryupdm::product::Product;
use std::collections::BTreeMap;

/// The files a part's hash, defaults and properties read.
const PATHS: [&str; 16] = [
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
    let mut files: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    let m = ryueda::project::Manifest::new(
        ryueda::id::ProjectId::from_bytes([1; 16]),
        ryueda::units::Unit::Mm,
        kind,
    );
    files.insert("ryueda.json".into(), m.to_text().into_bytes());
    let mut it = rest.split(|b| *b == 0);
    while let Some(head) = it.next() {
        let Some((&at, bytes)) = head.split_first() else {
            continue;
        };
        files.insert(PATHS[usize::from(at) % PATHS.len()].to_owned(), bytes.to_vec());
    }
    let Ok(parts) = <ryueda::Eda as Product>::parts(&files) else {
        return;
    };
    for part in &parts {
        let a = <ryueda::Eda as Product>::content(&files, part).ok();
        assert_eq!(a, <ryueda::Eda as Product>::content(&files, part).ok());
        let _ = <ryueda::Eda as Product>::defaults(&files, part);
        for p in ["designs/d.json", "library/components/c.json", "blocks/top/block.json"] {
            let _ = <ryueda::Eda as Product>::part_of(p);
            let _ = <ryueda::Eda as Product>::shared(p);
        }
    }
    let _ = ryupdm::props::Pdm::of_for::<ryueda::Eda>(&files, parts, ryueda::units::Unit::Mm);
});
