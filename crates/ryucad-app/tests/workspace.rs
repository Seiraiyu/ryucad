//! An open project follows what agents do, against a real daemon.

mod common;

use common::{Project, ryucad};
use ryucad_app::core::workspace::Workspace;
use std::sync::Arc;
use std::time::Duration;

const WAIT: Duration = Duration::from_secs(30);

fn open(p: &Project) -> Workspace {
    Workspace::open(&p.root, ryucad(), "Dana", Arc::new(|| {})).unwrap()
}

/// The hash of the mesh the window holds for a feature of part `p`.
fn mesh_hash(ws: &Workspace, feature: &str) -> Option<String> {
    ws.model
        .meshes
        .get(&("p".to_owned(), feature.to_owned()))
        .map(|m| m.hash.clone())
}

/// Whether the window has a mesh for every body of part `p`.
fn meshed(ws: &Workspace) -> bool {
    ws.part("p").is_some_and(|p| {
        p.features
            .iter()
            .all(|f| f.hash.is_none() || mesh_hash(ws, &f.id) == f.hash)
    })
}

#[test]
fn the_window_loads_the_model_and_its_meshes() {
    let p = Project::new().with_part();
    let mut ws = open(&p);
    assert!(ws.wait_until(WAIT, |ws| ws.idle() && meshed(ws)));
    assert_eq!(ws.model.default_unit, "mm");
    let part = ws.part("p").unwrap();
    let kinds: Vec<_> = part
        .features
        .iter()
        .map(|f| (f.id.as_str(), f.ty.as_deref()))
        .collect();
    assert_eq!(kinds, [("base", Some("box")), ("boss", Some("cylinder"))]);
    let base = &ws.model.meshes[&("p".to_owned(), "base".to_owned())];
    let mesh = base.mesh.as_ref().unwrap();
    assert_eq!(mesh.faces.len(), 6);
    assert_eq!(mesh.edges.len(), 12);
    assert_eq!(ws.fetched, 2);
}

#[test]
fn an_edit_refetches_one_body() {
    let p = Project::new().with_part();
    let mut ws = open(&p);
    assert!(ws.wait_until(WAIT, |ws| ws.idle() && meshed(ws)));
    let (base, boss) = (mesh_hash(&ws, "base"), mesh_hash(&ws, "boss"));
    p.ok(&["feature", "edit", "p", "boss", "--param", "radius=4"]);
    assert!(ws.wait_until(WAIT, |ws| {
        mesh_hash(ws, "boss").is_some_and(|h| Some(h) != boss)
    }));
    assert_eq!(mesh_hash(&ws, "base"), base);
    assert_eq!(ws.fetched, 3);
}

#[test]
fn failures_deletions_and_new_parts_show() {
    let p = Project::new().with_part();
    let mut ws = open(&p);
    assert!(ws.wait_until(WAIT, |ws| ws.idle() && meshed(ws)));
    // A torus whose tube swallows its hole fails to build.
    p.ok(&[
        "feature",
        "add",
        "p",
        "torus",
        "--id",
        "ring",
        "--param",
        "major_radius=1",
        "--param",
        "minor_radius=1",
    ]);
    assert!(ws.wait_until(WAIT, |ws| {
        ws.part("p")
            .and_then(|p| p.feature("ring"))
            .is_some_and(|f| f.error.is_some() && f.ty.as_deref() == Some("torus"))
    }));
    p.ok(&["feature", "delete", "p", "boss"]);
    assert!(ws.wait_until(WAIT, |ws| mesh_hash(ws, "boss").is_none()
        && ws.part("p").unwrap().feature("boss").is_none()));
    p.ok(&["part", "create", "q"]);
    assert!(ws.wait_until(WAIT, |ws| ws.part("q").is_some()));
    p.ok(&["part", "delete", "q"]);
    assert!(ws.wait_until(WAIT, |ws| ws.part("q").is_none()));
}

#[test]
fn files_changed_behind_the_daemons_back_show() {
    let p = Project::new().with_part();
    let mut ws = open(&p);
    assert!(ws.wait_until(WAIT, |ws| ws.idle() && meshed(ws)));
    let file = p.root.join("parts/p/features/boss.json");
    let text = std::fs::read_to_string(&file).unwrap();
    std::fs::write(&file, text.replace("3 mm", "2 mm")).unwrap();
    let boss = mesh_hash(&ws, "boss");
    assert!(ws.wait_until(WAIT, |ws| {
        mesh_hash(ws, "boss").is_some_and(|h| Some(h) != boss)
    }));
    // A broken part list breaks only its part.
    std::fs::write(p.root.join("parts/p/part.json"), "{").unwrap();
    assert!(ws.wait_until(WAIT, |ws| ws.part("p").is_some_and(|p| p.error.is_some())));
}

#[test]
fn the_display_quality_refetches_every_mesh() {
    let p = Project::new().with_part();
    let mut ws = open(&p);
    assert!(ws.wait_until(WAIT, |ws| ws.idle() && meshed(ws)));
    let before = ws.model.meshes[&("p".to_owned(), "boss".to_owned())]
        .mesh
        .as_ref()
        .unwrap()
        .triangles
        .len();
    p.ok(&["config", "set", "display.quality", "fine"]);
    assert!(ws.wait_until(WAIT, |ws| ws.fetched == 4 && ws.idle() && meshed(ws)));
    let after = ws.model.meshes[&("p".to_owned(), "boss".to_owned())]
        .mesh
        .as_ref()
        .unwrap()
        .triangles
        .len();
    assert!(after > before, "{after} > {before}");
    p.ok(&["config", "set", "display.quality", "normal"]);
}
