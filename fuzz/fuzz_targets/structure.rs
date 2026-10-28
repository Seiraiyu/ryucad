//! Product structure from bytes: a project of parts and assemblies made
//! from the first bytes (`ryucad_doc::doc::fixtures::structure`), then
//! one of its files replaced by the rest, as a hand edit or a bad merge
//! might leave it. Every part's BOM, flat BOM, CSV and where-used must
//! answer without panicking; a BOM's diff with itself is empty; and while
//! no file is replaced, the flat BOM's totals are the sums the generator
//! worked out. Any panic is a bug.

#![no_main]

use libfuzzer_sys::fuzz_target;
use ryucad::doc::assembly::Rev;
use ryucad::doc::id::Id;
use ryucad::doc::project::Project;
use ryucad_doc::doc::fixtures::structure;
use ryupdm::bom::{csv_flat, csv_tree, diff, flat, tree};
use ryupdm::config::Config;
use ryupdm::structure::{Cache, Reader};
use ryupdm::where_used::{Query, where_used};
use std::path::Path;

fuzz_target!(|data: &[u8]| {
    let (head, tail) = data.split_at(data.len().min(64));
    let mut s = structure(head);
    if let Some((&pick, bytes)) = tail.split_first() {
        let paths: Vec<String> = s.files.keys().cloned().collect();
        let path = &paths[pick as usize % paths.len()];
        s.files.insert(path.clone(), bytes.to_vec());
    }
    let Ok(project) = Project::load_from(Path::new(""), &s.files) else {
        return;
    };
    let config = Config::read(&s.files).unwrap_or_default();
    let mut cache = Cache::new();
    let mut r = Reader::new(&s.files, &project, None, None, &mut cache);
    let parts: Vec<Id> = project.parts.keys().cloned().collect();
    for part in &parts {
        let Ok(row) = tree(&mut r, part, &Rev::Working, None) else {
            continue;
        };
        let rows = flat(&row, &config);
        csv_tree(&row, &config);
        csv_flat(&rows, &config);
        ryupdm::bom::findings(&row);
        let d = diff(Some(&row), Some(&row), &config);
        assert_eq!(d["summary"]["changed"], 0, "{part}");
        assert_eq!(d["summary"]["added"], 0, "{part}");
        let q = Query {
            part: part.clone(),
            rev: None,
            direct: false,
        };
        where_used(&mut r, &q, &config);
        if tail.is_empty() {
            assert!(ryupdm::bom::problems(&row).is_empty(), "{part}");
            let mut totals = std::collections::BTreeMap::new();
            for f in &rows {
                *totals.entry(f.row.part.to_string()).or_insert(0) += f.total;
            }
            assert_eq!(totals, s.totals(part.as_str()), "{part}");
        }
    }
});
