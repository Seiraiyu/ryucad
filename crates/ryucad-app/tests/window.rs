//! The main window, driven like a person would, against a real daemon.
//! It needs a graphics adapter (software ones count): see `common::gpu`.

mod common;

use common::{Project, ryucad};
use eframe::egui;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use ryucad_app::ui::app::App;
use ryucad_app::ui::style::Look;
use std::time::{Duration, Instant};

fn gpu() -> bool {
    common::gpu().is_some()
}

/// Held by each test for its whole run: lavapipe crashes (SIGSEGV) when
/// several windows draw at once (in the devcontainer, and on the CI
/// runners from 2026-10-22).
fn one_at_a_time() -> std::sync::MutexGuard<'static, ()> {
    static ONE: std::sync::Mutex<()> = std::sync::Mutex::new(());
    ONE.lock().unwrap_or_else(|e| e.into_inner())
}

fn window(p: &Project) -> Harness<'static, App> {
    let root = p.root.clone();
    Harness::builder()
        .with_size(egui::vec2(1000.0, 700.0))
        .wgpu()
        .build_eframe(move |cc| App::open(&cc.egui_ctx, Some(&root), ryucad()))
}

/// Opens the menu `menu` and clicks `item` in it (named after its text,
/// and its shortcut if it has one).
fn menu(h: &mut Harness<'static, App>, menu: &str, item: &str) {
    h.get_by_label(menu).click();
    let spaced = format!("{item} ");
    let named = |n: &egui_kittest::kittest::AccessKitNode<'_>| {
        n.label()
            .is_some_and(|l| l == item || l.starts_with(&spaced))
    };
    // A menu opens over a frame or two.
    for _ in 0..10 {
        h.step();
        if h.query_by(named).is_some() {
            break;
        }
    }
    h.get_by(named).click();
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
    match &app.screen {
        ryucad_app::ui::app::Screen::Opened(o) => o.ws.idle() && o.ws.model.meshes.len() == 2,
        ryucad_app::ui::app::Screen::Start(s) => {
            assert!(s.error.is_none(), "{:?}", s.error);
            false
        }
        _ => false,
    }
}

#[test]
fn the_tree_lists_features_and_frames_a_chosen_one() {
    let _one = one_at_a_time();
    if !gpu() {
        return;
    }
    let p = Project::new().with_part();
    let mut h = window(&p);
    frames(&mut h, loaded);
    h.get_by_label("base (box)");
    let before = h.state().opened().view.nav.camera.clone();
    h.get_by_label("boss (cylinder)").click();
    frames(&mut h, |a| a.opened().view.chosen.is_some());
    assert_eq!(
        h.state().opened().view.chosen,
        Some(("p".to_owned(), "boss".to_owned()))
    );
    assert_ne!(h.state().opened().view.nav.camera, before);
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
        a.opened()
            .ws
            .part("p")
            .is_some_and(|p| p.feature("ring").is_some())
    });
    h.step();
    h.get_by_label("ring (torus): failed (rebuild_failed)");
}

#[test]
fn the_tree_groups_features_by_solid() {
    let _one = one_at_a_time();
    if !gpu() {
        return;
    }
    let p = Project::new().with_part();
    p.ok(&[
        "feature",
        "add",
        "p",
        "box",
        "--id",
        "slot",
        "--operation",
        "cut",
        "--target",
        "base",
        "--param",
        "x=4",
        "--param",
        "y=20",
        "--param",
        "z=2",
        "--origin",
        "8,-5,4",
    ]);
    p.ok(&[
        "feature", "add", "p", "boolean", "--id", "whole", "--op", "unite", "--target", "base",
        "--tool", "boss",
    ]);
    let mut h = window(&p);
    frames(&mut h, |a| match &a.screen {
        ryucad_app::ui::app::Screen::Opened(o) => o.ws.idle() && o.ws.model.meshes.len() == 1,
        _ => false,
    });
    h.step();
    // The base's solid, its chain, and the boss it consumed under the
    // boolean.
    h.get_by_label("base (solid)");
    h.get_by_label("base (box)");
    h.get_by_label("whole (boolean, unite)");
    h.get_by_label("boss (tool, consumed)");
    h.get_by_label("slot (box, cut)").click();
    frames(&mut h, |a| {
        a.opened().ws.made.is_some() && a.opened().ws.form.is_some()
    });
    assert_eq!(
        h.state().opened().view.chosen,
        Some(("p".to_owned(), "slot".to_owned()))
    );
    // Its form says what it does to which solids.
    h.step();
    h.get_by(|n| {
        n.role() == egui::accesskit::Role::ComboBox && n.label().as_deref() == Some("operation")
    });
    h.get_by_label("targets: base");
}

#[test]
fn a_click_selects_and_agents_see_it() {
    let _one = one_at_a_time();
    if !gpu() {
        return;
    }
    let p = Project::new().with_part();
    let mut h = window(&p);
    frames(&mut h, loaded);
    h.step();
    click(&h, h.state().opened().view.rect.center());
    frames(&mut h, |a| {
        a.opened()
            .ws
            .myself()
            .is_some_and(|m| m.selection.len() == 1)
            && a.opened().ws.idle()
    });
    let mine = h.state().opened().ws.myself().unwrap().selection.clone();
    assert_eq!(p.ok(&["selection"])["selection"], serde_json::json!(mine));
    // Escape clears it.
    h.key_press(egui::Key::Escape);
    frames(&mut h, |a| {
        a.opened()
            .ws
            .myself()
            .is_some_and(|m| m.selection.is_empty())
    });
    click(&h, h.state().opened().view.rect.center());
    frames(&mut h, |a| {
        a.opened()
            .ws
            .myself()
            .is_some_and(|m| m.selection.len() == 1)
    });
    // Clicking the background clears it.
    click(
        &h,
        h.state().opened().view.rect.left_top() + egui::vec2(5.0, 5.0),
    );
    frames(&mut h, |a| {
        a.opened()
            .ws
            .myself()
            .is_some_and(|m| m.selection.is_empty())
    });
}

#[test]
fn orbiting_from_the_background_keeps_the_model_in_view() {
    let _one = one_at_a_time();
    if !gpu() {
        return;
    }
    let p = Project::new().with_part();
    let mut h = window(&p);
    frames(&mut h, loaded);
    h.step();
    let rect = h.state().opened().view.rect;
    // Zoom in towards a corner of the model, well off the middle.
    let corner = rect.center() + egui::vec2(rect.width() * 0.18, rect.height() * 0.12);
    h.event(egui::Event::PointerMoved(corner));
    for _ in 0..12 {
        h.event(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Line,
            delta: egui::vec2(0.0, 1.0),
            modifiers: egui::Modifiers::NONE,
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Wheel,
        });
        h.step();
    }
    // The model fills the middle of the view.
    let under_middle = |h: &Harness<'static, App>| {
        let o = h.state().opened();
        let (lo, hi) = ryucad_app::core::pick::Picker::new(&o.ws.model)
            .bounds()
            .unwrap();
        let centre = [0, 1, 2].map(|k| (lo[k] + hi[k]) / 2.0);
        let aspect = f64::from(rect.width() / rect.height());
        let pr = ryucad_app::core::nav::Projector::new(&o.view.nav.camera, aspect, centre, 0.05);
        let size = [f64::from(rect.width()), f64::from(rect.height())];
        ryucad_app::core::pick::Picker::new(&o.ws.model)
            .pick(&pr, size, [size[0] / 2.0, size[1] / 2.0])
            .is_some()
    };
    assert!(under_middle(&h), "zoomed in on the model");
    // Orbit from the background, a quarter turn or so.
    let start = rect.left_top() + egui::vec2(8.0, 8.0);
    h.event(egui::Event::PointerMoved(start));
    h.event(egui::Event::PointerButton {
        pos: start,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: egui::Modifiers::NONE,
    });
    for i in 1..10 {
        h.event(egui::Event::PointerMoved(
            start + egui::vec2(25.0 * i as f32, 0.0),
        ));
        h.step();
    }
    h.event(egui::Event::PointerButton {
        pos: start + egui::vec2(225.0, 0.0),
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    });
    h.step();
    // Turned about what was in the middle, the model is still there.
    assert!(
        under_middle(&h),
        "the orbit swung the model out of the middle"
    );
}

#[test]
fn a_trackpad_pans_orbits_and_pinches() {
    let _one = one_at_a_time();
    if !gpu() {
        return;
    }
    let p = Project::new().with_part();
    let mut h = window(&p);
    frames(&mut h, loaded);
    h.step();
    let rect = h.state().opened().view.rect;
    h.event(egui::Event::PointerMoved(rect.center()));
    h.step();
    let camera = |h: &Harness<'static, App>| h.state().opened().view.nav.camera.clone();
    let fingers = |h: &Harness<'static, App>, modifiers: egui::Modifiers| {
        for _ in 0..5 {
            h.event(egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(12.0, 6.0),
                modifiers,
                phase: egui::TouchPhase::Move,
                source: egui::MouseWheelSource::Trackpad,
            });
        }
    };
    let dist = |a: [f64; 3], b: [f64; 3]| {
        (0..3)
            .map(|k| (a[k] - b[k]) * (a[k] - b[k]))
            .sum::<f64>()
            .sqrt()
    };
    let unit = |c: &ryucad::view::Camera| {
        let d = dist(c.target, c.eye);
        [0, 1, 2].map(|k| (c.target[k] - c.eye[k]) / d)
    };
    // Two fingers pan: same distance and direction, moved.
    let before = camera(&h);
    fingers(&h, egui::Modifiers::NONE);
    h.step();
    let after = camera(&h);
    assert!(dist(after.target, before.target) > 1e-6, "panned");
    assert!((after.distance() - before.distance()).abs() < 1e-9);
    assert!(dist(unit(&after), unit(&before)) < 1e-9);
    // With Shift they orbit: the direction turns.
    let before = after;
    fingers(&h, egui::Modifiers::SHIFT);
    h.step();
    let after = camera(&h);
    assert!(dist(unit(&after), unit(&before)) > 1e-3, "orbited");
    // A pinch zooms in.
    let before = after;
    h.event(egui::Event::Zoom(1.5));
    h.step();
    assert!(
        camera(&h).distance() < before.distance() * 0.9,
        "pinched in"
    );
}

#[test]
fn the_first_view_frames_the_whole_model() {
    let _one = one_at_a_time();
    if !gpu() {
        return;
    }
    let p = Project::new();
    p.ok(&["part", "create", "p"]);
    // A tiny first body, then many well away from it: their meshes arrive
    // over many frames.
    p.ok(&[
        "feature",
        "add",
        "p",
        "sphere",
        "--id",
        "a",
        "--param",
        "radius=0.5",
    ]);
    for i in 0..40 {
        let (id, origin) = (format!("b{i}"), format!("{},0,0", 100 + 10 * i));
        p.ok(&[
            "feature", "add", "p", "box", "--id", &id, "--param", "x=10", "--param", "y=10",
            "--param", "z=10", "--origin", &origin,
        ]);
    }
    let mut h = window(&p);
    frames(&mut h, |a| match &a.screen {
        ryucad_app::ui::app::Screen::Opened(o) => o.ws.idle() && o.ws.model.meshes.len() == 41,
        _ => false,
    });
    for _ in 0..3 {
        h.step();
    }
    let o = h.state().opened();
    let (lo, hi) = ryucad_app::core::pick::Picker::new(&o.ws.model)
        .bounds()
        .unwrap();
    let centre = [0, 1, 2].map(|k| (lo[k] + hi[k]) / 2.0);
    let aspect = f64::from(o.view.rect.width() / o.view.rect.height());
    let pr = ryucad_app::core::nav::Projector::new(&o.view.nav.camera, aspect, centre, 0.2);
    for corner in 0..8 {
        let c = [0, 1, 2].map(|k| if corner >> k & 1 == 1 { hi[k] } else { lo[k] });
        let n = pr.ndc(c).unwrap();
        assert!(
            n[0].abs() <= 1.0 && n[1].abs() <= 1.0,
            "{c:?} is out of view at {n:?}"
        );
    }
}

#[test]
fn an_agents_highlight_shows_its_label() {
    let _one = one_at_a_time();
    if !gpu() {
        return;
    }
    let p = Project::new().with_part();
    let mut h = window(&p);
    frames(&mut h, loaded);
    let faces = p.ok(&["inspect", "p", "boss"])["bodies"][0]["faces"].clone();
    let top = format!("p/boss/{}", faces[0]["name"].as_str().unwrap());
    p.ok(&["highlight", &top, "--label", "check this"]);
    frames(&mut h, |a| {
        a.opened()
            .ws
            .people
            .all
            .values()
            .any(|p| !p.highlights.is_empty())
    });
    h.step();
    h.get_by_label("check this");
    // Cleared, it goes.
    p.ok(&["highlight", "--clear"]);
    frames(&mut h, |a| {
        a.opened()
            .ws
            .people
            .all
            .values()
            .all(|p| p.highlights.is_empty())
    });
    h.step();
    assert!(h.query_by_label("check this").is_none());
}

#[test]
fn a_highlight_label_stays_inside_the_view() {
    let _one = one_at_a_time();
    if !gpu() {
        return;
    }
    use ryucad_app::ui::app::Screen;
    let p = Project::new().with_part();
    let mut h = window(&p);
    frames(&mut h, loaded);
    let faces = p.ok(&["inspect", "p", "boss"])["bodies"][0]["faces"].clone();
    let top = format!("p/boss/{}", faces[0]["name"].as_str().unwrap());
    let text = "a long label that would run past the edge of the view";
    p.ok(&["highlight", &top, "--label", text]);
    frames(&mut h, |a| {
        a.opened()
            .ws
            .people
            .all
            .values()
            .any(|p| !p.highlights.is_empty())
    });
    h.step();
    h.get_by_label(text);
    // Panned in steps, the model goes to the right edge and past it; the
    // label is shown only while it fits, or kept inside.
    let mut shown = 0;
    for _ in 0..40 {
        if let Screen::Opened(o) = &mut h.state_mut().screen {
            o.view.nav.camera = ryucad::view::pan(&o.view.nav.camera, -0.03, 0.0);
        }
        h.run_steps(2);
        let view = h.state().opened().view.rect;
        if let Some(label) = h.query_by_label(text) {
            shown += 1;
            assert!(
                view.contains_rect(label.rect()),
                "{:?} outside {view:?}",
                label.rect()
            );
        }
    }
    assert!(shown > 0);
}

#[test]
fn the_history_tab_keeps_the_view_its_size() {
    let _one = one_at_a_time();
    if !gpu() {
        return;
    }
    let p = Project::new().with_part();
    let mut h = window(&p);
    frames(&mut h, loaded);
    h.run_steps(2);
    let before = h.state().opened().view.rect;
    // Whatever the history holds (here, the commits of the checkout the
    // tests run in, if any), the view stays as it was.
    h.get_by_label("History").click();
    frames(&mut h, |a| a.opened().ws.history.is_some());
    h.run_steps(2);
    assert_eq!(h.state().opened().view.rect, before);
}

#[test]
fn the_window_follows_an_agent_until_the_human_navigates() {
    let _one = one_at_a_time();
    if !gpu() {
        return;
    }
    let p = Project::new().with_part();
    let mut h = window(&p);
    frames(&mut h, loaded);
    p.ok(&["status", "drilling the boss"]);
    p.ok(&["camera", "--view", "front"]);
    frames(&mut h, |a| {
        a.opened()
            .ws
            .people
            .all
            .values()
            .any(|p| p.camera.is_some() && p.name == "agent")
    });
    h.step();
    h.get_by_label("drilling the boss");
    h.get_by_label("Follow").click();
    frames(&mut h, |a| a.opened().ws.following.is_some());
    h.step();
    let agent = h.state().opened().ws.followed_camera().unwrap().clone();
    assert_eq!(h.state().opened().view.nav.camera, agent);
    // The window shares the camera it shows: the agent's.
    let followed = h.state().opened().ws.following.clone().unwrap();
    let t = Instant::now();
    while p.ok(&["camera"])["camera"] != p.ok(&["camera", "--of", &followed])["camera"] {
        assert!(
            t.elapsed() < Duration::from_secs(10),
            "the window's camera isn't shared"
        );
        h.step();
        std::thread::sleep(Duration::from_millis(50));
    }
    // A drag in the viewport takes the camera back.
    let c = h.state().opened().view.rect.center();
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
    assert_eq!(h.state().opened().ws.following, None);
    assert_ne!(h.state().opened().view.nav.camera, agent);
}

#[test]
fn a_parameter_is_edited_in_the_property_panel() {
    let _one = one_at_a_time();
    if !gpu() {
        return;
    }
    let p = Project::new().with_part();
    let mut h = window(&p);
    frames(&mut h, loaded);
    h.get_by_label("boss (cylinder)").click();
    frames(&mut h, |a| a.opened().ws.form.is_some());
    h.step();
    // The radius field holds the value as written; a person types over it.
    let field = h.get_all_by_value("3 mm").next().unwrap();
    field.set_text_value_accesskit("4.5 mm");
    h.step();
    h.get_by_label("Apply").click();
    frames(&mut h, |a| {
        a.opened()
            .ws
            .form
            .as_ref()
            .is_some_and(|f| !f.saving && !f.dirty())
    });
    let shown = p.ok(&["feature", "show", "p", "boss"]);
    assert_eq!(shown["params"]["radius"]["value"], "4.5 mm");
}

#[test]
fn a_fillets_radius_is_edited_in_the_property_panel() {
    let _one = one_at_a_time();
    if !gpu() {
        return;
    }
    let p = Project::new().with_part();
    // The base's vertical edge at x = 20, y = 10 mm.
    let body = p.ok(&["inspect", "p", "base"])["bodies"][0].clone();
    let corner = body["edges"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| {
            let dir = &e["curve"]["dir"];
            let o = &e["curve"]["origin"];
            e["curve"]["type"] == "line"
                && dir[2].as_f64().unwrap().abs() == 1.0
                && (o[0].as_f64().unwrap() - 0.02).abs() < 1e-12
                && (o[1].as_f64().unwrap() - 0.01).abs() < 1e-12
        })
        .map(|e| format!("p/base/{}", e["name"].as_str().unwrap()))
        .unwrap();
    p.ok(&[
        "feature", "add", "p", "fillet", "--id", "round", "--radius", "1", "--edge", &corner,
    ]);
    let mut h = window(&p);
    frames(&mut h, loaded);
    h.get_by_label("round (fillet)").click();
    frames(&mut h, |a| a.opened().ws.form.is_some());
    h.step();
    // Its edge and propagation are shown; its radius is typed over.
    h.get_by_label(&format!("edge {}", &corner[corner.len() - 8..]));
    h.get_by_label("propagate along tangent chains");
    let field = h.get_all_by_value("1 mm").next().unwrap();
    field.set_text_value_accesskit("1.5 mm");
    h.step();
    h.get_by_label("Apply").click();
    frames(&mut h, |a| {
        a.opened()
            .ws
            .form
            .as_ref()
            .is_some_and(|f| !f.saving && !f.dirty())
    });
    let shown = p.ok(&["feature", "show", "p", "round"]);
    assert_eq!(shown["groups"][0]["radius"]["value"], "1.5 mm");
}

#[test]
fn the_activity_tab_undoes_and_ctrl_z_does_too() {
    let _one = one_at_a_time();
    if !gpu() {
        return;
    }
    let p = Project::new().with_part();
    let mut h = window(&p);
    frames(&mut h, loaded);
    p.ok(&["feature", "edit", "p", "boss", "--param", "radius=5"]);
    frames(&mut h, |a| !a.opened().ws.activity.entries.is_empty());
    h.step();
    // It names what it changed.
    h.get_by_label("feature.edit p/boss");
    // Newest first: the top Undo is the edit's.
    h.get_all_by_label("Undo").next().unwrap().click();
    frames(&mut h, |a| {
        a.opened()
            .ws
            .activity
            .entries
            .iter()
            .any(|e| e["kind"] == "undo")
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
        a.opened()
            .ws
            .activity
            .entries
            .iter()
            .any(|e| e["kind"] == "redo")
    });
    assert_eq!(radius(&p), "5 mm");
    // Ctrl+Y redoes too, as Windows has it.
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Z);
    frames(&mut h, |a| {
        a.opened().ws.idle()
            && a.opened()
                .ws
                .activity
                .entries
                .iter()
                .filter(|e| e["kind"] == "undo")
                .count()
                == 2
    });
    assert_eq!(radius(&p), "3 mm");
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Y);
    frames(&mut h, |a| {
        a.opened()
            .ws
            .activity
            .entries
            .iter()
            .filter(|e| e["kind"] == "redo")
            .count()
            == 2
    });
    assert_eq!(radius(&p), "5 mm");
}

#[test]
fn preferences_change_the_window_whoever_sets_them() {
    let _one = one_at_a_time();
    if !gpu() {
        return;
    }
    let p = Project::new().with_part();
    let mut h = window(&p);
    frames(&mut h, loaded);
    menu(&mut h, "Edit", "Preferences…");
    frames(&mut h, |a| {
        a.opened().dialogs.preferences && !a.opened().ws.settings.all.is_empty()
    });
    h.step();
    let edges = h.state().opened().view.edges;
    h.get_by_role_and_label(egui::accesskit::Role::CheckBox, "Draw edges")
        .click();
    frames(&mut h, |a| a.opened().view.edges != edges);
    assert_eq!(p.ok(&["config", "get", "display.edges"])["value"], !edges);
    // An agent picks the dark theme; the window follows.
    p.ok(&["config", "set", "ui.theme", "dark"]);
    frames(&mut h, |a| {
        a.opened().ws.settings.text("ui.theme", "") == "dark"
    });
    h.step();
    assert_eq!(
        h.ctx.options(|o| o.theme_preference),
        egui::ThemePreference::Dark
    );
    // And a style: the window takes its colours, the viewport too.
    p.ok(&["config", "set", "ui.style", "graphite"]);
    frames(&mut h, |a| a.opened().view.look == Look::Graphite);
    assert_eq!(
        h.ctx.style_of(egui::Theme::Dark).visuals.panel_fill,
        Look::Graphite.style(true).visuals.panel_fill
    );
    p.ok(&["config", "set", "ui.style", "drafting"]);
    p.ok(&["config", "set", "ui.theme", "system"]);
    p.ok(&["config", "set", "display.edges", "true"]);
}

#[test]
fn a_damaged_project_is_repaired_then_opened() {
    let _one = one_at_a_time();
    if !gpu() {
        return;
    }
    let p = Project::new().with_part();
    p.ok(&["daemon", "stop"]);
    std::fs::remove_file(p.root.join("ryucad.json")).unwrap();
    let mut h = window(&p);
    let damaged = |a: &App| match &a.screen {
        ryucad_app::ui::app::Screen::Damaged(d) => d.0.report.is_some(),
        _ => false,
    };
    frames(&mut h, damaged);
    h.step();
    h.get_by_label("missing_manifest");
    // It says what a repair changes, and promises no undo it can't give
    // (the project has no daemon, so no activity, until it opens).
    h.get_by_label(
        "Nothing is changed until you choose a repair, and a repair changes only the files in its planned changes. Git has the files as they were committed.",
    );
    h.get_by_label("Repair").click();
    frames(&mut h, |a| match &a.screen {
        ryucad_app::ui::app::Screen::Damaged(d) => d.0.openable() && !d.0.busy(),
        _ => false,
    });
    h.step();
    // It says so.
    h.get_by_label("This project is repaired and ready to open");
    assert!(h.query_by_label("This project needs repair").is_none());
    h.get_by_label("Open project").click();
    frames(&mut h, |a| {
        matches!(a.screen, ryucad_app::ui::app::Screen::Opened(_)) && loaded(a)
    });
}

#[test]
fn the_start_window_reopens_recent_projects_and_creates_new_ones() {
    let _one = one_at_a_time();
    if !gpu() {
        return;
    }
    use ryucad_app::ui::app::Screen;
    let p = Project::named("bracket").with_part();
    // Opening a project records it as recent.
    let mut h = window(&p);
    frames(&mut h, |a| loaded(a) && a.opened().ws.idle());
    // Other tests open projects too, so it is somewhere in the list.
    let recent = |p: &Project| p.ok(&["config", "get", "app.recent_projects"])["value"].clone();
    frames(&mut h, |_| {
        recent(&p)
            .as_array()
            .unwrap()
            .contains(&serde_json::json!(p.root))
    });
    // File > Close goes back to the start window, which lists it.
    menu(&mut h, "File", "Close");
    frames(
        &mut h,
        |a| matches!(&a.screen, Screen::Start(s) if s.recent.contains(&p.root)),
    );
    // Its thumbnail is a render of the project.
    frames(&mut h, |a| match &a.screen {
        Screen::Start(s) => matches!(s.thumbnails.done.get(&p.root), Some(Ok(_))),
        _ => false,
    });
    // Other tests' projects are recent too: scroll to this one first.
    h.step();
    h.get_by_label("bracket").scroll_to_me();
    // Scrolling is animated: click once it has stopped, or the click
    // lands on a neighbour (another test's project, maybe deleted).
    let mut at = h.get_by_label("bracket").rect();
    loop {
        h.step();
        let now = h.get_by_label("bracket").rect();
        if now == at {
            break;
        }
        at = now;
    }
    h.get_by_label("bracket").click();
    frames(&mut h, loaded);
    // A folder that isn't a project is offered as a new one.
    let empty = p.root.parent().unwrap().join("empty");
    std::fs::create_dir(&empty).unwrap();
    let mut h = Harness::builder()
        .with_size(egui::vec2(1000.0, 700.0))
        .wgpu()
        .build_eframe(move |cc| App::open(&cc.egui_ctx, Some(&empty), ryucad()));
    frames(
        &mut h,
        |a| matches!(&a.screen, Screen::Start(s) if s.not_a_project.is_some()),
    );
    h.step();
    h.get_by_label("Create a project here").click();
    frames(&mut h, |a| {
        matches!(a.screen, Screen::Opened(_)) && a.opened().ws.idle()
    });
    assert!(p.root.parent().unwrap().join("empty/ryucad.json").is_file());
}

#[test]
fn thumbnails_wait_until_the_start_window_shows() {
    let _one = one_at_a_time();
    use ryucad_app::ui::start::Start;
    let p = Project::named("lazy").with_part();
    let daemon = ryucad(); // and the shared config folder
    ryucad::config::remember_project(&p.root).unwrap();
    // A window opened on a project builds a start screen it never shows:
    // that renders nothing.
    let wake: ryucad_app::core::link::Wake = std::sync::Arc::new(|| {});
    let start = Start::new(daemon, &wake);
    assert!(start.recent.contains(&p.root));
    assert!(start.thumbnails.done.is_empty());
}

#[test]
fn ctrl_w_closes_the_project_and_menus_show_their_shortcuts() {
    let _one = one_at_a_time();
    if !gpu() {
        return;
    }
    use ryucad_app::ui::app::Screen;
    let p = Project::new().with_part();
    let mut h = window(&p);
    frames(&mut h, loaded);
    h.get_by_label("File").click();
    h.run_steps(2);
    let close = h.ctx.format_shortcut(&ryucad_app::ui::menu::CLOSE);
    h.get_by_label_contains(&close);
    h.key_press(egui::Key::Escape);
    h.step();
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::W);
    frames(&mut h, |a| matches!(a.screen, Screen::Start(_)));
}

#[test]
fn dialogs_open_clear_of_the_menus() {
    let _one = one_at_a_time();
    if !gpu() {
        return;
    }
    let p = Project::new().with_part();
    let mut h = window(&p);
    frames(&mut h, loaded);
    let menus = h.get_by_label("File").rect().bottom();
    for (m, item, title) in [
        ("Edit", "Preferences…", "Preferences"),
        ("Project", "Settings…", "Project settings"),
        ("Project", "Doctor…", "Doctor"),
    ] {
        menu(&mut h, m, item);
        h.run_steps(3);
        let top = h.get_by_label(title).rect().top();
        assert!(top > menus, "{title} at {top}, over the menus (to {menus})");
        h.key_press(egui::Key::Escape);
        h.run_steps(2);
    }
}

#[test]
fn a_typed_folder_path_opens_without_the_os_dialog() {
    let _one = one_at_a_time();
    if !gpu() {
        return;
    }
    use ryucad_app::ui::app::Screen;
    let p = Project::new().with_part();
    let mut h = Harness::builder()
        .with_size(egui::vec2(1000.0, 700.0))
        .wgpu()
        .build_eframe(|cc| App::open(&cc.egui_ctx, None, ryucad()));
    h.step();
    h.get_by_label("Folder path").click();
    h.step();
    h.get_by_label("Folder path")
        .type_text(&p.root.display().to_string());
    h.step();
    h.get_by_label("Open folder").click();
    frames(&mut h, |a| {
        matches!(a.screen, Screen::Opened(_)) && a.opened().ws.idle()
    });
    assert_eq!(h.state().opened().ws.root, p.root);
}

#[test]
fn the_viewport_shows_the_colours_it_draws() {
    let _one = one_at_a_time();
    if !gpu() {
        return;
    }
    let p = Project::new().with_part();
    let mut h = window(&p);
    frames(&mut h, loaded);
    h.run_steps(3);
    let rect = h.state().opened().view.rect;
    let (dark, ppp) = (
        h.ctx.global_style().visuals.dark_mode,
        h.ctx.pixels_per_point(),
    );
    let img = h.render().unwrap();
    // A corner of the viewport: background, framed with a margin.
    let at = rect.min + egui::vec2(6.0, 6.0);
    let px = img.get_pixel((at.x * ppp) as u32, (at.y * ppp) as u32).0;
    let want = h
        .state()
        .opened()
        .view
        .look
        .viewport_background(dark)
        .map(|c| (c * 255.0).round() as u8);
    assert!(
        px.iter().zip(want).all(|(a, b)| a.abs_diff(b) <= 3),
        "the background shows as {px:?}, drawn as {want:?}"
    );
}

fn sketched(app: &App) -> bool {
    loaded(app) && app.opened().ws.model.sketches.len() == 1
}

/// Where model point `p` is in the window now.
fn on_screen(h: &Harness<'static, App>, p: [f64; 3]) -> egui::Pos2 {
    let o = h.state().opened();
    let rect = o.view.rect;
    let aspect = f64::from(rect.width() / rect.height());
    let pr = ryucad_app::core::nav::Projector::new(&o.view.nav.camera, aspect, p, 0.05);
    let n = pr.ndc(p).unwrap();
    egui::pos2(
        rect.min.x + ((n[0] + 1.0) / 2.0) as f32 * rect.width(),
        rect.min.y + ((1.0 - n[1]) / 2.0) as f32 * rect.height(),
    )
}

#[test]
fn a_click_on_a_sketch_line_selects_it() {
    let _one = one_at_a_time();
    if !gpu() {
        return;
    }
    let p = Project::new().with_part().with_top_sketch();
    let mut h = window(&p);
    frames(&mut h, sketched);
    h.step();
    // The middle of line l1, on the box's top.
    let at = on_screen(&h, [0.011, 0.002, 0.005]);
    h.event(egui::Event::PointerMoved(at));
    h.step();
    let hover = h.state().opened().view.hover.clone().unwrap();
    assert_eq!(hover.entity, "p/top/l1");
    assert_eq!(hover.kind.as_str(), "line");
    click(&h, at);
    frames(&mut h, |a| {
        a.opened()
            .ws
            .myself()
            .is_some_and(|m| m.selection == ["p/top/l1"])
            && a.opened().ws.idle()
    });
    assert_eq!(
        p.ok(&["selection"])["selection"],
        serde_json::json!(["p/top/l1"])
    );
}

#[test]
fn a_dimension_label_opens_its_value() {
    let _one = one_at_a_time();
    if !gpu() {
        return;
    }
    let p = Project::new().with_part().with_top_sketch();
    let mut h = window(&p);
    frames(&mut h, sketched);
    h.step();
    h.get_by_label("1.5 mm").click();
    frames(&mut h, |a| a.opened().ws.dimension.is_some());
    h.step();
    let field = h.get_all_by_value("1.5 mm").next().unwrap();
    field.set_text_value_accesskit("2 mm");
    h.step();
    h.get_by_label("Apply").click();
    frames(&mut h, |a| {
        a.opened()
            .ws
            .dimension
            .as_ref()
            .is_some_and(|f| !f.saving && f.value.read == "2 mm")
    });
    let shown = p.ok(&["feature", "show", "p", "top"]);
    assert_eq!(shown["constraints"]["d1"]["value"]["value"], "2 mm");
    // The label follows.
    frames(&mut h, |a| {
        a.opened()
            .ws
            .model
            .sketches
            .values()
            .any(|d| d.labels[0].text == "2 mm")
    });
}

#[test]
fn the_tree_shows_sketches_and_what_features_read() {
    let _one = one_at_a_time();
    if !gpu() {
        return;
    }
    let p = Project::new().with_sketch();
    let mut h = window(&p);
    frames(&mut h, |a| {
        let o = a.opened();
        o.ws.idle() && o.ws.model.meshes.len() == 1 && o.ws.model.sketches.len() == 1
    });
    h.step();
    h.get_by_label("s (sketch, well)");
    h.get_by_label("e (extrude)");
    h.get_by_label("uses s");
    // A sketch that can't hold fails what reads it.
    let ops = r#"[{"op": "remove", "id": "c1"}]"#;
    p.ok(&["sketch", "edit", "p", "s", ops]);
    frames(&mut h, |a| {
        a.opened()
            .ws
            .part("p")
            .and_then(|p| p.feature("s"))
            .and_then(|f| f.sketch.as_ref())
            .is_some_and(|s| s.state == "under")
    });
    h.step();
    h.get_by_label("s (sketch, under, 1 dof)");
    // Choosing the sketch frames it.
    let before = h.state().opened().view.nav.camera.clone();
    h.get_by_label("s (sketch, under, 1 dof)").click();
    frames(&mut h, |a| a.opened().view.chosen.is_some());
    assert_ne!(h.state().opened().view.nav.camera, before);
}

#[test]
fn a_parameter_is_edited_in_its_panel() {
    let _one = one_at_a_time();
    if !gpu() {
        return;
    }
    let p = Project::new().with_sketch();
    let mut h = window(&p);
    frames(&mut h, |a| {
        let o = a.opened();
        o.ws.idle() && o.ws.params.is_some() && o.ws.model.sketches.len() == 1
    });
    h.step();
    h.get_by_label("Parameters of p");
    let field = h.get_all_by_value("20 mm").next().unwrap();
    field.set_text_value_accesskit("35 mm");
    h.step();
    h.get_by_label("Apply parameters").click();
    frames(&mut h, |a| {
        a.opened()
            .ws
            .params
            .as_ref()
            .is_some_and(|p| !p.saving && !p.dirty() && p.params[0].field.read == "35 mm")
    });
    let list = p.ok(&["param", "list", "p"]);
    assert_eq!(list["parameters"][0]["value"], "35 mm");
    // The sketch's label follows.
    frames(&mut h, |a| {
        a.opened()
            .ws
            .model
            .sketches
            .values()
            .any(|d| d.labels[0].text == "width = 35 mm")
    });
}

/// The compare has its diff and every ghost it wants.
fn compared(app: &App) -> bool {
    let o = app.opened();
    o.ws.idle()
        && o.ws
            .compare
            .as_ref()
            .is_some_and(|c| c.loaded && c.ghosts.len() == c.wanted.len())
}

#[test]
fn a_compare_marks_the_tree_and_shows_what_changed() {
    let _one = one_at_a_time();
    if !gpu() {
        return;
    }
    let p = Project::new().with_part().committed();
    p.ok(&["feature", "edit", "p", "base", "--param", "x=30"]);
    p.ok(&["feature", "delete", "p", "boss"]);
    let mut h = window(&p);
    frames(&mut h, |a| {
        a.opened().ws.idle() && a.opened().ws.model.meshes.len() == 1
    });
    menu(&mut h, "View", "Compare with HEAD");
    frames(&mut h, compared);
    h.step();
    h.get_by_label("base (box) [changed]");
    h.get_by_label("1 removed");
    // A removed feature is chosen for its changes.
    h.get_by_label("boss [removed]").click();
    frames(&mut h, |a| a.opened().view.chosen.is_some());
    h.step();
    h.get_by_label("Changes since HEAD");
    h.get_by_label("result: built -> absent");
    h.get_by_label("Stop comparing").click();
    frames(&mut h, |a| a.opened().ws.compare.is_none());
    h.step();
    assert!(h.query_by_label("base (box) [changed]").is_none());
    h.get_by_label("base (box)");
    // A commit in the history is compared with by its button.
    h.get_by_label("History").click();
    frames(&mut h, |a| a.opened().ws.history.is_some());
    h.step();
    h.get_by_label("Compare").click();
    frames(&mut h, compared);
    let head = p.ok(&["compare"])["compare"]["commit"].clone();
    assert_eq!(
        Some(h.state().opened().ws.compare.as_ref().unwrap().rev.as_str()),
        head.as_str()
    );
    // A revision typed in.
    menu(&mut h, "View", "Compare with…");
    h.step();
    h.get_by_label("revision to compare with")
        .set_text_value_accesskit("main");
    h.step();
    h.get_by_label("Start comparing").click();
    frames(&mut h, |a| {
        compared(a) && a.opened().ws.compare.as_ref().unwrap().rev == "main"
    });
}

#[test]
fn a_merge_conflict_is_settled_from_its_panel() {
    let _one = one_at_a_time();
    if !gpu() {
        return;
    }
    let p = Project::new().with_part().committed().diverged();
    assert!(!p.try_git(&["merge", "--no-edit", "theirs"]));
    let mut h = window(&p);
    frames(&mut h, |a| {
        let o = a.opened();
        // The conflicted box fails in its place; the cylinder builds.
        o.ws.idle() && o.ws.merge.is_some() && o.ws.model.meshes.len() == 1
    });
    h.step();
    assert!(h.state().title().contains("(merge in progress)"));
    h.get_by_label("merge in progress");
    h.get_by_label("value in base").click();
    h.step();
    h.get_by_label("ours: 12 mm");
    h.get_by_label("theirs: 15 mm");
    h.get_by_label("Show").click();
    frames(&mut h, |a| a.opened().view.chosen.is_some());
    h.get_by_label("Take theirs").click();
    frames(&mut h, |a| {
        a.opened()
            .ws
            .merge
            .as_ref()
            .is_some_and(|m| m.clean() && !m.resolving)
    });
    h.step();
    // Said in the panel and the status line.
    assert_eq!(
        h.get_all_by_label("merge: settled, ready to git add and commit")
            .count(),
        2
    );
    h.get_by_label("Settled, waiting for git add: parts/p/features/base.json");
    // Once committed, the panel goes.
    p.git(&["add", "."]);
    p.git(&["commit", "-q", "--no-edit"]);
    frames(&mut h, |a| a.opened().ws.merge.is_none());
    h.step();
    assert!(h.query_by_label("merge in progress").is_none());
    assert!(!h.state().title().contains("in progress"));
}
