//! A window whose daemon goes away keeps what it shows and reconnects.

mod common;

use common::{Project, ryucad};
use ryucad_app::core::workspace::Workspace;
use std::sync::Arc;
use std::time::{Duration, Instant};

const WAIT: Duration = Duration::from_secs(30);

/// Runs the window's frame loop (pump and tick) until `done` holds.
fn frames(ws: &mut Workspace, done: impl Fn(&Workspace) -> bool) {
    let t = Instant::now();
    while !done(ws) {
        assert!(t.elapsed() < WAIT, "timed out");
        ws.pump();
        ws.tick(Instant::now());
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn boss_hash(ws: &Workspace) -> Option<String> {
    ws.model
        .meshes
        .get(&("p".to_owned(), "boss".to_owned()))
        .map(|m| m.hash.clone())
}

#[test]
fn a_lost_daemon_is_replaced_and_the_window_catches_up() {
    let p = Project::new().with_part();
    let mut ws = Workspace::open(&p.root, ryucad(), "Dana", Arc::new(|| {})).unwrap();
    frames(&mut ws, |ws| ws.idle() && ws.model.meshes.len() == 2);
    let (before, me) = (boss_hash(&ws), ws.me().to_owned());
    // The window's camera, shared before the daemon goes.
    let camera = ryucad::view::frame(
        [0.0; 3],
        [0.01; 3],
        ryucad::view::StandardView::Front,
        ryucad::view::DEFAULT_PROJECTION,
    );
    ws.share_camera(&camera);
    frames(&mut ws, |ws| {
        ws.people.get(ws.me()).is_some_and(|m| m.camera.is_some())
    });
    p.ok(&["daemon", "stop"]);
    frames(&mut ws, |ws| ws.lost);
    // The last state stays on screen while the window is lost.
    assert_eq!(ws.model.meshes.len(), 2);
    assert!(ws.retry_at().is_some());
    // Changes made meanwhile, without any daemon.
    p.ok(&[
        "--no-daemon",
        "feature",
        "edit",
        "p",
        "boss",
        "--param",
        "radius=2",
    ]);
    p.ok(&["--no-daemon", "part", "create", "q"]);
    frames(&mut ws, |ws| {
        !ws.lost && ws.part("q").is_some() && boss_hash(ws).is_some_and(|h| Some(h) != before)
    });
    assert_ne!(ws.me(), me);
    assert!(ws.people.get(ws.me()).is_some());
    // The new session hears the window's camera again, unmoved.
    frames(&mut ws, |ws| {
        ws.people.get(ws.me()).is_some_and(|m| m.camera.is_some())
    });
    // While it was away a part went: it goes from the window too.
    p.ok(&["daemon", "stop"]);
    frames(&mut ws, |ws| ws.lost);
    p.ok(&["--no-daemon", "part", "delete", "q"]);
    frames(&mut ws, |ws| {
        !ws.lost && ws.idle() && ws.part("q").is_none()
    });
}
