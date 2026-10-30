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

/// A window the mockups' size: 1600×1000, where the default tiles give
/// the tree, the right tabs and the bottom tabs room.
fn window(p: &Project) -> Harness<'static, App> {
    sized(p, 1000.0)
}

/// A window `height` tall: the right tabs' lower sections need room.
fn sized(p: &Project, height: f32) -> Harness<'static, App> {
    // Every window test shares the preferences: each window starts from
    // the default tiles, whatever the last one left.
    p.ok(&["config", "set", "ui.layout", "{}"]);
    let root = p.root.clone();
    Harness::builder()
        .with_size(egui::vec2(1600.0, height))
        .wgpu()
        .build_eframe(move |cc| App::open(&cc.egui_ctx, Some(&root), ryucad()))
}

/// Opens the menu `menu` and clicks `item` in it (named after its text,
/// and its shortcut if it has one). Only the open menu is searched: the
/// toolbar has buttons named as some of its items.
fn menu(h: &mut Harness<'static, App>, menu: &str, item: &str) {
    h.get_by_label(menu).click();
    let spaced = format!("{item} ");
    let named = |n: &egui_kittest::kittest::AccessKitNode<'_>| {
        n.label()
            .is_some_and(|l| l == item || l.starts_with(&spaced))
    };
    let open = |h: &Harness<'static, App>| {
        h.query_all_by_role(egui::accesskit::Role::Menu)
            .last()
            .and_then(|m| m.query_by(named).map(|_| ()))
            .is_some()
    };
    // A menu opens over a frame or two.
    for _ in 0..10 {
        h.step();
        if open(h) {
            break;
        }
    }
    h.get_all_by_role(egui::accesskit::Role::Menu)
        .last()
        .unwrap()
        .get_by(named)
        .click();
}

/// Shows tile `id` (`parameters`, `part`) as View > Panels does: the
/// tiles show one tab of a group at a time, and a tab draws (and asks for
/// what it shows) only while it is the one showing. Not by a click: the
/// right tabs are narrower than their titles, and the last is clipped.
fn show_tab(h: &mut Harness<'static, App>, id: &str) {
    ryucad_app::ui::shell::open(&mut h.state_mut().opened_mut().tiles, id);
    h.run_steps(2);
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

/// Runs frames until something labelled `label` shows: a dialog takes a
/// frame or two to appear.
fn shown(h: &mut Harness<'static, App>, label: &str) {
    for _ in 0..20 {
        h.step();
        if h.query_by_label(label).is_some() {
            return;
        }
    }
    panic!("{label} never showed");
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
    // Clicking it again keeps it, and its form (#771); Ctrl/Cmd+click
    // lets it go.
    h.get_by_label("boss (cylinder)").click();
    h.run_steps(3);
    assert!(h.state().opened().view.chosen.is_some());
    h.get_by_label("boss (cylinder)")
        .click_modifiers(egui::Modifiers::COMMAND);
    frames(&mut h, |a| a.opened().view.chosen.is_none());
    h.get_by_label("boss (cylinder)").click();
    frames(&mut h, |a| a.opened().view.chosen.is_some());
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
fn the_trees_choice_follows_what_is_picked_and_cleared() {
    let _one = one_at_a_time();
    if !gpu() {
        return;
    }
    let p = Project::new().with_part();
    let mut h = window(&p);
    frames(&mut h, loaded);
    let choose = |h: &mut Harness<'static, App>| {
        h.get_by_label("boss (cylinder)").click();
        frames(h, |a| a.opened().view.chosen.is_some());
    };
    let chosen = |h: &Harness<'static, App>| h.state().opened().view.chosen.clone();
    // Esc clears it, even with nothing selected.
    choose(&mut h);
    h.key_press(egui::Key::Escape);
    h.step();
    assert_eq!(chosen(&h), None);
    // So does a click on the background.
    choose(&mut h);
    h.step();
    let rect = h.state().opened().view.rect;
    let corner = rect.min + egui::vec2(5.0, 5.0);
    h.event(egui::Event::PointerMoved(corner));
    h.step();
    assert!(h.state().opened().view.hover.is_none());
    click(&h, corner);
    h.step();
    assert_eq!(chosen(&h), None);
    // And a click on another feature's solid; one on its own keeps it.
    choose(&mut h);
    h.step();
    let rect = h.state().opened().view.rect;
    let mut found = std::collections::BTreeMap::new();
    for i in 1..20 {
        for j in 1..20 {
            let at = rect.min
                + egui::vec2(
                    rect.width() * i as f32 / 20.0,
                    rect.height() * j as f32 / 20.0,
                );
            h.event(egui::Event::PointerMoved(at));
            h.step();
            if let Some(hit) = &h.state().opened().view.hover {
                let solid = hit.entity.split('/').nth(1).unwrap().to_owned();
                found.entry(solid).or_insert(at);
            }
        }
    }
    let (Some(&base), Some(&boss)) = (found.get("base"), found.get("boss")) else {
        panic!("found only {found:?}");
    };
    click(&h, boss);
    h.step();
    assert_eq!(chosen(&h), Some(("p".to_owned(), "boss".to_owned())));
    click(&h, base);
    h.step();
    assert_eq!(chosen(&h), None);
}

#[test]
fn the_window_keeps_its_size() {
    let _one = one_at_a_time();
    if !gpu() {
        return;
    }
    let p = Project::new().with_part();
    let mut h = window(&p);
    frames(&mut h, loaded);
    // Once it has held still, the size goes to `app.window`, from which
    // the next window opens.
    let saved = |p: &Project| p.ok(&["config", "get", "app.window"])["value"].clone();
    let t = Instant::now();
    while saved(&p)["size"] != serde_json::json!([1000.0, 700.0]) {
        assert!(t.elapsed() < Duration::from_secs(30), "{}", saved(&p));
        h.step();
        std::thread::sleep(Duration::from_millis(100));
    }
    assert_eq!(saved(&p)["maximized"], false);
    let g = ryucad_app::ui::app::saved_geometry().unwrap();
    assert_eq!(g.size, [1000.0, 700.0]);
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
    // Escape that closes a menu leaves it (#769)...
    h.get_by_label("File").click();
    h.run_steps(2);
    assert!(egui::Popup::is_any_open(&h.ctx));
    h.key_press(egui::Key::Escape);
    h.run_steps(3);
    assert!(!egui::Popup::is_any_open(&h.ctx));
    assert_eq!(h.state().opened().ws.myself().unwrap().selection, mine);
    // ...and the next Escape clears it.
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
fn the_hover_goes_when_the_pointer_leaves_the_view() {
    let _one = one_at_a_time();
    if !gpu() {
        return;
    }
    let p = Project::new().with_part();
    let mut h = window(&p);
    frames(&mut h, loaded);
    h.step();
    let rect = h.state().opened().view.rect;
    let over = |h: &mut Harness<'static, App>, e: egui::Event| {
        h.event(e);
        h.step();
        h.step();
        h.state().opened().view.hover.is_some()
    };
    assert!(over(&mut h, egui::Event::PointerMoved(rect.center())));
    // Out of the window, and over a panel beside the view.
    assert!(!over(&mut h, egui::Event::PointerGone));
    assert!(over(&mut h, egui::Event::PointerMoved(rect.center())));
    let beside = egui::pos2(rect.min.x - 20.0, rect.center().y);
    assert!(!over(&mut h, egui::Event::PointerMoved(beside)));
    // Nor is its description left drawn.
    assert!(
        h.query_by(|n| n.label().is_some_and(|l| l.contains("face of")))
            .is_none()
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
fn a_patterns_count_is_edited_in_the_property_panel() {
    let _one = one_at_a_time();
    if !gpu() {
        return;
    }
    let p = Project::new().with_part();
    p.ok(&[
        "feature", "add", "p", "sketch", "--id", "s", "--origin", "0,0,5",
    ]);
    let ops = r#"[{"op": "add", "type": "point", "at": [14, 3]},
                  {"op": "add", "type": "fix", "a": "p1"}]"#;
    p.ok(&["sketch", "edit", "p", "s", ops]);
    p.ok(&[
        "feature",
        "add",
        "p",
        "hole",
        "--id",
        "h",
        "--sketch",
        "s",
        "--target",
        "base",
        "--diameter",
        "1",
        "--depth",
        "2",
    ]);
    p.ok(&[
        "feature",
        "add",
        "p",
        "pattern",
        "--id",
        "row",
        "--seed",
        "h",
        "--direction",
        "y",
        "--spacing",
        "2",
        "--count",
        "3",
    ]);
    let mut h = window(&p);
    frames(&mut h, loaded);
    h.get_by_label("row (pattern, cut)").click();
    frames(&mut h, |a| a.opened().ws.form.is_some());
    h.step();
    // Its seed and reversal are shown; its count is typed over.
    h.get_by_label("h");
    h.get_by_label("against the direction");
    let field = h.get_all_by_value("3").next().unwrap();
    field.set_text_value_accesskit("4");
    h.step();
    h.get_by_label("Apply").click();
    frames(&mut h, |a| {
        a.opened()
            .ws
            .form
            .as_ref()
            .is_some_and(|f| !f.saving && !f.dirty())
    });
    let shown = p.ok(&["feature", "show", "p", "row"]);
    assert_eq!(shown["count"]["value"], "4");
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
    // Newest first: the top Undo is the edit's (past the toolbar's).
    h.get_all_by_label("Undo").nth(1).unwrap().click();
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
    // It says what the disk cache holds, and clears it.
    frames(&mut h, |a| a.opened().ws.cache.is_some());
    h.step();
    h.get_by_role_and_label(egui::accesskit::Role::Button, "Clear cache")
        .click();
    frames(&mut h, |a| {
        a.opened()
            .ws
            .cache
            .as_ref()
            .is_some_and(|c| c["removed"].is_object())
    });
    assert_eq!(p.ok(&["cache", "stats"])["entries"], 0);
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
    // The shared cache: a server, and a token kept apart from the
    // preferences.
    let url = "https://cache.example.com";
    type_into(&mut h, "Shared cache server", url);
    h.key_press(egui::Key::Enter);
    frames(&mut h, |a| {
        a.opened().ws.settings.text("cache.remote", "") == url
    });
    assert_eq!(p.ok(&["config", "get", "cache.remote"])["value"], url);
    frames(&mut h, |a| {
        a.opened()
            .ws
            .cache
            .as_ref()
            .is_some_and(|c| c["remote"]["url"] == url)
    });
    type_into(&mut h, "Token", "ryc_window");
    h.get_by_role_and_label(egui::accesskit::Role::Button, "Save token")
        .click();
    frames(&mut h, |a| {
        a.opened()
            .ws
            .cache_token
            .as_ref()
            .is_some_and(|t| t["token"] == "file")
    });
    h.step();
    h.get_by_role_and_label(egui::accesskit::Role::Button, "Forget token")
        .click();
    frames(&mut h, |a| {
        a.opened()
            .ws
            .cache_token
            .as_ref()
            .is_some_and(|t| t["token"].is_null())
    });
    p.ok(&["config", "set", "ui.style", "drafting"]);
    p.ok(&["config", "set", "ui.theme", "system"]);
    p.ok(&["config", "set", "display.edges", "true"]);
    p.ok(&["config", "set", "cache.remote", "\"\""]);
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
    // What a repair would change is said only while one is left to make.
    let left = match &h.state().screen {
        ryucad_app::ui::app::Screen::Damaged(d) => d.0.report.as_ref().unwrap()["issues"]
            .as_array()
            .is_some_and(|i| !i.is_empty()),
        _ => unreachable!(),
    };
    assert_eq!(
        h.query_by_label(
            "Nothing is changed until you choose a repair, and a repair changes only the files in its planned changes. Git has the files as they were committed.",
        )
        .is_some(),
        left
    );
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
    let close = h.ctx.format_shortcut(&egui::KeyboardShortcut::new(
        egui::Modifiers::COMMAND,
        egui::Key::W,
    ));
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
fn a_click_on_a_cut_face_selects_it() {
    let _one = one_at_a_time();
    if !gpu() {
        return;
    }
    // A pocket 6 × 4 × 2 mm cut into the base's top.
    let p = Project::new().with_part();
    p.ok(&[
        "feature",
        "add",
        "p",
        "box",
        "--id",
        "pocket",
        "--operation",
        "cut",
        "--target",
        "base",
        "--param",
        "x=6",
        "--param",
        "y=4",
        "--param",
        "z=2",
        "--origin",
        "10,3,3",
    ]);
    let mut h = window(&p);
    frames(&mut h, loaded);
    // Looking down into the pocket, framed: its floor fills the view.
    if let ryucad_app::ui::app::Screen::Opened(o) = &mut h.state_mut().screen {
        o.view.nav.camera = ryucad::view::frame(
            [0.009, 0.002, 0.003],
            [0.017, 0.008, 0.005],
            ryucad::view::StandardView::Top,
            ryucad::view::DEFAULT_PROJECTION,
        );
    }
    h.run_steps(2);
    // The middle of the pocket's floor: a face of the base the cut made.
    let at = on_screen(&h, [0.013, 0.005, 0.003]);
    h.event(egui::Event::PointerMoved(at));
    h.step();
    let hover = h.state().opened().view.hover.clone().unwrap();
    assert_eq!(hover.kind.as_str(), "face");
    let name = hover.entity.strip_prefix("p/base/").unwrap().to_owned();
    let floor = p.ok(&["inspect", "p"])["bodies"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|b| b["faces"].as_array().unwrap().clone())
        .find(|f| f["name"] == name)
        .unwrap();
    assert_eq!(floor["origin"], serde_json::json!({"feature": "pocket"}));
    let z = floor["surface"]["frame"]["origin"][2].as_f64().unwrap();
    assert!((z - 0.003).abs() < 1e-12, "{floor}");
    click(&h, at);
    frames(&mut h, |a| {
        a.opened()
            .ws
            .myself()
            .is_some_and(|m| m.selection == [hover.entity.clone()])
            && a.opened().ws.idle()
    });
    assert_eq!(
        p.ok(&["selection"])["selection"],
        serde_json::json!([hover.entity])
    );
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
    frames(&mut h, |a| a.opened().ws.idle());
    show_tab(&mut h, "parameters");
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
    show_tab(&mut h, "changes");
    h.get_by_label("Changes since HEAD");
    h.get_by_label("result: built → absent");
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
    // The Conflicts tile opened, the first of the right tabs, showing.
    assert!(ryucad_app::ui::shell::is_open(
        &h.state().opened().tiles,
        "conflicts"
    ));
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
    // The tile closed itself with the merge.
    assert!(!ryucad_app::ui::shell::is_open(
        &h.state().opened().tiles,
        "conflicts"
    ));
    assert!(h.query_by_label("merge in progress").is_none());
    assert!(!h.state().title().contains("in progress"));
}

#[test]
fn parts_show_their_release_state_and_a_compare_marks_proposals() {
    let _one = one_at_a_time();
    if !gpu() {
        return;
    }
    let p = Project::new().with_part();
    p.ok(&["pdm", "init"]);
    p.ok(&["props", "set", "p", "number=100"]);
    let p = p.committed();
    let mut h = window(&p);
    frames(&mut h, |a| {
        loaded(a) && a.opened().ws.states.get("p").is_some()
    });
    h.step();
    h.get_by_label("p (in work)");
    p.ok(&["release", "propose", "p"]);
    frames(&mut h, |a| {
        a.opened().ws.states.badge("p").as_deref() == Some("in work, A proposed")
    });
    h.step();
    h.get_by_label("p (in work, A proposed)");
    // Compared with HEAD, the part is marked with what it proposes, and
    // its changes listed with a feature's.
    menu(&mut h, "View", "Compare with HEAD");
    frames(&mut h, |a| {
        compared(a)
            && a.opened()
                .ws
                .compare
                .as_ref()
                .is_some_and(|c| c.pdm.contains_key("p"))
    });
    h.step();
    h.get_by_label("p (in work, A proposed) [proposes A]");
    h.get_by_label("base (box)").click();
    frames(&mut h, |a| a.opened().view.chosen.is_some());
    h.step();
    show_tab(&mut h, "changes");
    h.get_by_label("Part p");
    h.get_by_label("state: in work → in work, A proposed");
}

#[test]
fn a_property_is_edited_in_the_part_section_and_pdm_set_up() {
    let _one = one_at_a_time();
    if !gpu() {
        return;
    }
    let p = Project::new().with_part();
    let mut h = sized(&p, 1200.0);
    frames(&mut h, loaded);
    show_tab(&mut h, "part");
    frames(&mut h, |a| {
        let o = a.opened();
        loaded(a) && o.ws.props.is_some() && o.ws.pdm_config.is_some()
    });
    h.step();
    h.get_by_label("Properties of p");
    // No ID yet: PDM is set up from the window.
    h.get_by_label("Set up PDM").click();
    // Set up, the window reads the part's properties again; typing before
    // that answer lands would be lost under it (seen on Windows CI, where
    // the round trip is slow). Wait for it: nothing pending.
    frames(&mut h, |a| {
        a.opened().ws.pdm_ready() && a.opened().ws.idle()
    });
    h.step();
    assert!(h.query_by_label("Set up PDM").is_none());
    h.get_by_label("number *").click();
    let field = h.get_by_label("number *");
    field.set_text_value_accesskit("100-0042");
    h.step();
    h.get_by_label("Apply properties").click();
    frames(&mut h, |a| {
        a.opened()
            .ws
            .props
            .as_ref()
            .is_some_and(|p| !p.saving && !p.dirty() && p.props[0].field.read == "100-0042")
    });
    let got = p.ok(&["props", "show", "p"]);
    assert_eq!(got["properties"][0]["value"], "100-0042");
}

#[test]
fn a_release_is_proposed_synced_and_made_obsolete_from_the_releases_tab() {
    let _one = one_at_a_time();
    if !gpu() {
        return;
    }
    let p = Project::new().with_part();
    p.ok(&["pdm", "init"]);
    p.ok(&["props", "set", "p", "number=100"]);
    let p = p.committed();
    let mut h = window(&p);
    frames(&mut h, loaded);
    h.get_by_label("Releases").click();
    frames(&mut h, |a| {
        a.opened().ws.idle() && a.opened().ws.releases.list.is_some()
    });
    h.step();
    h.get_by_label("Releases of p");
    h.get_by_label("No release records yet");
    // Propose: the dialog says the label, then the record is written.
    h.get_by_label("Propose…").click();
    frames(&mut h, |a| a.opened().ws.releases.proposal.is_some());
    shown(&mut h, "It will be revision A.");
    h.get_by_label("Note").click();
    h.get_by_label("Note").set_text_value_accesskit("first");
    h.step();
    h.get_by_label("Propose").click();
    frames(&mut h, |a| a.opened().ws.releases.records("p").len() == 1);
    shown(&mut h, "wrote releases/p/A.json");
    assert!(h.query_by_label("It will be revision A.").is_none());
    // Official once main holds it; its tag is synced after confirming.
    p.git(&["add", "."]);
    p.git(&["commit", "-q", "-m", "release p A"]);
    frames(&mut h, |a| {
        a.opened().ws.idle() && !a.opened().ws.releases.to_sync().is_empty()
    });
    h.step();
    h.get_by_label("Sync tags…").click();
    shown(&mut h, "create ryupdm/p/A");
    h.get_by_label("Sync").click();
    frames(&mut h, |a| {
        let t = &a.opened().ws.releases;
        a.opened().ws.idle() && !t.busy && t.done.is_some() && t.to_sync().is_empty()
    });
    shown(&mut h, "tags: 1 created, 0 moved");
    // Obsolete, confirmed.
    h.get_by_label("Obsolete…").click();
    shown(&mut h, "Make obsolete");
    h.get_by_label("Make obsolete").click();
    frames(&mut h, |a| a.opened().ws.releases.records("p").len() == 2);
    let list = p.ok(&["release", "list", "p"]);
    assert_eq!(list["records"][1]["kind"], "obsolete");
}

#[test]
fn an_assembly_lists_its_instances_and_offers_what_to_show() {
    let _one = one_at_a_time();
    if !gpu() {
        return;
    }
    let p = Project::new();
    p.ok(&["part", "create", "bolt"]);
    p.ok(&[
        "feature",
        "add",
        "bolt",
        "cylinder",
        "--id",
        "shank",
        "--param",
        "radius=3",
        "--param",
        "height=20",
    ]);
    p.ok(&["part", "create", "pair", "--assembly"]);
    p.ok(&[
        "feature", "add", "pair", "instance", "--id", "left", "--of", "bolt",
    ]);
    p.ok(&["part", "create", "frame", "--assembly"]);
    p.ok(&[
        "feature", "add", "frame", "instance", "--id", "p1", "--of", "pair", "--fixed",
    ]);
    let mut h = window(&p);
    frames(&mut h, |a| match &a.screen {
        ryucad_app::ui::app::Screen::Opened(o) => {
            o.ws.idle()
                && o.ws
                    .model
                    .meshes
                    .contains_key(&("frame".into(), "p1/left/shank".into()))
        }
        _ => false,
    });
    h.step();
    h.get_by(|n| {
        n.label()
            .is_some_and(|l| l.starts_with("frame") && l.ends_with("(assembly)"))
    });
    h.get_by_label("p1 (pair @ working, fixed)");
    // The pair's own instance, under the one that places it.
    let under = h.get_all_by_label("left (bolt @ working)").count();
    assert_eq!(under, 2, "in the frame, and in the pair itself");
    // View > Show offers the project's model and each part.
    h.get_by_label("View").click();
    shown(&mut h, "Show");
    h.get_by_label("Show").click();
    shown(&mut h, "The project's model");
    h.get_by_label("frame (assembly)");
}

#[test]
fn the_measure_tool_measures_two_faces_and_esc_leaves_it() {
    let _one = one_at_a_time();
    if !gpu() {
        return;
    }
    // The base's top at 5 mm, the boss's top 8 mm above it.
    let p = Project::new().with_part();
    let mut h = window(&p);
    frames(&mut h, loaded);
    if let ryucad_app::ui::app::Screen::Opened(o) = &mut h.state_mut().screen {
        o.view.nav.camera = ryucad::view::frame(
            [-0.004, -0.004, 0.0],
            [0.021, 0.011, 0.013],
            ryucad::view::StandardView::Top,
            ryucad::view::DEFAULT_PROJECTION,
        );
    }
    h.run_steps(2);
    menu(&mut h, "Tools", "Measure");
    frames(&mut h, |a| a.opened().ws.measure.on);
    let pick = |h: &mut Harness<'static, App>, p: [f64; 3]| {
        let at = on_screen(h, p);
        h.event(egui::Event::PointerMoved(at));
        h.step();
        click(h, at);
        h.step();
    };
    pick(&mut h, [0.015, 0.005, 0.005]);
    pick(&mut h, [0.0, 0.0, 0.013]);
    frames(&mut h, |a| {
        a.opened()
            .ws
            .measure
            .reading
            .as_ref()
            .is_some_and(|r| r.line.is_some())
    });
    // Picks measure; they don't select.
    assert!(
        h.state()
            .opened()
            .ws
            .myself()
            .is_some_and(|m| m.selection.is_empty())
    );
    let r = h.state().opened().ws.measure.reading.clone().unwrap();
    assert_eq!(r.label.as_deref(), Some("8 mm"), "{r:?}");
    shown(&mut h, "8 mm (exact)");
    h.get_by_label("Copy");
    // Esc leaves the tool, and its panel goes.
    h.key_press(egui::Key::Escape);
    frames(&mut h, |a| !a.opened().ws.measure.on);
    h.run_steps(2);
    assert!(h.query_by_label("8 mm (exact)").is_none());
}

#[test]
fn tools_appearance_colours_a_face_and_ctrl_z_undoes_it() {
    let _one = one_at_a_time();
    if !gpu() {
        return;
    }
    let p = Project::new().with_part();
    let mut h = window(&p);
    frames(&mut h, loaded);
    if let ryucad_app::ui::app::Screen::Opened(o) = &mut h.state_mut().screen {
        o.view.nav.camera = ryucad::view::frame(
            [-0.004, -0.004, 0.0],
            [0.021, 0.011, 0.013],
            ryucad::view::StandardView::Top,
            ryucad::view::DEFAULT_PROJECTION,
        );
    }
    h.run_steps(2);
    // The base's top, selected by a click.
    let at = on_screen(&h, [0.015, 0.005, 0.005]);
    h.event(egui::Event::PointerMoved(at));
    h.step();
    click(&h, at);
    frames(&mut h, |a| {
        a.opened()
            .ws
            .myself()
            .is_some_and(|m| m.selection.len() == 1)
    });
    let face = h.state().opened().ws.myself().unwrap().selection[0].clone();
    assert!(face.starts_with("p/base/"), "{face}");
    menu(&mut h, "Tools", "Appearance");
    shown(&mut h, "Colour what");
    assert_eq!(
        h.state().opened().ws.appearance.target(),
        Some(face.as_str())
    );
    // Typed, and written once Enter is pressed.
    h.get_by_label("Hex").click();
    h.step();
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.get_by_label("Hex").type_text("#ff0000");
    h.key_press(egui::Key::Enter);
    let file = p.root.join("parts/p/appearance.json");
    let name = face.rsplit('/').next().unwrap().to_owned();
    frames(&mut h, |a| {
        file.is_file()
            && a.opened()
                .ws
                .model
                .looks
                .get(&("p".to_owned(), "base".to_owned()))
                .is_some_and(|l| l.face(&name).colour == Some([1.0, 0.0, 0.0]))
    });
    assert_eq!(
        p.ok(&["appearance", "p"])["solids"][0]["faces"][&name]["colour"],
        "#ff0000"
    );
    // One undoable change: Ctrl+Z takes it back, and the face is plain.
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Z);
    frames(&mut h, |a| {
        !file.exists() && a.opened().ws.model.looks.is_empty()
    });
    // An opacity dragged previews on the model, written when let go.
    h.state_mut().opened_mut().ws.appearance.chosen = 1;
    h.state_mut()
        .opened_mut()
        .ws
        .preview_appearance(ryucad_app::core::appearance::Preview {
            colour: None,
            opacity: Some(0.4),
        });
    assert!(
        h.state()
            .opened()
            .ws
            .model
            .looks
            .values()
            .any(|l| l.translucent())
    );
    assert!(!file.exists(), "a preview writes nothing");
    h.state_mut().opened_mut().ws.appearance.opacity = 0.4;
    h.state_mut().opened_mut().ws.write_appearance("opacity");
    frames(&mut h, |a| {
        file.is_file()
            && a.opened().ws.appearance.dragging.is_none()
            && a.opened().ws.model.looks.values().any(|l| l.translucent())
    });
    assert_eq!(p.ok(&["appearance", "p"])["solids"][0]["opacity"], 0.4);
    h.state_mut().opened_mut().ws.close_appearance();
    h.step();
}

#[test]
fn tools_mass_properties_and_clearance_list_their_values() {
    let _one = one_at_a_time();
    if !gpu() {
        return;
    }
    // Blocks, not cylinders: a curved body's certified mass and clearance
    // take tens of seconds in a debug build (#195), and this test is about
    // the window, not the kernel's bounds.
    let p = two_bolts_as("box", &["x=6", "y=6", "z=20"]);
    // A third bolt 0.2 mm clear of the first.
    p.ok(&[
        "feature", "add", "frame", "instance", "--id", "b3", "--of", "bolt", "--origin", "6.2,0,0",
    ]);
    p.ok(&[
        "materials",
        "set",
        "steel",
        "--density",
        "7850 kg/m^3",
        "--default",
    ]);
    let mut h = window(&p);
    both_bolts(&mut h);
    menu(&mut h, "Tools", "Mass properties");
    frames(&mut h, |a| {
        a.opened().ws.mass.as_ref().is_some_and(|m| m.loaded)
    });
    let m = h.state().opened().ws.mass.clone().unwrap();
    assert!(m.error.is_none() && m.centre.is_some(), "{m:?}");
    assert_eq!(m.lines[0].0, "Mass", "{m:?}");
    shown(&mut h, "Centre of mass");
    menu(&mut h, "Tools", "Clearance");
    frames(&mut h, |a| {
        a.opened().ws.clearance.as_ref().is_some_and(|c| c.loaded)
    });
    let c = h.state().opened().ws.clearance.clone().unwrap();
    assert!(c.error.is_none(), "{c:?}");
    assert_eq!(c.gaps.len(), 1, "{c:?}");
    assert_eq!(
        (c.gaps[0].a.as_str(), c.gaps[0].b.as_str()),
        ("frame/b1/shank", "frame/b3/shank")
    );
    shown(&mut h, "Show");
    let before = h.state().opened().view.nav.camera.clone();
    h.get_by_label("Show").click();
    h.run_steps(4);
    assert_ne!(h.state().opened().view.nav.camera, before);
    assert_eq!(
        h.state().opened().ws.clearance.as_ref().unwrap().shown,
        Some(0)
    );
    h.state_mut().opened_mut().dialogs.clearance = false;
    h.state_mut().opened_mut().dialogs.mass = false;
    h.step();
    assert!(h.state().opened().ws.clearance.is_none() && h.state().opened().ws.mass.is_none());
}

#[test]
fn tools_check_interference_lists_and_paints_the_pairs() {
    let _one = one_at_a_time();
    if !gpu() {
        return;
    }
    let p = two_bolts();
    // A third bolt into the first.
    p.ok(&[
        "feature", "add", "frame", "instance", "--id", "b3", "--of", "bolt", "--origin", "4,0,0",
    ]);
    let mut h = window(&p);
    both_bolts(&mut h);
    menu(&mut h, "Tools", "Check interference");
    frames(&mut h, |a| {
        a.opened()
            .ws
            .interference
            .as_ref()
            .is_some_and(|i| i.loaded)
    });
    // A new window is laid out a frame or two after it opens.
    let row = |h: &Harness<'static, App>| {
        // A label's text is its value, on the label and its text run.
        h.query_all_by(|n| {
            [n.label(), n.value()]
                .into_iter()
                .flatten()
                .any(|l| l.starts_with("b1/shank and b3/shank: "))
        })
        .next()
        .is_some()
    };
    for _ in 0..20 {
        if row(&h) {
            break;
        }
        h.step();
    }
    assert!(
        row(&h),
        "no row for the pair (dialog open: {}): {:?}",
        h.state().opened().dialogs.interference,
        h.state().opened().ws.interference
    );
    // Its faces are painted, and Show frames them.
    let before = h.state().opened().view.nav.camera.clone();
    h.get_by_label("Show").click();
    for _ in 0..4 {
        h.step();
    }
    assert_ne!(h.state().opened().view.nav.camera, before);
    let i = h.state().opened().ws.interference.clone().unwrap();
    assert!(i.faces().count() >= 2, "{i:?}");
    // Closing the dialog ends the paint (#138).
    h.state_mut().opened_mut().dialogs.interference = false;
    h.step();
    assert!(h.state().opened().ws.interference.is_none());
}

#[test]
fn choosing_a_subassembly_frames_everything_it_places() {
    let _one = one_at_a_time();
    if !gpu() {
        return;
    }
    let p = Project::new();
    p.ok(&["part", "create", "bolt"]);
    p.ok(&[
        "feature",
        "add",
        "bolt",
        "cylinder",
        "--id",
        "shank",
        "--param",
        "radius=3",
        "--param",
        "height=20",
    ]);
    // A pair of bolts 80 mm apart, and a frame placing the pair, with a
    // third bolt well away so the pair alone isn't the whole view.
    p.ok(&["part", "create", "pair", "--assembly"]);
    p.ok(&[
        "feature", "add", "pair", "instance", "--id", "l", "--of", "bolt",
    ]);
    p.ok(&[
        "feature", "add", "pair", "instance", "--id", "r", "--of", "bolt", "--origin", "80,0,0",
    ]);
    p.ok(&["part", "create", "frame", "--assembly"]);
    p.ok(&[
        "feature", "add", "frame", "instance", "--id", "s1", "--of", "pair", "--fixed",
    ]);
    p.ok(&[
        "feature", "add", "frame", "instance", "--id", "far", "--of", "bolt", "--origin", "0,500,0",
    ]);
    let mut h = window(&p);
    frames(&mut h, |a| match &a.screen {
        ryucad_app::ui::app::Screen::Opened(o) => {
            o.ws.idle()
                && ["s1/l/shank", "s1/r/shank", "far/shank"].iter().all(|k| {
                    o.ws.model
                        .meshes
                        .contains_key(&("frame".into(), (*k).into()))
                })
        }
        _ => false,
    });
    h.step();
    h.get_by_label("s1 (pair @ working, fixed)").click();
    for _ in 0..4 {
        h.step();
    }
    // Both bolts framed (issue #107): the view's target between them,
    // not on the first.
    let t = h.state().opened().view.nav.camera.target;
    assert!((0.03..0.05).contains(&t[0]), "{t:?}");
    assert!(t[1].abs() < 0.01, "{t:?}");
}

#[test]
fn a_mates_angle_is_edited_in_the_property_panel() {
    let _one = one_at_a_time();
    if !gpu() {
        return;
    }
    let p = common::mated();
    p.ok(&["feature", "edit", "frame", "m1", "--angle", "30deg"]);
    let mut h = window(&p);
    frames(&mut h, |a| match &a.screen {
        ryucad_app::ui::app::Screen::Opened(o) => {
            o.ws.idle()
                && o.ws
                    .model
                    .meshes
                    .contains_key(&("frame".into(), "ring:3/pin/shank".into()))
        }
        _ => false,
    });
    h.step();
    // The tree: the assembly's solve, what its mates leave each instance,
    // the mate and the pattern's copies.
    h.get_by(|n| {
        n.label()
            .is_some_and(|l| l.starts_with("frame") && l.ends_with("(assembly, well)"))
    });
    h.get_by_label("pin (bolt @ working, 0 dof)");
    h.get_by_label("base (plate @ working, fixed)");
    h.get_by_label("ring (instance_pattern, 3 copies)");
    h.get_by_label("ring:2/pin");
    h.get_by_label("m1 (revolute: base → pin)").click();
    frames(&mut h, |a| a.opened().ws.form.is_some());
    // Chosen, the mate draws both its connectors' frames: three axes each,
    // from where the report says they are.
    let (lines, names) = {
        let o = h.state_mut().opened_mut();
        o.view.mate_frames(&o.ws)
    };
    assert_eq!(lines.len(), 6);
    assert_eq!(
        names.iter().map(|n| n.0.as_str()).collect::<Vec<_>>(),
        ["a", "b"]
    );
    let report = p.ok(&["rebuild", "frame"]);
    let m1 = report["parts"][0]["features"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["id"] == "m1")
        .unwrap()
        .clone();
    let origin = |v: &serde_json::Value| [0, 1, 2].map(|k| v[k].as_f64().unwrap());
    assert_eq!(names[0].1, origin(&m1["mate"]["frames"]["a"]["origin_m"]));
    assert_eq!(names[1].1, origin(&m1["mate"]["frames"]["b"]["origin_m"]));
    h.step();
    let field = h.get_all_by_value("30 deg").next().unwrap();
    field.set_text_value_accesskit("45 deg");
    h.step();
    h.get_by_label("Apply").click();
    frames(&mut h, |a| {
        a.opened()
            .ws
            .form
            .as_ref()
            .is_some_and(|f| !f.saving && !f.dirty())
    });
    let shown = p.ok(&["feature", "show", "frame", "m1"]);
    assert_eq!(shown["angle"]["value"], "45 deg");
    // The pin turned with it, written.
    let solved = p.ok(&["assembly", "solve", "frame"]);
    assert_eq!(solved["solve"]["stale"], false);
}

/// A bolt, and an assembly `frame` placing it twice, 40 mm apart.
fn two_bolts() -> Project {
    two_bolts_as("cylinder", &["radius=3", "height=20"])
}

/// [`two_bolts`] with each bolt's shank a `kind` primitive of `params`.
fn two_bolts_as(kind: &str, params: &[&str]) -> Project {
    let p = Project::new();
    p.ok(&["part", "create", "bolt"]);
    let mut add = vec!["feature", "add", "bolt", kind, "--id", "shank"];
    for x in params {
        add.extend(["--param", x]);
    }
    p.ok(&add);
    p.ok(&["part", "create", "frame", "--assembly"]);
    p.ok(&[
        "feature", "add", "frame", "instance", "--id", "b1", "--of", "bolt", "--fixed",
    ]);
    p.ok(&[
        "feature", "add", "frame", "instance", "--id", "b2", "--of", "bolt", "--origin", "40,0,0",
    ]);
    p
}

/// Runs frames until both bolts are drawn.
fn both_bolts(h: &mut Harness<'static, App>) {
    frames(h, |a| match &a.screen {
        ryucad_app::ui::app::Screen::Opened(o) => {
            o.ws.idle()
                && ["b1/shank", "b2/shank"].iter().all(|k| {
                    o.ws.model
                        .meshes
                        .contains_key(&("frame".into(), (*k).into()))
                })
        }
        _ => false,
    });
    h.step();
}

/// A point of the view over `feature`'s body, found by moving the
/// pointer across the view.
fn over_body(h: &mut Harness<'static, App>, feature: &str) -> egui::Pos2 {
    let rect = h.state().opened().view.rect;
    for j in 1..16 {
        for i in 1..24 {
            let at = egui::pos2(
                rect.min.x + rect.width() * i as f32 / 24.0,
                rect.min.y + rect.height() * j as f32 / 16.0,
            );
            h.event(egui::Event::PointerMoved(at));
            h.step();
            if h.state()
                .opened()
                .view
                .hover
                .as_ref()
                .is_some_and(|x| x.feature == feature)
            {
                return at;
            }
        }
    }
    panic!("{feature} isn't in view");
}

fn hidden(app: &App) -> Vec<String> {
    app.opened().ws.model.hidden.iter().cloned().collect()
}

/// Presses Tab with `modifiers` as the window gets it: through the app's
/// input hook, which takes Tab over the viewport away from egui (#136).
fn tab(h: &mut Harness<'static, App>, modifiers: egui::Modifiers) {
    let ctx = h.ctx.clone();
    let mut raw = egui::RawInput::default();
    for pressed in [true, false] {
        raw.events.push(egui::Event::Key {
            key: egui::Key::Tab,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers,
        });
    }
    eframe::App::raw_input_hook(h.state_mut(), &ctx, &mut raw);
    for e in raw.events {
        h.event(e);
    }
    h.step();
}

#[test]
fn occurrences_hide_and_show_with_tab_and_the_tree() {
    let _one = one_at_a_time();
    if !gpu() {
        return;
    }
    let p = two_bolts();
    let mut h = window(&p);
    both_bolts(&mut h);
    // Tab hides the bolt under the mouse: the session hears of it, and
    // it is no longer drawn or picked.
    let at = over_body(&mut h, "b2/shank");
    let focus = h.ctx.memory(|m| m.focused());
    tab(&mut h, egui::Modifiers::NONE);
    frames(&mut h, |a| hidden(a) == ["frame/b2"]);
    // Keyboard focus stays where it was (#136).
    assert_eq!(h.ctx.memory(|m| m.focused()), focus);
    let me = h.state().opened().ws.me().to_owned();
    let people = p.ok(&["participants"]);
    let mine = people["participants"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["id"] == me.as_str())
        .unwrap()
        .clone();
    assert_eq!(mine["hidden"], serde_json::json!(["frame/b2"]));
    assert_eq!(h.state().opened().ws.model.drawn().count(), 1);
    h.event(egui::Event::PointerMoved(at));
    h.step();
    h.step();
    assert!(
        h.state()
            .opened()
            .view
            .hover
            .as_ref()
            .is_none_or(|x| x.feature != "b2/shank")
    );
    // The tree says so; Shift+Tab over where it was shows it again.
    h.get_by_label("Show frame/b2");
    h.event(egui::Event::PointerMoved(at));
    h.step();
    tab(&mut h, egui::Modifiers::SHIFT);
    frames(&mut h, |a| hidden(a).is_empty());
    // The tree's checkbox hides and shows; Ctrl+Shift+Tab shows all.
    h.get_by_label("Show frame/b1").click();
    frames(&mut h, |a| hidden(a) == ["frame/b1"]);
    h.get_by_label("Show frame/b1").click();
    frames(&mut h, |a| hidden(a).is_empty());
    p.ok(&["hide", "frame/b1", "frame/b2"]);
    frames(&mut h, |a| hidden(a).len() == 2);
    h.event(egui::Event::PointerMoved(at));
    h.step();
    tab(&mut h, egui::Modifiers::COMMAND | egui::Modifiers::SHIFT);
    frames(&mut h, |a| hidden(a).is_empty());
    // Ctrl+Shift+Tab as macOS sends it, Ctrl not being Command there (#142).
    p.ok(&["hide", "frame/b1"]);
    frames(&mut h, |a| hidden(a).len() == 1);
    h.event(egui::Event::PointerMoved(at));
    h.step();
    tab(
        &mut h,
        egui::Modifiers {
            ctrl: true,
            shift: true,
            ..Default::default()
        },
    );
    frames(&mut h, |a| hidden(a).is_empty());
}

#[test]
fn a_free_instance_is_dragged_and_written_once() {
    let _one = one_at_a_time();
    if !gpu() {
        return;
    }
    let p = two_bolts();
    // Where the left button doesn't navigate, a left-drag moves.
    p.ok(&["config", "set", "ui.navigation", "solidworks"]);
    let mut h = window(&p);
    both_bolts(&mut h);
    frames(&mut h, |a| {
        a.opened().view.nav.style == ryucad_app::core::nav::Navigation::Solidworks
    });
    let moves = |p: &Project| {
        p.ok(&["activity"])["entries"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| e["method"] == "assembly.move")
            .count()
    };
    let before = moves(&p);
    let press = |h: &Harness<'static, App>, pos: egui::Pos2, pressed: bool| {
        h.event(egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        });
    };
    // The fixed bolt doesn't move, and the status line says why.
    let fixed = over_body(&mut h, "b1/shank");
    press(&h, fixed, true);
    for i in 1..=5 {
        h.event(egui::Event::PointerMoved(
            fixed + egui::vec2(8.0 * i as f32, 0.0),
        ));
        h.step();
    }
    press(&h, fixed + egui::vec2(40.0, 0.0), false);
    h.step();
    assert_eq!(
        h.state().opened().ws.drag_note.as_deref(),
        Some("b1 is fixed")
    );
    // The free one follows the pointer, previewed by solves, and one move
    // is written when it is let go.
    let at = over_body(&mut h, "b2/shank");
    press(&h, at, true);
    for i in 1..=10 {
        h.event(egui::Event::PointerMoved(
            at + egui::vec2(6.0 * i as f32, 0.0),
        ));
        h.step();
        std::thread::sleep(Duration::from_millis(20));
    }
    frames(&mut h, |a| !a.opened().ws.model.preview.is_empty());
    press(&h, at + egui::vec2(60.0, 0.0), false);
    frames(&mut h, |a| {
        a.opened().ws.drag.is_none() && a.opened().ws.idle()
    });
    let t = Instant::now();
    while moves(&p) == before {
        assert!(t.elapsed() < Duration::from_secs(30), "no move written");
        h.step();
    }
    assert_eq!(moves(&p), before + 1, "one write for the whole drag");
    let shown = p.ok(&["feature", "show", "frame", "b2"]);
    let x = shown["pose"]["origin_m"][0].as_f64().unwrap();
    assert!(x > 0.04 + 1e-4, "moved along x: {x}");
    // Undone like any other change.
    let entries = p.ok(&["activity"])["entries"].clone();
    let seq = entries
        .as_array()
        .unwrap()
        .iter()
        .rfind(|e| e["method"] == "assembly.move")
        .unwrap()["seq"]
        .to_string();
    p.ok(&["undo", "--entry", &seq]);
    let back = p.ok(&["feature", "show", "frame", "b2"]);
    assert_eq!(back["pose"]["origin_m"][0], 0.04);
    p.ok(&["config", "set", "ui.navigation", "ryucad"]);
}

fn occt(name: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../ryustep/tests/data/occt")
        .join(name)
}

/// Types `text` into the field labelled `label`.
fn type_into(h: &mut Harness<'static, App>, label: &str, text: &str) {
    h.get_by_label(label).click();
    h.step();
    h.get_by_label(label).type_text(text);
    h.step();
}

#[test]
fn file_import_step_plans_then_imports() {
    let _one = one_at_a_time();
    if !gpu() {
        return;
    }
    let p = Project::new();
    let mut h = window(&p);
    frames(&mut h, |a| a.opened().ws.idle());
    menu(&mut h, "File", "Import STEP…");
    shown(&mut h, "STEP file");
    type_into(
        &mut h,
        "STEP file",
        &occt("box_ap214.stp").display().to_string(),
    );
    h.get_by_label("Plan").click();
    frames(&mut h, |a| {
        a.opened()
            .ws
            .import
            .as_ref()
            .is_some_and(|i| i.current().is_some())
    });
    shown(&mut h, "part-2 (new part, product part 2)");
    assert!(!p.root.join("imports").exists(), "a plan writes nothing");
    h.get_by_label("Import").click();
    frames(&mut h, |a| {
        a.opened()
            .ws
            .part("part-2")
            .and_then(|p| p.feature("body"))
            .is_some_and(|f| f.import.is_some())
    });
    shown(&mut h, "body (import: box_ap214.step / part 2)");
    // Chosen, the import offers its update, for its own file.
    h.get_by_label("body (import: box_ap214.step / part 2)")
        .click();
    frames(&mut h, |a| a.opened().view.chosen.is_some());
    // The dialog's, past the tiles' tabs' close buttons.
    h.get_all_by_label("Close").last().unwrap().click();
    h.step();
    menu(&mut h, "File", "Update import…");
    shown(&mut h, "Update import");
    assert_eq!(
        h.state()
            .opened()
            .ws
            .import
            .as_ref()
            .unwrap()
            .update
            .as_deref(),
        Some("imports/box_ap214.step")
    );
}

#[test]
fn file_export_step_writes_the_shown_part() {
    let _one = one_at_a_time();
    if !gpu() {
        return;
    }
    let p = Project::new().with_part();
    let mut h = window(&p);
    frames(&mut h, loaded);
    menu(&mut h, "File", "Export STEP…");
    shown(&mut h, "Save as");
    let out = tempfile::tempdir().unwrap();
    let path = out.path().join("p.step");
    type_into(&mut h, "Save as", &path.display().to_string());
    h.get_by_label("Export").click();
    frames(&mut h, |a| {
        a.opened()
            .ws
            .export
            .as_ref()
            .is_some_and(|e| e.done.is_some() || e.error.is_some())
    });
    let e = h.state().opened().ws.export.clone().unwrap();
    assert!(e.error.is_none(), "{:?}", e.error);
    assert_eq!(e.part, "p");
    let bytes = std::fs::read(&path).unwrap();
    assert!(bytes.starts_with(b"ISO-10303-21;"));
    assert_eq!(e.done.unwrap().1, bytes.len());
}

/// A bolt (a cylinder) and a frame placing it twice, and a nut once.
fn framed_bolts() -> Project {
    let p = Project::new();
    p.ok(&["part", "create", "bolt"]);
    p.ok(&[
        "feature",
        "add",
        "bolt",
        "cylinder",
        "--id",
        "shank",
        "--param",
        "radius=3",
        "--param",
        "height=20",
    ]);
    p.ok(&["part", "create", "nut"]);
    p.ok(&["part", "create", "frame", "--assembly"]);
    for (id, of, at) in [
        ("b1", "bolt", "0,0,0"),
        ("b2", "bolt", "40,0,0"),
        ("n1", "nut", "0,0,0"),
    ] {
        p.ok(&[
            "feature", "add", "frame", "instance", "--id", id, "--of", of, "--origin", at,
        ]);
    }
    p
}

fn bolts_drawn(a: &App) -> bool {
    match &a.screen {
        ryucad_app::ui::app::Screen::Opened(o) => {
            o.ws.idle()
                && o.ws
                    .model
                    .meshes
                    .contains_key(&("frame".into(), "b2/shank".into()))
        }
        _ => false,
    }
}

/// Opens the BOM tab of a window on `framed_bolts`, read.
fn bom_tab(p: &Project) -> Harness<'static, App> {
    let mut h = window(p);
    frames(&mut h, bolts_drawn);
    h.get_by_label("BOM").click();
    frames(&mut h, |a| {
        a.opened().ws.idle() && a.opened().ws.bom.root.is_some()
    });
    shown(&mut h, "BOM of frame");
    h
}

#[test]
fn the_bom_tab_chooses_a_line_and_goes_flat() {
    let _one = one_at_a_time();
    if !gpu() {
        return;
    }
    let p = framed_bolts();
    let mut h = bom_tab(&p);
    // Choosing a line selects its occurrences and frames them.
    let before = h.state().opened().view.nav.camera.clone();
    h.get_by_label("BOM line 1: bolt").click();
    frames(&mut h, |a| {
        a.opened()
            .ws
            .myself()
            .is_some_and(|m| m.selection == ["frame/b1", "frame/b2"])
    });
    assert_ne!(h.state().opened().view.nav.camera, before);
    // Flat: one line per part.
    h.get_by_label("Flat").click();
    frames(&mut h, |a| {
        a.opened().ws.idle() && !a.opened().ws.bom.flat_lines.is_empty()
    });
    shown(&mut h, "BOM line: bolt working");
}

#[test]
fn an_item_number_is_set_in_the_bom_tab() {
    let _one = one_at_a_time();
    if !gpu() {
        return;
    }
    let p = framed_bolts();
    let mut h = bom_tab(&p);
    h.get_by_label("Item of bolt in frame").click();
    shown(&mut h, "Set item");
    h.get_by_label("Item of bolt in frame")
        .set_text_value_accesskit("7");
    h.step();
    h.get_by_label("Set item").click();
    frames(&mut h, |a| {
        let t = &a.opened().ws.bom;
        a.opened().ws.idle() && t.editing.is_none() && t.tree[0].item == Some(7)
    });
    let bom = p.ok(&["bom", "frame"]);
    assert_eq!(bom["tree"]["children"][0]["item"], 7);
    // The flat BOM's numbers aren't edited.
    h.get_by_label("Flat").click();
    frames(&mut h, |a| {
        a.opened().ws.idle() && !a.opened().ws.bom.flat_lines.is_empty()
    });
    h.step();
    assert!(h.query_by_label("Item of bolt in frame").is_none());
}

#[test]
fn where_used_shows_an_assembly() {
    let _one = one_at_a_time();
    if !gpu() {
        return;
    }
    let p = framed_bolts();
    let mut h = window(&p);
    frames(&mut h, bolts_drawn);
    // The bolt chosen in the tree: the Part section says where it is used.
    h.get_by_label("shank (cylinder)").click();
    show_tab(&mut h, "part");
    frames(&mut h, |a| {
        a.opened()
            .ws
            .where_used
            .as_ref()
            .is_some_and(|w| w.part == "bolt")
    });
    shown(&mut h, "Where bolt is used");
    h.get_by_label("frame (2)");
    // A click lands where the button is drawn, so the panel must have
    // settled with it in view: the chosen feature's form arrives on its
    // own and pushes the section down, past the window's foot where text
    // runs taller (Windows' software adapter).
    frames(&mut h, |a| a.opened().ws.idle());
    h.get_by_label("Show frame").scroll_to_me();
    for _ in 0..30 {
        h.step();
    }
    let r = h.get_by_label("Show frame").rect();
    assert!(r.bottom() <= 700.0, "Show frame is out of view at {r:?}");
    h.get_by_label("Show frame").click();
    frames(&mut h, |a| {
        a.opened().ws.model.focus.as_deref() == Some("frame")
    });
}

#[test]
fn a_compare_marks_the_bom_tab() {
    let _one = one_at_a_time();
    if !gpu() {
        return;
    }
    let p = framed_bolts().committed();
    let mut h = bom_tab(&p);
    // An agent adds a bolt; compared with HEAD, the tab says so.
    p.ok(&[
        "feature", "add", "frame", "instance", "--id", "b3", "--of", "bolt",
    ]);
    menu(&mut h, "View", "Compare with HEAD");
    frames(&mut h, |a| {
        compared(a) && a.opened().ws.idle() && a.opened().ws.bom.diff.is_some()
    });
    shown(&mut h, "Compared with HEAD: 0 added, 0 removed, 1 changed");
    shown(&mut h, "3 (was 2)");
    // A part placed since is added, and a line like any other: chosen,
    // it selects its occurrence, and its item number is set.
    p.ok(&["part", "create", "washer"]);
    p.ok(&[
        "feature", "add", "frame", "instance", "--id", "w1", "--of", "washer",
    ]);
    frames(&mut h, |a| {
        let t = &a.opened().ws.bom;
        a.opened().ws.idle() && t.summary().as_deref() == Some("1 added, 0 removed, 1 changed")
    });
    h.get_by_label("BOM line 3: washer").click();
    frames(&mut h, |a| {
        a.opened()
            .ws
            .myself()
            .is_some_and(|m| m.selection == ["frame/w1"])
    });
    h.get_by_label("Item of washer in frame").click();
    shown(&mut h, "Set item");
    h.get_by_label("Item of washer in frame")
        .set_text_value_accesskit("9");
    h.step();
    h.get_by_label("Set item").click();
    frames(&mut h, |a| {
        let t = &a.opened().ws.bom;
        a.opened().ws.idle() && t.editing.is_none() && t.tree.iter().any(|l| l.item == Some(9))
    });
    let bom = p.ok(&["bom", "frame"]);
    assert_eq!(bom["tree"]["children"][2]["part"], "washer");
    assert_eq!(bom["tree"]["children"][2]["item"], 9);
}

/// Runs frames until no turn to a view is under way.
fn turned(h: &mut Harness<'static, App>) {
    frames(h, |a| a.opened().view.nav.turn.is_none());
}

/// The window camera's direction from the target towards the eye.
fn back(app: &App) -> [f64; 3] {
    let (_, _, f) = app.opened().view.nav.camera.basis();
    f.map(|x| -x)
}

fn along(a: [f64; 3], b: [f64; 3]) -> bool {
    let n = (b[0] * b[0] + b[1] * b[1] + b[2] * b[2]).sqrt();
    (0..3).all(|k| (a[k] - b[k] / n).abs() < 1e-9)
}

/// The middle, on screen, of the cube's tile that gives the view `name`,
/// as the window's camera turns the cube.
fn tile(h: &Harness<'static, App>, name: &str) -> egui::Pos2 {
    let o = h.state().opened();
    let want = ryucad::view::StandardView::named(name).unwrap();
    let (_, pts, _) = o
        .view
        .cube_widget()
        .visible(&o.view.nav.camera)
        .into_iter()
        .find(|(t, ..)| ryucad::view::StandardView::cube(t.view) == Some(want))
        .unwrap_or_else(|| panic!("{name}'s tile isn't seen"));
    let n = pts.len() as f32;
    egui::pos2(
        pts.iter().map(|p| p[0]).sum::<f32>() / n,
        pts.iter().map(|p| p[1]).sum::<f32>() / n,
    )
}

#[test]
fn the_cube_turns_the_view_and_never_picks() {
    let _one = one_at_a_time();
    if !gpu() {
        return;
    }
    let p = Project::new().with_part();
    let mut h = window(&p);
    frames(&mut h, loaded);
    // The first view is iso: the cube shows its front, right and top.
    let top = tile(&h, "top");
    h.event(egui::Event::PointerMoved(top));
    h.step();
    click(&h, top);
    h.step();
    turned(&mut h);
    assert!(
        along(back(h.state()), [0.0, 0.0, 1.0]),
        "{:?}",
        back(h.state())
    );
    // The click turned the view; it picked nothing under the cube.
    let me = h.state().opened().ws.myself().unwrap().selection.clone();
    assert!(me.is_empty(), "{me:?}");
    // From the top, the top face's corner tiles are the cube's corners.
    let corner = tile(&h, "front_top_right");
    click(&h, corner);
    h.step();
    turned(&mut h);
    assert!(along(back(h.state()), [1.0, -1.0, 1.0]));
    // An arrow rolls the view a quarter turn about the line of sight.
    let (before_back, before_up) = (back(h.state()), h.state().opened().view.nav.camera.up);
    let cube = h.state().opened().view.cube_widget();
    let (_, tri) = cube
        .arrows()
        .into_iter()
        .find(|(hit, _)| *hit == ryucad_app::core::cube::Hit::Roll(1))
        .unwrap();
    let mid = egui::pos2(
        (tri[0][0] + tri[1][0] + tri[2][0]) / 3.0,
        (tri[0][1] + tri[1][1] + tri[2][1]) / 3.0,
    );
    h.event(egui::Event::PointerMoved(mid));
    h.step();
    click(&h, mid);
    h.step();
    turned(&mut h);
    let c = h.state().opened().view.nav.camera.clone();
    assert!(along(back(h.state()), before_back));
    let dot: f64 = (0..3).map(|k| c.up[k] * before_up[k]).sum();
    assert!(dot.abs() < 1e-9, "{:?} against {before_up:?}", c.up);
    // The camera the session holds is where the turn ended.
    frames(&mut h, |a| {
        a.opened()
            .ws
            .myself()
            .is_some_and(|m| m.camera.as_ref() == Some(&c))
    });
}

#[test]
fn space_opens_the_view_selector_and_named_views_restore_and_save() {
    let _one = one_at_a_time();
    if !gpu() {
        return;
    }
    let p = Project::new().with_part();
    p.ok(&[
        "view",
        "save",
        "p",
        "detail",
        "--eye",
        "60,-40,30",
        "--target",
        "10,5,2",
        "--up",
        "0,0,1",
    ]);
    let mut h = window(&p);
    frames(&mut h, |a| {
        loaded(a) && a.opened().ws.views.camera("detail").is_some()
    });
    let mid = h.state().opened().view.rect.center();
    h.event(egui::Event::PointerMoved(mid));
    h.step();
    h.key_press(egui::Key::Space);
    shown(&mut h, "View selector");
    h.get_by_label("detail").click();
    h.step();
    turned(&mut h);
    let c = h.state().opened().view.nav.camera.clone();
    assert!(
        (0..3).all(|k| (c.eye[k] - [0.06, -0.04, 0.03][k]).abs() < 1e-12),
        "{:?}",
        c.eye
    );
    h.run_steps(2);
    assert!(h.query_by_label("View selector").is_none());
    // Save view… keeps the window's camera under a name, in Git.
    h.event(egui::Event::PointerMoved(mid));
    h.key_press(egui::Key::Space);
    shown(&mut h, "View selector");
    h.get_by_label("Save view…").click();
    h.run_steps(2);
    h.event(egui::Event::Text("close".into()));
    h.run_steps(2);
    h.get_by_label("Save").click();
    frames(&mut h, |a| a.opened().ws.views.camera("close").is_some());
    assert!(p.root.join("parts/p/views/close.json").is_file());
    let saved = p.ok(&["view", "list", "p"]);
    let close = saved["views"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["view"] == "close")
        .unwrap();
    assert_eq!(close["camera"]["eye"], serde_json::json!(c.eye));
    // Esc closes it.
    h.event(egui::Event::PointerMoved(mid));
    h.key_press(egui::Key::Space);
    shown(&mut h, "View selector");
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    assert!(h.query_by_label("View selector").is_none());
}

#[test]
fn the_navigation_styles_preset_keys_turn_the_view() {
    let _one = one_at_a_time();
    if !gpu() {
        return;
    }
    let p = Project::new().with_part();
    let mut h = window(&p);
    frames(&mut h, loaded);
    // ryucad's own: 1 is the front, as in FreeCAD.
    h.key_press(egui::Key::Num1);
    h.step();
    turned(&mut h);
    assert!(along(back(h.state()), [0.0, -1.0, 0.0]));
    // SolidWorks': Ctrl+5 is the top; 1 alone no longer turns anything.
    p.ok(&["config", "set", "ui.navigation", "solidworks"]);
    frames(&mut h, |a| {
        a.opened().view.nav.style == ryucad_app::core::nav::Navigation::Solidworks
    });
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Num5);
    h.step();
    turned(&mut h);
    assert!(along(back(h.state()), [0.0, 0.0, 1.0]));
    h.key_press(egui::Key::Num1);
    h.step();
    turned(&mut h);
    assert!(along(back(h.state()), [0.0, 0.0, 1.0]));
    // The View menu shows the preset's keys.
    h.get_by_label("View").click();
    h.run_steps(2);
    let named = |n: &egui_kittest::kittest::AccessKitNode<'_>| {
        n.label()
            .is_some_and(|l| l.starts_with("Front ") && l.contains('1'))
    };
    assert!(h.query_by(named).is_some());
    // Every window test shares the preferences.
    p.ok(&["config", "set", "ui.navigation", "ryucad"]);
}

#[test]
fn the_branches_tab_previews_and_compares_another_worktree() {
    let _one = one_at_a_time();
    if !gpu() {
        return;
    }
    let p = Project::new().with_part().committed();
    let v = p.ok(&[
        "--as",
        "agent-b",
        "worktree",
        "add",
        "../arm",
        "-b",
        "arm",
        "--no-warm",
    ]);
    let arm = std::path::PathBuf::from(v["path"].as_str().unwrap());
    // Both set the box's length: merging arm in would conflict.
    let out = std::process::Command::new(&ryucad().exe)
        .args(["feature", "edit", "p", "base", "--param", "x=15"])
        .current_dir(&arm)
        // In process: a daemon there would say who is attached.
        .env("RYUCAD_NO_DAEMON", "1")
        .output()
        .unwrap();
    assert!(out.status.success());
    p.ok(&["feature", "edit", "p", "base", "--param", "x=12"]);
    let mut h = window(&p);
    frames(&mut h, loaded);
    show_tab(&mut h, "branches");
    let arm_s = arm.to_str().unwrap().to_owned();
    frames(&mut h, |a| {
        a.opened().ws.branches.previews.contains_key(&arm_s)
    });
    shown(&mut h, "1 conflict");
    h.get_by_label("agent-b (expected)");
    h.get_by_label("Show preview").click();
    shown(&mut h, "Merging arm in would leave");
    h.get_by_label("value in base");
    // Compare puts the window into a compare with arm's working files.
    h.get_by_label("Compare").click();
    frames(&mut h, |a| {
        a.opened()
            .ws
            .compare
            .as_ref()
            .is_some_and(|c| c.rev == format!("worktree:{arm_s}") && c.loaded)
    });
}

#[test]
fn the_palette_runs_a_command_and_goes_to_a_feature() {
    let _one = one_at_a_time();
    if !gpu() {
        return;
    }
    let p = Project::new().with_part();
    let mut h = window(&p);
    frames(&mut h, loaded);
    let edges = h.state().opened().view.edges;
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::K);
    shown(&mut h, "Search commands");
    h.get_by_label("Search commands").click();
    h.run_steps(2);
    h.get_by_label("Search commands").type_text("edges");
    h.run_steps(2);
    h.key_press(egui::Key::Enter);
    frames(&mut h, |a| a.opened().view.edges != edges);
    assert!(h.state().opened().palette.is_none());
    assert_eq!(h.state().opened().recent_commands, ["view.edges"]);
    // `@` goes to a feature, choosing and framing it as the tree does.
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::K);
    shown(&mut h, "Search commands");
    h.get_by_label("Search commands").click();
    h.run_steps(2);
    h.get_by_label("Search commands").type_text("@boss");
    h.run_steps(2);
    h.get_by_label("p/boss (cylinder)");
    h.key_press(egui::Key::Enter);
    frames(&mut h, |a| {
        a.opened().view.chosen == Some(("p".to_owned(), "boss".to_owned()))
    });
    // Esc closes it, and Mod+K again too.
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::K);
    shown(&mut h, "Search commands");
    h.key_press(egui::Key::Escape);
    frames(&mut h, |a| a.opened().palette.is_none());
    p.ok(&[
        "config",
        "set",
        "display.edges",
        if edges { "true" } else { "false" },
    ]);
}

#[test]
fn a_rebound_key_is_used_and_a_conflict_refused() {
    let _one = one_at_a_time();
    if !gpu() {
        return;
    }
    use ryucad_app::core::keys::Chord;
    let p = Project::new().with_part();
    let mut h = window(&p);
    frames(&mut h, loaded);
    menu(&mut h, "Help", "Keyboard shortcuts");
    shown(&mut h, "Search shortcuts");
    h.get_by_label("Search shortcuts").click();
    h.run_steps(2);
    h.get_by_label("Search shortcuts").type_text("measure");
    h.run_steps(2);
    h.get_by_label("Keys of Measure").click();
    h.run_steps(2);
    // Mod+K is the palette's: refused, naming it, and nothing written.
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::K);
    h.run_steps(2);
    h.get_by_label_contains("is already Command palette (palette.open)");
    assert_eq!(
        p.ok(&["config", "get", "ui.keys"])["value"],
        serde_json::json!({})
    );
    // G is free.
    h.key_press(egui::Key::G);
    frames(&mut h, |a| {
        a.opened().keymap.chords("measure.tool") == [Chord::key(egui::Key::G)]
    });
    assert_eq!(
        p.ok(&["config", "get", "ui.keys"])["value"],
        serde_json::json!({"measure.tool": ["G"]})
    );
    // And it is the key now.
    let over = h.state().opened().view.rect.center();
    h.event(egui::Event::PointerMoved(over));
    h.step();
    h.key_press(egui::Key::G);
    frames(&mut h, |a| a.opened().ws.measure.on);
    h.key_press(egui::Key::Escape);
    frames(&mut h, |a| !a.opened().ws.measure.on);
    // Every window test shares the preferences.
    h.get_by_label("Reset all").click();
    frames(&mut h, |a| {
        a.opened().keymap.chords("measure.tool") == [Chord::key(egui::Key::M)]
    });
}

#[test]
fn tiles_close_reopen_and_the_layout_resets() {
    let _one = one_at_a_time();
    if !gpu() {
        return;
    }
    use ryucad_app::core::layout;
    use ryucad_app::ui::shell;
    let p = Project::new().with_part();
    let mut h = window(&p);
    frames(&mut h, loaded);
    let open = |h: &Harness<'static, App>, id: &str| shell::is_open(&h.state().opened().tiles, id);
    assert!(open(&h, "history"));
    // View > Panels checks each tile; choosing an open one closes it.
    menu(&mut h, "View", "Panels");
    h.run_steps(2);
    h.get_all_by_role(egui::accesskit::Role::Menu)
        .last()
        .unwrap()
        .get_by_label("History")
        .click();
    frames(&mut h, |a| !shell::is_open(&a.opened().tiles, "history"));
    // The layout is saved, at most once a second.
    let saved = || p.ok(&["config", "get", "ui.layout"])["value"].clone();
    let t = std::time::Instant::now();
    while saved() == serde_json::json!({}) {
        h.step();
        assert!(t.elapsed() < Duration::from_secs(30), "never saved");
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(!layout::read(&saved()).root.tiles().contains(&"history"));
    // Chosen again, it comes back at its place, showing.
    menu(&mut h, "View", "Panels");
    h.run_steps(2);
    h.get_all_by_role(egui::accesskit::Role::Menu)
        .last()
        .unwrap()
        .get_by_label("History")
        .click();
    frames(&mut h, |a| shell::is_open(&a.opened().tiles, "history"));
    // Reset layout: the default, and `{}` in the preferences.
    menu(&mut h, "View", "Reset layout");
    frames(&mut h, |a| {
        shell::node(&a.opened().tiles) == Some(layout::default())
    });
    let t = std::time::Instant::now();
    while saved() != serde_json::json!({}) {
        h.step();
        assert!(t.elapsed() < Duration::from_secs(30), "never reset");
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Clicks `item` in the context menu a right-click just opened.
fn context_item(h: &mut Harness<'static, App>, item: &str) {
    for _ in 0..10 {
        h.step();
        let open = h
            .query_all_by_role(egui::accesskit::Role::Menu)
            .last()
            .is_some_and(|m| m.query_by_label(item).is_some());
        if open {
            break;
        }
    }
    h.get_all_by_role(egui::accesskit::Role::Menu)
        .last()
        .unwrap()
        .get_by_label(item)
        .click();
}

#[test]
fn context_menus_follow_what_was_right_clicked() {
    let _one = one_at_a_time();
    if !gpu() {
        return;
    }
    use egui_kittest::kittest::NodeT;
    use ryucad_app::ui::shell;
    let p = Project::new().with_part();
    let mut h = window(&p);
    frames(&mut h, loaded);
    // A feature's row frames what it made.
    let before = h.state().opened().view.nav.camera.clone();
    h.get_by_label("boss (cylinder)").click_secondary();
    context_item(&mut h, "Zoom to fit");
    frames(&mut h, |a| a.opened().view.nav.camera != before);
    // A tile's tab closes it...
    h.get_by_role_and_label(egui::accesskit::Role::Button, "History")
        .click_secondary();
    context_item(&mut h, "Close");
    frames(&mut h, |a| !shell::is_open(&a.opened().tiles, "history"));
    // ...but not the viewport's.
    h.get_by_role_and_label(egui::accesskit::Role::Button, "Viewport")
        .click_secondary();
    for _ in 0..10 {
        h.step();
    }
    let close = h
        .get_all_by_role(egui::accesskit::Role::Menu)
        .last()
        .unwrap()
        .get_by_label("Close")
        .accesskit_node()
        .is_disabled();
    assert!(close, "the viewport's tab can't be closed");
    h.key_press(egui::Key::Escape);
    h.step();
    // Every window test shares the preferences.
    menu(&mut h, "View", "Reset layout");
    frames(&mut h, |a| shell::is_open(&a.opened().tiles, "history"));
}

/// Presses or lets go of the primary button at `pos`.
fn press(h: &Harness<'static, App>, pos: egui::Pos2, pressed: bool) {
    h.event(egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    });
}

/// Drags from `from` to `to` in `steps` moves, a frame each.
fn drag(h: &mut Harness<'static, App>, from: egui::Pos2, to: egui::Pos2, steps: u32) {
    h.event(egui::Event::PointerMoved(from));
    h.step();
    press(h, from, true);
    h.step();
    for i in 1..=steps {
        let t = i as f32 / steps as f32;
        h.event(egui::Event::PointerMoved(from + (to - from) * t));
        h.step();
    }
    press(h, to, false);
    h.step();
}

#[test]
fn a_section_plane_is_dragged_by_its_arrow_and_written_once() {
    let _one = one_at_a_time();
    if !gpu() {
        return;
    }
    let p = Project::new().with_part();
    let mut h = window(&p);
    frames(&mut h, loaded);
    // An agent cuts the human's window: the box is 5 mm tall.
    p.ok(&["section", "xy", "--offset", "3"]);
    frames(&mut h, |a| a.opened().ws.sections.planes.len() == 1);
    menu(&mut h, "View", "Section view");
    shown(&mut h, "Plane 1");
    assert!(h.state().opened().ws.sections.handles);
    // The arrow runs up from the plane's origin: drag it up half its
    // length on screen.
    let (base, tip) = {
        let o = h.state_mut().opened_mut();
        let length = o.view.handle_length(&o.ws);
        let plane = o.ws.sections.planes[0];
        let tip = [0, 1, 2].map(|k| plane.origin_m[k] + plane.normal[k] * length);
        (plane.origin_m, tip)
    };
    let (b, t) = (on_screen(&h, base), on_screen(&h, tip));
    assert!(
        b.y - t.y > 10.0,
        "the arrow points up the screen: {b:?} {t:?}"
    );
    let from = b + (t - b) * 0.7;
    drag(&mut h, from, from + (t - b) * 0.5, 6);
    // Written once let go: the session's plane is higher.
    let t0 = Instant::now();
    loop {
        let z = p.ok(&["section"])["sections"][0]["origin_m"][2]
            .as_f64()
            .unwrap();
        if z > 0.0031 {
            assert!(h.state().opened().ws.sections.dragging.is_none());
            break;
        }
        assert!(t0.elapsed() < Duration::from_secs(30), "never written: {z}");
        h.step();
    }
    // Remove all takes the planes away for the session too.
    h.get_by_label("Remove all").click();
    let t0 = Instant::now();
    while p.ok(&["section"])["sections"] != serde_json::json!([]) {
        assert!(t0.elapsed() < Duration::from_secs(30), "never removed");
        h.step();
    }
}

#[test]
fn an_explode_is_chosen_and_played_from_its_tile() {
    let _one = one_at_a_time();
    if !gpu() {
        return;
    }
    let p = two_bolts();
    p.ok(&[
        "explode",
        "save",
        "frame",
        "apart",
        "--steps",
        r#"[{"instances": ["b2"], "translate": [0, 0, 30]}]"#,
    ]);
    let mut h = window(&p);
    both_bolts(&mut h);
    let b2 = ("frame".to_owned(), "b2/shank".to_owned());
    let z = |h: &Harness<'static, App>| h.state().opened().ws.model.pose(&b2).origin[2];
    let before = z(&h);
    menu(&mut h, "View", "Exploded view");
    frames(&mut h, |a| a.opened().ws.explodes.list.len() == 1);
    shown(&mut h, "apart");
    h.get_by_label("apart").click();
    frames(&mut h, |a| {
        a.opened()
            .ws
            .model
            .pose(&("frame".into(), "b2/shank".into()))
            .origin[2]
            > before + 0.029
    });
    // The slider plays it back: dragged towards its start, the bolt comes
    // down with it, and the fraction is written when let go.
    shown(&mut h, "explode fraction");
    let r = h.get_by_label("explode fraction").rect();
    let from = egui::pos2(r.max.x - 2.0, r.center().y);
    drag(
        &mut h,
        from,
        egui::pos2(r.min.x + r.width() * 0.4, r.center().y),
        8,
    );
    let lifted = z(&h) - before;
    assert!(
        lifted > 0.001 && lifted < 0.029,
        "part of the way: {lifted}"
    );
    let t0 = Instant::now();
    loop {
        let f = p.ok(&["participants"])["participants"]
            .as_array()
            .unwrap()
            .iter()
            .find(|x| x["kind"] == "human")
            .map(|x| x["explode"]["fraction"].as_f64().unwrap_or(1.0))
            .unwrap();
        if f < 1.0 {
            assert!((f * 0.03 - lifted).abs() < 1e-6, "{f} {lifted}");
            break;
        }
        assert!(t0.elapsed() < Duration::from_secs(30), "never written");
        h.step();
    }
    // Collapse: the bolt back where its assembly puts it.
    h.get_by_label("Collapse").click();
    frames(&mut h, |a| {
        a.opened()
            .ws
            .model
            .pose(&("frame".into(), "b2/shank".into()))
            .origin[2]
            == before
    });
}
