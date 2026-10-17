//! Picking on the meshes a real daemon sends.

mod common;

use common::{Project, ryucad};
use ryucad::entity::EntityKind;
use ryucad::view::{self, StandardView};
use ryucad_app::core::nav::Projector;
use ryucad_app::core::pick::Picker;
use ryucad_app::core::workspace::Workspace;
use std::sync::Arc;
use std::time::Duration;

#[test]
fn the_middle_of_a_top_view_is_the_boss_top() {
    let p = Project::new().with_part();
    let mut ws = Workspace::open(&p.root, ryucad(), "Dana", Arc::new(|| {})).unwrap();
    assert!(ws.wait_until(Duration::from_secs(30), |ws| ws.idle()
        && ws.model.meshes.len() == 2));
    let picker = Picker::new(&ws.model);
    let (lo, hi) = picker.bounds().unwrap();
    // The cylinder stands on the box, centred on the origin.
    let camera = view::frame(lo, hi, StandardView::Top, view::DEFAULT_PROJECTION);
    let centre = [0, 1, 2].map(|k| (lo[k] + hi[k]) / 2.0);
    let pr = Projector::new(&camera, 1.0, centre, 0.05);
    let size = [600.0, 600.0];
    let n = pr.ndc([0.0, 0.0, hi[2]]).unwrap();
    let px = [(n[0] + 1.0) / 2.0 * size[0], (1.0 - n[1]) / 2.0 * size[1]];
    let hit = picker.pick(&pr, size, px).unwrap();
    assert_eq!((hit.kind, hit.feature.as_str()), (EntityKind::Face, "boss"));
    assert!((hit.point[2] - hi[2]).abs() < 1e-6, "{:?}", hit.point);
    // The daemon resolves the reference to a face of the cylinder.
    let name = hit.entity.rsplit('/').next().unwrap();
    let e = p.ok(&["inspect", "p", "boss", "--entity", name]);
    assert_eq!(e["kind"], "face", "{e}");
    p.ok(&["select", &hit.entity]);
}
