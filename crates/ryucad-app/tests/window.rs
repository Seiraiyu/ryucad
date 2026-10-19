//! The main window, driven like a person would, against a real daemon.
//! It needs a graphics adapter (software ones count): see `common::gpu`.

mod common;

use common::{Project, ryucad};
use eframe::egui;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use ryucad_app::ui::app::App;
use std::time::{Duration, Instant};

fn gpu() -> bool {
    common::gpu().is_some()
}

fn window(p: &Project) -> Harness<'static, App> {
    let root = p.root.clone();
    Harness::builder()
        .with_size(egui::vec2(1000.0, 700.0))
        .wgpu()
        .build_eframe(move |cc| App::open(&cc.egui_ctx, Some(&root), ryucad()))
}

/// Opens the menu `menu` and clicks `item` in it.
fn menu(h: &mut Harness<'static, App>, menu: &str, item: &str) {
    h.get_by_label(menu).click();
    // A menu opens over a frame or two.
    for _ in 0..10 {
        h.step();
        if h.query_by_label(item).is_some() {
            break;
        }
    }
    h.get_by_label(item).click();
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
fn the_first_view_frames_the_whole_model() {
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
fn the_activity_tab_undoes_and_ctrl_z_does_too() {
    if !gpu() {
        return;
    }
    let p = Project::new().with_part();
    let mut h = window(&p);
    frames(&mut h, loaded);
    p.ok(&["feature", "edit", "p", "boss", "--param", "radius=5"]);
    frames(&mut h, |a| !a.opened().ws.activity.entries.is_empty());
    h.step();
    h.get_by_label("feature.edit");
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
    h.get_by_role(egui::accesskit::Role::CheckBox).click();
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
    p.ok(&["config", "set", "ui.theme", "system"]);
    p.ok(&["config", "set", "display.edges", "true"]);
}

#[test]
fn a_damaged_project_is_repaired_then_opened() {
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
    h.get_by_label("Open project").click();
    frames(&mut h, |a| {
        matches!(a.screen, ryucad_app::ui::app::Screen::Opened(_)) && loaded(a)
    });
}

#[test]
fn the_start_window_reopens_recent_projects_and_creates_new_ones() {
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
    h.run_steps(2);
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
fn a_typed_folder_path_opens_without_the_os_dialog() {
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
    let want = ryucad_app::ui::style::viewport_background(dark).map(|c| (c * 255.0).round() as u8);
    assert!(
        px.iter().zip(want).all(|(a, b)| a.abs_diff(b) <= 3),
        "the background shows as {px:?}, drawn as {want:?}"
    );
}
