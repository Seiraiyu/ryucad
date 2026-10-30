//! STEP from the window against a real daemon: an import planned, then
//! made, its feature shown with its file and product; an update of the
//! file planned and made; and the part exported where the person chose.

mod common;

use common::{Project, ryucad};
use ryucad_app::core::step::{Export, Import};
use ryucad_app::core::workspace::Workspace;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

const WAIT: Duration = Duration::from_secs(60);

fn occt(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../ryustep/tests/data/occt")
        .join(name)
}

#[test]
fn a_file_is_planned_imported_updated_and_exported() {
    let p = Project::new();
    let mut ws = Workspace::open(&p.root, ryucad(), "Dana", Arc::new(|| {})).unwrap();
    assert!(ws.wait_until(WAIT, |ws| ws.idle()));
    // Planned: nothing is written.
    ws.import = Some(Import::new());
    ws.import.as_mut().unwrap().path = occt("box_ap214.stp").display().to_string();
    ws.plan_import();
    assert!(ws.wait_until(WAIT, |ws| {
        ws.import
            .as_ref()
            .is_some_and(|i| !i.waiting && i.current().is_some())
    }));
    let plan = ws.import.as_ref().unwrap().current().unwrap().clone();
    assert_eq!(plan.stored, "imports/box_ap214.step");
    assert_eq!(plan.parts.len(), 1);
    assert_eq!(plan.parts[0].0, "part-2 (new part, product part 2)");
    assert_eq!(
        plan.parts[0].1,
        ["body (body 0): 6 faces, 12 edges, 8 vertices"]
    );
    assert!(!p.root.join("imports").exists(), "a plan writes nothing");
    // Imported: the tree shows the import with its file and product.
    ws.run_import();
    assert!(ws.wait_until(WAIT, |ws| {
        ws.import.as_ref().is_some_and(|i| i.done.is_some())
            && ws
                .part("part-2")
                .and_then(|p| p.feature("body"))
                .is_some_and(|f| f.import.is_some())
    }));
    let f = ws.part("part-2").unwrap().feature("body").unwrap().clone();
    let info = f.import.unwrap();
    assert_eq!(info.file, "imports/box_ap214.step");
    assert_eq!(info.product.as_ref().unwrap().0, "part 2");
    assert!(info.heal.iter().any(|(k, _)| k == "sewn"));
    assert_eq!(
        ryucad_app::ui::tiles::tree::row_text(
            ws.part("part-2").unwrap().feature("body").unwrap(),
            0
        ),
        "body (import: box_ap214.step / part 2)"
    );
    // Exported where the person chose, as the CLI would write it.
    let out = tempfile::tempdir().unwrap();
    let path = out.path().join("box.step");
    ws.export = Some(Export::new("part-2"));
    ws.export.as_mut().unwrap().path = path.display().to_string();
    ws.export.as_mut().unwrap().unit = "m".into();
    ws.run_export();
    assert!(ws.wait_until(WAIT, |ws| {
        ws.export
            .as_ref()
            .is_some_and(|e| e.done.is_some() || e.error.is_some())
    }));
    let e = ws.export.as_ref().unwrap();
    assert!(e.error.is_none(), "{:?}", e.error);
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(e.done.as_ref().unwrap().1, bytes.len());
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains("'box.step'"), "the header names the file");
    assert!(text.contains(".METRE."), "in meters");
    // An update's plan says what would stop building: the AP203 box is
    // product `part 3`, so `part 2`'s body would be gone.
    ws.import = Some(Import::update("imports/box_ap214.step"));
    ws.import.as_mut().unwrap().path = occt("box_ap203.stp").display().to_string();
    ws.plan_import();
    assert!(ws.wait_until(WAIT, |ws| {
        ws.import
            .as_ref()
            .is_some_and(|i| !i.waiting && i.current().is_some())
    }));
    let plan = ws.import.as_ref().unwrap().current().unwrap().clone();
    assert_eq!(plan.stored, "imports/box_ap214.step");
    assert_eq!(plan.failing, ["part-2/body: import_product_missing"]);
    assert_eq!(plan.vanished, ["part 2 (in part-2)"]);
    // Updated with its own export instead: the same product, so the
    // feature only takes the new bytes' hash.
    let new = ryucad::doc::project::ImportFile::new(bytes).blake3;
    ws.import.as_mut().unwrap().path = path.display().to_string();
    ws.plan_import();
    assert!(ws.wait_until(WAIT, |ws| {
        ws.import
            .as_ref()
            .is_some_and(|i| !i.waiting && i.current().is_some())
    }));
    assert!(
        ws.import
            .as_ref()
            .unwrap()
            .current()
            .unwrap()
            .failing
            .is_empty()
    );
    ws.run_import();
    assert!(ws.wait_until(WAIT, |ws| {
        ws.part("part-2")
            .and_then(|p| p.feature("body"))
            .and_then(|f| f.import.as_ref())
            .is_some_and(|i| i.blake3 == new)
            && ws.import.as_ref().is_some_and(|i| i.done.is_some())
    }));
    let f = ws.part("part-2").unwrap().feature("body").unwrap();
    assert!(f.error.is_none(), "{:?}", f.error);
    // A file that isn't STEP is refused in the plan.
    ws.import = Some(Import::new());
    let junk = out.path().join("junk.step");
    std::fs::write(&junk, b"not STEP").unwrap();
    ws.import.as_mut().unwrap().path = junk.display().to_string();
    ws.plan_import();
    assert!(ws.wait_until(WAIT, |ws| {
        ws.import.as_ref().is_some_and(|i| !i.waiting)
    }));
    let i = ws.import.as_ref().unwrap();
    assert_eq!(
        i.error.as_ref().unwrap().code,
        ryucad::error::Code::StepUnreadable
    );
}
