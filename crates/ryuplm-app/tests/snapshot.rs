//! The window as lavapipe draws it, in both themes, within a tolerance
//! (design 938, Testing), and every control named for accesskit.

mod common;

use common::{catalogue, daemon, gpu, one_at_a_time};
use eframe::egui;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use ryuplm_app::ui::app::App;
use std::path::Path;
use std::time::{Duration, Instant};

const W: f32 = 1200.0;
const H: f32 = 800.0;
const CHANNEL: u8 = 24;
const SHARE: f64 = 0.01;

/// The seeded catalogue drawn with its first item open, idle, flashes off.
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
    let end = Instant::now() + Duration::from_secs(30);
    let mut opened = false;
    loop {
        h.step();
        std::thread::sleep(Duration::from_millis(20));
        let w = h.state_mut().opened_mut().unwrap();
        if !opened && let Some(r) = w.rows.get(0).cloned() {
            w.open_item(Some(r.id));
            opened = true;
        }
        let w = h.state().opened().unwrap();
        if opened && w.card.value.is_some() && w.bom.value.is_some() && w.idle() {
            break;
        }
        assert!(Instant::now() < end, "the window never settled");
    }
    let w = h.state_mut().opened_mut().unwrap();
    w.flash_on = false;
    w.flash.clear();
    for _ in 0..4 {
        h.step();
    }
    h
}

fn looks_as_it_did(img: &image::RgbaImage, name: &str) {
    if let Some(why) = differs(img, name) {
        panic!("{why}");
    }
}

/// Why `img` doesn't look as snapshot `name` did (its `.new.png` written
/// beside it), or `None` when it does, isn't compared here, or was saved.
fn differs(img: &image::RgbaImage, name: &str) -> Option<String> {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("tests/snapshots/{name}-lavapipe.png"));
    if std::env::var_os("UPDATE_SNAPSHOTS").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        img.save(&path).unwrap();
        return None;
    }
    let (adapter, _, _) = ryuplm_app::ui::smoke::headless().unwrap();
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
        return None;
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
    if differ <= limit {
        return None;
    }
    img.save(path.with_extension("new.png")).unwrap();
    Some(format!(
        "{differ} pixels differ (at most {limit}); see {name}-lavapipe.new.png"
    ))
}

#[test]
fn the_window_looks_as_it_did_in_both_themes() {
    let _one = one_at_a_time();
    if !gpu() {
        return;
    }
    let root = catalogue("snapshot", 300);
    for (dark, name) in [(false, "window-light"), (true, "window-dark")] {
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
    let root = catalogue("names", 300);
    let h = loaded(&root, false);
    for label in [
        "Navigator",
        "Items",
        "Item",
        "BOM",
        "Where used",
        "Findings",
        "Changes",
        "Releases",
        "Imports",
        "Documents",
        "Activity",
        "Edit",
        "View",
        "Tools",
        "Copy reference",
        "Copy number",
    ] {
        assert!(
            h.query_all_by_label(label).next().is_some(),
            "nothing named {label}"
        );
    }
    let unnamed = h
        .query_all_by(|n| {
            n.role() == egui::accesskit::Role::Button
                && n.label().is_none_or(|l| l.trim().is_empty())
        })
        .count();
    assert_eq!(unnamed, 0, "unnamed buttons");
}
