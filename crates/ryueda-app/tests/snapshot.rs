//! The canvas as lavapipe draws it, in both themes, within a tolerance
//! (design 519, Testing approach), and every control named for
//! accesskit (ryucad 11.E's rule).

mod common;

use common::{breakout, daemon, gpu, one_at_a_time};
use eframe::egui;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use ryueda_app::ui::app::App;
use std::path::Path;
use std::time::{Duration, Instant};

const W: f32 = 1200.0;
const H: f32 = 800.0;
/// A channel further off than this makes a pixel differ.
const CHANNEL: u8 = 24;
/// At most this share of pixels may differ.
const SHARE: f64 = 0.01;

fn loaded(root: &Path, dark: bool) -> Harness<'static, App> {
    let root = root.to_owned();
    let mut h = Harness::builder()
        .with_size(egui::vec2(W, H))
        .wgpu()
        .build_eframe(move |cc| App::open(&cc.egui_ctx, &root, daemon()));
    h.ctx.set_theme(if dark {
        egui::Theme::Dark
    } else {
        egui::Theme::Light
    });
    let want = if dark { "dark" } else { "light" };
    let end = Instant::now() + Duration::from_secs(30);
    while !h
        .state()
        .opened()
        .and_then(|w| w.scene.as_ref())
        .is_some_and(|s| s.theme == want && s.block == "breakout")
        || !h.state().opened().is_some_and(|w| w.idle())
    {
        assert!(Instant::now() < end, "the sheet never drew");
        h.step();
        std::thread::sleep(Duration::from_millis(20));
    }
    // Flashes off: a picture mustn't catch a fade.
    h.state_mut().opened_mut().unwrap().flash_on = false;
    h.state_mut().opened_mut().unwrap().flash.clear();
    for _ in 0..4 {
        h.step();
    }
    h
}

fn looks_as_it_did(img: &image::RgbaImage, name: &str) {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("tests/snapshots/{name}-lavapipe.png"));
    if std::env::var_os("UPDATE_SNAPSHOTS").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        img.save(&path).unwrap();
        return;
    }
    let (adapter, _, _) = ryueda_app::ui::smoke::headless().unwrap();
    let info = adapter.get_info();
    if !format!("{} {}", info.name, info.driver).contains("llvmpipe") {
        let drawn = img
            .pixels()
            .filter(|p| p.0[..3] != img.get_pixel(0, 0).0[..3])
            .count();
        assert!(drawn > 20_000, "{} drew {drawn} pixels", info.name);
        eprintln!(
            "{}: drawn, not compared (the picture is lavapipe's)",
            info.name
        );
        return;
    }
    let want = image::open(&path)
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
        .to_rgba8();
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

#[test]
fn the_canvas_looks_as_it_did_in_both_themes() {
    let _one = one_at_a_time();
    if !gpu() {
        return;
    }
    let root = breakout("snapshot");
    for (dark, name) in [(false, "canvas-light"), (true, "canvas-dark")] {
        let mut h = loaded(&root, dark);
        let img = h.render().unwrap();
        looks_as_it_did(&img, name);
    }
}

#[test]
fn every_control_is_named() {
    let _one = one_at_a_time();
    if !gpu() {
        return;
    }
    let root = breakout("names");
    let h = loaded(&root, false);
    for label in [
        "Schematic sheet",
        "Navigator",
        "Sheet",
        "Properties",
        "Findings",
        "Instructions",
        "Activity",
        "Edit",
        "View",
        "Tools",
        "Run check",
        "Breakout (breakout)",
    ] {
        assert!(h.query_by_label(label).is_some(), "nothing named {label}");
    }
    let unnamed = h
        .query_all_by(|n| {
            n.role() == egui::accesskit::Role::Button
                && n.label().is_none_or(|l| l.trim().is_empty())
        })
        .count();
    assert_eq!(unnamed, 0, "unnamed buttons");
}
