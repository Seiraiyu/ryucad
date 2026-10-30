//! STEP from the window against a real daemon: an import planned, then
//! made, its feature shown with its file and product; an update of the
//! file planned and made; and the part exported where the person chose,
//! by the daemon, as STEP and as every other format File > Export…
//! offers.

mod common;

use common::{Project, ryucad};
use ryucad_app::core::export::Format;
use ryucad_app::core::step::Import;
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
    ws.open_export("part-2");
    ws.export.as_mut().unwrap().path = path.display().to_string();
    ws.export.as_mut().unwrap().choices.step_unit = "m".into();
    ws.run_export();
    assert!(ws.wait_until(WAIT, |ws| {
        ws.export
            .as_ref()
            .is_some_and(|e| e.done.is_some() || e.error.is_some())
    }));
    let e = ws.export.as_ref().unwrap();
    assert!(e.error.is_none(), "{:?}", e.error);
    let bytes = std::fs::read(&path).unwrap();
    let done = e.done.as_ref().unwrap();
    assert_eq!(done.bytes, bytes.len() as u64);
    assert_eq!(done.out, path.display().to_string());
    assert_eq!(done.blake3.len(), 64, "the BLAKE3 of what was written");
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

/// File > Export… in every other format: the daemon writes each file
/// where the dialog says, the path's extension following the format, and
/// the next export starts from the last one's choices.
#[test]
fn every_format_is_exported_by_the_daemon() {
    let p = Project::new().with_part();
    let mut ws = Workspace::open(&p.root, ryucad(), "Dana", Arc::new(|| {})).unwrap();
    assert!(ws.wait_until(WAIT, |ws| ws.idle() && ws.part("p").is_some()));
    let out = tempfile::tempdir().unwrap();
    let mut path = out.path().join("p.step").display().to_string();
    for (format, encoding, ext, magic) in [
        (Format::Stl, "usda", "stl", &b"ryucad "[..]),
        (Format::ThreeMf, "usda", "3mf", &b"PK\x03\x04"[..]),
        (Format::Usd, "usda", "usda", &b"#usda 1.0"[..]),
        (Format::Usd, "usdc", "usdc", &b"PXR-USDC"[..]),
        (Format::Usd, "usdz", "usdz", &b"PK\x03\x04"[..]),
    ] {
        ws.open_export("p");
        let e = ws.export.as_mut().unwrap();
        e.path = path.clone();
        e.choices.format = format;
        e.choices.encoding = encoding.into();
        e.choices.chord = "0.1 mm".into();
        e.follow_format();
        assert!(e.path.ends_with(&format!("p.{ext}")), "{}", e.path);
        path = e.path.clone();
        ws.run_export();
        assert!(ws.wait_until(WAIT, |ws| {
            ws.export
                .as_ref()
                .is_some_and(|e| e.done.is_some() || e.error.is_some())
        }));
        let e = ws.export.as_ref().unwrap();
        assert!(e.error.is_none(), "{ext}: {:?}", e.error);
        let bytes = std::fs::read(&path).unwrap();
        assert!(bytes.starts_with(magic), "{ext}");
        let done = e.done.as_ref().unwrap();
        assert_eq!(done.bytes, bytes.len() as u64, "{ext}");
        // Meshes are counted where the file holds them.
        assert!(done.triangles.is_some_and(|t| t > 0), "{ext}");
    }
    // The next export starts where the last left off.
    ws.open_export("p");
    let c = &ws.export.as_ref().unwrap().choices;
    assert_eq!((c.format, c.encoding.as_str(), c.chord.as_str()), (Format::Usd, "usdz", "0.1 mm"));
    // A chord the daemon refuses comes back as its refusal, and nothing
    // is written.
    let e = ws.export.as_mut().unwrap();
    e.choices.chord = "1 nm".into();
    e.path = out.path().join("refused.usdz").display().to_string();
    ws.run_export();
    assert!(ws.wait_until(WAIT, |ws| {
        ws.export.as_ref().is_some_and(|e| e.error.is_some())
    }));
    let err = ws.export.as_ref().unwrap().error.clone().unwrap();
    assert_eq!(err.code, ryucad::error::Code::InvalidParam, "{err:?}");
    assert!(!out.path().join("refused.usdz").exists());
}
