//! An open project follows what agents do, against a real daemon.

mod common;

use common::{Project, ryucad};
use ryucad_app::core::workspace::{HISTORY_EVERY, Workspace};
use std::sync::Arc;
use std::time::{Duration, Instant};

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
fn a_burst_of_edits_fetches_each_body_once() {
    let p = Project::new().with_part();
    let mut ws = open(&p);
    assert!(ws.wait_until(WAIT, |ws| ws.idle() && meshed(ws)));
    // The window reads nothing while the agent works, so every report
    // arrives before any mesh: each one names all the new bodies.
    for i in 0..5 {
        let origin = format!("--origin={},0,0", 30 * (i + 1));
        p.ok(&[
            "feature", "add", "p", "sphere", "--param", "radius=2", &origin,
        ]);
    }
    assert!(ws.wait_until(
        WAIT,
        |ws| ws.part("p").is_some_and(|p| p.features.len() == 7) && ws.idle() && meshed(ws)
    ));
    assert_eq!(ws.fetched, 2 + 5);
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

#[test]
fn the_history_follows_changes_and_commits() {
    let p = Project::new().with_part().committed();
    let mut ws = open(&p);
    let count = |ws: &Workspace, key: &str| {
        ws.history
            .as_ref()
            .and_then(|h| h[key].as_array().map(Vec::len))
    };
    assert!(ws.wait_until(WAIT, |ws| ws.idle() && meshed(ws)));
    ws.load_history();
    assert!(ws.wait_until(WAIT, |ws| ws.idle() && ws.history.is_some()));
    assert_eq!(count(&ws, "uncommitted"), Some(0));
    let before = count(&ws, "commits").unwrap();
    // An edit reads it again: the change is uncommitted.
    p.ok(&["feature", "edit", "p", "boss", "--param", "radius=4"]);
    assert!(ws.wait_until(WAIT, |ws| ws.idle() && count(ws, "uncommitted") == Some(1)));
    // A commit from a terminal changes no project file: the tab reads the
    // history again once its reading is old enough, and not before.
    p.git(&["commit", "-q", "-am", "radius"]);
    let now = Instant::now();
    assert!(ws.history_shown(now) > Duration::ZERO);
    assert!(ws.idle());
    ws.history_shown(now + HISTORY_EVERY);
    assert!(ws.wait_until(WAIT, |ws| ws.idle()
        && count(ws, "commits") == Some(before + 1)));
    assert_eq!(count(&ws, "uncommitted"), Some(0));
}

#[test]
fn the_window_follows_appearance_and_previews_before_it_writes() {
    use ryucad_app::core::appearance::Preview;
    let p = Project::new().with_part();
    let mut ws = open(&p);
    assert!(ws.wait_until(WAIT, |ws| ws.idle() && meshed(ws)));
    assert!(ws.model.looks.is_empty(), "plain: no looks");
    // An agent colours the part: both its solids look so.
    p.ok(&["colour", "p", "--colour", "#ff0000"]);
    assert!(ws.wait_until(WAIT, |ws| ws.idle() && ws.model.looks.len() == 2));
    let base = ("p".to_owned(), "base".to_owned());
    assert_eq!(ws.model.looks[&base].base.colour, Some([1.0, 0.0, 0.0]));
    // A material's colour changes looks with no part's revision.
    p.ok(&[
        "materials",
        "set",
        "brass",
        "--density",
        "8.5 g/cm^3",
        "--colour",
        "#b5a642",
        "--default",
    ]);
    p.ok(&["colour", "p", "--material"]);
    assert!(ws.wait_until(WAIT, |ws| {
        ws.idle()
            && ws.model.looks[&base]
                .base
                .colour
                .is_some_and(|c| c[2] < 0.5)
    }));
    // The Appearance window on the part: a dragged opacity previews,
    // writing nothing, and is written when let go.
    ws.open_appearance();
    assert_eq!(ws.appearance.target(), Some("p"));
    ws.preview_appearance(Preview {
        colour: None,
        opacity: Some(0.5),
    });
    assert!(ws.model.looks.values().all(|l| l.translucent()));
    let file = p.root.join("parts/p/appearance.json");
    assert!(!std::fs::read_to_string(&file).unwrap().contains("opacity"));
    ws.appearance.opacity = 0.5;
    ws.write_appearance("opacity");
    assert!(ws.wait_until(WAIT, |ws| ws.idle() && ws.appearance.dragging.is_none()));
    assert!(
        std::fs::read_to_string(&file)
            .unwrap()
            .contains("\"opacity\": 0.5")
    );
    assert!(ws.model.looks.values().all(|l| l.translucent()));
    // A refusal stays in the window, and the preview goes.
    ws.appearance.targets = vec!["p/nothing".to_owned()];
    ws.preview_appearance(Preview {
        colour: Some([0.0, 0.0, 1.0]),
        opacity: None,
    });
    ws.write_appearance("colour");
    assert!(ws.wait_until(WAIT, |ws| ws.idle() && ws.appearance.error.is_some()));
    assert_eq!(
        ws.appearance.error.as_ref().unwrap().code,
        ryucad::error::Code::NotFound
    );
    ws.clear_appearance();
    ws.close_appearance();
    assert!(ws.model.looks.values().all(|l| l.translucent()));
}

/// `p` placed twice, `one` and `two`, in assembly `asm`: the project's
/// model.
fn assembled(p: &Project) {
    p.ok(&["part", "create", "asm", "--assembly"]);
    p.ok(&[
        "feature", "add", "asm", "instance", "--id", "one", "--of", "p",
    ]);
    p.ok(&[
        "feature", "add", "asm", "instance", "--id", "two", "--of", "p", "--origin", "50,0,0",
    ]);
}

#[test]
fn the_window_cuts_and_explodes_as_its_participant_does() {
    use ryucad::section::Plane;
    let p = Project::new().with_part();
    assembled(&p);
    let mut ws = open(&p);
    let two = ("asm".to_owned(), "two/base".to_owned());
    assert!(ws.wait_until(WAIT, |ws| ws.idle() && ws.model.meshes.contains_key(&two)));
    // An agent's plane goes to the window's human, and the window cuts.
    p.ok(&["section", "xy", "--offset", "3"]);
    let xy = Plane::new([0.0, 0.0, 0.003], [0.0, 0.0, 1.0]).unwrap();
    assert!(ws.wait_until(WAIT, |ws| ws.sections.planes == [xy]));
    // The window's own: drawn at once, and the session's after.
    let lower = xy.moved(-0.001);
    ws.set_sections(vec![lower]);
    assert_eq!(ws.sections.planes, [lower]);
    assert!(ws.wait_until(WAIT, |ws| ws.idle()));
    assert_eq!(ws.myself().unwrap().sections, [lower]);
    let got = p.ok(&["section"]);
    assert_eq!(got["sections"][0]["origin_m"][2], 0.002, "{got}");
    // An explode an agent shows moves `two` up where it is drawn.
    p.ok(&[
        "explode",
        "save",
        "asm",
        "apart",
        "--steps",
        r#"[{"instances": ["two"], "translate": [0, 0, 30]}]"#,
    ]);
    let before = ws.model.pose(&two).origin;
    p.ok(&["explode", "show", "apart"]);
    assert!(ws.wait_until(WAIT, |ws| ws.model.pose(&two).origin[2] > before[2] + 0.029));
    // The slider moves it at once, and is written when let go.
    ws.slide_explode(0.5);
    assert!((ws.model.pose(&two).origin[2] - before[2] - 0.015).abs() < 1e-12);
    ws.show_explode(None, Some(0.5), false);
    assert!(ws.wait_until(WAIT, |ws| {
        ws.idle()
            && ws
                .myself()
                .and_then(|m| m.explode.clone())
                .is_some_and(|e| e.fraction == 0.5)
    }));
    // A step dragged out of the selection is written after the others.
    ws.select(
        vec!["asm/one".to_owned()],
        ryucad::live::SelectMode::Replace,
    );
    assert!(ws.wait_until(WAIT, |ws| ws.idle()));
    ws.start_step();
    let adding = ws.explodes.adding.as_mut().expect("one is selected");
    assert_eq!(adding.instances, ["one"]);
    adding.axis = Some(0);
    adding.distance = -0.02;
    ws.finish_step();
    let file = p.root.join("parts/asm/explodes/apart.json");
    assert!(ws.wait_until(WAIT, |ws| ws.idle()
        && std::fs::read_to_string(&file).unwrap().contains("-20 mm")));
    assert!(ws.wait_until(WAIT, |ws| {
        ws.explodes
            .list
            .first()
            .is_some_and(|e| e.resolved.len() == 2)
    }));
    // Its last step taken out, the explode goes.
    ws.remove_step(1);
    assert!(ws.wait_until(WAIT, |ws| ws.idle()
        && !std::fs::read_to_string(&file).unwrap().contains("-20 mm")));
    assert!(ws.wait_until(WAIT, |ws| {
        ws.explodes
            .list
            .first()
            .is_some_and(|e| e.resolved.len() == 1)
    }));
    ws.remove_step(0);
    assert!(ws.wait_until(WAIT, |ws| ws.idle() && !file.exists()));
    p.ok(&["explode", "show", "--off"]);
    assert!(ws.wait_until(WAIT, |ws| ws.model.pose(&two).origin == before));
}
