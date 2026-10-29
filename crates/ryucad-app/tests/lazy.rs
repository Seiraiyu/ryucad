//! The window over a project too large to load whole, against a real
//! daemon: design 192's benchmark project at 1,000 parts (20
//! subassemblies of 50 parts and 500 occurrences each, under 2 modules
//! and a root; no imported race bodies, which take minutes to make). The
//! window lists every part and loads only the root it shows, with a mesh
//! for every body its 10,000 occurrences place, fetched coarse first (the
//! view draws more than `lod::COARSE_FIRST`) and compact; a part's report
//! comes when its row opens. Release builds only: the daemon builds every part
//! first.

mod common;

use common::ryucad;
use ryucad::bench::scale::{Spec, write};
use ryucad_app::core::workspace::Workspace;
use std::sync::Arc;
use std::time::{Duration, Instant};

const WAIT: Duration = Duration::from_secs(900);

#[test]
#[cfg_attr(debug_assertions, ignore = "minutes in a debug build")]
fn a_large_project_loads_what_the_window_shows() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("p");
    let spec = Spec {
        races: false,
        ..Spec::parts(1000)
    };
    let made = write(&root, &spec, &[]);
    let mut ws = Workspace::open(&root, ryucad(), "Dana", Arc::new(|| {})).unwrap();
    let t = Instant::now();
    let drawn = |ws: &Workspace| {
        ws.part("root").is_some_and(|r| {
            r.occurrences.len() == made.occurrences
                && r.solids.iter().all(|s| {
                    ws.model
                        .meshes
                        .contains_key(&("root".to_owned(), s.id.clone()))
                })
        })
    };
    assert!(ws.wait_until(WAIT, |ws| ws.idle()
        && ws.model.listed.len() == made.parts + made.assemblies
        && drawn(ws)));
    println!(
        "the window over {} parts and {} occurrences: every body meshed in {:.1} s, {:.1} MB \
         fetched",
        made.parts,
        made.occurrences,
        t.elapsed().as_secs_f64(),
        ws.received as f64 / 1e6
    );
    // Coarse first: every mesh the window holds is coarse.
    let base = ws.model.base;
    assert!(
        ws.model
            .meshes
            .values()
            .all(|m| { ryucad_app::core::lod::level_of(m, base) == ryucad::mesh::Quality::Coarse })
    );
    // Only the root was loaded: every other part is a line in the tree.
    let loaded: Vec<&String> = ws.model.parts.keys().collect();
    assert_eq!(loaded, ["root"]);
    assert_eq!(ws.model.shown(), ["root"]);
    // A row opened loads its part, once.
    ws.load_part("sub-000");
    ws.load_part("sub-000");
    assert!(ws.wait_until(WAIT, |ws| ws.idle() && ws.part("sub-000").is_some()));
    assert_eq!(ws.part("sub-000").unwrap().occurrences.len(), 500);
    assert_eq!(ws.model.parts.len(), 2);
}
