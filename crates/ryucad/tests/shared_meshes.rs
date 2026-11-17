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
use std::time::Instant;

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
            let i = f
                .as_ref()
                .unwrap()
                .import()
                .expect("a flattened part imports");
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
/// Healed under each feature's own key, copies came out alone (the
/// block's second copy in `assembly.stp` with flipped face senses, 4 of
/// 12 on the seeded board): ryukei's healer breaks ties by names, which
/// follow the key. A placed import is healed under its shape key and
/// renamed, so none is. Copies that share it share entity ids:
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
    assert_eq!(
        alone, 0,
        "copies of one component healed to different shapes"
    );
}

#[test]
fn copies_of_one_shape_are_meshed_once_and_moved() {
    let (_dir, mut s) = common::project();
    must(
        &mut s,
        "import",
        json!({"path": occt("assembly.stp"), "flatten": true, "part": "board"}),
    );
    let board = Id::parse("board", "part").unwrap();
    let normal = ryucad::mesh::Quality::Normal;
    let before = s.caches().meshes().made();
    let meshes = ryucad::scene::meshes(&mut s, &board, None, normal).unwrap();
    assert_eq!(
        s.caches().meshes().made() - before,
        2,
        "the block's mesh and the pin's, the block's moved to its second copy"
    );
    let c = components(&mut s, "board");
    let (_, blocks) = c
        .iter()
        .find(|(_, v)| v.len() == 2)
        .expect("the block twice");
    let mesh_of = |solid: &str| {
        let id = solid.rsplit('/').next().unwrap();
        meshes
            .iter()
            .find(|(f, _)| f.as_str() == id)
            .map(|(_, m)| m.as_ref().unwrap().mesh.clone())
            .unwrap()
    };
    let (a, b) = (mesh_of(&blocks[0].0), mesh_of(&blocks[1].0));
    let (ma, mb) = (a.mesh.as_ref().unwrap(), b.mesh.as_ref().unwrap());
    assert_eq!(ma.triangles, mb.triangles, "one mesh");
    assert_eq!(
        a.params.chord.to_bits(),
        b.params.chord.to_bits(),
        "one chord"
    );
    // Each copy's mesh is the shared one moved by its own pose.
    let (ra, rb) = (
        blocks[0].1.moved.as_ref().unwrap(),
        blocks[1].1.moved.as_ref().unwrap(),
    );
    let back = |p: ryukei::geom::Point3, r: &ryukei::geom::Rigid| r.inverse().apply_point(p);
    for (pa, pb) in ma.positions.iter().zip(&mb.positions) {
        let (qa, qb) = (back(*pa, &ra.pose), back(*pb, &rb.pose));
        assert!((qa - qb).length() < 1e-12, "{qa:?} {qb:?}");
    }
    // Asked again: nothing is made.
    let before = s.caches().meshes().made();
    ryucad::scene::meshes(&mut s, &board, None, normal).unwrap();
    assert_eq!(s.caches().meshes().made(), before);
}

/// A flattened part's STL: the block's two copies come from one mesh, so
/// the file holds the same number of triangles for each.
#[test]
fn a_flattened_parts_export_meshes_each_shape_once() {
    let (dir, mut s) = common::project();
    must(
        &mut s,
        "import",
        json!({"path": occt("assembly.stp"), "flatten": true, "part": "board"}),
    );
    let c = components(&mut s, "board");
    let bodies: Vec<Arc<Built>> = c.values().flatten().map(|(_, b)| Arc::clone(b)).collect();
    let refs: Vec<&Built> = bodies.iter().map(Arc::as_ref).collect();
    let params = ryukei::mesh::MeshParams::new(5e-5, 0.35);
    let mut cache = ryucad::mesh::Cache::with_disk(None);
    let made = cache.export_all(&refs, &params, usize::MAX);
    assert_eq!(cache.made(), 2, "the block and the pin");
    let (_, blocks) = c.iter().find(|(_, v)| v.len() == 2).unwrap();
    let count = |b: &Arc<Built>| {
        let k = bodies.iter().position(|x| Arc::ptr_eq(x, b)).unwrap();
        made[k].mesh.as_ref().unwrap().triangles.len()
    };
    assert_eq!(count(&blocks[0].1), count(&blocks[1].1));
    // And the export itself writes.
    let out = dir.path().join("board.stl");
    must(
        &mut s,
        "export",
        json!({"part": "board", "out": out.display().to_string(), "chord": "0.05 mm"}),
    );
    assert!(std::fs::metadata(&out).unwrap().len() > 84);
}

/// Design 698: a flattened reference is weighed from its moved mesh. The
/// block's two copies weigh alike within their bounds; the printed
/// values, before the change and after, are what the PR states.
#[test]
fn a_flattened_references_mass_reads_its_moved_mesh() {
    let (_dir, mut s) = common::project();
    must(
        &mut s,
        "materials.set",
        json!({"name": "steel", "density": "7850 kg/m^3", "default": true}),
    );
    must(
        &mut s,
        "import",
        json!({"path": occt("assembly.stp"), "flatten": true, "part": "board"}),
    );
    let c = components(&mut s, "board");
    let mut seen = Vec::new();
    for (component, copies) in &c {
        for (solid, _) in copies {
            let v = must(&mut s, "mass", json!({"of": solid}));
            let (vol, bound) = (
                v["volume"]["m3"].as_f64().unwrap(),
                v["volume"]["bound_m3"].as_f64().unwrap_or(0.0),
            );
            println!("{component} {solid}: volume {vol:e} m3 ± {bound:e}");
            seen.push((component.clone(), vol, bound));
        }
    }
    for w in seen.windows(2) {
        if w[0].0 == w[1].0 {
            assert!(
                (w[0].1 - w[1].1).abs() <= w[0].2 + w[1].2,
                "two copies of {} weigh apart: {:?}",
                w[0].0,
                w
            );
        }
    }
}

/// `mesh.get` says, for each moved body, the hash of the mesh it is moved
/// from and the pose that moves it: the block's copies name one `shape`;
/// a body nothing moves says neither. The binary encodings don't change.
#[test]
fn mesh_get_says_each_bodys_shape_and_pose() {
    let (_dir, mut s) = common::project();
    must(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "box", "id": "b",
               "params": {"x": "1 mm", "y": "1 mm", "z": "1 mm"}}),
    );
    must(
        &mut s,
        "import",
        json!({"path": occt("assembly.stp"), "flatten": true, "part": "board"}),
    );
    let v = must(&mut s, "mesh.get", json!({"part": "board"}));
    let bodies = v["bodies"].as_array().unwrap();
    assert_eq!(bodies.len(), 3);
    let mut shapes = std::collections::BTreeMap::<String, usize>::new();
    for b in bodies {
        let shape = b["shape"]
            .as_str()
            .expect("a flattened body says its shape");
        assert_eq!(shape.len(), 32);
        assert!(b["pose"]["origin_m"].is_array() && b["pose"]["z"].is_array());
        assert_ne!(b["shape"], b["hash"]);
        *shapes.entry(shape.to_owned()).or_default() += 1;
    }
    assert_eq!(
        shapes.values().copied().collect::<Vec<_>>().iter().max(),
        Some(&2)
    );
    let v = must(&mut s, "mesh.get", json!({"part": "p"}));
    assert!(v["bodies"][0].get("shape").is_none());
    assert!(v["bodies"][0].get("pose").is_none());
}

/// The bar itself with `RYUCAD_BARS`, else ten times looser.
fn slack() -> f64 {
    if std::env::var_os("RYUCAD_BARS").is_some() {
        1.0
    } else {
        10.0
    }
}

/// Design 698's bar (#813): a seeded board of 40 components placed 1,000
/// times, turned about all three axes. Flattened, it is meshed exactly
/// once per shape, every copy's display mesh has its shape's triangles
/// and chord, and meshing it takes at most twice what meshing the same
/// file imported per product (40 parts, 1,000 instances) takes. Release
/// builds only.
#[test]
#[cfg_attr(debug_assertions, ignore = "a thousand placed bodies in a debug build")]
fn a_board_of_forty_components_placed_a_thousand_times_meshes_forty() {
    use ryucad::mesh::Quality;
    const DISTINCT: usize = 40;
    const COPIES: usize = 1000;
    let src = tempfile::tempdir().unwrap();
    let file = src.path().join("board.step");
    std::fs::write(&file, common::board::board(DISTINCT, COPIES)).unwrap();
    let (_dir, mut s) = common::project();
    must(
        &mut s,
        "import",
        json!({"path": file, "flatten": true, "part": "board"}),
    );
    must(&mut s, "import", json!({"path": file, "part": "whole"}));
    // Built first: the bar times meshing, not reading the file.
    let c = components(&mut s, "board");
    let bodies: Vec<&Arc<Built>> = c.values().flatten().map(|(_, b)| b).collect();
    assert_eq!(bodies.len(), COPIES);
    let shapes: std::collections::BTreeSet<&str> = bodies
        .iter()
        .map(|b| b.moved.as_ref().unwrap().source.hash.as_str())
        .collect();
    assert_eq!(
        shapes.len(),
        DISTINCT,
        "one shape per component (the proof)"
    );
    let whole = Id::parse("whole", "part").unwrap();
    let model = s
        .model(&whole, &ryucad::doc::assembly::Rev::Working)
        .unwrap();
    let mut seen = std::collections::BTreeSet::new();
    let solids: Vec<ryucad::assembly::Solid> = model
        .occurrences
        .iter()
        .flat_map(|o| o.solids.iter().cloned())
        .filter(|sd| seen.insert(sd.built.hash.clone()))
        .collect();
    let placed: usize = model.occurrences.iter().map(|o| o.solids.len()).sum();
    assert_eq!((solids.len(), placed), (DISTINCT, COPIES));
    // Flattened: 40 meshes made, 1,000 moved.
    let board = Id::parse("board", "part").unwrap();
    let before = s.caches().meshes().made();
    let t = Instant::now();
    let flat = ryucad::scene::meshes(&mut s, &board, None, Quality::Normal).unwrap();
    let flat_s = t.elapsed().as_secs_f64();
    let flat_made = s.caches().meshes().made() - before;
    // Per product: 40 meshes, placed by the assembly.
    let refs: Vec<&ryucad::assembly::Solid> = solids.iter().collect();
    let before = s.caches().meshes().made();
    let t = Instant::now();
    ryucad::scene::occurrence_meshes(&mut s, &refs, Quality::Normal).unwrap();
    let whole_s = t.elapsed().as_secs_f64();
    let whole_made = s.caches().meshes().made() - before;
    let mut by_shape: BTreeMap<String, (usize, u64)> = BTreeMap::new();
    let mut triangles = 0;
    for (solid, m) in &flat {
        let m = m.as_ref().unwrap_or_else(|e| panic!("{solid}: {e}"));
        let (shape, _) = m.shape.as_ref().expect("moved");
        let n = m.mesh.mesh.as_ref().map_or(0, |x| x.triangles.len());
        triangles += n;
        let got = (n, m.mesh.params.chord.to_bits());
        let want = *by_shape.entry(shape.clone()).or_insert(got);
        assert_eq!(got, want, "{solid}: its shape's triangles and chord");
    }
    println!(
        "shared-mesh bar on {}: {COPIES} placements of {DISTINCT} components, {triangles} \
         triangles; flattened {flat_s:.3} s ({flat_made} meshes made), per product \
         {whole_s:.3} s ({whole_made} made), {:.2} times",
        bar_project::machine(),
        flat_s / whole_s,
    );
    // Every number is printed before any bar is judged.
    assert_eq!(flat_made as usize, DISTINCT, "one mesh per shape");
    assert_eq!(whole_made as usize, DISTINCT);
    assert!(
        flat_s <= 2.0 * whole_s * slack(),
        "flattened {flat_s:.3} s, per product {whole_s:.3} s"
    );
}
