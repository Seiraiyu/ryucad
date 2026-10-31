//! Product structure from bytes: a project of parts and assemblies made
//! from the first bytes (`ryucad_doc::doc::fixtures::structure`), then
//! one of its files replaced by the rest, as a hand edit or a bad merge
//! might leave it. Every part's BOM, flat BOM, CSV and where-used must
//! answer without panicking; a BOM's diff with itself is empty; and while
//! no file is replaced, the flat BOM's totals are the sums the generator
//! worked out. Any panic is a bug.
//!
//! With an odd first byte the rest makes projects that depend on one
//! another (`ryucad_doc::doc::fixtures::projects`), the others held in
//! memory (`ryupdm::structure::Fixed`): the root's BOMs follow their
//! parts, and while no file is replaced the flat totals are the
//! generator's across projects.

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

/// Several projects: the root's BOMs, CSV, diff and `deps uses`, with
/// every other project's files held in memory, one of the root's files
/// replaced by `tail`.
fn across(data: &[u8]) {
    use ryucad_doc::doc::fixtures::projects;
    use ryupdm::structure::Fixed;
    let (head, tail) = data.split_at(data.len().min(193));
    let mut p = projects(head);
    if let Some((&pick, bytes)) = tail.split_first() {
        let paths: Vec<String> = p.root.files.keys().cloned().collect();
        let path = &paths[pick as usize % paths.len()];
        p.root.files.insert(path.clone(), bytes.to_vec());
    }
    let Ok(project) = Project::load_from(Path::new(""), &p.root.files) else {
        return;
    };
    let config = Config::read(&p.root.files).unwrap_or_default();
    let mut others = Fixed(
        p.others
            .iter()
            .map(|(id, s)| (id.clone(), s.files.clone()))
            .collect(),
    );
    let mut cache = Cache::new();
    let mut r =
        Reader::new(&p.root.files, &project, None, None, &mut cache).with_others(&mut others);
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
        if tail.is_empty() {
            assert!(ryupdm::bom::problems(&row).is_empty(), "{part}");
            let mut totals = std::collections::BTreeMap::new();
            for f in &rows {
                *totals.entry(f.row.key().to_string()).or_insert(0) += f.total;
            }
            assert_eq!(totals, p.totals(part.as_str()), "{part}");
        }
    }
    ryupdm::where_used::placed_from_others(&mut r, &config);
}

fuzz_target!(|data: &[u8]| {
    if let Some((&first, rest)) = data.split_first()
        && first % 2 == 1
    {
        across(rest);
        return;
    }
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
            project: None,
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
