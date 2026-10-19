//! The window and the agents see each other, against a real daemon.

mod common;

use common::{Project, ryucad};
use ryucad::live::{Kind, SelectMode, Style};
use ryucad::view::Camera;
use ryucad_app::core::workspace::Workspace;
use std::sync::Arc;
use std::time::Duration;

const WAIT: Duration = Duration::from_secs(30);

fn open(p: &Project, name: &str) -> Workspace {
    let mut ws = Workspace::open(&p.root, ryucad(), name, Arc::new(|| {})).unwrap();
    assert!(ws.wait_until(WAIT, |ws| ws.idle() && ws.model.meshes.len() == 2));
    ws
}

/// A face of the box, by reference.
fn face(ws: &Workspace) -> String {
    let base = &ws.model.meshes[&("p".to_owned(), "base".to_owned())];
    let name = &base.mesh.as_ref().unwrap().faces[0].name;
    format!("p/base/{}", ryucad::mesh::name_hex(name))
}

fn agent(ws: &Workspace) -> Option<&ryucad_app::core::people::Person> {
    ws.people.all.values().find(|p| p.kind == Kind::Agent)
}

#[test]
fn the_window_sees_what_agents_do() {
    let p = Project::new().with_part();
    let mut ws = open(&p, "Dana");
    let f = face(&ws);
    p.ok(&["status", "drilling the boss"]);
    p.ok(&["highlight", &f, "--style", "pending", "--label", "next"]);
    p.ok(&["select", &f]);
    p.ok(&["camera", "--view", "front"]);
    assert!(ws.wait_until(WAIT, |ws| agent(ws).is_some_and(|a| a.camera.is_some())));
    let a = agent(&ws).unwrap();
    assert_eq!(a.status.as_ref().unwrap().0, "drilling the boss");
    assert_eq!(a.highlights[0].style, Style::Pending);
    assert_eq!(a.highlights[0].entities, std::slice::from_ref(&f));
    assert_eq!(a.selection, [f]);
    // Following the agent shows its camera.
    let (id, camera) = (a.id.clone(), a.camera.clone());
    ws.following = Some(id);
    assert_eq!(ws.followed_camera(), camera.as_ref());
}

#[test]
fn agents_see_what_the_window_selects_and_looks_at() {
    let p = Project::new().with_part();
    let mut ws = open(&p, "Dana");
    let f = face(&ws);
    ws.select(vec![f.clone()], SelectMode::Replace);
    assert_eq!(ws.myself().unwrap().selection, std::slice::from_ref(&f));
    assert!(ws.wait_until(WAIT, |ws| ws.idle()));
    assert_eq!(p.ok(&["selection"])["selection"], serde_json::json!([f]));
    let camera = Camera {
        eye: [0.1, -0.2, 0.3],
        target: [0.0, 0.0, 0.0025],
        up: [0.0, 0.0, 1.0],
        projection: ryucad::view::Projection::Orthographic { height: 0.05 },
    };
    // Several moves in a burst: the first goes at once, the last after a pause.
    let burst = std::time::Instant::now();
    ws.share_camera(&ryucad::view::pan(&camera, 0.5, 0.0));
    ws.share_camera(&camera);
    // Held back, unless a slow machine took the pause between the two
    // (Windows CI has): then it went at once, as it should.
    if burst.elapsed() < ryucad_app::core::workspace::CAMERA_EVERY {
        assert!(ws.camera_due());
    }
    std::thread::sleep(ryucad_app::core::workspace::CAMERA_EVERY);
    ws.tick(std::time::Instant::now());
    assert!(!ws.camera_due());
    assert!(ws.wait_until(WAIT, |ws| ws.idle()));
    let seen: Camera = serde_json::from_value(p.ok(&["camera"])["camera"].clone()).unwrap();
    assert_eq!(seen, camera);
}

#[test]
fn windows_come_and_go() {
    let p = Project::new().with_part();
    let mut ws = open(&p, "Dana");
    let other = open(&p, "Kai");
    let kai = other.me().to_owned();
    assert!(ws.wait_until(WAIT, |ws| ws.people.get(&kai).is_some()));
    ws.following = Some(kai.clone());
    drop(other);
    assert!(ws.wait_until(WAIT, |ws| ws.people.get(&kai).is_none()));
    assert_eq!(ws.following, None);
}

#[test]
fn ryucad_open_asks_the_window_forward() {
    let p = Project::new().with_part();
    let mut ws = open(&p, "Dana");
    // With a window attached, nothing is started.
    let out = std::process::Command::new(&ryucad().exe)
        .arg("open")
        .current_dir(&p.root)
        .env(ryucad::client::APP_VAR, p.root.join("no-such-app"))
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["result"]["opened"], "existing", "{v}");
    assert!(ws.wait_until(WAIT, |ws| ws.raised));
}
