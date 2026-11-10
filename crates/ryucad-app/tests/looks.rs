//! The window and the icon sheet in every look (design 199, Testing):
//! Drafting, Graphite and Studio, light and dark, and high contrast, each
//! compared with a picture of it within a tolerance, as the viewport's
//! snapshots are (`snapshot.rs`): the pictures are lavapipe's, other
//! adapters only check that something was drawn. The window is the
//! sample project's, 1600 by 1000, as the mockups draw it.
//! `UPDATE_SNAPSHOTS=1` draws them again; a failing run leaves its
//! picture beside its own as `*.new.png`.

mod common;

use common::{Project, gpu, ryucad};
use eframe::egui;
use egui_kittest::Harness;
use ryucad_app::ui::app::{App, Screen};
use std::path::Path;
use std::time::{Duration, Instant};

/// A channel further off than this counts as a different pixel.
const CHANNEL: u8 = 24;
/// At most this share of pixels may differ.
const SHARE: f64 = 0.01;

fn looks_as_it_did(lavapipe: bool, img: &image::RgbaImage, name: &str) {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("tests/snapshots/{name}-lavapipe.png"));
    if std::env::var_os("UPDATE_SNAPSHOTS").is_some() {
        img.save(&path).unwrap();
        return;
    }
    let first = img.get_pixel(0, 0);
    let drawn = img.pixels().filter(|p| *p != first).count();
    if !lavapipe {
        assert!(drawn > 5000, "{name}: drew {drawn} pixels");
        eprintln!("{name}: drawn, not compared (the picture is lavapipe's)");
        return;
    }
    let want = image::open(&path).unwrap().to_rgba8();
    assert_eq!(want.dimensions(), img.dimensions(), "{name}");
    let differ = img
        .pixels()
        .zip(want.pixels())
        .filter(|(a, b)| a.0.iter().zip(b.0).any(|(x, y)| x.abs_diff(y) > CHANNEL))
        .count();
    let limit = (f64::from(img.width() * img.height()) * SHARE) as usize;
    if differ > limit {
        img.save(path.with_extension("new.png")).unwrap();
    }
    assert!(
        differ <= limit,
        "{differ} pixels differ (at most {limit}); see {name}-lavapipe.new.png"
    );
}

/// The looks, themes and contrasts the snapshots are taken in, with
/// their names.
const VARIANTS: [(&str, &str, &str, &str); 8] = [
    ("drafting", "light", "normal", "drafting-light"),
    ("drafting", "dark", "normal", "drafting-dark"),
    ("graphite", "light", "normal", "graphite-light"),
    ("graphite", "dark", "normal", "graphite-dark"),
    ("studio", "light", "normal", "studio-light"),
    ("studio", "dark", "normal", "studio-dark"),
    ("drafting", "light", "high", "high-light"),
    ("drafting", "dark", "high", "high-dark"),
];

/// Runs frames until `done` holds.
fn frames<T>(h: &mut Harness<'static, T>, done: impl Fn(&T) -> bool, seen: impl Fn(&T) -> String) {
    let t = Instant::now();
    loop {
        h.step();
        if done(h.state()) {
            return;
        }
        assert!(
            t.elapsed() < Duration::from_secs(60),
            "timed out: {}",
            seen(h.state())
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// One test, so the preferences each variant sets and the window's user
/// name stay this test's alone.
#[test]
fn the_window_and_the_icon_sheet_look_as_they_did() {
    let Some((adapter, _, _)) = gpu() else {
        return;
    };
    let info = adapter.get_info();
    let lavapipe = format!("{} {}", info.name, info.driver).contains("llvmpipe");
    // The window's name is the OS user's: the same in every picture.
    // SAFETY: set before any window reads it, in this binary's one test.
    unsafe { std::env::set_var("USER", "dana") };
    // The sample, as Open sample… writes it.
    let p = Project::named("sample");
    std::fs::remove_file(p.root.join("ryucad.json")).unwrap();
    std::fs::remove_file(p.root.join(".gitattributes")).unwrap();
    ryucad_app::core::start::open_sample(ryucad(), &p.root).unwrap();
    for (look, theme, contrast, name) in VARIANTS {
        for (key, value) in [
            ("ui.style", look),
            ("ui.theme", theme),
            ("ui.contrast", contrast),
            ("ui.scale", "1"),
            ("ui.layout", "{}"),
        ] {
            p.ok(&["config", "set", key, value]);
        }
        // The icon sheet, in the look's palette.
        let (l, c) = (
            ryucad_app::ui::style::Look::parse(look),
            ryucad_app::ui::style::Contrast::parse(contrast),
        );
        let dark = theme == "dark";
        let mut sheet = Harness::builder()
            .with_size(egui::vec2(800.0, 1920.0))
            .wgpu()
            .build_ui(move |ui| {
                ryucad_app::ui::fonts::install(ui.ctx());
                ryucad_app::ui::style::apply(ui.ctx(), l, c);
                ui.ctx().set_theme(if dark {
                    egui::Theme::Dark
                } else {
                    egui::Theme::Light
                });
                let p = l.colours(dark, c);
                egui::Frame::new()
                    .fill(ryucad_app::ui::style::rgb(p.app))
                    .inner_margin(16)
                    .show(ui, |ui| {
                        ui.set_min_size(ui.available_size());
                        ryucad_app::ui::tiles::tree::sheet(ui, &p);
                    });
            });
        sheet.run_steps(3);
        looks_as_it_did(lavapipe, &sheet.render().unwrap(), &format!("icons-{name}"));
        // The window over the sample, its model drawn and nothing under
        // the pointer.
        let root = p.root.clone();
        let mut h = Harness::builder()
            .with_size(egui::vec2(1600.0, 1000.0))
            .wgpu()
            .build_eframe(move |cc| App::open(&cc.egui_ctx, Some(&root), ryucad()));
        frames(
            &mut h,
            |a| match &a.screen {
                Screen::Opened(o) => {
                    o.ws.idle()
                        && o.ws.model.meshes.len() >= 2
                        && o.ws.settings.text("ui.style", "") == look
                        && o.ws.settings.text("ui.contrast", "") == contrast
                }
                _ => false,
            },
            |a| match &a.screen {
                Screen::Opened(o) => format!(
                    "idle {}, {} meshes, style {:?}, contrast {:?}",
                    o.ws.idle(),
                    o.ws.model.meshes.len(),
                    o.ws.settings.get("ui.style"),
                    o.ws.settings.get("ui.contrast")
                ),
                _ => "no project open".to_owned(),
            },
        );
        h.event(egui::Event::PointerGone);
        for _ in 0..20 {
            h.step();
            std::thread::sleep(Duration::from_millis(20));
        }
        // The settling frames may send a request of their own (the layout
        // saved, a reading asked again): wait for it to be answered rather
        // than judge one frame, so the picture is of the window at rest.
        frames(
            &mut h,
            |a| a.opened().ws.idle(),
            |a| format!("idle {}", a.opened().ws.idle()),
        );
        looks_as_it_did(lavapipe, &h.render().unwrap(), &format!("window-{name}"));
    }
    // Annotate and Direct edit (ryucad 19.B): the sample, Drafting light,
    // the default tiles. One open comment, pinned with a point on the
    // bracket where `rig` places it as `plate` (the window shows `rig`,
    // and draws markers only of pins on what it shows), so Annotate's
    // picture has a marker; the agent's, so the window's own count is 0.
    for (key, value) in [
        ("ui.style", "drafting"),
        ("ui.theme", "light"),
        ("ui.contrast", "normal"),
        ("ui.layout", "{}"),
    ] {
        p.ok(&["config", "set", key, value]);
    }
    let body = p.ok(&["inspect", "bracket"])["bodies"][0].clone();
    let pin = format!(
        "rig/plate/{}/{}",
        body["solid"].as_str().unwrap(),
        body["faces"][0]["name"].as_str().unwrap()
    );
    p.ok(&[
        "annotations",
        "add",
        &pin,
        "--comment",
        "Thicker here",
        "--point",
        "0,0,0",
    ]);
    for (mode, name) in [
        (ryucad_app::core::modes::Mode::Annotate, "annotate"),
        (ryucad_app::core::modes::Mode::Edit, "direct"),
    ] {
        let root = p.root.clone();
        let mut h = Harness::builder()
            .with_size(egui::vec2(1600.0, 1000.0))
            .wgpu()
            .build_eframe(move |cc| App::open(&cc.egui_ctx, Some(&root), ryucad()));
        frames(
            &mut h,
            |a| {
                matches!(&a.screen, Screen::Opened(o) if o.ws.idle()
                    && o.ws.model.meshes.len() >= 2
                    && o.ws.annotations.loaded)
            },
            |_| "loading".into(),
        );
        h.state_mut().opened_mut().ws.set_mode(mode);
        h.event(egui::Event::PointerGone);
        for _ in 0..20 {
            h.step();
            std::thread::sleep(Duration::from_millis(20));
        }
        frames(
            &mut h,
            |a| a.opened().ws.idle(),
            |a| format!("idle {}", a.opened().ws.idle()),
        );
        looks_as_it_did(lavapipe, &h.render().unwrap(), name);
    }
    // Marking up (ryucad 20.B): the Markup bar open in Annotate on the
    // sample, an arrow, an ellipse and a note on the glass and a stroke on
    // the bracket's top, in the person's colour, the camera locked.
    let root = p.root.clone();
    let mut h = Harness::builder()
        .with_size(egui::vec2(1600.0, 1000.0))
        .wgpu()
        .build_eframe(move |cc| App::open(&cc.egui_ctx, Some(&root), ryucad()));
    frames(
        &mut h,
        |a| {
            matches!(&a.screen, Screen::Opened(o) if o.ws.idle()
                && o.ws.model.meshes.len() >= 2
                && o.ws.annotations.loaded)
        },
        |_| "loading".into(),
    );
    for _ in 0..10 {
        h.step();
    }
    {
        use ryucad_app::core::markup::Tool;
        let o = h.state_mut().opened_mut();
        o.ws.set_mode(ryucad_app::core::modes::Mode::Annotate);
        let camera = o.view.nav.camera.clone();
        let hh = camera.half_height(camera.distance());
        o.ws.open_markup(camera).unwrap();
        let m = o.ws.markup.as_mut().unwrap();
        let at = |x: f64, y: f64| [x * hh, y * hh];
        m.press(at(-0.6, 0.5));
        m.drag(at(-0.2, 0.15), false);
        m.release(1e-4);
        m.choose(Tool::Ellipse);
        m.press(at(0.1, -0.1));
        m.drag(at(0.45, 0.2), false);
        m.release(1e-4);
        m.choose(Tool::PenFaces);
        let key = ("rig".to_owned(), "plate/e1".to_owned());
        m.press_face(key.clone(), "rig/plate".into(), [0.01, 0.025, 0.008]);
        m.drag_face(Some((key.clone(), [0.02, 0.03, 0.008])));
        m.drag_face(Some((key, [0.035, 0.025, 0.008])));
        m.release(1e-4);
        m.choose(Tool::Text);
        m.press(at(-0.6, 0.6));
        m.typing.as_mut().unwrap().1 = "move the pin here".into();
        m.commit_text();
        m.comment = "And round this edge 2 mm".into();
    }
    h.event(egui::Event::PointerGone);
    for _ in 0..20 {
        h.step();
        std::thread::sleep(Duration::from_millis(20));
    }
    frames(
        &mut h,
        |a| a.opened().ws.idle(),
        |a| format!("idle {}", a.opened().ws.idle()),
    );
    looks_as_it_did(lavapipe, &h.render().unwrap(), "redlines");
}
