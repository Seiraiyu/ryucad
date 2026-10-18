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

/// An edge between two visible faces is picked from either side, anywhere
/// within reach, not only on its exact pixel.
#[test]
fn edges_are_picked_from_either_side() {
    // A box on its own: nothing else on screen near its edges.
    let p = Project::new();
    p.ok(&["part", "create", "p"]);
    p.ok(&[
        "feature", "add", "p", "box", "--id", "base", "--param", "x=20", "--param", "y=10",
        "--param", "z=5",
    ]);
    let mut ws = Workspace::open(&p.root, ryucad(), "Dana", Arc::new(|| {})).unwrap();
    assert!(ws.wait_until(Duration::from_secs(30), |ws| ws.idle()
        && ws.model.meshes.len() == 1));
    let picker = Picker::new(&ws.model);
    let (lo, hi) = picker.bounds().unwrap();
    let camera = view::frame(lo, hi, StandardView::Iso, view::DEFAULT_PROJECTION);
    let centre = [0, 1, 2].map(|k| (lo[k] + hi[k]) / 2.0);
    let pr = Projector::new(&camera, 1.0, centre, 0.05);
    let size = [800.0, 800.0];
    let screen = |p: [f64; 3]| {
        let n = pr.ndc(p).unwrap();
        [(n[0] + 1.0) / 2.0 * size[0], (1.0 - n[1]) / 2.0 * size[1]]
    };
    // The box's top edges, by their ends.
    let ([x0, y0, _], [x1, y1, z]) = (lo, hi);
    let edges = [
        ([x0, y0, z], [x1, y0, z]),
        ([x1, y0, z], [x1, y1, z]),
        ([x1, y1, z], [x0, y1, z]),
        ([x0, y1, z], [x0, y0, z]),
    ];
    let mut tried = 0;
    for (a, b) in edges {
        let (sa, sb) = (screen(a), screen(b));
        let mid = [(sa[0] + sb[0]) / 2.0, (sa[1] + sb[1]) / 2.0];
        let on = picker.pick(&pr, size, mid);
        if on.as_ref().map(|h| h.kind) != Some(EntityKind::Edge) {
            continue; // Hidden from this side.
        }
        tried += 1;
        let along = [sb[0] - sa[0], sb[1] - sa[1]];
        let len = (along[0] * along[0] + along[1] * along[1]).sqrt();
        let normal = [-along[1] / len, along[0] / len];
        for off in [-3.0, -1.5, 1.5, 3.0] {
            let px = [mid[0] + normal[0] * off, mid[1] + normal[1] * off];
            let hit = picker.pick(&pr, size, px).unwrap();
            assert_eq!(
                (hit.kind, &hit.entity),
                (EntityKind::Edge, &on.as_ref().unwrap().entity),
                "{off} px beside the edge {a:?}-{b:?}"
            );
        }
    }
    assert!(tried >= 2, "two top edges face the iso view");
}
