//! BOMs of seeded designs (design 629): populated quantities sum to the
//! netlist's populated occurrences per library component, a diff of a
//! BOM with itself is empty, and a diff after one line changes lists
//! exactly that line.

#[path = "support/generated.rs"]
mod generated;

use std::collections::BTreeMap;

#[test]
fn seeded_boms_count_diff_and_key_their_lines() {
    for seed in 0..32u64 {
        let d = tempfile::tempdir().unwrap();
        generated::seeded(d.path(), seed);
        let p = ryueda::project::Project::load(d.path()).unwrap();
        let design = ryueda::id::Id::parse("d", "design").unwrap();
        let n = ryueda::netlist::flatten(&p, &design).unwrap();
        let occs: Vec<&ryueda::netlist::Occurrence> = n.occurrences.iter().collect();
        let lines = ryueda::pdm::bom::lines_of(&p, &occs, 1);
        let mut by_line: BTreeMap<String, u64> = BTreeMap::new();
        for l in &lines {
            *by_line.entry(l.part.clone()).or_default() += l.quantity;
        }
        let mut by_net: BTreeMap<String, u64> = BTreeMap::new();
        for o in &n.occurrences {
            let id = o.of.rsplit('/').next().unwrap();
            let e = by_net.entry(format!("self/{id}")).or_default();
            if !ryueda::erc::occurrence_dnp(&p, o) {
                *e += 1;
            }
        }
        by_net.retain(|k, _| by_line.contains_key(k));
        assert_eq!(by_line, by_net, "seed {seed}");
        let config = ryupdm::config::Config::default();
        let same = ryupdm::lines::diff(&lines, &lines, &config, &ryueda::pdm::bom::COMPARED);
        assert_eq!(same["summary"]["changed"], 0, "seed {seed}");
        // One line's quantity changed: exactly that line is changed.
        let mut changed = lines.clone();
        let first = changed.iter_mut().find(|l| l.quantity > 0).unwrap();
        first.quantity -= 1;
        let d2 = ryupdm::lines::diff(&lines, &changed, &config, &ryueda::pdm::bom::COMPARED);
        assert_eq!(d2["summary"]["changed"], 1, "seed {seed}: {d2}");
        assert_eq!(d2["summary"]["added"], 0);
    }
}
