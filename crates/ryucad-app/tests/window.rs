//! The main window, driven like a person would, against a real daemon.
//! It needs a graphics adapter (software ones count): without one the
//! tests pass with a note, unless RYUCAD_REQUIRE_GPU is set.

mod common;

use common::{Project, ryucad};
use eframe::egui;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use ryucad_app::ui::app::App;
use std::time::{Duration, Instant};

fn gpu() -> bool {
    let ok = ryucad_app::ui::scene::headless().is_some();
    if !ok {
        assert!(
            std::env::var_os("RYUCAD_REQUIRE_GPU").is_none(),
            "no GPU adapter"
        );
        eprintln!("no GPU adapter: skipped");
    }
    ok
}

fn window(p: &Project) -> Harness<'static, App> {
    let root = p.root.clone();
    Harness::builder()
        .with_size(egui::vec2(1000.0, 700.0))
        .wgpu()
        .build_eframe(move |cc| App::open(&cc.egui_ctx, &root, ryucad()).unwrap())
}

/// Runs frames until `done` holds.
fn frames(h: &mut Harness<'static, App>, done: impl Fn(&App) -> bool) {
    let t = Instant::now();
    loop {
        h.step();
        if done(h.state()) {
            return;
        }
        assert!(t.elapsed() < Duration::from_secs(30), "timed out");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn click(h: &Harness<'static, App>, pos: egui::Pos2) {
    h.event(egui::Event::PointerMoved(pos));
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        });
    }
}

fn loaded(app: &App) -> bool {
    app.ws.idle() && app.ws.model.meshes.len() == 2
}

#[test]
fn the_tree_lists_features_and_frames_a_chosen_one() {
    if !gpu() {
        return;
    }
    let p = Project::new().with_part();
    let mut h = window(&p);
    frames(&mut h, loaded);
    h.get_by_label("base (box)");
    let before = h.state().view.nav.camera.clone();
    h.get_by_label("boss (cylinder)").click();
    frames(&mut h, |a| a.view.chosen.is_some());
    assert_eq!(
        h.state().view.chosen,
        Some(("p".to_owned(), "boss".to_owned()))
    );
    assert_ne!(h.state().view.nav.camera, before);
    // A failed feature is marked.
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
    frames(&mut h, |a| {
        a.ws.part("p").is_some_and(|p| p.feature("ring").is_some())
    });
    h.step();
    h.get_by_label("ring (torus): failed");
}

#[test]
fn a_click_selects_and_agents_see_it() {
    if !gpu() {
        return;
    }
    let p = Project::new().with_part();
    let mut h = window(&p);
    frames(&mut h, loaded);
    h.step();
    click(&h, h.state().view.rect.center());
    frames(&mut h, |a| {
        a.ws.myself().is_some_and(|m| m.selection.len() == 1) && a.ws.idle()
    });
    let mine = h.state().ws.myself().unwrap().selection.clone();
    assert_eq!(p.ok(&["selection"])["selection"], serde_json::json!(mine));
    // Clicking the background clears it.
    click(&h, h.state().view.rect.left_top() + egui::vec2(5.0, 5.0));
    frames(&mut h, |a| {
        a.ws.myself().is_some_and(|m| m.selection.is_empty())
    });
}

#[test]
fn the_window_follows_an_agent_until_the_human_navigates() {
    if !gpu() {
        return;
    }
    let p = Project::new().with_part();
    let mut h = window(&p);
    frames(&mut h, loaded);
    p.ok(&["status", "drilling the boss"]);
    p.ok(&["camera", "--view", "front"]);
    frames(&mut h, |a| {
        a.ws.people
            .all
            .values()
            .any(|p| p.camera.is_some() && p.name == "agent")
    });
    h.step();
    h.get_by_label("drilling the boss");
    h.get_by_label("Follow").click();
    frames(&mut h, |a| a.ws.following.is_some());
    h.step();
    let agent = h.state().ws.followed_camera().unwrap().clone();
    assert_eq!(h.state().view.nav.camera, agent);
    // A drag in the viewport takes the camera back.
    let c = h.state().view.rect.center();
    h.event(egui::Event::PointerMoved(c));
    h.event(egui::Event::PointerButton {
        pos: c,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: egui::Modifiers::NONE,
    });
    for i in 1..6 {
        h.event(egui::Event::PointerMoved(
            c + egui::vec2(10.0 * i as f32, 0.0),
        ));
        h.step();
    }
    h.event(egui::Event::PointerButton {
        pos: c + egui::vec2(50.0, 0.0),
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    });
    h.step();
    assert_eq!(h.state().ws.following, None);
    assert_ne!(h.state().view.nav.camera, agent);
}

#[test]
fn a_parameter_is_edited_in_the_property_panel() {
    if !gpu() {
        return;
    }
    let p = Project::new().with_part();
    let mut h = window(&p);
    frames(&mut h, loaded);
    h.get_by_label("boss (cylinder)").click();
    frames(&mut h, |a| a.ws.form.is_some());
    h.step();
    // The radius field holds the value as written; a person types over it.
    let field = h.get_all_by_value("3 mm").next().unwrap();
    field.set_text_value_accesskit("4.5 mm");
    h.step();
    h.get_by_label("Apply").click();
    frames(&mut h, |a| {
        a.ws.form.as_ref().is_some_and(|f| !f.saving && !f.dirty())
    });
    let shown = p.ok(&["feature", "show", "p", "boss"]);
    assert_eq!(shown["params"]["radius"]["value"], "4.5 mm");
}

#[test]
fn the_activity_tab_undoes_and_ctrl_z_does_too() {
    if !gpu() {
        return;
    }
    let p = Project::new().with_part();
    let mut h = window(&p);
    frames(&mut h, loaded);
    p.ok(&["feature", "edit", "p", "boss", "--param", "radius=5"]);
    frames(&mut h, |a| !a.ws.activity.entries.is_empty());
    h.step();
    h.get_by_label("feature.edit");
    // Newest first: the top Undo is the edit's.
    h.get_all_by_label("Undo").next().unwrap().click();
    frames(&mut h, |a| {
        a.ws.activity.entries.iter().any(|e| e["kind"] == "undo")
    });
    let radius =
        |p: &Project| p.ok(&["feature", "show", "p", "boss"])["params"]["radius"]["value"].clone();
    assert_eq!(radius(&p), "3 mm");
    // That undo was the window's own: Shift+Ctrl+Z redoes it.
    h.key_press_modifiers(
        egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
        egui::Key::Z,
    );
    frames(&mut h, |a| {
        a.ws.activity.entries.iter().any(|e| e["kind"] == "redo")
    });
    assert_eq!(radius(&p), "5 mm");
}
