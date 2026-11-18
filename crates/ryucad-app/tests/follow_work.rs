//! Following the agents' work (#812) against a real daemon: an agent's
//! write far from the view brings the view to it, and a hand on the
//! camera pauses that. The viewport's own easing is `tests/window.rs`'s.

mod common;

use common::{Project, ryucad};
use ryucad::view::Camera;
use ryucad_app::core::pick::Picker;
use ryucad_app::core::work::{self, Sight};
use ryucad_app::core::workspace::Workspace;
use serde_json::json;
use std::sync::Arc;
use std::time::{Duration, Instant};

const WAIT: Duration = Duration::from_secs(30);
const ASPECT: f64 = 1.5;

/// A window on `p`, following the agents' work. Every test of this
/// binary turns it on, so they don't race over the shared preferences.
fn open(p: &Project) -> Workspace {
    let mut ws = common::open_editing(&p.root, ryucad(), "Dana", Arc::new(|| {})).unwrap();
    assert!(ws.wait_until(WAIT, |ws| ws.idle() && ws.model.meshes.len() == 2));
    ws.set_config("view.follow_work", json!(true));
    assert!(ws.wait_until(WAIT, |ws| ws.follows_work()));
    ws
}

/// The first view's camera on what the window shows.
fn framed(ws: &Workspace) -> Camera {
    let (lo, hi) = Picker::new(&ws.model).bounds().unwrap();
    ryucad::view::frame(
        lo,
        hi,
        ryucad::view::StandardView::Iso,
        ryucad::view::DEFAULT_PROJECTION,
    )
}

/// The agent adds a 5 mm box `id` at x = `x` mm, and the window has its
/// mesh.
fn far_box(p: &Project, ws: &mut Workspace, id: &str, x: i64) {
    let origin = format!("{x},0,0");
    p.ok(&[
        "feature", "add", "p", "box", "--id", id, "--param", "x=5", "--param", "y=5", "--param",
        "z=5", "--origin", &origin,
    ]);
    // Short of the box's corner by a millimetre: its mesh is f32, so its
    // far corner may round either side of it.
    let reach = (x as f64 - x.signum() as f64) / 1000.0;
    assert!(ws.wait_until(WAIT, |ws| {
        ws.idle()
            && Picker::new(&ws.model)
                .bounds()
                .is_some_and(|(lo, hi)| lo[0] <= reach.min(0.0) && hi[0] >= reach.max(0.0))
    }));
}

#[test]
fn an_agents_far_write_is_brought_into_view() {
    let p = Project::new().with_part();
    let mut ws = open(&p);
    let camera = framed(&ws);
    far_box(&p, &mut ws, "far", 400);
    let now = Instant::now();
    let picker = Picker::new(&ws.model);
    let (lo, hi) = ws
        .view
        .work
        .bounds(now, &ws.model, &picker)
        .expect("the agent's work");
    assert!(hi[0] >= 0.4, "{lo:?} {hi:?}");
    assert!(!work::in_view(&camera, ASPECT, lo, hi, work::INNER));
    let sight = Sight {
        camera: &camera,
        aspect: ASPECT,
        following: false,
        every: ws.publish_every(),
    };
    let to = ws
        .view
        .work
        .frame(now, &sight, Some((lo, hi)))
        .expect("eased out to it");
    assert!(work::in_view(&to, ASPECT, lo, hi, 1.0));
    for c in work::view_corners(&camera, ASPECT) {
        assert!(work::in_view(&to, ASPECT, c, c, 1.0 + 1e-9), "{c:?}");
    }
}

#[test]
fn a_hand_on_the_camera_pauses_it() {
    let p = Project::new().with_part();
    let mut ws = open(&p);
    let camera = framed(&ws);
    far_box(&p, &mut ws, "far", -400);
    let picker = Picker::new(&ws.model);
    let now = Instant::now();
    let b = ws
        .view
        .work
        .bounds(now, &ws.model, &picker)
        .expect("the agent's work");
    ws.hand_moved();
    let sight = Sight {
        camera: &camera,
        aspect: ASPECT,
        following: false,
        every: ws.publish_every(),
    };
    assert_eq!(ws.view.work.frame(Instant::now(), &sight, Some(b)), None);
    // Three seconds with no hand: it follows again.
    let later = Instant::now() + work::PAUSE;
    assert!(ws.view.work.frame(later, &sight, Some(b)).is_some());
}
