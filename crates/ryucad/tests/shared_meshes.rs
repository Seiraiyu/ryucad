//! Meshes shared across placements (#813, design 698, plan 33.C): a
//! flattened import's copies of one component share one shape hash, so
//! one mesh, moved per copy; the proof on the committed OCCT assembly
//! and a seeded board, the cache's counts, mass, and the bar.

mod bar_project;
mod common;

use common::must;
use ryucad::doc::id::Id;
use ryucad::rebuild::Built;
use ryucad::session::Session;
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::Arc;

fn occt(name: &str) -> String {
    format!(
        "{}/../ryustep/tests/data/occt/{name}",
        env!("CARGO_MANIFEST_DIR")
    )
}

/// `part`'s flattened imports by component (product and body), each
/// copy's solid as `<part>/<solid>` with its body.
fn components(s: &mut Session, part: &str) -> BTreeMap<String, Vec<(String, Arc<Built>)>> {
    let pid = Id::parse(part, "part").unwrap();
    let imports: Vec<(Id, ryucad::doc::import::Import)> = s
        .project()
        .part(&pid)
        .unwrap()
        .features
        .iter()
        .map(|(id, f)| {
            let i = f.as_ref().unwrap().import().expect("a flattened part imports");
            assert!(i.place.is_some(), "{id} is placed");
            (id.clone(), i.clone())
        })
        .collect();
    let r = s.rebuilt(&pid).unwrap().clone();
    assert_eq!(r.failed(), 0, "{:?}", r.features);
    let mut out: BTreeMap<String, Vec<(String, Arc<Built>)>> = BTreeMap::new();
    for (id, i) in imports {
        let component = match &i.product {
            Some(p) => format!("{} {} {} / {}", p.id, p.name, p.ordinal, i.body),
            None => format!("- / {}", i.body),
        };
        for (solid, b) in r.output(&id).unwrap().solids() {
            out.entry(component.clone())
                .or_default()
                .push((format!("{part}/{solid}"), Arc::clone(b)));
        }
    }
    out
}

/// How many copies have a shape other than their component's first
/// copy's (each meshed alone).
///
/// Measured 2026-11-17 (plan 33.C, Task 4): on `assembly.stp` the block's
/// second copy is alone (its unplaced body heals with flipped face senses:
/// ryukei's healer breaks ties by names, which follow each feature's key),
/// and on the seeded board 4 of 12 copies are alone. Held for a decision,
/// not weakened. Copies that share it share entity ids:
/// every face of one mesh of the first is a live face of each.
fn alone(components: &BTreeMap<String, Vec<(String, Arc<Built>)>>) -> usize {
    let params = ryukei::mesh::MeshParams::new(1e-3, 0.5);
    let mut alone = 0;
    for (component, copies) in components {
        let first = &copies[0]
            .1
            .moved
            .as_ref()
            .expect("a flattened body is moved")
            .source;
        let mesh = ryukei::mesh::tessellate(&first.body, &params).unwrap();
        for (solid, b) in copies {
            let m = b
                .moved
                .as_ref()
                .unwrap_or_else(|| panic!("{solid} is moved"));
            if m.source.hash != first.hash {
                alone += 1;
                continue;
            }
            assert_eq!(
                m.source.body.dump_unnamed(),
                first.body.dump_unnamed(),
                "{component}: {solid}"
            );
            assert_eq!(b.body.faces().count(), first.body.faces().count());
            for f in &mesh.faces {
                assert!(
                    b.body.face(f.face).is_some(),
                    "{solid}: a face of the shared mesh"
                );
            }
            for e in &mesh.edges {
                assert!(
                    b.body.edge(e.edge).is_some(),
                    "{solid}: an edge of the shared mesh"
                );
            }
        }
    }
    alone
}

#[test]
fn copies_of_a_committed_component_share_one_shape() {
    for solid in [false, true] {
        let (_dir, mut s) = common::project();
        must(
            &mut s,
            "import",
            json!({"path": occt("assembly.stp"), "flatten": true, "part": "board", "solid": solid}),
        );
        let c = components(&mut s, "board");
        let copies: usize = c.values().map(Vec::len).sum();
        assert_eq!(
            (copies, c.len()),
            (3, 2),
            "solid {solid}: the block twice, the pin once"
        );
        assert_eq!(alone(&c), 0, "solid {solid}");
    }
}

#[test]
fn copies_of_a_seeded_component_share_one_shape() {
    let src = tempfile::tempdir().unwrap();
    let file = src.path().join("board.step");
    std::fs::write(&file, common::board::board(4, 12)).unwrap();
    let (_dir, mut s) = common::project();
    must(
        &mut s,
        "import",
        json!({"path": file, "flatten": true, "part": "board"}),
    );
    let c = components(&mut s, "board");
    let copies: usize = c.values().map(Vec::len).sum();
    assert_eq!((copies, c.len()), (12, 4));
    let alone = alone(&c);
    println!(
        "seeded board: {copies} copies of {} components, {alone} meshed alone",
        c.len()
    );
    assert_eq!(alone, 0, "copies of one component healed to different shapes");
}
